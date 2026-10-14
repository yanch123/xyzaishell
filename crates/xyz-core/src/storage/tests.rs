use super::{credentials::*, profiles::*, task_credentials::*, trust::*, *};
use std::path::PathBuf;
use uuid::Uuid;

struct Fixture {
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("xyz-storage-{}", Uuid::new_v4()));
        private_files::PrivateDirectory::open_or_create(&path).unwrap();
        Self { path }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
fn draft(id: &str) -> ProfileDraft {
    ProfileDraft {
        id: id.into(),
        label: "Synthetic profile".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "synthetic-user".into(),
        auth_kind: AuthKind::Password,
        private_key_path: None,
    }
}
fn session() -> CredentialStore {
    CredentialStore::new(CredentialMode::SessionOnly).unwrap()
}
fn field(name: &str) -> SecretFieldRequest {
    SecretFieldRequest {
        name: name.into(),
        required: true,
    }
}
fn edit(fixture: &Fixture, name: &str, bytes: &[u8]) {
    private_files::PrivateDirectory::open_or_create(&fixture.path)
        .unwrap()
        .update(name, 1024 * 1024, |_| {
            Ok(((), Some(zeroize::Zeroizing::new(bytes.to_vec()))))
        })
        .unwrap();
}

#[test]
fn profiles_revision_and_session_values_never_persist_or_serialize() {
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    let credentials = session();
    store.initialize(CredentialMode::SessionOnly).unwrap();
    let id = Uuid::new_v4().to_string();
    assert_eq!(
        store
            .save(0, draft(&id), CredentialChange::Keep, &credentials)
            .unwrap(),
        1
    );
    credentials
        .set_session(
            &id,
            CredentialPurpose::SshPassword,
            SecretValue::new(b"synthetic-ssh-secret".to_vec()).unwrap(),
        )
        .unwrap();
    let listing = serde_json::to_string(&store.list(&credentials).unwrap()).unwrap();
    assert!(listing.contains("\"credential_present\":true"));
    assert!(!listing.contains("synthetic-ssh-secret"));
    let bytes = std::fs::read(f.path.join("connections.toml")).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("synthetic-ssh-secret"));
    assert!(!text.contains("protected_password"));
    assert!(!text.contains("secret_ref"));
    assert_eq!(
        store
            .resolve_for_authentication(&id, 1, &credentials)
            .unwrap()
            .credential
            .unwrap()
            .expose(),
        b"synthetic-ssh-secret"
    );
    assert_eq!(
        store.save(0, draft(&id), CredentialChange::Keep, &credentials),
        Err(StorageError::Conflict)
    );
    assert_eq!(store.list(&credentials).unwrap().revision, "1");
    assert!(store
        .resolve_for_authentication(&id, 1, &session())
        .is_err());
}
#[test]
fn strict_profile_schema_forbids_plaintext_unknown_fields_and_platform_mixing() {
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    let id = Uuid::new_v4();
    let base=format!("schema_version=1\nrevision=0\n[storage]\nsecret_protection='session_only'\n[[connections]]\nid='{id}'\nlabel='test'\nhost='localhost'\nusername='u'\n[connections.auth]\nkind='password'\n");
    for invalid in [
        format!("{base}password='TOP_SECRET'\n"),
        format!("{base}protected_password='TOP_SECRET'\n"),
        format!("{base}password_secret_ref='TOP_SECRET'\n"),
        base.replace("schema_version=1", "schema_version=9"),
        base.replace("kind='password'", "kind='agent'"),
    ] {
        edit(&f, "connections.toml", invalid.as_bytes());
        let error = store.list(&session()).unwrap_err();
        assert!(!format!("{error:?}").contains("TOP_SECRET"));
    }
    let mode = if cfg!(windows) {
        "linux_secret_service"
    } else {
        "dpapi_current_user"
    };
    edit(
        &f,
        "connections.toml",
        base.replace("session_only", mode).as_bytes(),
    );
    assert!(matches!(
        store.list(&session()),
        Err(StorageError::BackendMismatch)
    ));
}
#[test]
fn profile_input_rejects_path_ids_hosts_and_invalid_auth_combinations() {
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    store.initialize(CredentialMode::SessionOnly).unwrap();
    for host in [
        "ssh://localhost",
        "u@localhost",
        "localhost;rm",
        "host.example.invalid",
        "-option",
        "hello world",
        "localhost\n",
    ] {
        let mut d = draft(&Uuid::new_v4().to_string());
        d.host = host.into();
        assert!(store
            .save(0, d, CredentialChange::Keep, &session())
            .is_err());
    }
    assert!(store
        .save(0, draft("../other"), CredentialChange::Keep, &session())
        .is_err());
    let mut d = draft(&Uuid::new_v4().to_string());
    d.private_key_path = Some(PathBuf::from("relative"));
    assert!(store
        .save(0, d, CredentialChange::Keep, &session())
        .is_err());
}
#[test]
fn task_templates_only_requested_names_and_preserve_filled_values() {
    let f = Fixture::new();
    let store = TaskCredentialStore::open(&f.path).unwrap();
    let id = Uuid::new_v4().to_string();
    let initial = store
        .ensure_template(&id, &[field("deploy_password")], None)
        .unwrap();
    assert_eq!(initial.fields.len(), 1);
    assert!(!initial.fields[0].present);
    let path = store.local_edit_path(&id).unwrap();
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("value = \"\"", "value = \"synthetic task $secret é\"");
    std::fs::write(&path, text).unwrap();
    assert!(matches!(
        store.ensure_template(&id, &[field("next_token")], Some(&initial.snapshot_id)),
        Err(StorageError::Conflict)
    ));
    let loaded = store.load_snapshot(&id).unwrap();
    let reference = loaded.reference("deploy_password").unwrap();
    assert_eq!(
        loaded
            .resolve_for_approved_adapter(&id, &reference)
            .unwrap(),
        "synthetic task $secret é"
    );
    let metadata = serde_json::to_string(&loaded.status()).unwrap();
    assert!(!metadata.contains("synthetic task"));
    assert!(!format!("{loaded:?}").contains("synthetic task"));
    let extended = store
        .ensure_template(
            &id,
            &[field("next_token")],
            Some(&loaded.status().snapshot_id),
        )
        .unwrap();
    assert_eq!(extended.fields.len(), 2);
    let reloaded = store.load_snapshot(&id).unwrap();
    assert!(matches!(
        reloaded.resolve_for_approved_adapter(&id, &reference),
        Err(StorageError::SnapshotStale)
    ));
    assert_eq!(
        reloaded
            .resolve_for_approved_adapter(&id, &reloaded.reference("deploy_password").unwrap())
            .unwrap(),
        "synthetic task $secret é"
    );
}
#[test]
fn task_identity_unknown_fields_and_missing_values_fail_closed() {
    let f = Fixture::new();
    let store = TaskCredentialStore::open(&f.path).unwrap();
    let a = Uuid::new_v4().to_string();
    let b = Uuid::new_v4().to_string();
    assert!(store
        .ensure_template("../../escape", &[field("x")], None)
        .is_err());
    assert!(store
        .ensure_template(&a, &[field("Shell$(id)")], None)
        .is_err());
    store.ensure_template(&a, &[field("token")], None).unwrap();
    store.ensure_template(&b, &[field("token")], None).unwrap();
    let path = store.local_edit_path(&a).unwrap();
    let base = std::fs::read_to_string(&path).unwrap();
    for text in [
        format!("{base}\nunknown = 'DO_NOT_LEAK'"),
        base.replace(&a, &b),
    ] {
        std::fs::write(&path, text).unwrap();
        let error = store.load_snapshot(&a).unwrap_err();
        assert!(!format!("{error:?}").contains("DO_NOT_LEAK"));
    }
    std::fs::write(
        &path,
        base.replace("value = \"\"", "value = \"only-task-a\""),
    )
    .unwrap();
    let sa = store.load_snapshot(&a).unwrap();
    let sb = store.load_snapshot(&b).unwrap();
    let r = sa.reference("token").unwrap();
    assert!(matches!(
        sb.resolve_for_approved_adapter(&b, &r),
        Err(StorageError::TaskMismatch)
    ));
    assert!(matches!(
        sa.resolve_for_approved_adapter(&b, &r),
        Err(StorageError::TaskMismatch)
    ));
    assert!(matches!(
        sb.reference("token"),
        Err(StorageError::SecretMissing)
    ));
    assert!(matches!(
        sb.reference("absent"),
        Err(StorageError::UnknownField)
    ));
}
#[test]
fn host_trust_requires_exact_single_use_confirmation_and_persists() {
    use base64::Engine;
    let f = Fixture::new();
    let store = KnownHostsStore::open(&f.path).unwrap();
    let one = format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode([1u8; 32])
    );
    let two = format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode([2u8; 32])
    );
    let challenge = match store.check("localhost", 22, &one).unwrap() {
        HostTrustCheck::ApprovalRequired {
            challenge_id,
            previous_fingerprint,
            ..
        } => {
            assert!(previous_fingerprint.is_none());
            challenge_id
        }
        _ => panic!("untrusted host accepted"),
    };
    assert!(store
        .accept_from_trust_ui(&challenge, "localhost", 23, &one)
        .is_err());
    assert!(store
        .accept_from_trust_ui(&challenge, "localhost", 22, &one)
        .is_err());
    let challenge = match store.check("localhost", 22, &one).unwrap() {
        HostTrustCheck::ApprovalRequired { challenge_id, .. } => challenge_id,
        _ => panic!(),
    };
    store
        .accept_from_trust_ui(&challenge, "localhost", 22, &one)
        .unwrap();
    assert!(matches!(
        KnownHostsStore::open(&f.path)
            .unwrap()
            .check("localhost", 22, &one),
        Ok(HostTrustCheck::Trusted)
    ));
    assert!(matches!(
        store.check("localhost", 23, &one),
        Ok(HostTrustCheck::ApprovalRequired { .. })
    ));
    match store.check("localhost", 22, &two).unwrap() {
        HostTrustCheck::ApprovalRequired {
            previous_fingerprint,
            ..
        } => assert_eq!(previous_fingerprint, Some(one)),
        _ => panic!("changed key accepted"),
    }
}
#[test]
fn credential_descriptors_bind_profile_and_purpose_and_debug_redacts() {
    let id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4();
    let d = StoredCredential {
        mode: CredentialMode::LinuxSecretService,
        encoded: format!("xyzaishell:v1:{id}:ssh_password:{token}"),
    };
    assert!(credentials::validate_descriptor(&id, CredentialPurpose::SshPassword, &d).is_ok());
    assert!(credentials::validate_descriptor(
        &Uuid::new_v4().to_string(),
        CredentialPurpose::SshPassword,
        &d
    )
    .is_err());
    assert!(
        credentials::validate_descriptor(&id, CredentialPurpose::SshKeyPassphrase, &d).is_err()
    );
    assert!(!format!("{d:?}").contains(&token.to_string()));
    let secret = SecretValue::new(b"synthetic-secret".to_vec()).unwrap();
    assert!(!format!("{secret:?}").contains("synthetic-secret"));
    let invalid = StoredCredential {
        mode: CredentialMode::DpapiCurrentUser,
        encoded: "dpapi:v1:cGxhaW50ZXh0".into(),
    };
    assert!(
        credentials::validate_descriptor(&id, CredentialPurpose::SshPassword, &invalid).is_err()
    );
}
#[cfg(target_os = "linux")]
#[test]
fn private_modes_symlinks_hardlinks_and_oversized_files_are_rejected() {
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    store.initialize(CredentialMode::SessionOnly).unwrap();
    let path = f.path.join("connections.toml");
    assert_eq!(std::fs::metadata(&f.path).unwrap().mode() & 0o777, 0o700);
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store.list(&session()).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, f.path.join("alias")).unwrap();
    assert!(store.list(&session()).is_err());
    std::fs::remove_file(f.path.join("alias")).unwrap();
    std::fs::rename(&path, f.path.join("other")).unwrap();
    symlink(f.path.join("other"), &path).unwrap();
    assert!(store.list(&session()).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(f.path.join("other"), &path).unwrap();
    std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(matches!(
        store.list(&session()),
        Err(StorageError::TooLarge)
    ));
}

