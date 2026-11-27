#![recursion_limit = "512"]
//! `rigel` — the CLI half of the app.
//!
//! Everything here goes through the same core the window uses, which is what makes the
//! Phase 1 exit test meaningful: two separate processes, one database, one session. Phase 2
//! adds the skills surface, and with it the rule from §7.2 that the UI never parses human
//! output — every command the renderer shells out to has `--json`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use rigel_core::{SessionService, SkillIndex, SkillsError, Store, default_index_dir};
use serde_json::json;

mod audit;
mod backup;
mod claims;
mod cloud;
mod completions;
mod db;
mod doctor;
mod handoff;
mod harness;
mod hooks;
mod impact;
mod local;
mod mail;
mod network;
mod phase3;
mod plugins;
mod proof;
mod route;
mod rules;
mod serve;
mod skills;
mod stats;
mod testing;
mod tools;
mod update;
mod worktree;

use harness::{CodexCommand, HarnessCommand};
use phase3::{AgentCommand, KeyCommand, MemoryCommand, PatchCommand, SnapshotCommand};

#[derive(Parser)]
#[command(name = "rigel", version, about = "Rigel — an agent workspace")]
pub(crate) struct Cli {
    /// Where `index.db` and `blobs/` live. Defaults to the per-user app data directory.
    #[arg(long, global = true, env = "RIGEL_INDEX_DIR", value_name = "DIR")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report local state. Exit 0 is all clear, 1 has warnings, 2 has failures.
    Doctor {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// Also write a support archive (versions, schema, redacted settings and logs).
        #[arg(long, value_name = "FILE")]
        support_bundle: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        /// Apply the repairs the report lists, instead of only reporting them.
        #[arg(long)]
        fix: bool,
    },

    /// Local turn metrics: latency, tokens, retries and cache hits. No network call.
    Stats {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Create or migrate a project's session database.
    Migrate {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
    },

    /// Inspect or change at-rest encryption for a project database.
    Db {
        #[command(subcommand)]
        command: db::DbCommand,
    },

    /// Session operations for one project.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },

    /// Generate and install shell completions from this command tree.
    Completions {
        #[command(subcommand)]
        command: completions::CompletionCommand,
    },

    /// Package a session's unfinished work as a prompt for another agent.
    Handoff(handoff::HandoffArgs),

    /// Optional git hooks: secret scan, attribution trailer and commit check.
    Hooks {
        #[command(subcommand)]
        command: hooks::HooksCommand,
    },

    /// The skill index: search it offline, extend it from the network.
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },

    /// Installed plugins and the curated catalogue.
    Plugins {
        #[command(subcommand)]
        command: plugins::PluginCommand,
    },

    /// The native agent: one turn, billed to the session.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },

    /// Proposed and applied file patches.
    Patch {
        #[command(subcommand)]
        command: PatchCommand,
    },

