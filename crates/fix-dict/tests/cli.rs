//! Tests of the `fixsim-profile` command.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn profile_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/profiles")
        .join(name)
}

fn run(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_fixsim-profile"))
        .args(args)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn a_valid_profile_prints_the_group_tree() {
    let path = profile_path("valid.toml");
    let (code, out, err) = run(&["check", path.to_str().unwrap()]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(out.contains("message AB (NewOrderMultileg)"));
    assert!(out.contains("    group NoLegSecurityAltID(604)"));
}

#[test]
fn an_invalid_profile_prints_each_error_with_its_place_and_exits_with_1() {
    let path = profile_path("bad_default.toml");
    let path = path.to_str().unwrap();
    let (code, out, err) = run(&["check", path]);
    assert_eq!(code, 1);
    assert_eq!(out, "");
    assert!(err.contains(&format!(
        "{path}:4:13: the default \"9\" is not a valid value of tag 21"
    )));
    assert!(err.contains(&format!("{path}:6:9: ")));
    assert!(err.ends_with("2 error(s)\n"));
}

#[test]
fn a_wrong_command_line_or_a_missing_file_exits_with_2() {
    assert_eq!(run(&[]).0, 2);
    assert_eq!(run(&["verify", "x.toml"]).0, 2);
    let (code, _, err) = run(&["check", "/nonexistent/profile.toml"]);
    assert_eq!(code, 2);
    assert!(err.contains("cannot read"));
}
