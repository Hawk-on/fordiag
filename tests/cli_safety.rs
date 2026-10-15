//! End-to-end safety tests against the compiled `fordiag` binary.
//!
//! * A full simulated session (connect, all battery tests, DTC scans on both
//!   buses, report) is run with `--mock`, and every command that was actually
//!   sent, as recorded in the raw logs, is checked against the allowlist with
//!   `fordiag check`.
//! * Without `--yes` and without keyboard input, no measurement starts and
//!   nothing is sent.
//! * A tampered raw log cannot smuggle a command in through `--replay`.
//! * `fordiag check` rejects every forbidden service.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_fordiag"))
}

/// A fresh, empty data directory for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fordiag-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(dir: &Path, args: &[&str]) -> Output {
    bin()
        .arg("--data-dir")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn assert_ok(out: &Output, what: &str) {
    assert!(
        out.status.success(),
        "{what} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Every `tx` entry in all raw logs under `dir`.
fn sent_commands(dir: &Path) -> Vec<String> {
    let logs = dir.join("logs");
    let Ok(entries) = std::fs::read_dir(&logs) else {
        return vec![];
    };
    let mut sent = Vec::new();
    for e in entries {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        for line in text.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            if v["dir"] == "tx" {
                sent.push(v["data"].as_str().unwrap().to_string());
            }
        }
    }
    sent
}

const FORBIDDEN: [&str; 16] = [
    "04", "08", "10", "11", "14", "27", "28", "2E", "2F", "31", "34", "35", "36", "37", "3D", "85",
];

#[test]
fn full_mock_session_sends_only_allowed_commands() {
    let dir = temp_dir("session");
    let d = &dir;
    for args in [
        vec!["--mock", "connect", "--probe"],
        vec!["--mock", "connect", "--bus", "ms"],
        vec!["--mock", "battery", "--rest", "--seconds", "2"],
        vec!["--mock", "battery", "--crank", "--seconds", "2"],
        vec!["--mock", "battery", "--charging", "--seconds", "2"],
        vec!["--mock", "dtc", "--bus", "hs"],
        vec!["--mock", "dtc", "--bus", "ms"],
        vec!["battery", "--summary"],
        vec!["report", "--vehicle", "test"],
    ] {
        assert_ok(&run(d, &args), &args.join(" "));
    }

    let sent = sent_commands(d);
    assert!(sent.len() > 50, "expected many commands, got {}", sent.len());
    let unique: BTreeSet<&String> = sent.iter().collect();
    for wire in &unique {
        assert!(!wire.is_empty(), "an empty line was sent");
        assert!(
            wire.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "unexpected characters in \"{wire}\""
        );
        if !wire.starts_with("AT") {
            assert!(
                !FORBIDDEN.iter().any(|f| wire.starts_with(f)),
                "forbidden service sent: {wire}"
            );
        }
        // The binary's own check must agree that this command is allowed.
        let out = bin().args(["check", wire]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(
            text.starts_with("ALLOWED"),
            "\"{wire}\" was sent but is not allowed: {text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_is_sent_without_yes_and_without_keyboard() {
    let dir = temp_dir("noyes");
    for args in [
        vec!["battery", "--rest"],
        vec!["battery", "--crank"],
        vec!["battery", "--charging"],
        vec!["dtc", "--bus", "hs"],
        vec!["dtc", "--bus", "ms"],
        vec!["connect", "--bus", "ms"],
    ] {
        // A port that does not exist: if fordiag tried to open it, the error
        // would be different from the one we expect.
        let mut full = vec!["--port", "/dev/fordiag-test-no-such-port"];
        full.extend(&args);
        let out = run(&dir, &full);
        assert!(!out.status.success(), "{args:?} should stop without keyboard input");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("no keyboard input"), "{args:?}: unexpected error: {err}");
    }
    assert!(sent_commands(&dir).is_empty(), "nothing may be sent");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tampered_log_cannot_smuggle_commands_through_replay() {
    let dir = temp_dir("tamper");
    // A log whose very first "recorded" command is ClearDiagnosticInformation.
    let log = dir.join("evil.jsonl");
    std::fs::write(
        &log,
        concat!(
            "{\"ts\":\"t\",\"dir\":\"tx\",\"data\":\"14FFFFFF\"}\n",
            "{\"ts\":\"t\",\"dir\":\"rx\",\"data\":\"54\\r\\r\"}\n",
            "{\"ts\":\"t\",\"dir\":\"tx\",\"data\":\"ATZ\"}\n",
            "{\"ts\":\"t\",\"dir\":\"rx\",\"data\":\"ELM327 v1.5\\r\\r\"}\n",
        ),
    )
    .unwrap();
    let out = run(&dir, &["--replay", log.to_str().unwrap(), "-y", "dtc", "--bus", "hs"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("diverged"), "unexpected error: {err}");
    // fordiag only tried its own first command (ATZ); the recorded 14 was never used.
    assert_eq!(sent_commands(&dir), vec!["ATZ".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_rejects_every_forbidden_service() {
    for s in FORBIDDEN {
        for args in ["", " 00", " 01", " FF FF FF", " 01 02 03 04 05 06 07"] {
            let text = format!("{s}{args}");
            let out = bin().args(["check", &text]).output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(stdout.starts_with("REJECTED"), "\"{text}\": {stdout}");
        }
    }
    for text in ["ATCAF0", "ATMA", "ATPP2CSV81", "ATSH123", "ATZ\r14", "01 00\n04"] {
        let out = bin().args(["check", text]).output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with("REJECTED"), "{text:?}: {stdout}");
    }
}

#[test]
fn mock_and_replay_cannot_be_combined() {
    let out = bin()
        .args(["--mock", "--replay", "x.jsonl", "connect"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn help_shows_the_disclaimer() {
    let out = bin().arg("--help").output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("DISCLAIMER: Use at your own risk"));
}