#[cfg(windows)]
#[test]
fn native_dpapi_current_user_roundtrip_and_binding_failure() {
    let store = CredentialStore::new(CredentialMode::DpapiCurrentUser).unwrap();
    verify_native_profile_recovery(&store);
    let id = Uuid::new_v4().to_string();
    let value = SecretValue::new(b"synthetic-Windows-only-secret".to_vec()).unwrap();
    let descriptor = store
        .store(&id, CredentialPurpose::SshPassword, &value)
        .unwrap();
    assert!(!descriptor.encoded.contains("synthetic-Windows-only-secret"));
    assert_eq!(
        store
            .resolve(&id, CredentialPurpose::SshPassword, Some(&descriptor))
            .unwrap()
            .expose(),
        value.expose()
    );
    assert!(store
        .resolve(
            &Uuid::new_v4().to_string(),
            CredentialPurpose::SshPassword,
            Some(&descriptor)
        )
        .is_err());
    assert!(store
        .resolve(&id, CredentialPurpose::SshKeyPassphrase, Some(&descriptor))
        .is_err());
}
#[cfg(not(windows))]
#[test]
#[ignore = "Windows DPAPI runtime unavailable on this platform; mandatory on Windows CI"]
fn native_dpapi_current_user_roundtrip_and_binding_failure() {
    unreachable!();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "Requires explicit XYZ_TEST_ISOLATED_KEYRING=1 and a disposable D-Bus/keyring session; never run against a user's keyring"]
