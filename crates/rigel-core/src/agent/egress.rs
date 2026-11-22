//! Network egress with per-origin consent (D33).
//!
//! `fetch_url` is the only way the native agent reaches the network. A fetch is approved per
//! **origin** — scheme, host and port — not per URL, so one approval covers the pages of a site
//! without letting a redirect carry that consent somewhere else. The answer can cover one
//! request, the rest of the session, or every session on this machine; only the last one is
//! written to `<index dir>/approved-origins.json`.
//!
//! The fetcher itself is deliberately narrow: `http`/`https` only, redirects followed by hand so
//! each new origin is approved, redirects capped, a 256 KiB body cap, and a content-type
//! allowlist of text-like types. Everything it returns is untrusted input, labelled as such.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Url;
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LOCATION};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

/// The default per-response cap. A project may raise it up to
/// [`MAX_EGRESS_RESPONSE_BYTES`]; the model only ever sees [`crate::agent::tools`]' result cap on
/// top of that.
pub const MAX_FETCH_BYTES: usize = 256 * 1024;
const MAX_REDIRECTS: usize = 5;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const APPROVED_FILE: &str = "approved-origins.json";
const NETWORK_FILE: &str = ".rigel/network.json";

/// The response cap accepts a custom value up to this hard ceiling. The hard ceiling keeps one
/// hostile response from becoming an unbounded allocation even when the policy is edited.
pub const MAX_EGRESS_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EGRESS_WINDOW_SECONDS: u64 = 24 * 60 * 60;
pub const MAX_EGRESS_REQUESTS: u32 = 100_000;

/// Bounds keep one hand-edited policy from becoming a second data store. The UI uses the same
/// limits through the save validation.
pub const MAX_NETWORK_RULES: usize = 200;
pub const MAX_NETWORK_PATTERN_CHARS: usize = 200;
pub const MAX_NETWORK_NOTE_CHARS: usize = 200;
pub const MAX_DISABLED_SESSIONS: usize = 200;

/// What a matching rule does. `Allow` still needs the per-origin question: a project policy says
/// which doors may be discussed, not that consent is already given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAction {
    #[default]
    Allow,
    Deny,
}

/// What happens to an origin no rule names. `Ask` is the normal egress path; `Deny` makes the
/// rule list the only way through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkDefault {
    #[default]
    Ask,
    Deny,
}

/// One ordered rule. Patterns are an exact origin (`https://api.example.com`), an exact host
/// (`api.example.com`, any scheme), a host and port (`api.example.com:8443`), a bare subdomain
/// wildcard (`*.example.com` — subdomains, not the apex), or `*`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRule {
    pub action: NetworkAction,
    pub pattern: String,
    /// Shown in the editor and in an explain result; never sent anywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Q240 / roadmap 61: the project's network policy, evaluated before a per-origin question.
///
/// Rules are ordered and the first match wins. A non-empty legacy `allowlist` is appended as
/// `allow` rules and turns the default to `deny`, which is exactly the old OR semantics. The
/// legacy field is kept in the serialized shape so an older file still loads; `normalized`
/// clears it and writes the equivalent rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkPolicy {
    /// `false` is the kill switch: no request leaves, even to an already-approved origin.
    #[serde(default = "network_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub default: NetworkDefault,
    #[serde(default)]
    pub rules: Vec<NetworkRule>,
    /// Roadmap 62: the request window, response cap and address policy. A file written by an
    /// earlier build has no `limits`; it loads with the safe defaults below.
    #[serde(default)]
    pub limits: EgressLimits,
    /// Legacy exact-origin allowlist. Converted to allow rules on load/save.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowlist: Vec<String>,
    /// Sessions whose own kill switch is on. The project switch still applies to every turn;
    /// this list refuses one session without silencing the others.
    #[serde(default)]
    pub disabled_sessions: Vec<String>,
}

/// The answer for one origin, with the reason and the part of the policy that decided it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkDecision {
    pub allowed: bool,
    /// `NETWORK_OFF`, `RULE_DENY`, `NOT_ALLOWLISTED`, `DEFAULT_DENY` or empty when allowed.
    pub code: String,
    /// `session`, `kill-switch`, `rule`, `allowlist` or `default`.
    pub source: String,
    pub reason: String,
}

impl NetworkDecision {
    fn allow(source: &str) -> Self {
        Self {
            allowed: true,
            code: String::new(),
            source: source.to_string(),
            reason: "origin may ask for approval".to_string(),
        }
    }

    fn deny(code: &str, source: &str, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            code: code.to_string(),
            source: source.to_string(),
            reason: reason.into(),
        }
    }
}

/// The origin a URL names, plus its decision. `rigel network check` and the editor's check button
/// use this shape so they cannot disagree about the origin being judged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetworkCheck {
    pub origin: String,
    pub decision: NetworkDecision,
}

/// Roadmap 62: the project's egress budget. The counters are per process and live in
/// [`Egress`]; this is the persisted configuration only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressLimits {
    /// The fixed accounting window. A new window starts when this many seconds have elapsed.
    #[serde(default = "default_egress_window")]
    pub window_seconds: u64,
    /// Requests to one origin within one window. `0` means no origin cap.
    #[serde(default = "default_origin_requests")]
    pub per_origin_requests: u32,
    /// Requests made by one agent across origins within one window. `0` means no agent cap.
    #[serde(default = "default_agent_requests")]
    pub per_agent_requests: u32,
    /// The body cap. `fetch_url` truncates at this value; an API response that declares or
    /// exceeds it is refused rather than parsed from a partial document.
    #[serde(default = "default_response_bytes")]
    pub max_response_bytes: usize,
    /// Resolve once, validate the answer, then connect only to that selected address. Turning
    /// this off still validates the addresses, but cannot stop a later DNS answer from moving
    /// the connection.
    #[serde(default = "default_pin_dns")]
    pub pin_dns: bool,
    /// Loopback, private, link-local, metadata, multicast and reserved addresses are refused
    /// unless this is on. It is off by default; local development can opt in explicitly.
    #[serde(default)]
    pub allow_private_addresses: bool,
}

fn default_egress_window() -> u64 {
    60
}

fn default_origin_requests() -> u32 {
    30
}

fn default_agent_requests() -> u32 {
    120
}

fn default_response_bytes() -> usize {
    MAX_FETCH_BYTES
}

fn default_pin_dns() -> bool {
    true
}

impl Default for EgressLimits {
    fn default() -> Self {
        Self {
            window_seconds: default_egress_window(),
            per_origin_requests: default_origin_requests(),
            per_agent_requests: default_agent_requests(),
            max_response_bytes: default_response_bytes(),
            pin_dns: true,
            allow_private_addresses: false,
        }
    }
}

