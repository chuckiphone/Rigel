//! Roadmap 62: fixed-window egress limits, address validation and DNS pinning.
//!
//! The resolver is scripted so a rebind can be proved without controlling real DNS. Requests
//! that do get through go to a raw loopback server; the policy explicitly opts into private
//! addresses only for these tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rigel_core::{
    AddressResolver, BudgetService, Cancels, DenyOrigins, Egress, EgressLimits, EgressResponse,
    NetworkPolicy, OriginApprover, OriginDecision, OriginRequest, SessionService, StopAt, Tools,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct ScriptedResolver {
    answers: Mutex<VecDeque<Vec<std::net::IpAddr>>>,
    calls: AtomicUsize,
}

impl ScriptedResolver {
    fn new(answers: Vec<Vec<std::net::IpAddr>>) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AddressResolver for ScriptedResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<std::net::IpAddr>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| format!("no scripted answer left for {host}:{port}"))
    }
}

struct Approver {
    calls: AtomicUsize,
}

#[async_trait]
impl OriginApprover for Approver {
    async fn approve(&self, _request: &OriginRequest) -> OriginDecision {
        self.calls.fetch_add(1, Ordering::SeqCst);
        OriginDecision::AllowSession
    }
}

async fn tools(
    root: &std::path::Path,
    policy: NetworkPolicy,
    resolver: Arc<dyn AddressResolver>,
) -> (Tools, Arc<Approver>) {
    let service = SessionService::open(root).await.unwrap();
    let cancels = Arc::new(Cancels::new());
    let budget = Arc::new(BudgetService::new(
        service.pool().clone(),
        cancels as Arc<dyn rigel_core::InterruptSink>,
    ));
    let approver = Arc::new(Approver {
        calls: AtomicUsize::new(0),
    });
    let tools = Tools::new(
        root.to_path_buf(),
        service,
        budget,
        root.join("index"),
        StopAt::Never,
    )
    .with_origin_approval(approver.clone())
    .with_network_policy(policy)
    .with_egress_resolver(resolver);
    (tools, approver)
}

async fn fetch(tools: &Tools, agent: &str, url: &str) -> rigel_core::ToolOutcome {
    tools
        .execute(
            "session",
            agent,
            "fetch_url",
            serde_json::json!({ "url": url }),
        )
        .await
}

fn test_policy(limits: EgressLimits) -> NetworkPolicy {
    NetworkPolicy {
        limits,
        ..NetworkPolicy::default()
    }
}

fn private_limits() -> EgressLimits {
    EgressLimits {
        allow_private_addresses: true,
        ..EgressLimits::default()
    }
}

/// One response per accepted connection, in order.
async fn serve(bodies: Vec<String>) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for body in bodies {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = vec![0u8; 8_192];
            let read = socket.read(&mut buffer).await.unwrap_or(0);
            log.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buffer[..read]).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (port, seen)
}

fn ip(address: &str) -> std::net::IpAddr {
    address.parse().unwrap()
}

#[tokio::test]
async fn a_pinned_request_connects_to_the_resolved_address_and_reports_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, seen) = serve(vec!["pinned page".to_string()]).await;
    let resolver = ScriptedResolver::new(vec![vec![ip("127.0.0.1")], vec![ip("127.0.0.1")]]);
    let (tools, approver) =
        tools(tmp.path(), test_policy(private_limits()), resolver.clone()).await;

    let outcome = fetch(&tools, "agent", &format!("http://pinned.test:{port}/page")).await;

    assert!(outcome.ok, "{:?}", outcome.body);
    assert_eq!(outcome.body["text"], "pinned page");
    assert_eq!(outcome.body["address"], "127.0.0.1");
    assert_eq!(resolver.calls(), 2, "the address is resolved twice");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1);
    assert!(
        seen.lock().unwrap()[0]
            .to_ascii_lowercase()
            .contains(&format!("host: pinned.test:{port}"))
    );
}

#[tokio::test]
async fn a_changed_second_answer_fails_the_request_as_a_rebind() {
    let tmp = tempfile::tempdir().unwrap();
    let resolver =
        ScriptedResolver::new(vec![vec![ip("93.184.216.34")], vec![ip("93.184.216.35")]]);
    let (tools, approver) = tools(
        tmp.path(),
        test_policy(EgressLimits::default()),
        resolver.clone(),
    )
    .await;

    let outcome = fetch(&tools, "agent", "http://rebind.test/page").await;

    assert!(!outcome.ok);
    assert_eq!(outcome.body["error"]["code"], "DNS_REBIND");
    assert!(
        outcome.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("changed its address"),
        "{:?}",
        outcome.body
    );
    assert_eq!(resolver.calls(), 2);
    assert_eq!(
        approver.calls.load(Ordering::SeqCst),
        0,
        "the refusal happens before the origin prompt"
    );
}