fn native_secret_service_isolated_roundtrip() {
    assert_eq!(
        std::env::var("XYZ_TEST_ISOLATED_KEYRING").as_deref(),
        Ok("1")
    );
    assert!(std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok());
    let store = CredentialStore::new(CredentialMode::LinuxSecretService).unwrap();
    verify_native_profile_recovery(&store);
    let id = Uuid::new_v4().to_string();
    let value = SecretValue::new(b"synthetic-disposable-keyring-secret".to_vec()).unwrap();
    let descriptor = store
        .store(&id, CredentialPurpose::SshPassword, &value)
        .unwrap();
    let loaded = store.resolve(&id, CredentialPurpose::SshPassword, Some(&descriptor));
    let removed = store.remove(&id, CredentialPurpose::SshPassword, Some(&descriptor));
    assert_eq!(loaded.unwrap().expose(), value.expose());
    removed.unwrap();
    assert!(matches!(
        store.resolve(&id, CredentialPurpose::SshPassword, Some(&descriptor)),
        Err(StorageError::CredentialMissing)
    ));
}
#[cfg(not(target_os = "linux"))]
#[test]
#[ignore = "Secret Service runtime unavailable on this platform; isolated Linux CI required"]
fn native_secret_service_isolated_roundtrip() {
    unreachable!();
}