impl EgressLimits {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_EGRESS_WINDOW_SECONDS).contains(&self.window_seconds) {
            return Err(format!(
                "the egress window must be between 1 and {MAX_EGRESS_WINDOW_SECONDS} seconds"
            ));
        }
        if self.per_origin_requests > MAX_EGRESS_REQUESTS {
            return Err(format!(
                "the per-origin limit cannot exceed {MAX_EGRESS_REQUESTS} requests"
            ));
        }
        if self.per_agent_requests > MAX_EGRESS_REQUESTS {
            return Err(format!(
                "the per-agent limit cannot exceed {MAX_EGRESS_REQUESTS} requests"
            ));
        }
        if !(1..=MAX_EGRESS_RESPONSE_BYTES).contains(&self.max_response_bytes) {
            return Err(format!(
                "the response cap must be between 1 and {MAX_EGRESS_RESPONSE_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

/// A runtime snapshot of one window. It is returned to the window/CLI and is never persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EgressCounters {
    pub origin_requests: u32,
    pub agent_requests: u32,
    pub remaining_seconds: u64,
}

/// The result of the one resolution that is allowed to choose the connection address. The
/// second address is compared before the socket opens, so a changed answer is a rebind refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PinnedResolution {
    pub host: String,
    pub port: u16,
    pub address: IpAddr,
    pub addresses: Vec<IpAddr>,
}

/// A resolved HTTP response from the pinned client. `address` is the validated address the
/// request was sent to; Exa includes it in its tool result.
#[derive(Debug)]
pub struct EgressResponse {
    pub status: u16,
    pub address: IpAddr,
    pub body: Vec<u8>,
}

/// The injectable DNS seam. Production uses [`SystemResolver`]; a test can return two different
/// answers to prove the rebind refusal without controlling the machine's resolver.
#[async_trait]
pub trait AddressResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

#[async_trait]
impl AddressResolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let addresses = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| format!("could not resolve {host}:{port}: {error}"))?;
        Ok(addresses.map(|address| address.ip()).collect())
    }
}

fn network_enabled() -> bool {
    true
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            default: NetworkDefault::Ask,
            rules: Vec::new(),
            limits: EgressLimits::default(),
            allowlist: Vec::new(),
            disabled_sessions: Vec::new(),
        }
    }
}

impl NetworkPolicy {
    pub fn new(enabled: bool, allowlist: impl IntoIterator<Item = String>) -> Self {
        Self {
            enabled,
            allowlist: allowlist.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Read `<project>/.rigel/network.json`. A missing file is the normal ask-first policy; a
    /// file that cannot be parsed or validated fails closed, so a typo cannot silently widen the
    /// policy. The legacy allowlist is migrated in memory and written back on the next save.
    pub fn load(project_root: &Path) -> Self {
        let path = project_root.join(NETWORK_FILE);
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(policy) = serde_json::from_str::<Self>(&text) else {
            return Self::deny_all();
        };
        policy.normalized().unwrap_or_else(|_| Self::deny_all())
    }

    fn deny_all() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Validate and write the project policy. Returns the normalized form the caller should show;
    /// a rule the matcher does not understand is an error, never a row that looks saved but is
    /// ignored on the next request.
    pub fn save(&self, project_root: &Path) -> Result<Self, String> {
        let policy = self.normalized()?;
        let path = project_root.join(NETWORK_FILE);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&policy)
            .map_err(|error| format!("could not encode the network policy: {error}"))?;
        fs::write(&path, format!("{text}\n"))
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        Ok(policy)
    }

    /// Trim the editable fields, migrate the legacy allowlist, and reject anything the matcher
    /// cannot enforce.
    pub fn normalized(&self) -> Result<Self, String> {
        let mut policy = self.clone();
        policy.rules = policy
            .rules
            .into_iter()
            .map(|mut rule| {
                rule.pattern = rule.pattern.trim().to_string();
                rule.note = rule
                    .note
                    .map(|note| note.trim().to_string())
                    .filter(|note| !note.is_empty());
                rule
            })
            .collect();
        policy.allowlist = policy
            .allowlist
            .into_iter()
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        policy.disabled_sessions = policy
            .disabled_sessions
            .into_iter()
            .map(|session| session.trim().to_string())
            .filter(|session| !session.is_empty())
            .collect();

        if !policy.allowlist.is_empty() {
            for pattern in &policy.allowlist {
                parse_pattern(pattern)
                    .map_err(|error| format!("legacy allowlist entry {pattern:?}: {error}"))?;
                policy.rules.push(NetworkRule {
                    action: NetworkAction::Allow,
                    pattern: pattern.clone(),
                    note: Some("legacy allowlist".to_string()),
                });
            }
            policy.allowlist.clear();
            policy.default = NetworkDefault::Deny;
        }

        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.limits.validate()?;
        if self.rules.len() > MAX_NETWORK_RULES {
            return Err(format!(
                "at most {MAX_NETWORK_RULES} network rules are supported"
            ));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.pattern.chars().count() > MAX_NETWORK_PATTERN_CHARS {
                return Err(format!(
                    "rule {} pattern is longer than {MAX_NETWORK_PATTERN_CHARS} characters",
                    index + 1
                ));
            }
            if rule
                .note
                .as_deref()
                .is_some_and(|note| note.chars().count() > MAX_NETWORK_NOTE_CHARS)
            {
                return Err(format!(
                    "rule {} note is longer than {MAX_NETWORK_NOTE_CHARS} characters",
                    index + 1
                ));
            }
            parse_pattern(&rule.pattern).map_err(|error| format!("rule {}: {error}", index + 1))?;
        }
        for (index, entry) in self.allowlist.iter().enumerate() {
            parse_pattern(entry)
                .map_err(|error| format!("allowlist entry {}: {error}", index + 1))?;
        }
        if self.disabled_sessions.len() > MAX_DISABLED_SESSIONS {
            return Err(format!(
                "at most {MAX_DISABLED_SESSIONS} disabled sessions are supported"
            ));
        }
        for session in &self.disabled_sessions {
            if session.chars().count() > 200 {
                return Err("a disabled session id is longer than 200 characters".to_string());
            }
        }
        Ok(())
    }

    /// The one decision every egress surface uses. Rules are ordered; the first match wins, and
    /// a rule can deny an origin the legacy allowlist would have admitted.
    pub fn check(&self, origin: &str, session_id: Option<&str>) -> NetworkDecision {
        if let Some(session) = session_id
            && self.session_disabled(session)
        {
            return NetworkDecision::deny(
                "NETWORK_OFF",
                "session",
                "the network kill switch is on for this session",
            );
        }
        if !self.enabled {
            return NetworkDecision::deny(
                "NETWORK_OFF",
                "kill-switch",
                "network access is switched off for this project",
            );
        }
        let Some(parsed) = origin_parts(origin) else {
            return NetworkDecision::deny(
                "BAD_ORIGIN",
                "origin",
                format!("{origin} is not an http(s) origin"),
            );
        };
        for rule in &self.rules {
            if parse_pattern(&rule.pattern)
                .map(|pattern| pattern.matches(&parsed))
                .unwrap_or(false)
            {
                let named = rule
                    .note
                    .as_deref()
                    .map(|note| format!(" ({note})"))
                    .unwrap_or_default();
                return match rule.action {
                    NetworkAction::Allow => NetworkDecision::allow("rule"),
                    NetworkAction::Deny => NetworkDecision::deny(
                        "RULE_DENY",
                        "rule",
                        format!(
                            "{origin} is denied by the network rule `{}`{named}",
                            rule.pattern
                        ),
                    ),
                };
            }
        }
        if !self.allowlist.is_empty() {
            let allowed = self
                .allowlist
                .iter()
                .any(|entry| pattern_matches(entry, &parsed));
            return if allowed {
                NetworkDecision::allow("allowlist")
            } else {
                NetworkDecision::deny(
                    "NOT_ALLOWLISTED",
                    "allowlist",
                    format!(
                        "{origin} is not on this project's network allowlist; nothing was sent"
                    ),
                )
            };
        }
        match self.default {
            NetworkDefault::Ask => NetworkDecision::allow("default"),
            NetworkDefault::Deny => NetworkDecision::deny(
                "DEFAULT_DENY",
                "default",
                format!("{origin} is denied by the default network policy"),
            ),
        }
    }

    pub fn check_url(&self, url: &str, session_id: Option<&str>) -> Result<NetworkCheck, String> {
        let origin = origin_of(url)?;
        Ok(NetworkCheck {
            decision: self.check(&origin, session_id),
            origin,
        })
    }

    /// Whether this origin may be considered at all. This is not an approval.
    pub fn allows(&self, origin: &str) -> bool {
        self.check(origin, None).allowed
    }

    pub fn status(&self, origin: &str) -> &'static str {
        let decision = self.check(origin, None);
        if decision.allowed {
            return "origin may ask for approval";
        }
        match decision.code.as_str() {
            "NETWORK_OFF" => "network disabled",
            "RULE_DENY" => "origin is denied by a network rule",
            "NOT_ALLOWLISTED" => "origin is not on this project's network allowlist",
            "DEFAULT_DENY" => "the default network policy denies this origin",
            _ => "origin is not usable",
        }
    }

