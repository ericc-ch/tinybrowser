//! Subcommand selection and flag parsing at the binary boundary.

use crate::fixture::Fixture;

fn run(fixture: &Fixture, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_tinybrowser"))
        .args(args)
        .env("XDG_RUNTIME_DIR", fixture.runtime())
        .env("XDG_DATA_HOME", fixture.data())
        .output()
        .expect("run")
}

fn commands_table(help: &str) -> &str {
    let rest = help.split("Commands:\n").nth(1).expect("Commands heading");
    rest.split("\n\n").next().unwrap_or(rest)
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
fn version_is_global() {
    let fixture = Fixture::new("tinybrowser-version-global");
    for args in [
        &["daemon", "-v"][..],
        &["renderer", "--version"],
        &["webdriver", "-v"],
    ] {
        let output = run(&fixture, args);
        assert!(output.status.success(), "{args:?}");
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(out.contains("tinybrowser"), "{args:?}: {out}");
        assert!(out.contains("0.1.0"), "{args:?}: {out}");
    }
}

#[test]
fn rejects_unknown_log_level() {
    let fixture = Fixture::new("tinybrowser-log-level");
    let output = run(&fixture, &["--log-level=noisy"]);
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unknown log level"), "{err}");
}

#[test]
fn help_lists_process_commands() {
    let fixture = Fixture::new("tinybrowser-help");
    let output = run(&fixture, &["--help"]);
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    let commands = commands_table(&out);
    assert!(commands.contains("  daemon"), "{commands}");
    assert!(commands.contains("  renderer"), "{commands}");
    assert!(commands.contains("  webdriver"), "{commands}");
}

#[test]
fn no_command_prints_help() {
    let fixture = Fixture::new("tinybrowser-no-command");
    let output = run(&fixture, &[]);
    assert_eq!(output.status.code(), Some(2));
    let out = String::from_utf8_lossy(&output.stdout);
    let commands = commands_table(&out);
    assert!(commands.contains("  daemon"), "{commands}");
    assert!(commands.contains("  renderer"), "{commands}");
    assert!(commands.contains("  webdriver"), "{commands}");
}

#[test]
fn old_mode_flags_are_rejected() {
    let fixture = Fixture::new("tinybrowser-old-flags");
    for args in [&["--daemon"][..], &["--renderer"], &["--webdriver=9"]] {
        let output = run(&fixture, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn double_dash_rejects_positionals() {
    let fixture = Fixture::new("tinybrowser-double-dash");
    for args in [&["--", "daemon"][..], &["--", "nonsense"]] {
        let output = run(&fixture, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn repeated_global_flags_are_rejected() {
    let fixture = Fixture::new("tinybrowser-repeated-flags");
    for args in [
        &["--verbose", "--verbose", "daemon"][..],
        &["--log-level=info", "--log-level=debug", "daemon"],
    ] {
        let output = run(&fixture, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn help_with_unknown_subcommand_is_rejected() {
    let fixture = Fixture::new("tinybrowser-help-bogus");
    let output = run(&fixture, &["help", "bogus"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn help_with_known_subcommand_prints_help() {
    let fixture = Fixture::new("tinybrowser-help-known");
    for args in [&["help"][..], &["help", "webdriver"], &["daemon", "--help"]] {
        let output = run(&fixture, args);
        assert!(output.status.success(), "{args:?}");
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(out.contains("daemon"), "{args:?}: {out}");
    }
}

#[test]
fn webdriver_requires_port() {
    let fixture = Fixture::new("tinybrowser-webdriver-port");
    let output = run(&fixture, &["webdriver"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn webdriver_rejects_an_invalid_proxy_before_starting() {
    let fixture = Fixture::new("tinybrowser-webdriver-proxy");
    let output = run(
        &fixture,
        &["webdriver", "--port=0", "--proxy=http://proxy.test/path"],
    );
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("proxy URI"), "{err}");
}

#[test]
fn webdriver_rejects_an_invalid_resolve_spec_before_starting() {
    let fixture = Fixture::new("tinybrowser-webdriver-resolve");
    let output = run(&fixture, &["webdriver", "--port=0", "--resolve=not-a-spec"]);
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("resolve spec"), "{err}");
}

#[test]
fn webdriver_rejects_an_unparseable_tls_ca_before_starting() {
    let fixture = Fixture::new("tinybrowser-webdriver-tls-ca");
    let ca = fixture.data().join("not-a-cert.pem");
    std::fs::write(&ca, b"not a certificate").expect("write CA");
    let output = run(
        &fixture,
        &[
            "webdriver",
            "--port=0",
            &format!("--tls-ca={}", ca.display()),
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("--tls-ca"), "{err}");
}

#[test]
fn webdriver_rejects_a_missing_tls_ca_file_before_starting() {
    let fixture = Fixture::new("tinybrowser-webdriver-tls-ca-missing");
    let output = run(
        &fixture,
        &["webdriver", "--port=0", "--tls-ca=/nonexistent"],
    );
    assert_eq!(output.status.code(), Some(2));
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("--tls-ca"), "{err}");
}

#[test]
fn profile_and_resolve_stay_on_their_commands() {
    let fixture = Fixture::new("tinybrowser-flag-placement");
    for args in [
        &["--profile=default"][..],
        &["--profile=default", "daemon"],
        &["renderer", "--profile=default"],
        &["renderer", "--resolve=*.test=127.0.0.1"],
        &["--resolve=*.test=127.0.0.1"],
        &["--proxy=http://127.0.0.1:1"],
        &["renderer", "--proxy=http://127.0.0.1:1"],
        &["daemon", "--webdriver=9"],
        &["daemon", "--resolve=*.test=127.0.0.1"],
        &["daemon", "--proxy=http://127.0.0.1:1"],
        &["daemon", "--port=9"],
    ] {
        let output = run(&fixture, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn log_file_flag_names_the_log_file() {
    let fixture = Fixture::new("tinybrowser-log-file");
    let path = fixture.data().join("explicit.log");
    // The logger is installed before the command runs, so the file's existence
    // pins that --log-file reached `logging::install`. The exit code is the
    // no-command help path, which `no_command_prints_help` already asserts.
    run(
        &fixture,
        &["--log-file", path.to_str().expect("utf-8 path")],
    );
    assert!(path.is_file(), "no log file at {path:?}");
}

#[test]
fn log_file_rejects_missing_and_repeated_values() {
    let fixture = Fixture::new("tinybrowser-log-file-bad");
    for args in [
        &["--log-file"][..],
        &["--log-file="],
        &["--log-file=a.log", "--log-file=b.log"],
    ] {
        let output = run(&fixture, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}