#[cfg(target_os = "linux")]
#[test]
fn native_secret_service_unavailable_is_fail_closed() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "storage::tests::native_secret_service_unavailable_child",
            "--nocapture",
        ])
        .env("XYZ_TEST_MISSING_KEYRING_CHILD", "1")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/tmp/xyz-absent-dbus-{}", Uuid::new_v4()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "unavailable fail-closed child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("NATIVE_UNAVAILABLE_VERIFIED"));
}
#[cfg(target_os = "linux")]
#[test]
fn native_secret_service_unavailable_child() {
    if std::env::var("XYZ_TEST_MISSING_KEYRING_CHILD").as_deref() != Ok("1") {
        return;
    }
    let store = CredentialStore::new(CredentialMode::LinuxSecretService).unwrap();
    assert_eq!(
        store.status().credential_availability,
        CredentialAvailability::Unavailable
    );
    let id = Uuid::new_v4().to_string();
    let error = store
        .store(
            &id,
            CredentialPurpose::SshPassword,
            &SecretValue::new(b"synthetic-secret".to_vec()).unwrap(),
        )
        .unwrap_err();
    assert_eq!(error, StorageError::CredentialUnavailable);
    assert_eq!(store.mode(), CredentialMode::LinuxSecretService);
    assert!(!store.session_present(&id, CredentialPurpose::SshPassword));
    let f = Fixture::new();
    let profiles = ProfileStore::open(&f.path).unwrap();
    profiles
        .initialize(CredentialMode::LinuxSecretService)
        .unwrap();
    assert_eq!(
        profiles.save(
            0,
            draft(&id),
            CredentialChange::Set(SecretValue::new(b"synthetic-never-plaintext".to_vec()).unwrap()),
            &store
        ),
        Err(StorageError::CredentialUnavailable)
    );
    assert_eq!(profiles.list(&store).unwrap().revision, "0");
    assert!(profiles.list(&store).unwrap().connections.is_empty());
    assert!(!std::fs::read_to_string(f.path.join("connections.toml"))
        .unwrap()
        .contains("synthetic-never-plaintext"));
    println!("NATIVE_UNAVAILABLE_VERIFIED");
}
#[test]
fn profile_cross_process_revision_conflict_keeps_stable_lock() {
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    store.initialize(CredentialMode::SessionOnly).unwrap();
    let child = || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "storage::tests::profile_writer_child",
                "--nocapture",
            ])
            .env("XYZ_TEST_PROFILE_CHILD_ROOT", &f.path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    };
    let a = child();
    let b = child();
    let a = a.wait_with_output().unwrap();
    let b = b.wait_with_output().unwrap();
    assert!(a.status.success());
    assert!(b.status.success());
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&b.stdout)
    );
    assert_eq!(text.matches("SAVE_SUCCESS").count(), 1);
    assert_eq!(text.matches("SAVE_CONFLICT").count(), 1);
    let listing = store.list(&session()).unwrap();
    assert_eq!(listing.revision, "1");
    assert_eq!(listing.connections.len(), 1);
    assert!(f.path.join(".connections.toml.lock").is_file());
}
#[test]
fn profile_writer_child() {
    let Some(root) = std::env::var_os("XYZ_TEST_PROFILE_CHILD_ROOT") else {
        return;
    };
    let store = ProfileStore::open(&PathBuf::from(root)).unwrap();
    match store.save(
        0,
        draft(&Uuid::new_v4().to_string()),
        CredentialChange::Keep,
        &session(),
    ) {
        Ok(1) => println!("SAVE_SUCCESS"),
        Err(StorageError::Conflict) => println!("SAVE_CONFLICT"),
        result => panic!("unexpected synthetic save result: {result:?}"),
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "Requires explicit isolated disposable Secret Service; manipulates only its synthetic default collection"]
fn native_secret_service_isolated_locked_is_fail_closed() {
    assert_eq!(
        std::env::var("XYZ_TEST_ISOLATED_KEYRING").as_deref(),
        Ok("1")
    );
    assert!(std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok());
    let service =
        secret_service::blocking::SecretService::connect(secret_service::EncryptionType::Dh)
            .unwrap();
    let collection = service.get_default_collection().unwrap();
    collection.ensure_unlocked().unwrap();
    let store = CredentialStore::new(CredentialMode::LinuxSecretService).unwrap();
    let id = Uuid::new_v4().to_string();
    let descriptor = store
        .store(
            &id,
            CredentialPurpose::SshPassword,
            &SecretValue::new(b"synthetic-locked-test".to_vec()).unwrap(),
        )
        .unwrap();
    collection.lock().unwrap();
    assert_eq!(
        store.status().credential_availability,
        CredentialAvailability::Locked
    );
    assert!(matches!(
        store.resolve(&id, CredentialPurpose::SshPassword, Some(&descriptor)),
        Err(StorageError::CredentialLocked)
    ));
    assert!(matches!(
        store.store(
            &id,
            CredentialPurpose::SshPassword,
            &SecretValue::new(b"synthetic-no-fallback".to_vec()).unwrap()
        ),
        Err(StorageError::CredentialLocked)
    ));
    // The surrounding disposable session is destroyed by the test runner;
    // do not trigger an unlock prompt or modify a user's real collection.
}

#[test]
fn session_credential_clear_requires_explicit_session_operation() {
    let f = Fixture::new();
    let store = ProfileStore::open(&f.path).unwrap();
    let credentials = session();
    store.initialize(CredentialMode::SessionOnly).unwrap();
    let id = Uuid::new_v4().to_string();
    store
        .save(0, draft(&id), CredentialChange::Keep, &credentials)
        .unwrap();
    credentials
        .set_session(
            &id,
            CredentialPurpose::SshPassword,
            SecretValue::new(b"synthetic-to-clear".to_vec()).unwrap(),
        )
        .unwrap();
    assert_eq!(
        store.save(1, draft(&id), CredentialChange::Clear, &credentials),
        Err(StorageError::SessionCredentialUpdateRequired)
    );
    assert!(credentials.session_present(&id, CredentialPurpose::SshPassword));
    credentials
        .clear_session(&id, CredentialPurpose::SshPassword)
        .unwrap();
    assert!(!credentials.session_present(&id, CredentialPurpose::SshPassword));
    assert!(!store.list(&credentials).unwrap().connections[0].credential_present);
}

#[cfg(target_os = "linux")]
#[test]
fn persistent_profile_listing_omits_keyring_descriptors_and_rejects_transplants() {
    let f = Fixture::new();
    let id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    let reference = format!("xyzaishell:v1:{id}:ssh_password:{token}");
    let config=format!("schema_version=1\nrevision=7\n[storage]\nsecret_protection='linux_secret_service'\n[[connections]]\nid='{id}'\nlabel='synthetic'\nhost='localhost'\nusername='u'\n[connections.auth]\nkind='password'\npassword_secret_ref='{reference}'\n");
    edit(&f, "connections.toml", config.as_bytes());
    let store = ProfileStore::open(&f.path).unwrap();
    let credentials = CredentialStore::new(CredentialMode::LinuxSecretService).unwrap();
    let listed = serde_json::to_string(&store.list(&credentials).unwrap()).unwrap();
    assert!(listed.contains("\"credential_present\":true"));
    assert!(!listed.contains(&reference));
    assert!(!listed.contains(&token));
    assert!(!listed.contains("secret_ref"));
    let transplanted = config.replacen(
        &format!("id='{id}'"),
        &format!("id='{}'", Uuid::new_v4()),
        1,
    );
    edit(&f, "connections.toml", transplanted.as_bytes());
    assert!(matches!(
        store.list(&credentials),
        Err(StorageError::CredentialInvalid)
    ));
}
#[test]
fn task_schema_bounds_and_multiline_values_are_preserved() {
    let f = Fixture::new();
    let store = TaskCredentialStore::open(&f.path).unwrap();
    let id = Uuid::new_v4().to_string();
    store.ensure_template(&id, &[field("token")], None).unwrap();
    let path = store.local_edit_path(&id).unwrap();
    let base = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        base.replace(
            "value = \"\"",
            "value = \"\"\"line 1\nline $2 'quoted' \\\\ end\"\"\"",
        ),
    )
    .unwrap();
    let snapshot = store.load_snapshot(&id).unwrap();
    let reference = snapshot.reference("token").unwrap();
    assert_eq!(
        snapshot
            .resolve_for_approved_adapter(&id, &reference)
            .unwrap(),
        "line 1\nline $2 'quoted' \\ end"
    );
    std::fs::write(
        &path,
        base.replace(
            "value = \"\"",
            &format!("value = \"{}\"", "x".repeat(16385)),
        ),
    )
    .unwrap();
    assert!(matches!(
        store.load_snapshot(&id),
        Err(StorageError::TooLarge)
    ));
    let many: Vec<_> = (0..65).map(|n| field(&format!("key_{n}"))).collect();
    assert!(store
        .ensure_template(&Uuid::new_v4().to_string(), &many, None)
        .is_err());
}

