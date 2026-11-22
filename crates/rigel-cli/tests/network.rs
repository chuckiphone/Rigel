//! `rigel network`: the same policy table the window edits, through the real CLI entry point.
//!
//! The test starts no server. `check` must explain the decision without contacting anything, so a
//! wrong answer is a policy bug, not a network flake.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn rigel(data: &Path, args: &[&str]) -> (i32, String) {
    let exe = env!("CARGO_BIN_EXE_rigel");
    let output = Command::new(exe)
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .expect("rigel runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[tokio::test]
async fn set_show_and_check_share_one_policy() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();

    let policy = tmp.path().join("policy.json");
    std::fs::write(
        &policy,
        r#"{
          "enabled": true,
          "default": "deny",
          "rules": [
            {"action": "allow", "pattern": "https://example.com", "note": "docs"},
            {"action": "allow", "pattern": "https://1.1.1.1"},
            {"action": "allow", "pattern": "127.0.0.1:9"},
            {"action": "deny", "pattern": "*.tracker.example"}
          ],
          "limits": {
            "window_seconds": 30,
            "per_origin_requests": 2,
            "per_agent_requests": 4,
            "max_response_bytes": 1024,
            "pin_dns": true,
            "allow_private_addresses": false
          },
          "disabled_sessions": ["blocked-session"]
        }"#,
    )
    .unwrap();

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "set",
            "--project",
            project.to_str().unwrap(),
            "--file",
            policy.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    let saved: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(saved["default"], "deny");
    assert_eq!(saved["rules"].as_array().unwrap().len(), 4);
    assert_eq!(saved["limits"]["window_seconds"], 30);
    assert_eq!(saved["limits"]["per_origin_requests"], 2);
    assert_eq!(saved["limits"]["max_response_bytes"], 1024);
    assert!(saved["allowlist"].is_null() || saved["allowlist"].as_array().unwrap().is_empty());

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "show",
            "--project",
            project.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let shown: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(shown["rules"][0]["note"], "docs");
    assert_eq!(shown["limits"]["per_agent_requests"], 4);
    assert_eq!(shown["limits"]["pin_dns"], true);

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "check",
            "--project",
            project.to_str().unwrap(),
            "--url",
            "https://example.com/page",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    let allowed: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(allowed["decision"]["allowed"], true);
    assert_eq!(allowed["origin"], "https://example.com");
    assert_eq!(allowed["limits"]["per_origin_requests"], 2);
    assert!(allowed["pinned"].is_null());

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "check",
            "--project",
            project.to_str().unwrap(),
            "--url",
            "https://other.example/page",
            "--json",
        ],
    );
    assert_eq!(code, 1);
    let denied: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(denied["decision"]["allowed"], false);
    assert_eq!(denied["decision"]["code"], "DEFAULT_DENY");

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "check",
            "--project",
            project.to_str().unwrap(),
            "--url",
            "https://example.com/page",
            "--session",
            "blocked-session",
            "--json",
        ],
    );
    assert_eq!(code, 1);
    let switched: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(switched["decision"]["code"], "NETWORK_OFF");

    // A literal public address resolves locally: this proves `--resolve` is a dry run and does
    // not need DNS or a request.
    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "check",
            "--project",
            project.to_str().unwrap(),
            "--url",
            "https://1.1.1.1/page",
            "--resolve",
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    let pinned: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(pinned["pinned"]["address"], "1.1.1.1");

    // A loopback literal is refused by the address policy even when the origin rule allows it.
    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "check",
            "--project",
            project.to_str().unwrap(),
            "--url",
            "http://127.0.0.1:9/page",
            "--resolve",
            "--json",
        ],
    );
    assert_eq!(code, 1, "{stdout}");
    let refused: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(refused["pin_error"]["code"], "ADDRESS_BLOCKED");
}

#[tokio::test]
async fn an_unenforceable_rule_is_refused_and_the_previous_policy_stays() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();

    let good = tmp.path().join("good.json");
    std::fs::write(
        &good,
        r#"{"enabled":true,"rules":[{"action":"deny","pattern":"tracker.example"}]}"#,
    )
    .unwrap();
    let (code, _) = rigel(
        &data,
        &[
            "network",
            "set",
            "--project",
            project.to_str().unwrap(),
            "--file",
            good.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0);

    let bad = tmp.path().join("bad.json");
    std::fs::write(
        &bad,
        r#"{"enabled":true,"rules":[{"action":"deny","pattern":"example.com/path"}]}"#,
    )
    .unwrap();
    let (code, _) = rigel(
        &data,
        &[
            "network",
            "set",
            "--project",
            project.to_str().unwrap(),
            "--file",
            bad.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2, "a rule the matcher ignores is not persisted");

    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "show",
            "--project",
            project.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0);
    let shown: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(shown["rules"][0]["pattern"], "tracker.example");
}

#[tokio::test]
async fn a_legacy_allowlist_is_migrated_to_allow_rules_and_a_deny_default() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&project).unwrap();

    let legacy = tmp.path().join("legacy.json");
    std::fs::write(
        &legacy,
        r#"{"enabled":true,"allowlist":["https://example.com"]}"#,
    )
    .unwrap();
    let (code, stdout) = rigel(
        &data,
        &[
            "network",
            "set",
            "--project",
            project.to_str().unwrap(),
            "--file",
            legacy.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(code, 0, "{stdout}");
    let saved: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(saved["default"], "deny");
    assert_eq!(saved["rules"][0]["action"], "allow");
    assert!(saved["allowlist"].is_null() || saved["allowlist"].as_array().unwrap().is_empty());
}
