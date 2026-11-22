//! Item 49 conformance: every adapter's real event stream, replayed from the golden fixtures and
//! compared with one checked-in unified fixture.
//!
//! The golden streams in `tests/fixtures/golden/` are the wire inputs. This test runs the same
//! production `run_turn_unified` / `prompt_unified` paths the CLI and window use, then compares
//! the resulting [`HarnessEvent`] stream with `tests/fixtures/events/<harness>.json`. A decode
//! change that alters an event, or a conversion that drops one, fails here.
//!
//! Regenerate the fixtures deliberately with:
//!
//! ```text
//! RIGEL_UPDATE_EVENTS=1 cargo test -p rigel-adapters --test events
//! ```
//!
//! The normal test never writes: a missing or stale fixture is a failure, not a silent update.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rigel_adapters::acp::{AcpConfig, ApprovalDecision, ApprovalRequest, Approver};
use rigel_adapters::{
    AcpClient, ClaudeAdapter, ClaudeConfig, CodexAdapter, CodexConfig, Harness, HarnessEvent,
    MuseAdapter, MuseConfig, PermissionMode, StreamRenderer, Transport,
};
use rigel_core::credentials::Secret;
use serde::{Deserialize, Serialize};

const SCENARIOS: [&str; 5] = ["start", "tool_call", "approval", "error", "interrupt"];
const HARNESSES: [&str; 4] = ["claude", "codex", "acp", "muse"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Conformance {
    harness: String,
    scenarios: BTreeMap<String, Vec<HarnessEvent>>,
}

fn golden(harness: &str, scenario: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden")
        .join(harness)
        .join(format!("{scenario}.jsonl"))
}

fn events_fixture(harness: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/events")
        .join(format!("{harness}.json"))
}

fn read_fixture(harness: &str) -> Conformance {
    let path = events_fixture(harness);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing conformance fixture {}: {error}\n\
             run RIGEL_UPDATE_EVENTS=1 cargo test -p rigel-adapters --test events",
            path.display()
        )
    });
    let fixture: Conformance = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!("{} is not valid conformance JSON: {error}", path.display())
    });
    assert_eq!(fixture.harness, harness, "{}", path.display());
    fixture
}

fn updating_fixtures() -> bool {
    std::env::var_os("RIGEL_UPDATE_EVENTS").is_some()
}

fn maybe_write_fixture(harness: &str, conformance: &Conformance) {
    if !updating_fixtures() {
        return;
    }
    let path = events_fixture(harness);
    std::fs::create_dir_all(path.parent().unwrap()).expect("create events fixture directory");
    let mut text = serde_json::to_string_pretty(conformance).expect("serialise conformance");
    text.push('\n');
    std::fs::write(&path, text).expect("write conformance fixture");
    eprintln!("updated {}", path.display());
}

fn conformance(harness: &str, scenarios: BTreeMap<String, Vec<HarnessEvent>>) -> Conformance {
    let conformance = Conformance {
        harness: harness.to_string(),
        scenarios,
    };
    maybe_write_fixture(harness, &conformance);
    let expected = read_fixture(harness);
    assert_eq!(
        conformance, expected,
        "{harness} unified events drifted from the checked-in fixture"
    );
    conformance
}

/* ---------------------------------- Claude --------------------------------- */

fn claude_config(cwd: &Path, scenario: &str) -> ClaudeConfig {
    let mut config = ClaudeConfig::new(cwd);
    config.exe = PathBuf::from(env!("CARGO_BIN_EXE_fake-claude"));
    config.permission_mode = PermissionMode::DontAsk;
    config.timeout = Duration::from_secs(10);
    config.vendor_login = None;
    config
        .env
        .insert("ANTHROPIC_API_KEY".into(), Secret::new("sk-ant-events"));
    config.env.insert(
        "FAKE_CLAUDE_FIXTURE".into(),
        Secret::new(golden("claude", scenario).display().to_string()),
    );
    config
}

async fn claude_events(cwd: &Path, scenario: &str) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    let _ = ClaudeAdapter::new(claude_config(cwd, scenario))
        .run_turn_unified("replay", |event| events.push(event))
        .await;
    events
}

/* ---------------------------------- Codex ---------------------------------- */

fn codex_config(cwd: &Path, scenario: &str) -> CodexConfig {
    let mut config = CodexConfig::router(cwd);
    config.exe = PathBuf::from(env!("CARGO_BIN_EXE_fake-codex"));
    config.transport = Transport::AppServer;
    config.timeout = Duration::from_secs(10);
    config.vendor_login = None;
    config
        .env
        .insert("OPENAI_API_KEY".into(), Secret::new("sk-events"));
    config.env.insert(
        "FAKE_CODEX_FIXTURE".into(),
        Secret::new(golden("codex", scenario).display().to_string()),
    );
    config
}

async fn codex_events(cwd: &Path, scenario: &str) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    let _ = CodexAdapter::new(codex_config(cwd, scenario))
        .run_turn_unified("replay", |event| events.push(event))
        .await;
    events
}

/* ------------------------------------ ACP ---------------------------------- */

struct AllowOnce {
    seen: Mutex<Vec<ApprovalRequest>>,
}

#[async_trait]
impl Approver for AllowOnce {
    async fn approve(&self, request: &ApprovalRequest) -> ApprovalDecision {
        self.seen.lock().unwrap().push(request.clone());
        ApprovalDecision::AllowOnce
    }
}