    /// Whether this session's own kill switch is on. The project switch is checked separately,
    /// so an allowlist refusal keeps its own error code and a session refusal is `NETWORK_OFF`.
    pub fn session_disabled(&self, session_id: &str) -> bool {
        self.disabled_sessions
            .iter()
            .any(|disabled| disabled == session_id)
    }
}

/// The origin a URL names, in the form the policy compares. Only http(s) reaches egress.
pub fn origin_of(url: &str) -> Result<String, String> {
    let parsed = Url::parse(url.trim()).map_err(|error| format!("{url} is not a URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "{} is not allowed; only http and https are supported",
            parsed.scheme()
        ));
    }
    Ok(parsed.origin().ascii_serialization())
}

#[derive(Debug, Clone)]
enum NetworkMatcher {
    Any,
    Origin {
        scheme: String,
        host: HostMatcher,
        port: u16,
    },
    Host {
        host: HostMatcher,
        port: Option<u16>,
    },
}

#[derive(Debug, Clone)]
enum HostMatcher {
    Exact(String),
    /// `*.example.com`: a subdomain, not the apex.
    Subdomains(String),
}

impl NetworkMatcher {
    fn matches(&self, origin: &(String, String, u16)) -> bool {
        match self {
            Self::Any => true,
            Self::Origin { scheme, host, port } => {
                scheme == &origin.0 && host.matches(&origin.1) && port == &origin.2
            }
            Self::Host { host, port } => {
                host.matches(&origin.1) && port.is_none_or(|port| port == origin.2)
            }
        }
    }
}

impl HostMatcher {
    fn matches(&self, host: &str) -> bool {
        match self {
            Self::Exact(expected) => expected == host,
            Self::Subdomains(suffix) => {
                host.len() > suffix.len() + 1 && host.ends_with(&format!(".{suffix}"))
            }
        }
    }
}

fn parse_pattern(pattern: &str) -> Result<NetworkMatcher, String> {
    let raw = pattern.trim().to_ascii_lowercase();
    if raw.is_empty() {
        return Err("a pattern cannot be empty".to_string());
    }
    if raw.chars().count() > MAX_NETWORK_PATTERN_CHARS {
        return Err(format!(
            "a pattern is longer than {MAX_NETWORK_PATTERN_CHARS} characters"
        ));
    }
    if raw == "*" {
        return Ok(NetworkMatcher::Any);
    }
    if raw.contains('?') || raw.contains('#') || raw.contains(char::is_whitespace) {
        return Err(format!("`{pattern}` has a query, fragment or whitespace"));
    }
    if let Some((scheme, rest)) = raw.split_once("://") {
        if !matches!(scheme, "http" | "https") {
            return Err(format!("`{pattern}` is not an http(s) origin"));
        }
        let origin_only = rest.trim_end_matches('/');
        if origin_only.is_empty() || origin_only.contains('/') {
            return Err(format!(
                "`{pattern}` names a path; restrict the origin or host instead"
            ));
        }
        if rest.starts_with("*.") {
            return Err(format!(
                "`{pattern}` needs no scheme for a wildcard; write `*.{}`",
                rest.trim_start_matches("*.")
            ));
        }
        let url = Url::parse(&raw).map_err(|error| format!("`{pattern}`: {error}"))?;
        let host = url
            .host_str()
            .ok_or_else(|| format!("`{pattern}` has no host"))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| format!("`{pattern}` has no port"))?;
        return Ok(NetworkMatcher::Origin {
            scheme: scheme.to_string(),
            host: HostMatcher::Exact(host.to_string()),
            port,
        });
    }
    if raw.contains('/') {
        return Err(format!(
            "`{pattern}` contains a path; use an origin or host pattern"
        ));
    }
    if let Some(suffix) = raw.strip_prefix("*.") {
        let (host, port) = split_bare_host_port(suffix, pattern)?;
        return Ok(NetworkMatcher::Host {
            host: HostMatcher::Subdomains(host),
            port,
        });
    }
    let url = Url::parse(&format!("http://{raw}"))
        .map_err(|error| format!("`{pattern}` is not a host or origin: {error}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| format!("`{pattern}` has no host"))?;
    Ok(NetworkMatcher::Host {
        host: HostMatcher::Exact(host.to_string()),
        port: url.port(),
    })
}

fn split_bare_host_port(value: &str, original: &str) -> Result<(String, Option<u16>), String> {
    let (host, port) = match value.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("`{original}` has an invalid port"))?;
            (host, Some(port))
        }
        _ => (value, None),
    };
    if host.is_empty() || host.contains(':') || host.contains('*') {
        return Err(format!("`{original}` is not a host wildcard"));
    }
    Ok((host.to_string(), port))
}