#[test]
fn final_dispatch_snapshot_guard_rejects_reload_disk_edits_and_cross_task() {
    let f = Fixture::new();
    let store = TaskCredentialStore::open(&f.path).unwrap();
    let id = Uuid::new_v4().to_string();
    store.ensure_template(&id, &[field("token")], None).unwrap();
    let path = store.local_edit_path(&id).unwrap();
    let bytes = std::fs::read_to_string(&path)
        .unwrap()
        .replace("value = \"\"", "value = \"synthetic-original\"");
    std::fs::write(&path, &bytes).unwrap();
    let old = store.load_snapshot(&id).unwrap();
    let reference = old.reference("token").unwrap();
    store.validate_current_reference(&id, &reference).unwrap();
    assert!(matches!(
        store.validate_current_reference(&Uuid::new_v4().to_string(), &reference),
        Err(StorageError::TaskMismatch)
    ));
    std::fs::write(
        &path,
        bytes.replace("synthetic-original", "synthetic-changed"),
    )
    .unwrap();
    assert!(matches!(
        store.validate_current_reference(&id, &reference),
        Err(StorageError::SnapshotStale)
    ));
    let current = store.load_snapshot(&id).unwrap();
    assert!(matches!(
        store.validate_current_reference(&id, &reference),
        Err(StorageError::SnapshotStale)
    ));
    store
        .validate_current_snapshot(&id, &current.status().snapshot_id)
        .unwrap();
    store
        .validate_current_reference(&id, &current.reference("token").unwrap())
        .unwrap();
    // Immutability is not revocation: the host MUST use the guard above.
    assert_eq!(
        old.resolve_for_approved_adapter(&id, &reference).unwrap(),
        "synthetic-original"
    );
}

