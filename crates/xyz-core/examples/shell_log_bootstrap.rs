//! Synthetic fixture generator for tests/shell_log_pty.py, not an SSH executor.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--envelope") {
        let sequence: u64 = args[3].parse().expect("sequence");
        let session = xyz_core::ssh::SessionRef {
            session_id: "fixture-session".into(),
            generation: "1".into(),
        };
        let ready = xyz_core::shell_integration::ReadyBinding {
            shell_id: "0123456789abcdef0123456789abcdef".into(),
            sequence: sequence - 1,
            revision: 1,
            shadowed_commands: vec![],
        };
        print!(
            "{}",
            xyz_core::shell_integration::action_envelope(&session, &args[2], &args[4], &ready)
                .expect("valid envelope")
        );
        return;
    }
    if std::env::args().any(|arg| arg == "--startup") {
        print!(
            "{}",
            xyz_core::shell_log_protocol::startup_command("0123456789abcdef0123456789abcdef")
                .expect("fixed synthetic nonce")
        );
        return;
    }
    if std::env::args().any(|arg| arg == "--legacy") {
        print!(
            "{}",
            xyz_core::shell_log_protocol::legacy_bootstrap_command(
                "0123456789abcdef0123456789abcdef"
            )
            .expect("fixed synthetic nonce")
        );
        return;
    }
    print!(
        "{}",
        xyz_core::shell_log_protocol::bootstrap_command("0123456789abcdef0123456789abcdef")
            .expect("fixed synthetic nonce")
    );
}