fn pattern_matches(pattern: &str, origin: &(String, String, u16)) -> bool {
    parse_pattern(pattern)
        .map(|matcher| matcher.matches(origin))
        .unwrap_or(false)
}

fn origin_parts(origin: &str) -> Option<(String, String, u16)> {
    let url = Url::parse(origin.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    Some((
        url.scheme().to_ascii_lowercase(),
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default()?,
    ))
}

/// What is being approved: one URL, belonging to one origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginRequest {
    pub url: String,
    /// `scheme://host[:port]`, lowercased and without a trailing slash.
    pub origin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginDecision {
    AllowOnce,
    /// Until this process exits. A CLI turn is one process.
    AllowSession,
    /// Persisted under the index dir, so it survives restarts.
    AllowAlways,
    Deny,
}

/// Who answers an [`OriginRequest`]. Implementations must fail closed: an error, EOF or an
/// unreadable answer denies.
#[async_trait]
pub trait OriginApprover: Send + Sync {
    async fn approve(&self, request: &OriginRequest) -> OriginDecision;
}

/// Refuses every origin. The default for a headless or not-yet-wired caller.
pub struct DenyOrigins;

#[async_trait]
impl OriginApprover for DenyOrigins {
    async fn approve(&self, _request: &OriginRequest) -> OriginDecision {
        OriginDecision::Deny
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressError {
    /// A stable code the tool returns: `BAD_URL`, `UNSUPPORTED_SCHEME`, `DENIED`, ...
    pub code: &'static str,
    pub message: String,
    /// Present on `EGRESS_RATE_LIMIT` and `EGRESS_AGENT_LIMIT`; the caller can wait this long.
    pub retry_after_secs: Option<u64>,
}

impl EgressError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    fn with_retry(code: &'static str, message: impl Into<String>, retry_after_secs: u64) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: Some(retry_after_secs.max(1)),
        }
    }
}

/// One fixed accounting window. Tests drive `now` explicitly so a refill is not a sleep.
struct EgressWindow {
    started: Instant,
    origins: HashMap<String, u32>,
    agents: HashMap<String, u32>,
}

impl EgressWindow {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            origins: HashMap::new(),
            agents: HashMap::new(),
        }
    }

    fn try_acquire(
        &mut self,
        origin: &str,
        agent: &str,
        limits: &EgressLimits,
        now: Instant,
    ) -> Result<EgressCounters, (&'static str, u64, String)> {
        let window = Duration::from_secs(limits.window_seconds);
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed >= window {
            self.started = now;
            self.origins.clear();
            self.agents.clear();
        }
        let remaining_seconds = window
            .saturating_sub(now.saturating_duration_since(self.started))
            .as_secs();
        let origin_used = self.origins.get(origin).copied().unwrap_or_default();
        if limits.per_origin_requests > 0 && origin_used >= limits.per_origin_requests {
            return Err((
                "EGRESS_RATE_LIMIT",
                remaining_seconds,
                format!(
                    "{origin} has used its {} request(s) in this {}s window; retry in {remaining_seconds}s",
                    limits.per_origin_requests, limits.window_seconds
                ),
            ));
        }
        let agent_used = self.agents.get(agent).copied().unwrap_or_default();
        if limits.per_agent_requests > 0 && agent_used >= limits.per_agent_requests {
            return Err((
                "EGRESS_AGENT_LIMIT",
                remaining_seconds,
                format!(
                    "agent {agent} has used its {} request(s) in this {}s window; retry in {remaining_seconds}s",
                    limits.per_agent_requests, limits.window_seconds
                ),
            ));
        }
        self.origins.insert(origin.to_string(), origin_used + 1);
        self.agents.insert(agent.to_string(), agent_used + 1);
        Ok(EgressCounters {
            origin_requests: origin_used + 1,
            agent_requests: agent_used + 1,
            remaining_seconds,
        })
    }
}

/// The reason an address cannot be contacted under the current limits. The returned label is
/// stable enough for a refusal message and a test; it is not a user-authored value.
fn address_refusal(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(ip) => {
            if ip.is_unspecified() {
                Some("unspecified")
            } else if ip.is_loopback() {
                Some("loopback")
            } else if ip.is_private() {
                Some("private")
            } else if ip.is_link_local() {
                Some("link-local")
            } else if ip.is_broadcast() {
                Some("broadcast")
            } else if ip.is_multicast() {
                Some("multicast")
            } else {
                let octets = ip.octets();
                let shared = octets[0] == 100 && (64..=127).contains(&octets[1]);
                let benchmark = octets[0] == 198 && matches!(octets[1], 18 | 19);
                let reserved = octets[0] == 0
                    || (octets[0] == 192 && octets[1] == 0 && octets[2] <= 2)
                    || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                    || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                    || octets[0] >= 240;
                if shared {
                    Some("shared")
                } else if benchmark {
                    Some("benchmark")
                } else if reserved {
                    Some("documentation or reserved")
                } else {
                    None
                }
            }
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return address_refusal(IpAddr::V4(mapped));
            }
            let segments = ip.segments();
            if ip.is_unspecified() {
                Some("unspecified")
            } else if ip.is_loopback() {
                Some("loopback")
            } else if ip.is_unique_local() {
                Some("private")
            } else if ip.is_unicast_link_local() {
                Some("link-local")
            } else if ip.is_multicast() {
                Some("multicast")
            } else if segments[0] == 0x2001 && segments[1] == 0x0db8 {
                Some("documentation")
            } else if segments[0] == 0x0064 && segments[1] == 0xff9b {
                Some("nat64")
            } else if segments[0] == 0x2002 {
                Some("6to4")
            } else {
                None
            }
        }
    }
}

fn normalized_addresses(addresses: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = addresses
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Prefer IPv4 when a dual-stack answer offers both. The selected address is what the
    // pinned client contacts, so it must be the one recorded in the request result.
    addresses.sort_by_key(|address| matches!(address, IpAddr::V6(_)));
    addresses
}

fn host_literal(host: &str) -> Option<IpAddr> {
    host.trim_matches(['[', ']']).parse().ok()
}