#[test]
fn database_bridge_distinguishes_creation_and_preserves_existing_contents() {
    let f = Fixture::new();
    let directory = private_files::PrivateDirectory::open_or_create(&f.path).unwrap();
    assert!(!directory
        .validate_database_file("history.sqlite3", 1024)
        .unwrap());
    assert!(directory
        .prepare_database_file("history.sqlite3", 1024)
        .unwrap());
    assert!(!directory
        .prepare_database_file("history.sqlite3", 1024)
        .unwrap());
    let path = f.path.join("history.sqlite3");
    std::fs::write(&path, b"synthetic database bytes").unwrap();
    let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3", 1024)
        .unwrap());
    assert!(!directory
        .prepare_database_file("history.sqlite3", 1024)
        .unwrap());
    assert!(matches!(
        directory.validate_database_file("history.sqlite3", 1),
        Err(StorageError::TooLarge)
    ));
    drop(writer);
    assert_eq!(std::fs::read(&path).unwrap(), b"synthetic database bytes");
    assert!(directory
        .validate_database_file("model.toml", 1024)
        .is_err());
    assert!(directory
        .prepare_database_file("history.sqlite3-journal", 1024)
        .is_err());
    assert!(!directory
        .validate_database_file("history.sqlite3-journal", 1024)
        .unwrap());
}
#[test]
fn database_bridge_concurrent_create_has_one_winner() {
    let f = Fixture::new();
    let directory = private_files::PrivateDirectory::open_or_create(&f.path).unwrap();
    let created = std::thread::scope(|scope| {
        let one = scope.spawn(|| directory.prepare_database_file("history.sqlite3", 1024));
        let two = scope.spawn(|| directory.prepare_database_file("history.sqlite3", 1024));
        [one.join().unwrap().unwrap(), two.join().unwrap().unwrap()]
    });
    assert_eq!(created.into_iter().filter(|v| *v).count(), 1);
}
#[cfg(target_os = "linux")]
#[test]
fn database_bridge_rejects_broad_links_and_nonfiles() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    let directory = private_files::PrivateDirectory::open_or_create(&f.path).unwrap();
    directory
        .prepare_database_file("history.sqlite3", 1024)
        .unwrap();
    let path = f.path.join("history.sqlite3");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3", 1024)
        .is_err());
    assert!(directory
        .prepare_database_file("history.sqlite3", 1024)
        .is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, f.path.join("history.sqlite3-journal")).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3", 1024)
        .is_err());
    std::fs::remove_file(f.path.join("history.sqlite3-journal")).unwrap();
    symlink(&path, f.path.join("history.sqlite3-journal")).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3-journal", 1024)
        .is_err());
    std::fs::create_dir(f.path.join("history.sqlite3-wal")).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3-wal", 1024)
        .is_err());
}
#[cfg(windows)]
#[test]
fn database_bridge_accepts_private_inherited_sqlite_sidecar_acl() {
    let f = Fixture::new();
    let directory = private_files::PrivateDirectory::open_or_create(&f.path).unwrap();
    let path = f.path.join("history.sqlite3-journal");
    // SQLite also creates its own sidecars using directory inheritance.
    std::fs::write(&path, b"synthetic journal").unwrap();
    let writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    assert!(directory
        .validate_database_file("history.sqlite3-journal", 1024)
        .unwrap());
    drop(writer);
    // Secret TOML reads still insist on explicit protection; only this metadata
    // bridge permits private inheritance for SQLite-managed files.
    assert!(directory.read("history.sqlite3-journal", 1024).is_err());
}