#[tokio::test]
async fn a_metadata_address_is_refused_before_the_origin_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let resolver = ScriptedResolver::new(vec![vec![ip("169.254.169.254")]]);
    let (tools, approver) = tools(
        tmp.path(),
        test_policy(EgressLimits::default()),
        resolver.clone(),
    )
    .await;

    let outcome = fetch(&tools, "agent", "http://metadata.test/latest").await;

    assert!(!outcome.ok);
    assert_eq!(outcome.body["error"]["code"], "ADDRESS_BLOCKED");
    assert!(
        outcome.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("link-local"),
        "{:?}",
        outcome.body
    );
    assert_eq!(resolver.calls(), 1);
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_per_origin_limit_refuses_with_a_retry_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _) = serve(vec!["one".to_string(), "two".to_string()]).await;
    let limits = EgressLimits {
        per_origin_requests: 1,
        per_agent_requests: 100,
        ..private_limits()
    };
    let resolver = ScriptedResolver::new(vec![vec![ip("127.0.0.1")], vec![ip("127.0.0.1")]]);
    let (tools, approver) = tools(tmp.path(), test_policy(limits), resolver).await;
    let url = format!("http://pinned.test:{port}/page");

    assert!(fetch(&tools, "agent", &url).await.ok);
    let refused = fetch(&tools, "agent", &url).await;

    assert!(!refused.ok);
    assert_eq!(refused.body["error"]["code"], "EGRESS_RATE_LIMIT");
    assert!(
        refused.body["error"]["retry_after_secs"]
            .as_u64()
            .is_some_and(|seconds| seconds >= 1),
        "{:?}",
        refused.body
    );
    assert_eq!(
        approver.calls.load(Ordering::SeqCst),
        1,
        "the second request never reaches consent or the socket"
    );
}

#[tokio::test]
async fn per_agent_limits_are_shared_across_origins() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _) = serve(vec![
        "first".to_string(),
        "second".to_string(),
        "third".to_string(),
    ])
    .await;
    let limits = EgressLimits {
        per_origin_requests: 100,
        per_agent_requests: 1,
        ..private_limits()
    };
    let resolver = ScriptedResolver::new(vec![
        vec![ip("127.0.0.1")],
        vec![ip("127.0.0.1")],
        vec![ip("127.0.0.1")],
        vec![ip("127.0.0.1")],
        vec![ip("127.0.0.1")],
        vec![ip("127.0.0.1")],
    ]);
    let (tools, _) = tools(tmp.path(), test_policy(limits), resolver).await;
    let url = format!("http://pinned.test:{port}/page");

    assert!(fetch(&tools, "agent-a", &url).await.ok);
    assert!(fetch(&tools, "agent-b", &url).await.ok);
    let refused = fetch(&tools, "agent-a", &url).await;

    assert!(!refused.ok);
    assert_eq!(refused.body["error"]["code"], "EGRESS_AGENT_LIMIT");
    assert!(
        refused.body["error"]["retry_after_secs"]
            .as_u64()
            .is_some_and(|seconds| seconds >= 1),
        "{:?}",
        refused.body
    );
}

#[tokio::test]
async fn the_configured_response_cap_is_used_for_fetch_url() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _) = serve(vec!["0123456789".to_string()]).await;
    let limits = EgressLimits {
        max_response_bytes: 4,
        ..private_limits()
    };
    let resolver = ScriptedResolver::new(vec![vec![ip("127.0.0.1")], vec![ip("127.0.0.1")]]);
    let (tools, _) = tools(tmp.path(), test_policy(limits), resolver).await;

    let outcome = fetch(&tools, "agent", &format!("http://pinned.test:{port}/page")).await;

    assert!(outcome.ok, "{:?}", outcome.body);
    assert_eq!(outcome.body["bytes"], 4);
    assert_eq!(outcome.body["text"], "0123");
    assert_eq!(outcome.body["truncated"], true);
}

#[tokio::test]
async fn a_json_post_reports_the_pinned_address() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _) = serve(vec!["{}".to_string()]).await;
    let resolver = ScriptedResolver::new(vec![vec![ip("127.0.0.1")], vec![ip("127.0.0.1")]]);
    let egress = Egress::new(Arc::new(DenyOrigins), tmp.path().join("index"))
        .with_policy(test_policy(private_limits()))
        .with_resolver(resolver);

    let reply = egress
        .post_json(
            &format!("http://pinned.test:{port}/search"),
            "session",
            "agent",
            &[],
            &serde_json::json!({ "query": "x" }),
        )
        .await
        .unwrap();

    assert_eq!(reply.status, 200);
    assert_eq!(reply.address, ip("127.0.0.1"));
    assert_eq!(reply.body, b"{}");
}

#[tokio::test]
async fn the_exa_post_path_uses_the_same_window_and_size_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _) = serve(vec!["0123456789".to_string()]).await;
    let policy = test_policy(EgressLimits {
        max_response_bytes: 4,
        ..private_limits()
    });
    let resolver = ScriptedResolver::new(vec![vec![ip("127.0.0.1")], vec![ip("127.0.0.1")]]);
    let egress = Egress::new(Arc::new(DenyOrigins), tmp.path().join("index"))
        .with_policy(policy)
        .with_resolver(resolver);

    let result: Result<EgressResponse, _> = egress
        .post_json(
            &format!("http://pinned.test:{port}/search"),
            "session",
            "agent",
            &[],
            &serde_json::json!({ "query": "x" }),
        )
        .await;

    let error = result.unwrap_err();
    assert_eq!(error.code, "EGRESS_SIZE_LIMIT");
    assert!(error.message.contains("4-byte"), "{}", error.message);
}