/// One fetched page. `text` is decoded lossily as UTF-8; `truncated` says the body cap cut it.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub url: String,
    pub origin: String,
    /// The validated address the pinned client connected to, as the tool reports it.
    pub address: String,
    pub status: u16,
    pub content_type: String,
    pub bytes: usize,
    pub truncated: bool,
    pub text: String,
}

impl Fetched {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct StoredOrigins {
    #[serde(default)]
    origins: Vec<String>,
}

/// Per-process consent cache plus the persisted `always` list. Cloneable so a subagent's tool
/// set shares the same grants.
#[derive(Clone)]
pub struct Egress {
    client: reqwest::Client,
    approver: Arc<dyn OriginApprover>,
    store: std::path::PathBuf,
    session: Arc<Mutex<HashSet<String>>>,
    always: Arc<Mutex<Option<HashSet<String>>>>,
    policy: Arc<Mutex<NetworkPolicy>>,
    resolver: Arc<dyn AddressResolver>,
    /// Fixed-window counters. Shared by clones, so a turn and its subagents draw from one
    /// per-agent and per-origin budget.
    window: Arc<Mutex<EgressWindow>>,
}

impl Egress {
    pub fn new(
        approver: Arc<dyn OriginApprover>,
        index_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        let client = Self::client_builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let index_dir = index_dir.into();
        Self {
            client,
            approver,
            store: index_dir.join(APPROVED_FILE),
            session: Arc::new(Mutex::new(HashSet::new())),
            always: Arc::new(Mutex::new(None)),
            policy: Arc::new(Mutex::new(NetworkPolicy::default())),
            resolver: Arc::new(SystemResolver),
            window: Arc::new(Mutex::new(EgressWindow::new(Instant::now()))),
        }
    }

    fn client_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            // Redirects are followed by hand: a 302 to another origin must not inherit the
            // first origin's approval.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .user_agent("rigel/0.1")
    }

    pub fn with_approver(mut self, approver: Arc<dyn OriginApprover>) -> Self {
        self.approver = approver;
        self
    }

