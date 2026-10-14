//! Synthetic fixture generator for tests/shell_log_pty.py, not an SSH executor.
fn main() {
    print!(
        "{}",
        xyz_core::shell_log_protocol::bootstrap_command("0123456789abcdef0123456789abcdef")
            .expect("fixed synthetic nonce")
    );
}