#[test]
fn legacy_session_migration_preserves_identity_and_can_use_native_backend_again() {
    let f = Fixture::new();
    let id = Uuid::new_v4().to_string();
    let old = format!("schema_version=1\nrevision=9\n[storage]\nsecret_protection='session_only'\n[[connections]]\nid='{id}'\nlabel='legacy label'\nhost='localhost'\nport=2200\nusername='legacy-user'\n[connections.auth]\nkind='password'\n");
    edit(&f, "connections.toml", old.as_bytes());
    let store = ProfileStore::open(&f.path).unwrap();
    store.migrate().unwrap();
    store.migrate().unwrap();
    let mode = if cfg!(windows) {
        CredentialMode::DpapiCurrentUser
    } else {
        CredentialMode::LinuxSecretService
    };
    let native = CredentialStore::new(mode).unwrap();
    let result = store.list(&native).unwrap();
    assert_eq!(result.revision, "10");
    assert_eq!(result.connections[0].id, id);
    assert_eq!(result.connections[0].label, "legacy label");
    assert_eq!(result.connections[0].port, 2200);
    assert_eq!(
        result.connections[0].credential_policy,
        CredentialPolicy::AskOnConnect
    );
    assert!(!result.connections[0].credential_present);
    assert_eq!(
        std::fs::read_to_string(f.path.join("connections.v1.backup.toml")).unwrap(),
        old
    );
    let text = std::fs::read_to_string(f.path.join("connections.toml")).unwrap();
    assert!(text.contains("schema_version = 2"));
    assert!(!text.contains("protected_password"));
    assert!(matches!(
        store.resolve_for_authentication(&id, 10, &native),
        Err(StorageError::CredentialMissing)
    ));
}

#[test]
fn migration_preserves_native_references_and_per_profile_policies_are_independent() {
    let f = Fixture::new();
    let id = Uuid::new_v4().to_string();
    let other = Uuid::new_v4().to_string();
    let reference = format!("xyzaishell:v1:{id}:ssh_password:{}", Uuid::new_v4());
    // A copied Linux profile remains readable even on Windows. Resolving the
    // foreign protected secret still fails closed at the native adapter.
    let old = format!("schema_version=1\nrevision=2\n[storage]\nsecret_protection='linux_secret_service'\n[[connections]]\nid='{id}'\nlabel='secure'\nhost='localhost'\nusername='user'\n[connections.auth]\nkind='password'\npassword_secret_ref='{reference}'\n[[connections]]\nid='{other}'\nlabel='ask'\nhost='localhost'\nusername='user'\n[connections.auth]\nkind='password'\n");
    edit(&f, "connections.toml", old.as_bytes());
    let store = ProfileStore::open(&f.path).unwrap();
    store.migrate().unwrap();
    let credentials = CredentialStore::new(if cfg!(windows) {
        CredentialMode::DpapiCurrentUser
    } else {
        CredentialMode::LinuxSecretService
    })
    .unwrap();
    let listing = store.list(&credentials).unwrap();
    assert_eq!(
        listing.connections[0].credential_policy,
        CredentialPolicy::SystemSecure
    );
    assert_eq!(
        listing.connections[1].credential_policy,
        CredentialPolicy::AskOnConnect
    );
    let mut renamed = store.draft_for_authentication(&other, 3).unwrap();
    renamed.label = "ask renamed".into();
    store
        .save(3, renamed, CredentialChange::Keep, &credentials)
        .unwrap();
    assert!(std::fs::read_to_string(f.path.join("connections.toml"))
        .unwrap()
        .contains(&reference));
    let mut changed = store.draft_for_authentication(&id, 4).unwrap();
    changed.host = "different.example".into();
    assert_eq!(
        store.save(4, changed, CredentialChange::Keep, &credentials),
        Err(StorageError::CredentialTargetChanged)
    );
    let original = store.draft_for_authentication(&id, 4).unwrap();
    store
        .save(4, original, CredentialChange::Clear, &credentials)
        .unwrap();
    let listing = store.list(&credentials).unwrap();
    assert!(listing
        .connections
        .iter()
        .all(|p| p.credential_policy == CredentialPolicy::AskOnConnect));
    assert_eq!(listing.connections[1].label, "ask renamed");
    assert!(
        std::fs::read_to_string(f.path.join("connections.v1.backup.toml"))
            .unwrap()
            .contains(&reference)
    );
}

