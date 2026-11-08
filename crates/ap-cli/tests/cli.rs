//! End-to-end tests of the `agentphone` binary that need no phone.

use assert_cmd::Command;
use predicates::prelude::*;

fn agentphone(cfg: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("agentphone").unwrap();
    c.env("AGENTPHONE_CONFIG", cfg)
        .env_remove("AGENTPHONE_DEVICE")
        .env("NO_COLOR", "1");
    c
}

#[test]
fn version_and_help() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    agentphone(&cfg)
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("agentphone "));
    agentphone(&cfg)
        .args(["tap", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--px").and(predicate::str::contains("0.5 0.5")));
    agentphone(&cfg)
        .args(["selftest", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--yes").and(predicate::str::contains("--out")));
    // No arguments and no terminal: print help, never start the wizard.
    agentphone(&cfg)
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: agentphone"));
    assert!(!cfg.exists());
}

#[test]
fn config_set_and_show_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("sub").join("config.toml");
    agentphone(&cfg)
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config.toml"));
    agentphone(&cfg)
        .args(["config", "set", "llm.model", "claude-sonnet-5-5"])
        .assert()
        .success();
    agentphone(&cfg)
        .args(["config", "set", "safety.max_steps", "20"])
        .assert()
        .success();
    let out = agentphone(&cfg)
        .args(["config", "show", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["llm"]["model"], "claude-sonnet-5-5");
    assert_eq!(v["safety"]["max_steps"], 20);
    assert_eq!(v["safety"]["confirm_irreversible"], true);
    agentphone(&cfg)
        .args(["config", "show"])
        .assert()
        .success()
        .stdout(predicate::str::contains("model = \"claude-sonnet-5-5\""));
}

#[test]
fn friendly_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    agentphone(&cfg)
        .args(["config", "set", "llm.api_key", "sk-secret"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("Error:")
                .and(predicate::str::contains("environment variable"))
                .and(predicate::str::contains("Setup {").not()),
        );
    agentphone(&cfg)
        .args(["config", "set", "safety.max_steps", "lots"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("whole number"));
    agentphone(&cfg)
        .args(["setup"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("interactive terminal"));
}

#[test]
fn completions() {
    let dir = tempfile::tempdir().unwrap();
    agentphone(&dir.path().join("c.toml"))
        .args(["completions", "powershell"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agentphone"));
}
