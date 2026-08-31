// Regression test: snapshot of `ct watch --help` to prevent accidental
// removal of flags. Asserts all four flags from spec §21 are listed:
//   --once, --json, --once-timeout, --foreground
// (--foreground is a no-op in v1 but exists for spec parity + future
// systemd-style background mode.)

#[test]
fn ct_watch_help_lists_all_flags() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ct"))
        .args(["watch", "--help"])
        .output()
        .expect("failed to run ct watch --help");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--once"), "--once flag missing from --help");
    assert!(stdout.contains("--json"), "--json flag missing from --help");
    assert!(
        stdout.contains("--once-timeout"),
        "--once-timeout flag missing from --help"
    );
    assert!(
        stdout.contains("--foreground"),
        "--foreground flag missing from --help"
    );
}
