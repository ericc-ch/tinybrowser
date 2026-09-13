//! Mode selection and flag parsing at the binary boundary.

mod common;

use common::Fixture;

fn run(fixture: &Fixture, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", &fixture.runtime)
        .env("XDG_DATA_HOME", &fixture.data)
        .output()
        .expect("run")
}

#[test]
fn short_v_prints_the_version() {
    let fixture = Fixture::new("tinybrowser-version");
    let output = run(&fixture, &["-v"]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("tinybrowser"), "{out}");
    assert!(out.contains("0.1.0"), "{out}");
}

#[test]
fn rejects_unknown_log_level() {
    let fixture = Fixture::new("tinybrowser-log-level");
    let output = run(&fixture, &["--log-level=noisy"]);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unknown log level"), "{err}");
}

#[test]
fn daemon_rejects_webdriver_and_resolve() {
    let fixture = Fixture::new("tinybrowser-mode-flags");

    let webdriver = run(&fixture, &["--daemon", "--webdriver=9"]);
    assert!(!webdriver.status.success());
    let err = String::from_utf8_lossy(&webdriver.stderr);
    assert!(
        err.contains("--daemon and --webdriver are mutually exclusive"),
        "{err}"
    );

    let resolve = run(&fixture, &["--daemon", "--resolve=*.test=127.0.0.1"]);
    assert!(!resolve.status.success());
    let err = String::from_utf8_lossy(&resolve.stderr);
    assert!(
        err.contains("--daemon and --resolve are mutually exclusive"),
        "{err}"
    );
}
