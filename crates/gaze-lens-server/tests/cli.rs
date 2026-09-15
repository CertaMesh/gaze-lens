use std::process::Command;
#[test]
fn help_is_a_flag_not_a_third_command() {
    let help = Command::new(env!("CARGO_BIN_EXE_gaze-lens-server"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("serve"));
    assert!(text.contains("check"));
    let extra = Command::new(env!("CARGO_BIN_EXE_gaze-lens-server"))
        .arg("help")
        .output()
        .unwrap();
    assert!(!extra.status.success());
}