#[test]
fn failed_migration_never_overwrites_the_only_legacy_file() {
    let f = Fixture::new();
    let old = "schema_version=1\nrevision=0\nconnections=[]\n[storage]\nsecret_protection='session_only'\n";
    edit(&f, "connections.toml", old.as_bytes());
    edit(
        &f,
        "connections.v1.backup.toml",
        b"different protected recovery data",
    );
    let store = ProfileStore::open(&f.path).unwrap();
    assert_eq!(store.migrate(), Err(StorageError::Conflict));
    assert_eq!(
        std::fs::read_to_string(f.path.join("connections.toml")).unwrap(),
        old
    );
    edit(
        &f,
        "connections.toml",
        old.replace("schema_version=1", "schema_version=99")
            .as_bytes(),
    );
    assert_eq!(store.migrate(), Err(StorageError::UnsupportedSchema));
}

#[test]
fn concurrent_migration_is_idempotent_and_keeps_the_exact_backup() {
    let f = Fixture::new();
    let old = "schema_version=1\nrevision=4\nconnections=[]\n[storage]\nsecret_protection='session_only'\n";
    edit(&f, "connections.toml", old.as_bytes());
    let run = || {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "storage::tests::migration_child", "--nocapture"])
            .env("XYZ_TEST_MIGRATION_ROOT", &f.path)
            .spawn()
            .unwrap()
    };
    let mut a = run();
    let mut b = run();
    assert!(a.wait().unwrap().success());
    assert!(b.wait().unwrap().success());
    assert_eq!(
        ProfileStore::open(&f.path)
            .unwrap()
            .list(&session())
            .unwrap()
            .revision,
        "5"
    );
    assert_eq!(
        std::fs::read_to_string(f.path.join("connections.v1.backup.toml")).unwrap(),
        old
    );
}
#[test]
fn migration_child() {
    let Some(root) = std::env::var_os("XYZ_TEST_MIGRATION_ROOT") else {
        return;
    };
    ProfileStore::open(&PathBuf::from(root))
        .unwrap()
        .migrate()
        .unwrap();
}

fn verify_native_profile_recovery(store: &CredentialStore) {
    let f = Fixture::new();
    let id = Uuid::new_v4().to_string();
    let other = Uuid::new_v4().to_string();
    let old = format!("schema_version=1\nrevision=0\n[storage]\nsecret_protection='session_only'\n[[connections]]\nid='{id}'\nlabel='recover'\nhost='localhost'\nusername='user'\n[connections.auth]\nkind='password'\n[[connections]]\nid='{other}'\nlabel='ask'\nhost='localhost'\nusername='user'\n[connections.auth]\nkind='password'\n");
    edit(&f, "connections.toml", old.as_bytes());
    let profiles = ProfileStore::open(&f.path).unwrap();
    profiles.migrate().unwrap();
    let first = profiles.draft_for_authentication(&id, 1).unwrap();
    profiles
        .save(
            1,
            first,
            CredentialChange::Set(
                SecretValue::new(b"synthetic-recovered-secret".to_vec()).unwrap(),
            ),
            store,
        )
        .unwrap();
    let listing = profiles.list(store).unwrap();
    assert_eq!(
        listing.connections[0].credential_policy,
        CredentialPolicy::SystemSecure
    );
    assert_eq!(
        listing.connections[1].credential_policy,
        CredentialPolicy::AskOnConnect
    );
    assert_eq!(
        profiles
            .resolve_for_authentication(&id, 2, store)
            .unwrap()
            .credential
            .unwrap()
            .expose(),
        b"synthetic-recovered-secret"
    );
    let mut rename = profiles.draft_for_authentication(&other, 2).unwrap();
    rename.label = "still ask".into();
    profiles
        .save(2, rename, CredentialChange::Keep, store)
        .unwrap();
    assert_eq!(
        profiles
            .resolve_for_authentication(&id, 3, store)
            .unwrap()
            .credential
            .unwrap()
            .expose(),
        b"synthetic-recovered-secret"
    );
    let disk = std::fs::read_to_string(f.path.join("connections.toml")).unwrap();
    assert!(!disk.contains("synthetic-recovered-secret"));
    let document: toml::Value = toml::from_str(&disk).unwrap();
    let field = if store.mode() == CredentialMode::DpapiCurrentUser {
        "protected_password"
    } else {
        "password_secret_ref"
    };
    let descriptor = StoredCredential {
        mode: store.mode(),
        encoded: document["connections"][0]["auth"][field]
            .as_str()
            .unwrap()
            .to_owned(),
    };
    let first = profiles.draft_for_authentication(&id, 3).unwrap();
    profiles
        .save(3, first, CredentialChange::Clear, store)
        .unwrap();
    assert!(profiles
        .list(store)
        .unwrap()
        .connections
        .iter()
        .all(|p| p.credential_policy == CredentialPolicy::AskOnConnect));
    // Only this isolated test-created entry is removed after its reference was cleared.
    store
        .remove(&id, CredentialPurpose::SshPassword, Some(&descriptor))
        .unwrap();
}