fn acp_config(cwd: &Path, scenario: &str, harness: Harness) -> AcpConfig {
    let mut config = AcpConfig::new(harness, cwd);
    config.exe = PathBuf::from(env!("CARGO_BIN_EXE_fake-acp"));
    config.timeout = Duration::from_secs(10);
    config.vendor_login = None;
    // The fixture ignores which key name it receives; setting all three lets the same replay run
    // through Cursor, Cline and Gemini without a variant-specific branch in this test.
    for key in ["CURSOR_API_KEY", "CLINE_API_KEY", "GEMINI_API_KEY"] {
        config.env.insert(key.into(), Secret::new("events-replay"));
    }
    config.env.insert(
        "FAKE_ACP_FIXTURE".into(),
        Secret::new(golden("acp", scenario).display().to_string()),
    );
    config
}

async fn acp_events(cwd: &Path, scenario: &str, harness: Harness) -> Vec<HarnessEvent> {
    let mut client = AcpClient::spawn(
        acp_config(cwd, scenario, harness),
        Arc::new(AllowOnce {
            seen: Mutex::new(Vec::new()),
        }),
    )
    .await
    .expect("the replay agent initializes");
    client.new_session().await.expect("session/new");
    let mut events = Vec::new();
    let _ = client
        .prompt_unified("replay", |event| events.push(event))
        .await;
    events
}

/* ----------------------------------- Muse ---------------------------------- */

fn muse_config(cwd: &Path, scenario: &str) -> MuseConfig {
    let mut config = MuseConfig::new(cwd);
    config.exe = PathBuf::from(env!("CARGO_BIN_EXE_fake-muse"));
    config.timeout = Duration::from_secs(10);
    config.vendor_login = None;
    config
        .env
        .insert("META_API_KEY".into(), Secret::new("muse-events"));
    config.env.insert(
        "FAKE_MUSE_FIXTURE".into(),
        Secret::new(golden("muse", scenario).display().to_string()),
    );
    config
}

async fn muse_events(cwd: &Path, scenario: &str) -> Vec<HarnessEvent> {
    let mut events = Vec::new();
    let _ = MuseAdapter::new(muse_config(cwd, scenario))
        .run_turn_unified("replay", |event| events.push(event))
        .await;
    events
}

/* ---------------------------------- tests ---------------------------------- */

#[tokio::test]
async fn each_adapter_replays_into_its_conformance_fixture() {
    for harness in HARNESSES {
        let mut scenarios = BTreeMap::new();
        for scenario in SCENARIOS {
            let dir = tempfile::tempdir().unwrap();
            let events = match harness {
                "claude" => claude_events(dir.path(), scenario).await,
                "codex" => codex_events(dir.path(), scenario).await,
                "acp" => acp_events(dir.path(), scenario, Harness::Cursor).await,
                "muse" => muse_events(dir.path(), scenario).await,
                other => panic!("unhandled harness {other}"),
            };
            scenarios.insert(scenario.to_string(), events);
        }
        conformance(harness, scenarios);
    }
}

#[tokio::test]
async fn every_acp_variant_emits_the_same_start_events() {
    // Regeneration is one writing test; the readers must not race it. The normal gate still runs
    // all three, because this returns only when the update switch is set.
    if updating_fixtures() {
        return;
    }
    // Cursor, Cline and Gemini share one client. The wire fixture is the same and the unified
    // layer must not smuggle a display name or harness-specific shape into the events.
    let expected = read_fixture("acp")
        .scenarios
        .get("start")
        .expect("the ACP start scenario")
        .clone();
    for harness in [Harness::Cursor, Harness::Cline, Harness::Gemini] {
        let dir = tempfile::tempdir().unwrap();
        let events = acp_events(dir.path(), "start", harness).await;
        assert_eq!(events, expected, "{harness:?} start events");
    }
}

#[test]
fn one_renderer_consumes_every_harness_fixture() {
    // The CLI's renderer takes no harness argument. Feeding all four adapters' real replay
    // events through the same instance must produce the same transcript fields, which is the
    // behavioural half of "no branch on harness name".
    for harness in HARNESSES {
        let fixture = read_fixture(harness);
        for (scenario, events) in &fixture.scenarios {
            let mut renderer = StreamRenderer::default();
            for event in events {
                renderer.apply(event.clone());
            }

            let expected_text = events
                .iter()
                .filter_map(|event| match event {
                    HarnessEvent::Delta {
                        kind: rigel_core::ItemKind::Message,
                        text,
                    } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(
                renderer.text, expected_text,
                "{harness}/{scenario}: streamed text"
            );
            let expected_items = events
                .iter()
                .filter(|event| matches!(event, HarnessEvent::Item { .. }))
                .count();
            assert_eq!(
                renderer.items.len(),
                expected_items,
                "{harness}/{scenario}: item count"
            );
            let expected_approvals = events
                .iter()
                .filter(|event| matches!(event, HarnessEvent::ApprovalResolved { .. }))
                .count();
            assert_eq!(
                renderer.approvals.len(),
                expected_approvals,
                "{harness}/{scenario}: approval count"
            );
        }
    }
}

#[test]
fn every_adapter_has_exactly_one_conformance_fixture() {
    if updating_fixtures() {
        return;
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events");
    let mut found: Vec<String> = std::fs::read_dir(&dir)
        .expect("the events fixture directory")
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .map(str::to_string)
        })
        .collect();
    found.sort();
    let mut expected: Vec<String> = HARNESSES.iter().map(|name| name.to_string()).collect();
    expected.sort();
    assert_eq!(found, expected, "one conformance fixture per adapter");
}
