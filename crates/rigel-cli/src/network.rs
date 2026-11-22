//! `rigel network`: the project policy the window edits, readable and testable without a turn.
//!
//! `show` prints the normalized policy and its egress limits, `check` explains one URL through
//! the same decision the egress tools use (optionally resolving and pinning it), and `set`
//! writes a JSON file through the same validation as the editor.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Subcommand;
use rigel_core::{
    DenyOrigins, Egress, EgressError, NetworkAction, NetworkDefault, NetworkPolicy,
    PinnedResolution,
};

#[derive(Subcommand)]
pub enum NetworkCommand {
    /// Print the project's effective policy: switch, default, rules, limits and disabled sessions.
    Show {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Explain whether one URL may be considered, and which part of the policy decided it.
    /// Exit 1 when the policy refuses the URL; no request is made.
    Check {
        #[arg(long)]
        url: String,
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Judge the URL as this session, including its kill switch.
        #[arg(long)]
        session: Option<String>,
        /// Resolve the host and report the address that would be pinned. No request is sent.
        #[arg(long)]
        resolve: bool,
        #[arg(long)]
        json: bool,
    },

    /// Validate and save a policy file. The same rules and bounds as the window's editor.
    Set {
        /// A JSON policy, in the shape `show --json` prints.
        #[arg(long, value_name = "FILE")]
        file: PathBuf,
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(command: NetworkCommand) -> ExitCode {
    match command {
        NetworkCommand::Show { project, json } => show(&project, json),
        NetworkCommand::Check {
            url,
            project,
            session,
            resolve,
            json,
        } => check(&project, &url, session.as_deref(), resolve, json).await,
        NetworkCommand::Set {
            file,
            project,
            json,
        } => set(&project, &file, json),
    }
}

fn show(project: &std::path::Path, as_json: bool) -> ExitCode {
    let policy = NetworkPolicy::load(project);
    if as_json {
        return print_json(&policy);
    }
    println!("kill switch: {}", if policy.enabled { "off" } else { "on" });
    println!(
        "default:     {}",
        match policy.default {
            NetworkDefault::Ask => "ask",
            NetworkDefault::Deny => "deny",
        }
    );
    if policy.rules.is_empty() {
        println!("rules:       none");
    } else {
        println!("rules:");
        for (index, rule) in policy.rules.iter().enumerate() {
            let action = match rule.action {
                NetworkAction::Allow => "allow",
                NetworkAction::Deny => "deny",
            };
            let note = rule
                .note
                .as_deref()
                .map(|note| format!(" — {note}"))
                .unwrap_or_default();
            println!("  {}. {action} {}{note}", index + 1, rule.pattern);
        }
    }
    let limits = &policy.limits;
    println!(
        "limits:      {} request(s)/origin, {} request(s)/agent per {}s, {} byte cap",
        limits.per_origin_requests,
        limits.per_agent_requests,
        limits.window_seconds,
        limits.max_response_bytes
    );
    println!(
        "pinning:     {}; private addresses {}",
        if limits.pin_dns { "on" } else { "off" },
        if limits.allow_private_addresses {
            "allowed"
        } else {
            "refused"
        }
    );
    if policy.disabled_sessions.is_empty() {
        println!("disabled:    none");
    } else {
        println!("disabled:    {}", policy.disabled_sessions.join(", "));
    }
    ExitCode::SUCCESS
}

async fn check(
    project: &std::path::Path,
    url: &str,
    session: Option<&str>,
    resolve: bool,
    as_json: bool,
) -> ExitCode {
    let policy = NetworkPolicy::load(project);
    let result = match policy.check_url(url, session) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };
    let mut pin_error: Option<EgressError> = None;
    let pinned = if resolve && result.decision.allowed {
        let egress =
            Egress::new(Arc::new(DenyOrigins), project.to_path_buf()).with_policy(policy.clone());
        match egress
            .pin_preview(url, session.unwrap_or("network-check"))
            .await
        {
            Ok(pinned) => Some(pinned),
            Err(error) => {
                pin_error = Some(error);
                None
            }
        }
    } else {
        None
    };

    if as_json {
        let report = serde_json::json!({
            "origin": result.origin,
            "decision": result.decision,
            "limits": policy.limits,
            "pinned": pinned,
            "pin_error": pin_error.as_ref().map(|error| serde_json::json!({
                "code": error.code,
                "message": error.message,
                "retry_after_secs": error.retry_after_secs,
            })),
        });
        match serde_json::to_string_pretty(&report) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("rigel: {error}");
                return ExitCode::from(2);
            }
        }
    } else {
        println!("origin:  {}", result.origin);
        println!(
            "decision: {} ({})",
            if result.decision.allowed {
                "allowed"
            } else {
                "denied"
            },
            result.decision.source
        );
        if !result.decision.allowed {
            println!("code:    {}", result.decision.code);
        }
        println!("reason:  {}", result.decision.reason);
        let limits = &policy.limits;
        println!(
            "limits:  {} origin / {} agent per {}s, {} byte cap",
            limits.per_origin_requests,
            limits.per_agent_requests,
            limits.window_seconds,
            limits.max_response_bytes
        );
        if let Some(pinned) = &pinned {
            print_pin(pinned);
        }
        if let Some(error) = &pin_error {
            eprintln!("rigel: {}: {}", error.code, error.message);
        }
    }
    ExitCode::from(if result.decision.allowed && pin_error.is_none() {
        0
    } else {
        1
    })
}

fn print_pin(pinned: &PinnedResolution) {
    println!(
        "pin:     {}:{} -> {} (checked {} address(es))",
        pinned.host,
        pinned.port,
        pinned.address,
        pinned.addresses.len()
    );
}

fn set(project: &std::path::Path, file: &std::path::Path, as_json: bool) -> ExitCode {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("rigel: read {}: {error}", file.display());
            return ExitCode::from(2);
        }
    };
    let policy: NetworkPolicy = match serde_json::from_str(&text) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("rigel: parse {}: {error}", file.display());
            return ExitCode::from(2);
        }
    };
    let saved = match policy.save(project) {
        Ok(saved) => saved,
        Err(error) => {
            eprintln!("rigel: {error}");
            return ExitCode::from(2);
        }
    };
    if as_json {
        print_json(&saved)
    } else {
        println!(
            "saved {} rule(s), default {}, kill switch {}",
            saved.rules.len(),
            match saved.default {
                NetworkDefault::Ask => "ask",
                NetworkDefault::Deny => "deny",
            },
            if saved.enabled { "off" } else { "on" }
        );
        ExitCode::SUCCESS
    }
}

fn print_json(value: &impl serde::Serialize) -> ExitCode {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rigel: {error}");
            ExitCode::from(2)
        }
    }
}