    /// Replace the system resolver. A test uses a scripted one to prove a changed second answer
    /// is refused before a request; production never changes this.
    pub fn with_resolver(mut self, resolver: Arc<dyn AddressResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Install the project's Q240 policy. The policy is shared by every clone of this egress,
    /// so a kill switch set for the turn also covers its subagents.
    pub fn with_policy(mut self, policy: NetworkPolicy) -> Self {
        self.policy = Arc::new(Mutex::new(policy));
        self
    }

    pub fn set_policy(&self, policy: NetworkPolicy) {
        if let Ok(mut current) = self.policy.lock() {
            *current = policy;
        }
    }

    pub fn policy(&self) -> NetworkPolicy {
        self.policy
            .lock()
            .map(|policy| policy.clone())
            .unwrap_or_else(|_| NetworkPolicy {
                // A poisoned lock is not permission: fail closed.
                enabled: false,
                ..NetworkPolicy::default()
            })
    }

    /// The Q240 check, before the origin question and before any socket is opened.
    pub fn policy_status(&self, origin: &str) -> (&'static str, bool) {
        let policy = self.policy();
        (policy.status(origin), policy.allows(origin))
    }

    /// Fetch one `http(s)` URL after its origin is approved and its address is pinned. The whole
    /// operation is bounded, so a server that trickles bytes cannot hold the turn open.
    pub async fn fetch(
        &self,
        url: &str,
        session: &str,
        agent: &str,
    ) -> Result<Fetched, EgressError> {
        let start = parse_http_url(url)?;

        match tokio::time::timeout(FETCH_TIMEOUT, self.fetch_inner(start, session, agent)).await {
            Ok(result) => result,
            Err(_) => Err(EgressError::new(
                "TIMEOUT",
                format!(
                    "the fetch did not finish within {}s",
                    FETCH_TIMEOUT.as_secs()
                ),
            )),
        }
    }

    /// Approve one URL's origin without reading it, for a browser that navigates on its own.
    /// The preflight still resolves and validates the address, applies the window and records
    /// the address in the tool result; Obscura's engine then blocks private targets unless the
    /// policy explicitly allows them.
    pub async fn approve_origin(
        &self,
        url: &str,
        session: &str,
        agent: &str,
    ) -> Result<(String, String, IpAddr), EgressError> {
        let start = parse_http_url(url)?;
        let prepared = self.prepare(&start, session, agent).await?;
        self.authorize(&prepared.origin, start.as_str()).await?;
        Ok((prepared.origin, start.to_string(), prepared.pinned.address))
    }

    /// Resolve and validate a URL without sending, taking a window slot or asking for approval.
    /// `rigel network check --resolve` uses this as its explain output.
    pub async fn pin_preview(
        &self,
        url: &str,
        session: &str,
    ) -> Result<PinnedResolution, EgressError> {
        let start = parse_http_url(url)?;
        let origin = start.origin().ascii_serialization();
        let policy = self.policy();
        let decision = policy.check(&origin, Some(session));
        if !decision.allowed {
            return Err(policy_error(&decision));
        }
        self.resolve_pinned(&start, &policy.limits).await
    }

    /// One JSON POST through the same pinned client and window. Exa has no per-origin prompt
    /// (the destination is fixed), so this applies the policy, window and address rules without
    /// going through `authorize`.
    pub async fn post_json(
        &self,
        url: &str,
        session: &str,
        agent: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<EgressResponse, EgressError> {
        let parsed = parse_http_url(url)?;
        let prepared = self.prepare(&parsed, session, agent).await?;
        let mut request = prepared.client.post(parsed).json(body);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| EgressError::new("NETWORK", format!("{url}: {error}")))?;
        let status = response.status().as_u16();
        let body = read_limited(response, prepared.max_bytes).await?;
        Ok(EgressResponse {
            status,
            address: prepared.pinned.address,
            body,
        })
    }

    async fn prepare(
        &self,
        url: &Url,
        session: &str,
        agent: &str,
    ) -> Result<PreparedRequest, EgressError> {
        let origin = url.origin().ascii_serialization();
        let policy = self.policy();
        let decision = policy.check(&origin, Some(session));
        if !decision.allowed {
            return Err(policy_error(&decision));
        }
        {
            let mut window = self
                .window
                .lock()
                .map_err(|_| EgressError::new("NETWORK", "the egress window is unavailable"))?;
            window
                .try_acquire(&origin, agent, &policy.limits, Instant::now())
                .map_err(|(code, retry_after, message)| {
                    EgressError::with_retry(code, message, retry_after)
                })?;
        }
        let pinned = self.resolve_pinned(url, &policy.limits).await?;
        let client = self.pinned_client(url, &pinned, &policy.limits)?;
        Ok(PreparedRequest {
            origin,
            pinned,
            client,
            max_bytes: policy.limits.max_response_bytes,
        })
    }

    async fn resolve_pinned(
        &self,
        url: &Url,
        limits: &EgressLimits,
    ) -> Result<PinnedResolution, EgressError> {
        let host = url
            .host_str()
            .ok_or_else(|| EgressError::new("BAD_URL", "the URL has no host"))?
            .to_string();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| EgressError::new("BAD_URL", format!("{host} has no port")))?;
        let literal = host_literal(&host);
        let first = match literal {
            Some(address) => vec![address],
            None => self
                .resolver
                .resolve(&host, port)
                .await
                .map_err(|error| EgressError::new("DNS_LOOKUP", error))?,
        };
        let addresses = normalized_addresses(first);
        if addresses.is_empty() {
            return Err(EgressError::new(
                "DNS_LOOKUP",
                format!("{host}:{port} resolved to no addresses"),
            ));
        }
        if !limits.allow_private_addresses
            && let Some((address, reason)) = addresses
                .iter()
                .find_map(|address| address_refusal(*address).map(|reason| (*address, reason)))
        {
            return Err(EgressError::new(
                "ADDRESS_BLOCKED",
                format!(
                    "{host} resolved to {address}, a {reason} address; this project only fetches globally routable addresses"
                ),
            ));
        }
        if limits.pin_dns && literal.is_none() {
            let second = self
                .resolver
                .resolve(&host, port)
                .await
                .map_err(|error| EgressError::new("DNS_LOOKUP", error))?;
            let second = normalized_addresses(second);
            if second != addresses {
                return Err(EgressError::new(
                    "DNS_REBIND",
                    format!(
                        "{host} changed its address between the check and the connection ({} then {}); the request was refused",
                        addresses
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        second
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
        Ok(PinnedResolution {
            host,
            port,
            address: addresses[0],
            addresses,
        })
    }

    fn pinned_client(
        &self,
        url: &Url,
        pinned: &PinnedResolution,
        limits: &EgressLimits,
    ) -> Result<reqwest::Client, EgressError> {
        if !limits.pin_dns || host_literal(&pinned.host).is_some() {
            return Ok(self.client.clone());
        }
        let host = url.host_str().unwrap_or(&pinned.host);
        Self::client_builder()
            .resolve(host, SocketAddr::new(pinned.address, pinned.port))
            .build()
            .map_err(|error| {
                EgressError::new(
                    "NETWORK",
                    format!("could not build the pinned HTTP client: {error}"),
                )
            })
    }

    async fn fetch_inner(
        &self,
        start: Url,
        session: &str,
        agent: &str,
    ) -> Result<Fetched, EgressError> {
        let mut current = start.clone();
        // An `AllowOnce` holds for this fetch, including redirects inside the same origin.
        let mut once = HashSet::new();

        for _ in 0..=MAX_REDIRECTS {
            let prepared = self.prepare(&current, session, agent).await?;
            let origin = prepared.origin.clone();
            if !once.contains(&origin) {
                self.authorize(&origin, current.as_str()).await?;
                once.insert(origin.clone());
            }

            let response = prepared
                .client
                .get(current.clone())
                .send()
                .await
                .map_err(|error| EgressError::new("NETWORK", format!("{origin}: {error}")))?;
            let status = response.status();

            if status.is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let Some(location) = location else {
                    return Err(EgressError::new(
                        "BAD_REDIRECT",
                        format!("{origin} redirected with no Location header"),
                    ));
                };
                let next = current.join(&location).map_err(|error| {
                    EgressError::new(
                        "BAD_REDIRECT",
                        format!("{origin} redirected to {location}, which is not a URL: {error}"),
                    )
                })?;
                if !matches!(next.scheme(), "http" | "https") {
                    return Err(EgressError::new(
                        "UNSUPPORTED_SCHEME",
                        format!(
                            "{origin} redirected to {}, which is not http or https",
                            next
                        ),
                    ));
                }
                current = next;
                continue;
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            if !text_like(&content_type) {
                return Err(EgressError::new(
                    "UNSUPPORTED_TYPE",
                    format!(
                        "{origin} returned {:?}; fetch_url only reads text-like content",
                        if content_type.is_empty() {
                            "no content-type"
                        } else {
                            &content_type
                        }
                    ),
                ));
            }

            let cap = prepared.max_bytes;
            let declared = response
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok());
            let mut truncated = declared.is_some_and(|length| length > cap);
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk
                    .map_err(|error| EgressError::new("NETWORK", format!("{origin}: {error}")))?;
                if body.len() >= cap {
                    truncated = true;
                    break;
                }
                let take = (cap - body.len()).min(chunk.len());
                body.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    truncated = true;
                    break;
                }
            }

            let text = String::from_utf8_lossy(&body).to_string();
            return Ok(Fetched {
                url: current.to_string(),
                origin,
                address: prepared.pinned.address.to_string(),
                status: status.as_u16(),
                content_type,
                bytes: body.len(),
                truncated,
                text,
            });
        }

        Err(EgressError::new(
            "TOO_MANY_REDIRECTS",
            format!("more than {MAX_REDIRECTS} redirects from {start}"),
        ))
    }

    async fn authorize(&self, origin: &str, url: &str) -> Result<(), EgressError> {
        let policy = self.policy();
        let decision = policy.check(origin, None);
        if !decision.allowed {
            let code = match decision.code.as_str() {
                "NETWORK_OFF" => "NETWORK_OFF",
                "RULE_DENY" => "RULE_DENY",
                "NOT_ALLOWLISTED" => "NOT_ALLOWLISTED",
                "DEFAULT_DENY" => "DEFAULT_DENY",
                _ => "DENIED",
            };
            return Err(EgressError::new(code, decision.reason));
        }
        if self.granted(origin) {
            return Ok(());
        }
        let decision = self
            .approver
            .approve(&OriginRequest {
                url: url.to_string(),
                origin: origin.to_string(),
            })
            .await;
        match decision {
            OriginDecision::AllowOnce => Ok(()),
            OriginDecision::AllowSession => {
                self.remember_session(origin);
                Ok(())
            }
            OriginDecision::AllowAlways => {
                self.remember_session(origin);
                self.remember_always(origin);
                Ok(())
            }
            OriginDecision::Deny => Err(EgressError::new(
                "DENIED",
                format!("the user did not approve fetching {origin}; nothing was sent"),
            )),
        }
    }

    fn granted(&self, origin: &str) -> bool {
        // A session/always grant is not permission to ignore a later kill switch or a changed
        // allowlist. `authorize` checks the policy before this, so this only answers whether the
        // origin question can be skipped.
        self.session
            .lock()
            .map(|session| session.contains(origin))
            .unwrap_or(false)
            || self.always_has(origin)
    }

    fn remember_session(&self, origin: &str) {
        if let Ok(mut session) = self.session.lock() {
            session.insert(origin.to_string());
        }
    }

    fn always_has(&self, origin: &str) -> bool {
        let mut always = match self.always.lock() {
            Ok(always) => always,
            Err(_) => return false,
        };
        if always.is_none() {
            *always = Some(load_always(&self.store));
        }
        always
            .as_ref()
            .is_some_and(|origins| origins.contains(origin))
    }

    fn remember_always(&self, origin: &str) {
        let mut always = match self.always.lock() {
            Ok(always) => always,
            Err(_) => return,
        };
        let origins = always.get_or_insert_with(|| load_always(&self.store));
        origins.insert(origin.to_string());
        save_always(&self.store, origins);
    }
}

struct PreparedRequest {
    origin: String,
    pinned: PinnedResolution,
    client: reqwest::Client,
    max_bytes: usize,
}

fn policy_error(decision: &NetworkDecision) -> EgressError {
    let code = match decision.code.as_str() {
        "NETWORK_OFF" => "NETWORK_OFF",
        "RULE_DENY" => "RULE_DENY",
        "NOT_ALLOWLISTED" => "NOT_ALLOWLISTED",
        "DEFAULT_DENY" => "DEFAULT_DENY",
        _ => "DENIED",
    };
    EgressError::new(code, decision.reason.clone())
}

/// Read a JSON response only when the whole document fits the cap. A truncated JSON body is not
/// a smaller answer; it is an error, so Exa never parses a half document.
async fn read_limited(response: reqwest::Response, cap: usize) -> Result<Vec<u8>, EgressError> {
    if response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > cap)
    {
        return Err(EgressError::new(
            "EGRESS_SIZE_LIMIT",
            format!("the response declares more than the {cap}-byte network cap"),
        ));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| EgressError::new("NETWORK", error.to_string()))?;
        if body.len() + chunk.len() > cap {
            return Err(EgressError::new(
                "EGRESS_SIZE_LIMIT",
                format!("the response exceeded the {cap}-byte network cap"),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn parse_http_url(url: &str) -> Result<Url, EgressError> {
    let parsed = Url::parse(url.trim())
        .map_err(|error| EgressError::new("BAD_URL", format!("{url} is not a URL: {error}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(EgressError::new(
            "UNSUPPORTED_SCHEME",
            format!("{other} is not allowed; only http and https are supported"),
        )),
    }
}

fn load_always(path: &std::path::Path) -> HashSet<String> {
    // A missing or unreadable file is an empty list, never a panic and never a grant.
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<StoredOrigins>(&text).ok())
        .map(|stored| stored.origins.into_iter().collect())
        .unwrap_or_default()
}

fn save_always(path: &std::path::Path, origins: &HashSet<String>) {
    let mut sorted: Vec<&String> = origins.iter().collect();
    sorted.sort();
    let stored = StoredOrigins {
        origins: sorted.into_iter().cloned().collect(),
    };
    let Ok(text) = serde_json::to_string_pretty(&stored) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("json.tmp");
    if std::fs::write(&temporary, text).is_err() {
        return;
    }
    // Windows cannot rename over an existing file; the window between remove and rename only
    // means the next process asks again.
    let _ = std::fs::remove_file(path);
    let _ = std::fs::rename(&temporary, path);
}

/// The content types the tool will read. Anything else is refused rather than decoded as text.
pub fn text_like(content_type: &str) -> bool {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    base.starts_with("text/")
        || matches!(
            base.as_str(),
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/rss+xml"
                | "application/atom+xml"
                | "application/yaml"
                | "application/x-yaml"
                | "application/toml"
        )
        || base.ends_with("+json")
        || base.ends_with("+xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(action: NetworkAction, pattern: &str) -> NetworkRule {
        NetworkRule {
            action,
            pattern: pattern.to_string(),
            note: None,
        }
    }

    #[test]
    fn rules_are_ordered_and_deny_wins_before_consent() {
        let policy = NetworkPolicy {
            default: NetworkDefault::Deny,
            rules: vec![
                rule(NetworkAction::Deny, "https://api.example.com"),
                rule(NetworkAction::Allow, "https://api.example.com"),
                rule(NetworkAction::Allow, "*.example.com"),
            ],
            ..NetworkPolicy::default()
        };
        let denied = policy.check("https://api.example.com", None);
        assert!(!denied.allowed);
        assert_eq!(denied.code, "RULE_DENY");
        assert_eq!(denied.source, "rule");
        assert!(denied.reason.contains("api.example.com"));

        // A subdomain wildcard does not admit the apex, and matching is scheme-aware for an
        // origin pattern.
        assert!(policy.check("https://docs.example.com", None).allowed);
        assert!(!policy.check("https://example.com", None).allowed);
        assert!(policy.check("http://docs.example.com", None).allowed);
    }

    #[test]
    fn default_and_legacy_allowlist_have_one_meaning() {
        let deny = NetworkPolicy {
            default: NetworkDefault::Deny,
            ..NetworkPolicy::default()
        };
        assert_eq!(deny.check("https://example.com", None).code, "DEFAULT_DENY");

        let legacy = NetworkPolicy {
            allowlist: vec!["https://example.com".to_string()],
            ..NetworkPolicy::default()
        };
        let normalized = legacy.normalized().unwrap();
        assert!(normalized.allowlist.is_empty());
        assert_eq!(normalized.default, NetworkDefault::Deny);
        assert!(normalized.check("https://example.com/page", None).allowed);
        assert_eq!(
            normalized.check("https://other.example", None).code,
            "DEFAULT_DENY"
        );
    }

    #[test]
    fn a_session_switch_and_the_project_switch_refuse_before_matching() {
        let mut policy = NetworkPolicy {
            rules: vec![rule(NetworkAction::Allow, "*")],
            disabled_sessions: vec!["blocked".to_string()],
            ..NetworkPolicy::default()
        };
        assert_eq!(
            policy.check("https://example.com", Some("blocked")).code,
            "NETWORK_OFF"
        );
        policy.enabled = false;
        assert_eq!(
            policy.check("https://example.com", None).code,
            "NETWORK_OFF"
        );
    }

    #[test]
    fn patterns_the_matcher_cannot_enforce_are_refused() {
        for pattern in [
            "",
            "example.com/path",
            "ftp://example.com",
            "https://*.example.com",
        ] {
            let policy = NetworkPolicy {
                rules: vec![rule(NetworkAction::Deny, pattern)],
                ..NetworkPolicy::default()
            };
            assert!(policy.validate().is_err(), "{pattern} should be refused");
        }
        let too_many = NetworkPolicy {
            rules: (0..=MAX_NETWORK_RULES)
                .map(|index| rule(NetworkAction::Allow, &format!("host-{index}.example")))
                .collect(),
            ..NetworkPolicy::default()
        };
        assert!(too_many.validate().is_err());
    }

    #[test]
    fn a_saved_policy_round_trips_and_a_bad_rule_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = NetworkPolicy {
            rules: vec![rule(NetworkAction::Deny, "tracker.example")],
            limits: EgressLimits {
                window_seconds: 15,
                per_origin_requests: 7,
                per_agent_requests: 9,
                max_response_bytes: 4_096,
                pin_dns: false,
                allow_private_addresses: true,
            },
            ..NetworkPolicy::default()
        };
        let saved = policy.save(tmp.path()).unwrap();
        assert_eq!(saved.rules[0].action, NetworkAction::Deny);
        let loaded = NetworkPolicy::load(tmp.path());
        assert_eq!(loaded, saved);
        assert_eq!(loaded.limits.window_seconds, 15);
        assert_eq!(loaded.limits.per_origin_requests, 7);
        assert_eq!(loaded.limits.max_response_bytes, 4_096);

        let bad = NetworkPolicy {
            rules: vec![rule(NetworkAction::Allow, "example.com/path")],
            ..NetworkPolicy::default()
        };
        assert!(bad.save(tmp.path()).is_err());
        assert_eq!(
            NetworkPolicy::load(tmp.path()).rules[0].pattern,
            "tracker.example",
            "the refused save leaves the previous policy alone"
        );
    }

    #[test]
    fn a_malformed_policy_file_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".rigel")).unwrap();
        std::fs::write(tmp.path().join(".rigel/network.json"), "{not json").unwrap();
        let policy = NetworkPolicy::load(tmp.path());
        assert!(!policy.enabled);
        assert_eq!(
            policy.check("https://example.com", None).code,
            "NETWORK_OFF"
        );
    }