    /// Committed memory: read, write, rank and pin facts.
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },

    /// Provider keys in the OS keychain.
    Keys {
        #[command(subcommand)]
        command: KeyCommand,
    },

    /// Pre-destructive snapshots of uncommitted work.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },

    /// Encrypted backups of the global data directory.
    Backup {
        #[command(subcommand)]
        command: backup::BackupCommand,
    },

    /// Plan and run the tests a change can affect.
    Test {
        #[command(subcommand)]
        command: testing::TestCommand,
    },

    /// Show the packages, tests and owners a change reaches, without running anything.
    Impact {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        /// The git revision to compare with. Defaults to HEAD.
        #[arg(long, default_value = "HEAD", value_name = "REV")]
        since: String,
        /// Use this changed path instead of asking git; repeat for more than one.
        #[arg(long, value_name = "PATH")]
        changed_file: Vec<String>,
        /// Ask for the full suite even when the change is small and mapped.
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },

    /// Record and verify proof-carrying evidence for the current change.
    Proof {
        #[command(subcommand)]
        command: proof::ProofCommand,
    },

    /// Explain which provider and model a turn would use, and why.
    Route {
        #[command(subcommand)]
        command: route::RouteCommand,
    },

    /// Explain which project rule wins and why.
    Rules {
        #[command(subcommand)]
        command: rules::RulesCommand,
    },

    /// The project network policy: show it, explain a URL, or save a validated policy.
    Network {
        #[command(subcommand)]
        command: network::NetworkCommand,
    },

    /// Verify the tamper-evident approval chain, or read one command's provenance.
    Audit {
        #[command(subcommand)]
        command: audit::AuditCommand,
    },

    /// Advisory file claims: who holds what, and release one by hand.
    Claim {
        #[command(subcommand)]
        command: claims::ClaimCommand,
    },

    /// Tools on this machine: what is present, what a skill needs, what to install.
    Tools {
        #[command(subcommand)]
        command: tools::ToolCommand,
    },

    /// The arithmetic behind an install decision.
    Trust {
        #[command(subcommand)]
        command: tools::TrustCommand,
    },

    /// Signed updates; GitHub is checked at launch.
    Update {
        #[command(subcommand)]
        command: update::UpdateCommand,
    },

    /// The local llama.cpp server behind the `llamacpp` door: start it, stop it, show its mode.
    Local {
        #[command(subcommand)]
        command: Option<local::LocalCommand>,
    },

    /// The user's own Firebase project: sign in and mirror the shelf.
    Cloud {
        #[command(subcommand)]
        command: cloud::CloudCommand,
    },

    /// The AgentMail inbox the agent reads and sends from.
    Mail {
        #[command(subcommand)]
        command: mail::MailCommand,
    },

    /// One private git checkout per agent, merged back through the patch pipeline.
    Worktree {
        #[command(subcommand)]
        command: worktree::WorktreeCommand,
    },

    /// The Codex harness: probe it, or run one turn through it.
    Codex {
        #[command(subcommand)]
        command: CodexCommand,
    },

    /// The Claude Code harness: probe it, or run one turn through it.
    Claude {
        #[command(subcommand)]
        command: harness::ClaudeCommand,
    },

    /// Meta's Muse Code harness: probe it, or run one headless turn through `muse exec --json`.
    Muse {
        #[command(subcommand)]
        command: harness::MuseCommand,
    },

    /// Every harness on this machine, cached in `adapters.json`.
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },

    /// The model roster every provider with a credential offers, as the picker shows it.
    Models {
        /// One provider's own list instead of the merged roster.
        #[arg(long, value_name = "ID")]
        provider: Option<String>,
        /// The provider whose rows lead the merged list (the window's saved one is not read
        /// here; the local router is the app's default door).
        #[arg(long, value_name = "ID")]
        current: Option<String>,
        /// Include rows that cannot answer a chat turn: image checkpoints, video and music
        /// models, embedding and moderation endpoints. The picker hides these by default.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },

    /// Spend across every session in a project (D92).
    Spend {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Serve the coordinator and the project workspace over an authenticated HTTP API.
    Serve {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long, default_value_t = serve::DEFAULT_PORT)]
        port: u16,
        /// Address to bind: loopback, or one interface address such as a Tailscale IP. 0.0.0.0 is refused.
        #[arg(long, value_name = "ADDR")]
        host: Option<String>,
        /// Bind this machine's Tailscale IPv4 address, discovered with `tailscale ip -4`.
        #[arg(long, conflicts_with = "host")]
        tailscale: bool,
        /// Let the API read, write and edit any path this user can, and start a command in any
        /// directory. Off by default: the workspace stays inside the project.
        #[arg(long)]
        full_access: bool,
        /// The base URL a client should use, when a tunnel forwards to the loopback port.
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// Register a logon task that runs this command with the same flags, then exit.
        #[arg(long)]
        install: bool,
        /// Remove that logon task, then exit.
        #[arg(long)]
        uninstall: bool,
        /// Hide the console window at startup; the logon entry passes this.
        #[arg(long)]
        hide_window: bool,
        /// Replace the stored token before starting.
        #[arg(long)]
        rotate: bool,
        /// Print the token and exit without serving.
        #[arg(long)]
        print_token: bool,
        #[arg(long)]
        json: bool,
    },

    /// Run the stdio MCP server an adapter injects into an agent.
    Mcp {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        session: String,
        #[arg(long)]
        agent: String,
        /// The turn whose transcript receives MCP approval decisions, when one exists.
        #[arg(long)]
        turn: Option<String>,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    /// Start a session and print its id.
    Start {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        budget_cents: Option<i64>,
    },

    /// List sessions, newest first.
    List {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Name an untitled session with the cheapest model that answers.
    Title {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
        /// The first prompt: the title describes this work.
        #[arg(long)]
        prompt: String,
        #[arg(long)]
        json: bool,
    },

    /// Show the compact summaries stored when each turn closed.
    Summary {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// End a session.
    End {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
    },

    /// Permanently delete an archived session and every local row beneath it.
    Delete {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
        /// Repeat the session id here; deletion cannot be undone.
        #[arg(long, value_name = "ID")]
        confirm: String,
    },

    /// Reconcile a session with the tree and the patch journal after a kill.
    Resume {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        id: String,
        /// Continue on a tree that no longer matches the checkpoint (recorded as an intervention).
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SkillCommand {
    /// Save a JSON golden-task suite for a skill id before it is installed or updated.
    Eval {
        #[command(subcommand)]
        command: SkillEvalCommand,
    },
    /// Create a starter `SKILL.md` in a new skill directory.
    Init {
        #[arg(default_value = "SKILL.md", value_name = "FILE")]
        file: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },

    /// Validate frontmatter and run the same security scanner used during install.
    Lint {
        #[arg(default_value = "SKILL.md", value_name = "FILE")]
        file: PathBuf,
    },

    /// Exercise the install-time load gate for a local skill file.
    Test {
        #[arg(default_value = "SKILL.md", value_name = "FILE")]
        file: PathBuf,
    },

    /// Full-text search over the local index. Never touches the network.
    Search {
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Show one skill: metadata, static review, evidence, variants.
    Get {
        id: String,
        /// Write the body to `<DIR>/<name>/SKILL.md`.
        #[arg(long, value_name = "DIR")]
        to: Option<PathBuf>,
        /// Print the body hash a session will pin to (binding lands in Phase 3).
        #[arg(long)]
        pin: bool,
        #[arg(long)]
        json: bool,
    },

    /// Remove one row from the local shelf; the next `cloud push` writes a tombstone.
    Remove {
        id: String,
        #[arg(long)]
        json: bool,
    },

    /// Seed the shelf from the published catalogue: metadata only, no bodies, nothing reviewed.
    Seed {
        /// Override the catalogue URL (a local fixture, or another project's database).
        #[arg(long)]
        url: Option<String>,
        /// Stop after this many rows; 0 seeds everything.
        #[arg(long, default_value_t = 0)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Walk a GitHub repository (or URL) into the index.
    Index {
        target: String,
        /// Revalidate every cached URL instead of trusting the freshness window.
        #[arg(long)]
        force: bool,
        /// Cap the number of skills taken from one repository.
        #[arg(long, default_value_t = rigel_core::skills::github::DEFAULT_REPO_LIMIT)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Current health of every source.
    Status {
        #[arg(long)]
        json: bool,
    },

    /// Pull rows from a marketplace into the index.
    Sync {
        /// `skillsmp` or `skillhub`.
        source: String,
        #[arg(long, default_value = "agent")]
        query: String,
        #[arg(long, default_value_t = 30)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },

    /// Keep checking: one forced pass, then refresh whatever is due every interval until stopped.
    /// This is the headless form of the window's background freshness loop.
    Watch {
        /// Seconds between checks; values below 30 are raised to 30.
        #[arg(long, default_value_t = 300, value_name = "SECONDS")]
        interval: u64,
        #[arg(long)]
        json: bool,
    },

    /// Re-sync every source that is not fresh: the repositories this index holds (at most three,
    /// most recently seen first) and one page of each marketplace. What the window runs at launch
    /// and on its background passes when the Shelf's auto-update switch is on.
    Refresh {
        #[arg(long)]
        json: bool,
    },

    /// Sweep both marketplaces across many pages into the index (metadata only, no bodies).
    Sweep {
        /// SkillHub list pages to walk, 100 cards each. 0 skips the marketplace.
        #[arg(long, default_value_t = 0)]
        skillhub_pages: u32,
        /// SkillsMP search queries to walk, comma-separated, each up to `--skillsmp-pages` pages of 50.
        #[arg(
            long,
            default_value = "skill,claude,code,test,commit,review,deploy,automation,agent,data,docs,security,api,design,pdf,summarize,write,translate,debug,git"
        )]
        skillsmp_queries: String,
        /// SkillsMP pages per query, 50 rows each.
        #[arg(long, default_value_t = 0)]
        skillsmp_pages: u32,
        #[arg(long)]
        json: bool,
    },

    /// Show index size, blob count and source health.
    Stats {
        #[arg(long)]
        json: bool,
    },

    /// Import the prototype's export, or one skills.sh page with its dated audits.
    Import {
        /// The prototype's `skilldb:entries:v1` JSON export.
        #[arg(long, value_name = "FILE", conflicts_with = "skills_sh")]
        from: Option<PathBuf>,
        /// A skills.sh skill page, e.g. https://www.skills.sh/owner/repo/skill.
        #[arg(long = "skills-sh", value_name = "URL")]
        skills_sh: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Index the skills that ship with Rigel, and optionally copy them into the harness stores.
    Builtin {
        /// Copy every built-in body into `~/.agents/skills` and `~/.claude/skills` as well.
        #[arg(long)]
        install: bool,
        /// Install into this directory instead of the two default harness stores.
        #[arg(long, value_name = "DIR")]
        to: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Delete blobs no row references.
    Gc {
        #[arg(long)]
        json: bool,
    },

    /// Write `rigel.lock` with the selected skills' exact versions and hashes.
    Lock {
        ids: Vec<String>,
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long, value_name = "FILE")]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },

    /// Recreate skills from `rigel.lock` and local hash-addressed blobs, without opening SQLite.
    Restore {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long, value_name = "FILE")]
        lock: Option<PathBuf>,
        /// Install into these roots instead of the default harness stores.
        #[arg(long, value_name = "DIR")]
        to: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SkillEvalCommand {
    Set {
        id: String,
        #[arg(value_name = "FILE")]
        file: PathBuf,
    },
}

fn data_dir(cli: &Cli) -> PathBuf {
    cli.data_dir.clone().unwrap_or_else(default_index_dir)
}

/// Clap walks the whole command tree, and the run future holds every arm, on the thread that
/// calls it. The Windows main thread has 1 MiB; a tree this size overflows it in a debug build.
/// Give the real entry point room instead of tuning each command.
fn main() -> ExitCode {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(entry)
        .map(|handle| handle.join().unwrap_or_else(|_| ExitCode::from(2)))
        .unwrap_or_else(|error| {
            eprintln!("rigel: could not start the command thread: {error}");
            ExitCode::from(2)
        })
}

#[tokio::main]
async fn entry() -> ExitCode {
    let cli = Cli::parse();
    let dir = data_dir(&cli);

    match cli.command {
        Command::Doctor {
            project,
            support_bundle,
            json: as_json,
            fix,
        } => {
            if fix {
                doctor::fix::run(&project).await
            } else {
                doctor::run(&project, &dir, as_json, support_bundle.as_deref()).await
            }
        }

        Command::Stats {
            project,
            json: as_json,
        } => stats::run(&project, as_json).await,

        Command::Migrate { project } => match Store::open_project(&project).await {
            Ok(store) => {
                println!("{}", store.path().display());
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rigel: {error}");
                ExitCode::from(2)
            }
        },

        Command::Db { command } => db::run(command).await,

        Command::Serve {
            project,
            port,
            host,
            tailscale,
            full_access,
            public_url,
            rotate,
            print_token,
            install,
            uninstall,
            hide_window,
            json: as_json,
        } => {
            serve::run(serve::ServeOptions {
                project,
                data_dir: dir.clone(),
                host: host.unwrap_or_else(|| serve::DEFAULT_HOST.to_string()),
                port,
                tailscale,
                full_access,
                public_url,
                rotate,
                print_token,
                install,
                uninstall,
                hide_window,
                as_json,
            })
            .await
        }

        Command::Route { command } => route::run(command),

        Command::Rules { command } => rules::run(command),

        Command::Network { command } => network::run(command).await,

        Command::Session { command } => run_session(command).await,

        Command::Completions { command } => completions::run(command),

        // Boxed so this large command future does not inflate `main`'s already large state
        // machine; the native Windows stack is the limiting factor here.
        Command::Handoff(args) => Box::pin(handoff::run(args)).await,

        Command::Hooks { command } => hooks::run(command),

        Command::Skills { command } => run_skills(&dir, command).await,

        Command::Plugins { command } => plugins::run(&dir, command).await,

        Command::Agent { command } => phase3::run_agent(&dir, command).await,

        Command::Patch { command } => phase3::run_patch(command).await,

        Command::Memory { command } => phase3::run_memory(command).await,

        Command::Keys { command } => phase3::run_keys(command).await,

        Command::Snapshot { command } => phase3::run_snapshot(command, dir),

        Command::Backup { command } => backup::run(command, dir).await,

        Command::Test { command } => testing::run(command).await,

        Command::Impact {
            project,
            since,
            changed_file,
            full,
            json: as_json,
        } => impact::run(project, since, changed_file, full, as_json),

        Command::Proof { command } => proof::run(command).await,

        Command::Audit { command } => audit::run(command).await,

        Command::Claim { command } => claims::run_claim(command).await,

        Command::Codex { command } => harness::run_codex(command).await,

        Command::Claude { command } => harness::run_claude(command, &dir).await,

        Command::Muse { command } => harness::run_muse(command).await,

        Command::Harness { command } => harness::run_harness(command, &dir).await,

        Command::Models {
            provider,
            current,
            all,
            json: as_json,
        } => {
            let store = rigel_core::credentials::KeychainCredentialStore;
            // The window's saved provider lives in its own settings file, which this command does
            // not read; the router is the app's default door, so it leads unless one is named.
            let current = current.unwrap_or_else(|| rigel_core::credentials::OMNIROUTE.to_string());
            let rows = match provider {
                Some(id) => rigel_core::agent::catalog::catalog(&id, &store).await,
                None => rigel_core::agent::catalog::roster(&current, &store).await,
            };
            match rows {
                Ok(rows) => {
                    let hidden = rows.iter().filter(|option| !option.chat).count();
                    let shown: Vec<&rigel_core::agent::ModelOption> =
                        rows.iter().filter(|option| all || option.chat).collect();
                    if as_json {
                        // The machine-readable form carries **every** row with its `chat` flag:
                        // a caller that wants only chat rows can filter on the field, and a
                        // caller that wants the whole roster does not need a second request.
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&rows).unwrap_or_default()
                        );
                    } else {
                        for option in &shown {
                            // With `--all` the list mixes both kinds, so the tag says which
                            // rows the picker would have hidden instead of leaving it to guess.
                            let tag = match (all && !option.chat, option.free) {
                                (true, true) => "free · non-chat",
                                (true, false) => "non-chat",
                                (false, true) => "free",
                                (false, false) => "",
                            };
                            println!(
                                "  {:<46} {:<12} {:<16}{}",
                                truncate(&option.id, 46),
                                option.provider,
                                tag,
                                option
                                    .name
                                    .as_ref()
                                    .map(|name| format!("  {name}"))
                                    .unwrap_or_default()
                            );
                        }
                        if all {
                            println!(
                                "\n{} row(s) — {hidden} cannot answer a chat turn",
                                shown.len()
                            );
                        } else {
                            println!("\n{} chat row(s)", shown.len());
                            if hidden > 0 {
                                println!(
                                    "{hidden} row(s) cannot answer a chat turn — \
                                     image, video, audio or retrieval models. `--all` shows them."
                                );
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rigel: {error}");
                    ExitCode::from(2)
                }
            }
        }

        Command::Spend {
            project,
            json: as_json,
        } => match Store::open_project(&project).await {
            Ok(store) => match rigel_core::spend::report(store.pool()).await {
                Ok(report) => {
                    if as_json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report).unwrap_or_default()
                        );
                    } else {
                        println!("total  {}", dollars(report.total_micros));
                        println!("turns  {}", report.turns);
                        println!("\nby session");
                        for row in &report.by_session {
                            println!(
                                "  {:<40} {:>10}  ({} turns)",
                                truncate(&row.label, 40),
                                dollars(row.micros),
                                row.turns
                            );
                        }
                        println!("\nby day");
                        for row in &report.by_day {
                            println!(
                                "  {:<12} {:>10}  ({} turns)",
                                row.label,
                                dollars(row.micros),
                                row.turns
                            );
                        }
                        println!("\nby agent");
                        for row in &report.by_agent {
                            println!(
                                "  {:<40} {:>10}  ({} turns)",
                                truncate(&row.label, 40),
                                dollars(row.micros),
                                row.turns
                            );
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rigel: {error}");
                    ExitCode::from(2)
                }
            },
            Err(error) => {
                eprintln!("rigel: {error}");
                ExitCode::from(2)
            }
        },

        Command::Tools { command } => tools::run_tools(&dir, command).await,

        Command::Trust { command } => tools::run_trust(command),

        Command::Update { command } => update::run(&dir, command).await,

        Command::Local { command } => local::run(&dir, command).await,

        Command::Cloud { command } => cloud::run(&dir, command).await,

        Command::Mail { command } => mail::run(&dir, command).await,

        Command::Worktree { command } => worktree::run(&dir, command).await,

        Command::Mcp {
            project,
            session,
            agent,
            turn,
        } => phase3::run_mcp(&dir, project, session, agent, turn).await,
    }
}

async fn run_skills(dir: &Path, command: SkillCommand) -> ExitCode {
    match &command {
        SkillCommand::Init { file, name } => return skills::init(file, name.as_deref()),
        SkillCommand::Lint { file } => return skills::lint(file).await,
        SkillCommand::Test { file } => return skills::test(file).await,
        SkillCommand::Eval {
            command: SkillEvalCommand::Set { id, file },
        } => {
            let text = match std::fs::read_to_string(file) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("rigel: {}: {error}", file.display());
                    return ExitCode::from(1);
                }
            };
            let tasks = match serde_json::from_str::<Vec<rigel_core::skills::GoldenTask>>(&text) {
                Ok(tasks) => tasks,
                Err(error) => {
                    eprintln!("rigel: invalid golden task JSON: {error}");
                    return ExitCode::from(1);
                }
            };
            return match rigel_core::skills::GoldenSuiteStore::new(dir).set(id, tasks) {
                Ok(()) => {
                    println!("saved golden task suite for {id}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rigel: {error}");
                    ExitCode::from(1)
                }
            };
        }
        _ => {}
    }

    if let SkillCommand::Restore {
        project,
        lock,
        to,
        json: as_json,
    } = &command
    {
        let lock_path = lock.clone().unwrap_or_else(|| project.join("rigel.lock"));
        let lockfile = match rigel_core::skills::lockfile::SkillLockfile::read(&lock_path) {
            Ok(lockfile) => lockfile,
            Err(error) => return skills_failure(error, *as_json),
        };
        let roots = if to.is_empty() {
            plugins::skill_roots()
        } else {
            to.clone()
        };
        if roots.is_empty() {
            eprintln!("rigel: no HOME or USERPROFILE to restore skills into; pass --to <dir>");
            return ExitCode::from(2);
        }
        let report = match lockfile.restore(dir, project, &roots) {
            Ok(report) => report,
            Err(error) => return skills_failure(error, *as_json),
        };
        if *as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
        } else {
            println!("restored {} locked skills", report.restored);
            for path in report.paths {
                println!("  {}", path.display());
            }
        }
        return ExitCode::SUCCESS;
    }

    let index = match SkillIndex::open(dir).await {
        Ok(index) => index,
        Err(error) => return skills_failure(error, false),
    };

    match command {
        SkillCommand::Init { .. }
        | SkillCommand::Lint { .. }
        | SkillCommand::Test { .. }
        | SkillCommand::Eval { .. } => {
            unreachable!("authoring commands run before opening the skill index")
        }
        SkillCommand::Seed { url, limit, json } => {
            let index = match SkillIndex::open(dir).await {
                Ok(index) => index,

                Err(error) => return skills_failure(error, json),
            };

            let target = url.unwrap_or_else(|| SkillIndex::CATALOG_URL.to_string());

            match index.seed_catalog(&target, limit).await {
                Ok(report) => {
                    if json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&report).unwrap_or_default()
                        );
                    } else {
                        println!(
                            "seeded {} of {} rows from the published catalogue ({} had no id)",
                            report.written, report.rows, report.skipped
                        );

                        println!(
                            "no bodies were downloaded: seeded rows stay `not scanned` until their repo is indexed"
                        );
                    }

                    ExitCode::SUCCESS
                }

                Err(error) => skills_failure(error, json),
            }
        }

        SkillCommand::Search {
            query,
            limit,
            json: as_json,
        } => {
            let query = query.join(" ");
            match index.search(&query, limit).await {
                Ok(hits) => {
                    if as_json {
                        println!(
                            "{}",
                            json!({
                                "query": query,
                                "count": hits.len(),
                                "order": "grade -> stars -> recency",
                                "results": hits,
                            })
                        );
                    } else if hits.is_empty() {
                        println!(
                            "no skills match {query:?} — index a repo first: rigel skills index <repo>"
                        );
                    } else {
                        for hit in &hits {
                            println!(
                                "{}  {:<10}  {:<28}  {}",
                                hit.grade.clone().unwrap_or_else(|| "?".to_string()),
                                hit.source,
                                truncate(&hit.name, 28),
                                truncate(&hit.description, 80)
                            );
                            let mut notes = Vec::new();
                            if hit.variants > 1 {
                                notes.push(format!("{} variants", hit.variants));
                            }
                            if hit.unmeasured {
                                notes.push("unmeasured claim".to_string());
                            }
                            if !hit.has_body {
                                notes.push("no body".to_string());
                            }
                            if !notes.is_empty() {
                                println!("            {}", notes.join(" · "));
                            }
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(error, as_json),
            }
        }

        SkillCommand::Get {
            id,
            to,
            pin,
            json: as_json,
        } => match index.get(&id).await {
            Ok(Some(detail)) => {
                if let Some(dir) = &to {
                    let (body, sha) = match index.body(&id).await {
                        Ok(Some(body)) => body,
                        Ok(None) => {
                            return skills_failure(SkillsError::NoBody(id.clone()), as_json);
                        }
                        Err(error) => return skills_failure(error, as_json),
                    };
                    let folder = slugify(&detail.skill.name);
                    let destination = dir.join(&folder);
                    if let Err(error) = std::fs::create_dir_all(&destination) {
                        return skills_failure(SkillsError::Io(error), as_json);
                    }
                    let file = destination.join("SKILL.md");
                    if let Err(error) = std::fs::write(&file, body) {
                        return skills_failure(SkillsError::Io(error), as_json);
                    }
                    if as_json {
                        println!(
                            "{}",
                            json!({ "detail": detail, "written": file, "body_sha": sha, "pinned": pin })
                        );
                    } else {
                        println!("{}", file.display());
                        println!("  sha {sha}{}", if pin { " (pinned)" } else { "" });
                    }
                    return ExitCode::SUCCESS;
                }

                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&detail).unwrap_or_default()
                    );
                    return ExitCode::SUCCESS;
                }

                if pin {
                    match index.body(&id).await {
                        Ok(Some((_, sha))) => println!(
                            "pin       {sha} — a session will bind to this hash in Phase 3\n"
                        ),
                        _ => println!("pin       no body to pin yet\n"),
                    }
                }
                print_detail(&detail);
                ExitCode::SUCCESS
            }
            Ok(None) => skills_failure(SkillsError::NotIndexed(id), as_json),
            Err(error) => skills_failure(error, as_json),
        },

        SkillCommand::Remove { id, json: as_json } => match index.remove(&id).await {
            Ok(true) => {
                if as_json {
                    println!("{}", json!({ "ok": true, "removed": id }));
                } else {
                    println!("removed    {id}");
                    println!(
                        "           the next `rigel cloud push` writes a tombstone so other \
                         machines drop it too"
                    );
                }
                ExitCode::SUCCESS
            }
            Ok(false) => skills_failure(SkillsError::NotIndexed(id), as_json),
            Err(error) => skills_failure(error, as_json),
        },

        SkillCommand::Index {
            target,
            force,
            limit,
            json: as_json,
        } => {
            let suites = match rigel_core::skills::GoldenSuiteStore::new(dir).load() {
                Ok(suites) => suites,
                Err(error) => {
                    eprintln!("rigel: {error}");
                    return ExitCode::from(1);
                }
            };
            let runner = phase3::CliSkillEvalRunner;
            let policy = if force {
                rigel_core::SyncPolicy::forced()
            } else {
                rigel_core::SyncPolicy::normal()
            };
            match index
                .index_repo_with_limit_and_evals(&target, policy, limit, &suites, &runner)
                .await
            {
                Ok(report) => {
                    if as_json {
                        println!("{}", serde_json::to_string(&report).unwrap_or_default());
                    } else {
                        println!(
                            "{} skills from {}@{}  (+{} added, {} updated, {} unchanged)",
                            report.with_body,
                            report.repo.clone().unwrap_or_default(),
                            report.branch.clone().unwrap_or_default(),
                            report.added,
                            report.updated,
                            report.unchanged
                        );
                        println!(
                            "  {} of {} found · {} requests · {} clusters",
                            report.with_body, report.found, report.http_requests, report.clusters
                        );
                        if report.truncated {
                            println!(
                                "  note: GitHub truncated the tree; some skills were not seen"
                            );
                        }
                        if let Some(scan) = &report.scan {
                            println!("  SkillSpector {}", scan.summary());
                        }
                        if report.blocked > 0 {
                            println!(
                                "  SkillSpector blocked {} skill(s); they were not indexed",
                                report.blocked
                            );
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(error, as_json),
            }
        }

        SkillCommand::Status { json: as_json } => match index.status().await {
            Ok(statuses) => {
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&statuses).unwrap_or_default()
                    );
                } else if statuses.is_empty() {
                    println!("no sources yet — rigel skills index <repo>");
                } else {
                    for status in &statuses {
                        println!(
                            "{:<10} {:<8} last sync {}",
                            status.id,
                            format!("{:?}", status.state).to_lowercase(),
                            status
                                .last_sync_at
                                .map(rigel_core::skills::fmt_time)
                                .unwrap_or_else(|| "never".to_string())
                        );
                        if let Some(error) = &status.error {
                            println!("           {error}");
                        }
                    }
                }
                ExitCode::SUCCESS
            }
            Err(error) => skills_failure(error, as_json),
        },

        SkillCommand::Sync {
            source,
            query,
            limit,
            json: as_json,
        } => {
            let Some(kind) = rigel_core::skills::SourceKind::from_id(&source) else {
                return skills_failure(SkillsError::UnknownSource(source), as_json);
            };
            match index.sync_marketplace(kind, &query, limit).await {
                Ok(count) => {
                    if as_json {
                        println!("{}", json!({ "source": source, "rows": count }));
                    } else {
                        println!("{count} rows from {source}");
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(error, as_json),
            }
        }

        SkillCommand::Watch {
            interval,
            json: as_json,
        } => {
            let suites = match rigel_core::skills::GoldenSuiteStore::new(dir).load() {
                Ok(suites) => suites,
                Err(error) => {
                    return skills_failure(SkillsError::Other(error.to_string()), as_json);
                }
            };
            let runner = phase3::CliSkillEvalRunner;
            let every = Duration::from_secs(interval.max(30));
            // The first pass is forced, matching the window's launch pass: a shelf that looked
            // fresh when this process was started is still checked against its remote.
            match index.refresh_all_with_evals(&suites, &runner).await {
                Ok(report) => print_refresh(&report, as_json),
                Err(error) => return skills_failure(error, as_json),
            }
            loop {
                tokio::time::sleep(every).await;
                match index.refresh_stale_with_evals(&suites, &runner).await {
                    Ok(report) => print_refresh(&report, as_json),
                    Err(error) => return skills_failure(error, as_json),
                }
            }
        }

        SkillCommand::Refresh { json: as_json } => {
            let suites = match rigel_core::skills::GoldenSuiteStore::new(dir).load() {
                Ok(suites) => suites,
                Err(error) => {
                    return skills_failure(SkillsError::Other(error.to_string()), as_json);
                }
            };
            let runner = phase3::CliSkillEvalRunner;
            match index.refresh_stale_with_evals(&suites, &runner).await {
                Ok(report) => {
                    print_refresh(&report, as_json);
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(error, as_json),
            }
        }

        SkillCommand::Sweep {
            skillhub_pages,
            skillsmp_queries,
            skillsmp_pages,
            json: as_json,
        } => {
            let queries: Vec<String> = skillsmp_queries.split(',').map(str::to_owned).collect();
            match index.sweep(skillhub_pages, &queries, skillsmp_pages).await {
                Ok(report) => {
                    if as_json {
                        println!("{}", serde_json::to_string(&report).unwrap_or_default());
                    } else {
                        println!(
                            "skillhub    {} rows over {} page(s){}",
                            report.skillhub_rows,
                            report.skillhub_pages,
                            report
                                .skillhub_error
                                .as_deref()
                                .map(|error| format!(" — stopped: {error}"))
                                .unwrap_or_default()
                        );
                        println!(
                            "skillsmp    {} rows over {} query/ies × {} page(s){}",
                            report.skillsmp_rows,
                            report.skillsmp_queries,
                            report.skillsmp_pages,
                            report
                                .skillsmp_error
                                .as_deref()
                                .map(|error| format!(" — stopped: {error}"))
                                .unwrap_or_default()
                        );
                        println!("duplicates dropped {}", report.duplicates_dropped);
                        println!(
                            "index       {} → {} skills",
                            report.skills_before, report.skills_after
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(error, as_json),
            }
        }

        SkillCommand::Stats { json: as_json } => match index.stats().await {
            Ok(stats) => {
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&stats).unwrap_or_default()
                    );
                } else {
                    println!("skills      {}", stats.skills);
                    println!("with body   {}", stats.with_body);
                    println!("clusters    {}", stats.clusters);
                    println!(
                        "blobs       {} ({} KB)",
                        stats.blobs,
                        stats.blob_bytes / 1024
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => skills_failure(error, as_json),
        },

        SkillCommand::Import {
            from,
            skills_sh,
            json: as_json,
        } => match (from, skills_sh) {
            (Some(from), None) => {
                let text = match std::fs::read_to_string(&from) {
                    Ok(text) => text,
                    Err(error) => return skills_failure(SkillsError::Io(error), as_json),
                };
                match index.import_prototype_entries(&text).await {
                    Ok(report) => {
                        if as_json {
                            println!("{}", serde_json::to_string(&report).unwrap_or_default());
                        } else {
                            println!(
                                "imported {} · skipped {} (already present) · invalid {}",
                                report.imported, report.skipped, report.invalid
                            );
                        }
                        ExitCode::SUCCESS
                    }
                    Err(error) => skills_failure(error, as_json),
                }
            }
            (None, Some(url)) => {
                let policy = rigel_core::SyncPolicy::normal();
                match rigel_core::skills::skillssh::import(&index, &url, policy).await {
                    Ok(report) => {
                        if as_json {
                            println!("{}", serde_json::to_string(&report).unwrap_or_default());
                        } else {
                            println!(
                                "{}  {}",
                                report.repo,
                                report
                                    .skill_id
                                    .as_deref()
                                    .unwrap_or("(the repository no longer carries this skill)")
                            );
                            for audit in &report.audits {
                                println!(
                                    "  {:<22} {:<6} {}",
                                    audit.vendor,
                                    audit.verdict,
                                    audit.audited_on.as_deref().unwrap_or("date unavailable")
                                );
                            }
                            println!(
                                "  indexed {} skills ({} added, {} updated)",
                                report.indexed.found, report.indexed.added, report.indexed.updated
                            );
                        }
                        ExitCode::SUCCESS
                    }
                    Err(error) => skills_failure(error, as_json),
                }
            }
            (None, None) => {
                eprintln!(
                    "rigel: pass --from <file> for the prototype export, or --skills-sh <url> for a skills.sh page"
                );
                ExitCode::from(1)
            }
            (Some(_), Some(_)) => {
                eprintln!("rigel: pass one of --from or --skills-sh, not both");
                ExitCode::from(1)
            }
        },

        SkillCommand::Builtin {
            install,
            to,
            json: as_json,
        } => {
            let seeded = match index.install_builtin().await {
                Ok(report) => report,
                Err(error) => return skills_failure(error, as_json),
            };
            if !install {
                if as_json {
                    println!(
                        "{}",
                        json!({
                            "seeded": seeded,
                            "skills": rigel_core::skills::builtin::all().iter().map(|skill| json!({
                                "id": skill.id(),
                                "name": skill.name,
                                "path": skill.path(),
                            })).collect::<Vec<_>>()
                        })
                    );
                } else {
                    println!(
                        "indexed {} built-in skills ({}) — pass --install to copy them into the harness stores",
                        seeded.written,
                        rigel_core::skills::builtin::all()
                            .iter()
                            .map(|skill| skill.slug)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                return ExitCode::SUCCESS;
            }
            let roots = if to.is_empty() {
                plugins::skill_roots()
            } else {
                to
            };
            if roots.is_empty() {
                eprintln!("rigel: no HOME or USERPROFILE to install a skill into; pass --to <dir>");
                return ExitCode::from(2);
            }
            let mut installed = Vec::new();
            let suites = match rigel_core::skills::GoldenSuiteStore::new(dir).load() {
                Ok(suites) => suites,
                Err(error) => {
                    return skills_failure(SkillsError::Other(error.to_string()), as_json);
                }
            };
            let runner = phase3::CliSkillEvalRunner;
            for skill in rigel_core::skills::builtin::all() {
                let id = skill.id();
                let Some(tasks) = suites.get(&id) else {
                    return skills_failure(
                        SkillsError::Other(format!(
                            "skill {id} has no golden task suite; refusing to enable it"
                        )),
                        as_json,
                    );
                };
                match index.install_with_eval(&id, &roots, tasks, &runner).await {
                    Ok((report, _)) => installed.push(report),
                    Err(error) => return skills_failure(error, as_json),
                }
            }
            if as_json {
                println!(
                    "{}",
                    json!({ "seeded": seeded, "installed": installed, "roots": roots })
                );
            } else {
                for report in &installed {
                    println!(
                        "{} →  (SkillSpector {})",
                        report.name,
                        report.scan.summary()
                    );
                    for path in &report.paths {
                        println!("  {}", path.display());
                    }
                }
            }
            ExitCode::SUCCESS
        }

        SkillCommand::Gc { json: as_json } => {
            let live = match index.live_blobs().await {
                Ok(live) => live,
                Err(error) => return skills_failure(error, as_json),
            };
            match index.blobs().gc(&live) {
                Ok(removed) => {
                    if as_json {
                        println!("{}", json!({ "removed": removed, "live": live.len() }));
                    } else {
                        println!("removed {removed} orphan blobs, kept {}", live.len());
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => skills_failure(SkillsError::Io(error), as_json),
            }
        }

        SkillCommand::Lock {
            ids,
            project,
            output,
            json: as_json,
        } => {
            let lockfile =
                match rigel_core::skills::lockfile::SkillLockfile::generate(&index, &ids).await {
                    Ok(lockfile) => lockfile,
                    Err(error) => return skills_failure(error, as_json),
                };
            let path = output.unwrap_or_else(|| project.join("rigel.lock"));
            let serialized = match serde_json::to_vec_pretty(&lockfile) {
                Ok(serialized) => serialized,
                Err(error) => {
                    return skills_failure(SkillsError::Lockfile(error.to_string()), as_json);
                }
            };
            if let Err(error) = std::fs::write(&path, serialized) {
                return skills_failure(SkillsError::Io(error), as_json);
            }
            if as_json {
                println!(
                    "{}",
                    json!({ "written": path, "skills": lockfile.skills.len() })
                );
            } else {
                println!(
                    "locked {} skills to {}",
                    lockfile.skills.len(),
                    path.display()
                );
            }
            ExitCode::SUCCESS
        }

        SkillCommand::Restore { .. } => unreachable!("restore runs before opening the index"),
    }
}

fn print_detail(detail: &rigel_core::SkillDetail) {
    let skill = &detail.skill;
    println!("{}", skill.name);
    println!("  {}", skill.description);
    println!();
    println!("  id        {}", skill.id);
    println!("  source    {}", skill.source);
    if let Some(repo) = &skill.repo {
        println!("  repo      {repo}");
    }
    if let Some(url) = &skill.url {
        println!("  url       {url}");
    }
    println!(
        "  grade     {} ({})",
        skill
            .grade
            .clone()
            .unwrap_or_else(|| "not scanned".to_string()),
        skill.grade_scope.clone().unwrap_or_else(|| "—".to_string())
    );
    println!(
        "  body      {}",
        if skill.has_body {
            "indexed"
        } else {
            "not fetched"
        }
    );
    if let Some(seen) = skill.last_seen_at {
        println!("  last seen {}", rigel_core::skills::fmt_time(seen));
    }

    if !detail.capabilities.is_empty() {
        println!();
        println!("static review (does not prove a skill is harmless)");
        for capability in &detail.capabilities {
            println!("  [{}] {}", capability.severity.as_str(), capability.label);
            for line in &capability.evidence {
                println!("        {}:{}  {}", line.file, line.line, line.code);
            }
        }
    }

    if !detail.evidence.is_empty() {
        println!();
        println!("evidence");
        for row in &detail.evidence {
            if row.label == "audited" {
                // A directory's verdict is a date and a link, not a number: print it that way
                // so nobody reads the empty measured/advertised columns as a zero.
                println!(
                    "  {:<24} [audited] {}  {}",
                    row.metric,
                    rigel_core::skills::fmt_time(row.observed_at),
                    row.source
                );
                continue;
            }
            println!(
                "  {:<18} measured {:<6} advertised {:<6} [{}] {}",
                row.metric,
                row.measured
                    .map(|v| format!("{v}"))
                    .unwrap_or_else(|| "—".to_string()),
                row.advertised
                    .map(|v| format!("{v}"))
                    .unwrap_or_else(|| "—".to_string()),
                row.label,
                row.source
            );
        }
    }

    if !detail.variants.is_empty() {
        println!();
        println!("{} near-identical variant(s)", detail.variants.len());
        for variant in &detail.variants {
            println!("  {}  {}", variant.id, variant.delta);
        }
    }
}

/// Micro-dollars as `$0.1234`; the canonical unit is µ$ everywhere else.
fn dollars(micros: i64) -> String {
    format!(
        "${}.{:04}",
        micros / 1_000_000,
        (micros % 1_000_000).abs() / 100
    )
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let cut: String = value.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn slugify(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "skill".to_string()
    } else {
        collapsed
    }
}

/// §7.2: `1` user error, `2` precondition failed. The message goes to stderr in both cases;
/// `--json` still gets a machine-readable object so the UI never parses prose.
fn skills_failure(error: SkillsError, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", json!({ "ok": false, "error": error.to_string() }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(error.exit_code())
}

/// One refresh pass, in the two shapes the CLI promises: a JSON line for a script, or a count and
/// one row per source for a person. `watch` prints this on every tick as its heartbeat.
fn print_refresh(report: &rigel_core::skills::RefreshReport, as_json: bool) {
    if as_json {
        println!("{}", serde_json::to_string(report).unwrap_or_default());
        return;
    }
    println!(
        "{} source(s) known, {} refreshed",
        report.checked, report.refreshed
    );
    for row in &report.sources {
        println!("{:<10} {:<7} {}", row.id, row.state.as_str(), row.detail);
    }
}

async fn run_session(command: SessionCommand) -> ExitCode {
    match command {
        SessionCommand::Start {
            project,
            title,
            budget_cents,
        } => match SessionService::open(&project).await {
            Ok(service) => match service.start(title.as_deref(), budget_cents).await {
                Ok(session) => {
                    println!("{}", session.id);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        },

        SessionCommand::List {
            project,
            json: as_json,
        } => match SessionService::open(&project).await {
            Ok(service) => match service.list().await {
                Ok(sessions) => {
                    if as_json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&sessions).unwrap_or_default()
                        );
                    } else if sessions.is_empty() {
                        println!("no sessions");
                    } else {
                        for session in sessions {
                            println!(
                                "{}  {:<11}  {:<13}  {}",
                                &session.id[..8.min(session.id.len())],
                                format!("{:?}", session.status).to_lowercase(),
                                session.created_at,
                                session.title.unwrap_or_default()
                            );
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        },

        SessionCommand::Title {
            project,
            id,
            prompt,
            json: as_json,
        } => {
            let service = match SessionService::open(&project).await {
                Ok(service) => service,
                Err(error) => return fail(error),
            };
            let generator = rigel_core::agent::title::cheapest().await;
            let suggestion = tokio::time::timeout(
                rigel_core::agent::title::TITLE_TOTAL_TIMEOUT,
                generator.suggest(&prompt),
            )
            .await
            .ok()
            .flatten();
            let Some(suggestion) = suggestion else {
                eprintln!(
                    "rigel: no cheap title model answered; set a key for one of {} and try again",
                    rigel_core::agent::title::CHEAP_TITLE_MODELS
                        .iter()
                        .map(|(provider, _)| *provider)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return ExitCode::from(1);
            };
            match service.apply_auto_title(&id, &suggestion.title).await {
                Ok(stored) => {
                    if as_json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&json!({
                                "title": suggestion.title,
                                "provider": suggestion.provider,
                                "model": suggestion.model,
                                "usage": suggestion.usage,
                                "stored": stored.is_some(),
                            }))
                            .unwrap_or_default()
                        );
                    } else {
                        println!("title   {}", suggestion.title);
                        println!("model   {}/{}", suggestion.provider, suggestion.model);
                        println!(
                            "stored  {}",
                            if stored.is_some() {
                                "yes"
                            } else {
                                "no; the session already had a name"
                            }
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            }
        }

        SessionCommand::Summary {
            project,
            id,
            json: as_json,
        } => match SessionService::open(&project).await {
            Ok(service) => match service.get(&id).await {
                Ok(_) => match rigel_core::agent::summary::for_session(service.pool(), &id).await {
                    Ok(summaries) => {
                        if as_json {
                            println!(
                                "{}",
                                serde_json::to_string_pretty(&summaries).unwrap_or_default()
                            );
                        } else if summaries.is_empty() {
                            println!("no turn summaries recorded");
                        } else {
                            for summary in summaries {
                                println!(
                                    "{}  {:>2} items  {}",
                                    &summary.turn_id[..8.min(summary.turn_id.len())],
                                    summary.item_ids.len(),
                                    summary.summary
                                );
                            }
                        }
                        ExitCode::SUCCESS
                    }
                    Err(error) => fail(error),
                },
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        },

        SessionCommand::End { project, id } => match SessionService::open(&project).await {
            Ok(service) => match service.end(&id).await {
                Ok(session) => {
                    println!("{}", session.id);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        },

        SessionCommand::Delete {
            project,
            id,
            confirm,
        } => {
            if confirm != id {
                eprintln!(
                    "rigel: --confirm must repeat the session id exactly; nothing was deleted"
                );
                return ExitCode::from(2);
            }
            match SessionService::open(&project).await {
                Ok(service) => match service.delete_permanently(&id, &confirm).await {
                    Ok(()) => {
                        println!("deleted {id}");
                        ExitCode::SUCCESS
                    }
                    Err(error) => fail(error),
                },
                Err(error) => fail(error),
            }
        }

        SessionCommand::Resume {
            project,
            id,
            force,
            json: as_json,
        } => match SessionService::open(&project).await {
            Ok(service) => match rigel_core::resume::resume(&service, &project, &id, force).await {
                Ok(outcome) => {
                    if as_json {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&outcome).unwrap_or_default()
                        );
                    } else {
                        println!(
                            "{}  {}  tree {}",
                            &outcome.session.id[..8.min(outcome.session.id.len())],
                            if outcome.clean {
                                "clean"
                            } else {
                                "interrupted"
                            },
                            &outcome.tree_hash[..12.min(outcome.tree_hash.len())]
                        );
                        for turn in &outcome.closed_turns {
                            println!(
                                "  closed turn {} ({})",
                                turn.seq,
                                turn.stop_reason.clone().unwrap_or_default()
                            );
                        }
                        for patch in &outcome.patches {
                            println!(
                                "  patch {} {} {}",
                                patch.patch_id,
                                patch.path,
                                format!("{:?}", patch.state).to_lowercase()
                            );
                        }
                        if !outcome.reconcile.ok {
                            println!(
                                "  budget mismatch: recorded {}µ$ summed {}µ$",
                                outcome.reconcile.session_recorded_micros,
                                outcome.reconcile.session_summed_micros
                            );
                        }
                    }
                    if !outcome.clean && !force {
                        // §7.2: a resumed session that cannot continue cleanly is a failed
                        // precondition, not an error.
                        ExitCode::from(2)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(error) => {
                    eprintln!("rigel: {error}");
                    ExitCode::from(2)
                }
            },
            Err(error) => fail(error),
        },
    }
}

fn fail(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use super::{AgentCommand, Cli, Command};

    #[test]
    fn agent_run_takes_the_effort_separately_from_the_model() {
        let cli = <Cli as clap::Parser>::try_parse_from([
            "rigel",
            "agent",
            "run",
            "--prompt",
            "say pong",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "high",
        ])
        .expect("--effort is part of agent run");
        match cli.command {
            Command::Agent {
                command:
                    AgentCommand::Run {
                        effort,
                        model,
                        prompt,
                        ..
                    },
            } => {
                assert_eq!(effort.as_deref(), Some("high"));
                assert_eq!(model.as_deref(), Some("gpt-5.6-sol"));
                assert_eq!(prompt, "say pong");
            }
            _ => panic!("expected `rigel agent run`"),
        }
    }

    #[test]
    fn agent_run_without_an_effort_leaves_the_provider_default() {
        let cli = <Cli as clap::Parser>::try_parse_from(["rigel", "agent", "run", "--prompt", "x"])
            .expect("effort is optional");
        match cli.command {
            Command::Agent {
                command: AgentCommand::Run { effort, .. },
            } => assert_eq!(effort, None),
            _ => panic!("expected `rigel agent run`"),
        }
    }

    #[test]
    fn agent_run_collects_repeatable_images() {
        let cli = <Cli as clap::Parser>::try_parse_from([
            "rigel",
            "agent",
            "run",
            "--prompt",
            "read the sign",
            "--image",
            "https://example.com/sign.png",
            "--image",
            ".rigel/attachments/camera.png",
        ])
        .expect("--image is repeatable");
        match cli.command {
            Command::Agent {
                command: AgentCommand::Run { image, .. },
            } => assert_eq!(
                image,
                vec![
                    "https://example.com/sign.png".to_string(),
                    ".rigel/attachments/camera.png".to_string()
                ]
            ),
            _ => panic!("expected `rigel agent run`"),
        }
    }
}
