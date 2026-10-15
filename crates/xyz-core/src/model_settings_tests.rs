#![cfg(target_os = "linux")]

use super::*;
#[cfg(target_os = "linux")]
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

#[cfg(target_os = "linux")]
struct Fixture {
    dir: PathBuf,
    path: PathBuf,
}
#[cfg(target_os = "linux")]
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("xyz-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            path: dir.join("model.toml"),
            dir,
        }
    }
    fn write(&self, text: &str) {
        fs::write(&self.path, text).unwrap();
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn manager(&self) -> ModelConfigManager {
        ModelConfigManager::from_path(Ok(self.path.clone()))
    }
}
#[cfg(target_os = "linux")]
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
fn request(manager: &ModelConfigManager, key: serde_json::Value) -> SaveModelConfigRequest {
    serde_json::from_value(serde_json::json!({"expected_file_revision":manager.status().file_revision,"base_url":"https://example.com/v1", "model":"new-model","key_update":key,"reuse_key_for_destination":null})).unwrap()
}
fn text() -> String {
    "schema_version=1\n[model]\nprotocol='openai_chat_completions'\nbase_url='https://example.com/v1'\napi_key='synthetic-private-key'\nname='old-model'\n[request]\nconnect_timeout_seconds=23\nrequest_timeout_seconds=142\nstream_idle_timeout_seconds=42\nmax_output_tokens=777\n[transport]\nallow_loopback_http=true\n".into()
}
#[cfg(target_os = "linux")]
#[test]
fn saves_three_fields_preserves_advanced_and_never_returns_key_or_hash() {
    let fixture = Fixture::new();
    fixture.write(&text());
    let manager = fixture.manager();
    let before = manager.status();
    let saved = manager
        .save(request(&manager, serde_json::json!({"kind":"keep"})))
        .unwrap();
    assert!(saved.ready && saved.key_present && !saved.consented);
    assert_ne!(before.file_revision, saved.file_revision);
    let raw: ConfigFile = toml::from_str(&fs::read_to_string(&fixture.path).unwrap()).unwrap();
    assert_eq!(raw.model.api_key, "synthetic-private-key");
    assert_eq!(raw.model.name, "new-model");
    assert_eq!(raw.request.connect_timeout_seconds, 23);
    assert_eq!(raw.request.max_output_tokens, 777);
    assert!(raw.transport.allow_loopback_http);
    let public = serde_json::to_string(&saved).unwrap();
    assert!(!public.contains("synthetic-private-key"));
    assert!(uuid::Uuid::parse_str(saved.file_revision.as_deref().unwrap()).is_ok());
    assert_eq!(
        fs::metadata(&fixture.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
#[cfg(target_os = "linux")]
#[test]
fn initial_save_and_explicit_clear_make_key_state_unambiguous() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    assert!(manager.status().file_revision.is_some());
    assert!(!manager.status().key_present);
    let kept = manager
        .save(request(&manager, serde_json::json!({"kind":"keep"})))
        .unwrap();
    assert!(!kept.ready);
    assert!(!kept.key_present);
    let replaced = manager
        .save(request(
            &manager,
            serde_json::json!({"kind":"replace","value":"synthetic-new-key"}),
        ))
        .unwrap();
    assert!(replaced.ready);
    manager
        .confirm_destination(
            replaced.revision.as_deref().unwrap(),
            replaced.destination.as_deref().unwrap(),
        )
        .unwrap();
    let cleared = manager
        .save(request(&manager, serde_json::json!({"kind":"clear"})))
        .unwrap();
    assert!(!cleared.ready && !cleared.key_present && !cleared.consented);
    assert!(manager.authorized_snapshot().is_err());
    assert!(!fs::read_to_string(&fixture.path)
        .unwrap()
        .contains("synthetic-new-key"));
    assert!(!manager.reload().ready);
}
#[cfg(target_os = "linux")]
#[test]
fn changing_destination_requires_bound_explicit_key_reuse() {
    let fixture = Fixture::new();
    fixture.write(&text());
    let manager = fixture.manager();
    let mut change = request(&manager, serde_json::json!({"kind":"keep"}));
    change.base_url = "https://other.example/v1/".into();
    assert_eq!(
        manager.save(change).unwrap_err().code,
        "MODEL_KEY_REUSE_REQUIRED"
    );
    let mut change = request(&manager, serde_json::json!({"kind":"keep"}));
    change.base_url = "https://other.example/v1/".into();
    change.reuse_key_for_destination = Some("https://other.example/v1".into());
    let saved = manager.save(change).unwrap();
    assert_eq!(
        saved.destination.as_deref(),
        Some("https://other.example/v1")
    );
    assert!(!saved.consented);
}
#[cfg(target_os = "linux")]
#[test]
fn external_edits_and_other_instances_cannot_be_silently_overwritten() {
    let fixture = Fixture::new();
    fixture.write(&text());
    let first = fixture.manager();
    let second = fixture.manager();
    let stale = request(&second, serde_json::json!({"kind":"keep"}));
    first
        .save(request(&first, serde_json::json!({"kind":"clear"})))
        .unwrap();
    assert_eq!(second.save(stale).unwrap_err().code, "CONFIG_CONFLICT");
    assert!(second.status().file_revision.is_none());
    second.reload();
    let stale = request(&second, serde_json::json!({"kind":"keep"}));
    fixture.write(&(text() + "\n# external editor change\n"));
    assert_eq!(second.save(stale).unwrap_err().code, "CONFIG_CONFLICT");
    assert!(fs::read_to_string(&fixture.path)
        .unwrap()
        .contains("external editor"));
}
#[cfg(target_os = "linux")]
#[test]
fn corrupt_unknown_or_unsafe_files_remain_untouched() {
    let fixture = Fixture::new();
    for bad in [
        text().replace("schema_version=1", "schema_version=2"),
        text() + "unknown_field='synthetic-secret'\n",
        "bad='synthetic-private-key".into(),
    ] {
        fixture.write(&bad);
        let manager = fixture.manager();
        assert!(manager.status().file_revision.is_none());
        assert!(!manager.status().ready);
        let raw: SaveModelConfigRequest=serde_json::from_value(serde_json::json!({"expected_file_revision":"bad","base_url":"https://example.com","model":"m","key_update":{"kind":"clear"},"reuse_key_for_destination":null})).unwrap();
        assert!(manager.save(raw).is_err());
        assert_eq!(fs::read_to_string(&fixture.path).unwrap(), bad);
        assert!(!serde_json::to_string(&manager.status())
            .unwrap()
            .contains("synthetic-private-key"));
    }
    fixture.write(&text());
    fs::set_permissions(&fixture.path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!fixture.manager().status().ready);
}
#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_requires_exact_saved_version_and_confirmation_without_network() {
    let fixture = Fixture::new();
    fixture.write(&text());
    let manager = fixture.manager();
    let status = manager.status();
    let base = ModelTestRequest {
        test_id: uuid::Uuid::new_v4().to_string(),
        file_revision: status.file_revision.unwrap(),
        destination: status.destination.unwrap(),
        model: status.model.unwrap(),
        kind: ModelTestKind::ChatConnectivity,
        consent: false,
    };
    assert_eq!(
        manager.test_saved(base.clone()).await.unwrap_err().code,
        "MODEL_TEST_CONFIRMATION_REQUIRED"
    );
    let mut stale = base.clone();
    stale.consent = true;
    stale.file_revision = uuid::Uuid::new_v4().to_string();
    assert!(manager.test_saved(stale).await.is_err());
    let mut changed = base;
    changed.consent = true;
    fixture.write(&(text() + "\n# external edit\n"));
    assert_eq!(
        manager.test_saved(changed).await.unwrap_err().code,
        "CONFIG_CONFLICT"
    );
}

#[test]
fn diagnostic_kind_defaults_to_chat_and_unknown_kinds_are_rejected() {
    let base = serde_json::json!({
        "test_id":uuid::Uuid::new_v4().to_string(),
        "file_revision":"revision",
        "destination":"https://example.invalid/v1",
        "model":"model",
        "consent":true
    });
    let legacy: ModelTestRequest = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(legacy.kind, ModelTestKind::ChatConnectivity);
    let mut task = base.clone();
    task["kind"] = serde_json::json!("task_capability");
    assert_eq!(
        serde_json::from_value::<ModelTestRequest>(task)
            .unwrap()
            .kind,
        ModelTestKind::TaskCapability
    );
    let mut invalid = base;
    invalid["kind"] = serde_json::json!("execute_terminal");
    assert!(serde_json::from_value::<ModelTestRequest>(invalid).is_err());
}