    #[test]
    fn an_out_of_bounds_limit_fails_closed_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".rigel")).unwrap();
        std::fs::write(
            tmp.path().join(".rigel/network.json"),
            r#"{"enabled":true,"limits":{"window_seconds":0}}"#,
        )
        .unwrap();
        let policy = NetworkPolicy::load(tmp.path());
        assert!(!policy.enabled, "a malformed limit is not permission");
        assert_eq!(
            policy.check("https://example.com", None).code,
            "NETWORK_OFF"
        );
    }

    #[test]
    fn an_unknown_action_does_not_deserialize() {
        let text = r#"{"rules":[{"action":"block","pattern":"example.com"}]}"#;
        assert!(serde_json::from_str::<NetworkPolicy>(text).is_err());
    }

    #[test]
    fn limits_are_bounded_and_zero_caps_mean_unbounded_counts() {
        let defaults = EgressLimits::default();
        assert_eq!(defaults.window_seconds, 60);
        assert_eq!(defaults.per_origin_requests, 30);
        assert_eq!(defaults.per_agent_requests, 120);
        assert_eq!(defaults.max_response_bytes, MAX_FETCH_BYTES);
        assert!(defaults.pin_dns);
        assert!(!defaults.allow_private_addresses);
        assert!(defaults.validate().is_ok());

        let unlimited = EgressLimits {
            per_origin_requests: 0,
            per_agent_requests: 0,
            ..EgressLimits::default()
        };
        assert!(unlimited.validate().is_ok());

        for invalid in [
            EgressLimits {
                window_seconds: 0,
                ..EgressLimits::default()
            },
            EgressLimits {
                max_response_bytes: 0,
                ..EgressLimits::default()
            },
            EgressLimits {
                window_seconds: MAX_EGRESS_WINDOW_SECONDS + 1,
                ..EgressLimits::default()
            },
            EgressLimits {
                max_response_bytes: MAX_EGRESS_RESPONSE_BYTES + 1,
                ..EgressLimits::default()
            },
            EgressLimits {
                per_origin_requests: MAX_EGRESS_REQUESTS + 1,
                ..EgressLimits::default()
            },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?} should be refused");
        }
    }

    #[test]
    fn a_window_counts_per_origin_and_per_agent_then_refills() {
        let start = Instant::now();
        let limits = EgressLimits {
            window_seconds: 10,
            per_origin_requests: 1,
            per_agent_requests: 2,
            ..EgressLimits::default()
        };
        let mut window = EgressWindow::new(start);

        let first = window
            .try_acquire("https://a.test", "agent", &limits, start)
            .unwrap();
        assert_eq!(first.origin_requests, 1);
        assert_eq!(first.agent_requests, 1);
        assert_eq!(first.remaining_seconds, 10);

        let (code, retry, message) = window
            .try_acquire("https://a.test", "agent", &limits, start)
            .unwrap_err();
        assert_eq!(code, "EGRESS_RATE_LIMIT");
        assert_eq!(retry, 10);
        assert!(message.contains("https://a.test"));

        let other = window
            .try_acquire("https://b.test", "agent", &limits, start)
            .unwrap();
        assert_eq!(other.agent_requests, 2);
        let (code, _, _) = window
            .try_acquire("https://c.test", "agent", &limits, start)
            .unwrap_err();
        assert_eq!(code, "EGRESS_AGENT_LIMIT");

        // The next window starts exactly at the boundary and clears both counters.
        let refilled = window
            .try_acquire(
                "https://a.test",
                "agent",
                &limits,
                start + Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(refilled.origin_requests, 1);
        assert_eq!(refilled.agent_requests, 1);
        assert_eq!(refilled.remaining_seconds, 10);
    }

    #[test]
    fn local_and_special_addresses_are_refused_but_public_ones_are_not() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "198.18.0.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                address_refusal(address.parse().unwrap()).is_some(),
                "{address} should be refused"
            );
        }
        for address in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(
                address_refusal(address.parse().unwrap()).is_none(),
                "{address} should be allowed"
            );
        }
    }

    #[test]
    fn only_text_like_types_are_read() {
        for allowed in [
            "text/html; charset=utf-8",
            "text/plain",
            "application/json",
            "application/json; charset=utf-8",
            "application/ld+json",
            "application/xhtml+xml",
            "application/atom+xml",
            "application/problem+json",
            "application/soap+xml",
        ] {
            assert!(text_like(allowed), "{allowed} should be readable");
        }
        for refused in [
            "",
            "image/png",
            "application/pdf",
            "application/octet-stream",
            "video/mp4",
            "application/zip",
        ] {
            assert!(!text_like(refused), "{refused} should be refused");
        }
    }
}
