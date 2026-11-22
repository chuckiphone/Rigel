//! `rigel plugins` and the install half of the marketplace.
//!
//! Installing is deliberately unreachable from a keystroke: the catalogue is read from the
//! local index, an entry is resolved by name, and a plugin is either a bundled tree or a
//! repository at an exact 40-character commit. A skill entry seeds the built-in body into the
//! index and copies the same bytes into the harness stores the user asked for. Every command
//! has `--json`, because the window must never parse this output.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::plugin::{
    Catalog, CatalogEntry, CatalogKind, InstallReport as PluginInstallReport, PluginError,
    PluginManifest, PluginRuntime, PluginStore,
};
use rigel_core::{SkillIndex, SkillsError};
use serde_json::json;

#[derive(Subcommand)]
pub enum PluginCommand {
    /// Installed plugins, their tools, capabilities and enabled state.
    List {
        #[arg(long)]
        json: bool,
    },

    /// Search the curated catalogue. Offline; reads the compiled-in list.
    Search {
        query: Vec<String>,
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        tier: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Every catalogue entry, shelves and tiers included.
    Browse {
        #[arg(long)]
        category: Option<String>,
        #[arg(long)]
        tier: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// One catalogue entry, or an installed plugin when the name is not in the catalogue.
    Info {
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Install a catalogue name, a plugin directory, or a pinned repository.
    Install {
        /// Catalogue name, plugin directory, or `owner/repo@<40-char-sha>`.
        target: String,
        /// Replace an existing install.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },

    /// Remove an installed plugin. Skills are removed from the shelf, not from a harness store.
    Remove {
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Allow an installed plugin to offer its tools again.
    Enable {
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Stop offering a plugin's tools without deleting its files or receipt.
    Disable {
        name: String,
        #[arg(long)]
        json: bool,
    },

    /// Validate a directory's manifest without installing or running anything.
    Validate {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },

    /// Call one tool on an installed plugin, through the capability broker.
    Run {
        plugin: String,
        tool: String,
        /// A JSON object matching the tool's schema.
        #[arg(long, default_value = "{}")]
        args: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(dir: &Path, command: PluginCommand) -> ExitCode {
    match command {
        PluginCommand::List { json: as_json } => list(dir, as_json),
        PluginCommand::Search {
            query,
            category,
            tier,
            json: as_json,
        } => search(dir, &query.join(" "), category, tier, as_json),
        PluginCommand::Browse {
            category,
            tier,
            json: as_json,
        } => search(dir, "", category, tier, as_json),
        PluginCommand::Info {
            name,
            json: as_json,
        } => info(dir, &name, as_json),
        PluginCommand::Install {
            target,
            force,
            json: as_json,
        } => install(dir, &target, force, as_json).await,
        PluginCommand::Remove {
            name,
            json: as_json,
        } => match PluginStore::new(dir).remove(&name) {
            Ok(true) => {
                if as_json {
                    println!("{}", json!({ "ok": true, "removed": name }));
                } else {
                    println!("removed {name}");
                }
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!("rigel: no installed plugin named {name}");
                ExitCode::from(1)
            }
            Err(error) => failure(error, as_json),
        },
        PluginCommand::Enable {
            name,
            json: as_json,
        } => set_enabled(dir, &name, true, as_json),
        PluginCommand::Disable {
            name,
            json: as_json,
        } => set_enabled(dir, &name, false, as_json),
        PluginCommand::Validate {
            path,
            json: as_json,
        } => validate(&path, as_json),
        PluginCommand::Run {
            plugin,
            tool,
            args,
            json: as_json,
        } => run_tool(dir, &plugin, &tool, &args, as_json).await,
    }
}

fn list(dir: &Path, as_json: bool) -> ExitCode {
    let store = PluginStore::new(dir);
    let (plugins, broken) = match store.scan() {
        Ok(scan) => scan,
        Err(error) => return failure(error, as_json),
    };
    let rows: Vec<_> = plugins
        .iter()
        .map(|plugin| {
            json!({
                "name": plugin.manifest.name,
                "version": plugin.manifest.version,
                "description": plugin.manifest.description,
                "enabled": plugin.enabled,
                "tools": plugin.manifest.qualified_tools(),
                "capabilities": plugin.manifest.capabilities.summary(),
                "license": plugin.manifest.license,
                "source": plugin.receipt.as_ref().map(|receipt| receipt.source.clone()),
                "installed_at": plugin.receipt.as_ref().map(|receipt| receipt.installed_at),
            })
        })
        .collect();
    if as_json {
        println!(
            "{}",
            json!({ "plugins": rows, "broken": broken.iter().map(|(name, error)| json!({
                "name": name, "error": error
            })).collect::<Vec<_>>() })
        );
    } else if rows.is_empty() {
        println!("no plugins installed — `rigel plugins browse` lists the catalogue");
    } else {
        for plugin in &plugins {
            println!(
                "{} {}  {}",
                plugin.manifest.name,
                plugin.manifest.version,
                if plugin.enabled { "" } else { "(disabled)" }
            );
            if !plugin.manifest.description.is_empty() {
                println!("  {}", plugin.manifest.description);
            }
            println!("  tools: {}", plugin.manifest.qualified_tools().join(", "));
            println!(
                "  capabilities: {}",
                plugin.manifest.capabilities.summary().join(" · ")
            );
            if let Some(source) = plugin.receipt.as_ref() {
                println!("  source: {}", source.source);
            }
        }
        if !broken.is_empty() {
            println!();
            println!("broken installs (not offered to the model):");
            for (name, error) in &broken {
                println!("  {name}: {error}");
            }
        }
    }
    ExitCode::SUCCESS
}

fn search(
    dir: &Path,
    query: &str,
    category: Option<String>,
    tier: Option<String>,
    as_json: bool,
) -> ExitCode {
    let catalog = match Catalog::load(dir) {
        Ok(catalog) => catalog,
        Err(error) => return failure(error, as_json),
    };
    let hits = catalog.search(query, category.as_deref(), tier.as_deref());
    if as_json {
        println!(
            "{}",
            json!({ "entries": hits, "categories": catalog.categories() })
        );
        return ExitCode::SUCCESS;
    }
    if hits.is_empty() {
        println!("no catalogue entries match {query:?}");
        return ExitCode::SUCCESS;
    }
    for entry in hits {
        print_entry(&entry);
    }
    ExitCode::SUCCESS
}

fn print_entry(entry: &CatalogEntry) {
    println!(
        "{}  [{} · {}]",
        entry.name,
        entry.tier.as_str(),
        entry.kind.as_str()
    );
    println!("  {}", entry.title);
    if !entry.description.is_empty() {
        println!("  {}", entry.description);
    }
    let mut meta = vec![entry.category.clone()];
    if let Some(license) = &entry.license {
        meta.push(license.clone());
    }
    if !entry.capabilities.is_empty() {
        meta.push(format!("needs: {}", entry.capabilities.join(", ")));
    }
    println!("  {}", meta.join(" · "));
    if let Some(note) = &entry.note {
        println!("  note: {note}");
    }
    if let Some(install) = &entry.install {
        println!("  install: {install}");
    }
    if let Some(homepage) = &entry.homepage {
        println!("  {homepage}");
    }
}

fn info(dir: &Path, name: &str, as_json: bool) -> ExitCode {
    let catalog = match Catalog::load(dir) {
        Ok(catalog) => catalog,
        Err(error) => return failure(error, as_json),
    };
    if let Some(entry) = catalog.get(name) {
        if as_json {
            println!("{}", json!({ "entry": entry }));
        } else {
            print_entry(entry);
            println!(
                "  auto-install: {}",
                if entry.installable() { "yes" } else { "no" }
            );
        }
        return ExitCode::SUCCESS;
    }
    let store = PluginStore::new(dir);
    match store.scan() {
        Ok((plugins, _)) => {
            if let Some(plugin) = plugins.iter().find(|plugin| plugin.manifest.name == name) {
                if as_json {
                    println!(
                        "{}",
                        json!({
                            "name": plugin.manifest.name,
                            "version": plugin.manifest.version,
                            "description": plugin.manifest.description,
                            "enabled": plugin.enabled,
                            "directory": plugin.directory,
                            "tools": plugin.manifest.qualified_tools(),
                            "capabilities": plugin.manifest.capabilities.summary(),
                            "receipt": plugin.receipt,
                        })
                    );
                } else {
                    println!("{} {}", plugin.manifest.name, plugin.manifest.version);
                    println!("  {}", plugin.manifest.description);
                    println!("  directory: {}", plugin.directory.display());
                    println!("  tools: {}", plugin.manifest.qualified_tools().join(", "));
                    println!(
                        "  capabilities: {}",
                        plugin.manifest.capabilities.summary().join(" · ")
                    );
                }
                ExitCode::SUCCESS
            } else {
                eprintln!("rigel: no catalogue entry or installed plugin named {name}");
                ExitCode::from(1)
            }
        }
        Err(error) => failure(error, as_json),
    }
}

async fn install(dir: &Path, target: &str, force: bool, as_json: bool) -> ExitCode {
    let store = PluginStore::new(dir);
    if Path::new(target).exists() {
        return match store.install_local(Path::new(target), force) {
            Ok(report) => print_plugin_report(report, as_json),
            Err(error) => failure(error, as_json),
        };
    }

    let catalog = match Catalog::load(dir) {
        Ok(catalog) => catalog,
        Err(error) => return failure(error, as_json),
    };
    let Some(entry) = catalog.get(target) else {
        eprintln!("rigel: no catalogue entry or directory named {target}");
        return ExitCode::from(1);
    };

    match entry.kind {
        CatalogKind::Skill => install_skill(dir, entry, as_json).await,
        CatalogKind::Plugin => {
            if let Some(bundled) = &entry.bundled {
                match store.install_bundled(bundled, force) {
                    Ok(report) => print_plugin_report(report, as_json),
                    Err(error) => failure(error, as_json),
                }
            } else if let (Some(repo), Some(sha)) = (&entry.repo, &entry.sha) {
                match store.install_git(repo, sha, entry.subdir.as_deref(), force) {
                    Ok(report) => print_plugin_report(report, as_json),
                    Err(error) => failure(error, as_json),
                }
            } else {
                failure(
                    PluginError::NotInstallable(
                        entry.name.clone(),
                        "the entry has no pinned commit".to_string(),
                    ),
                    as_json,
                )
            }
        }
        CatalogKind::External => failure(
            PluginError::NotInstallable(
                entry.name.clone(),
                entry
                    .install
                    .clone()
                    .unwrap_or_else(|| "follow the project's own install guide".to_string()),
            ),
            as_json,
        ),
    }
}

async fn install_skill(dir: &Path, entry: &CatalogEntry, as_json: bool) -> ExitCode {
    let index = match SkillIndex::open(dir).await {
        Ok(index) => index,
        Err(error) => return skills_failure(error, as_json),
    };
    if !entry.skills.is_empty()
        && let Err(error) = index.install_builtin().await
    {
        return skills_failure(error, as_json);
    }
    let mut installed = Vec::new();
    for id in &entry.skills {
        let qualified = format!("builtin:{id}");
        match index.install(&qualified, &skill_roots()).await {
            Ok(report) => installed.push(report),
            Err(error) => return skills_failure(error, as_json),
        }
    }
    if as_json {
        println!(
            "{}",
            json!({
                "ok": true, "name": entry.name, "skills": installed,
                "tool_install": entry.install
            })
        );
    } else if installed.is_empty() {
        println!("{} is a skill entry with no built-in body", entry.name);
    } else {
        for report in &installed {
            println!("installed {} to:", report.name);
            for path in &report.paths {
                println!("  {}", path.display());
            }
        }
        if let Some(command) = &entry.install {
            println!("the skill is instructions; the tool itself is installed with: {command}");
        }
    }
    ExitCode::SUCCESS
}

fn print_plugin_report(report: PluginInstallReport, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", json!(report));
    } else {
        println!(
            "{} {}  installed at {}",
            report.name,
            report.version,
            report.directory.display()
        );
        println!("  tools: {}", report.tools.join(", "));
        println!("  capabilities: {}", report.capabilities.join(" · "));
        println!("  tree sha256: {}", report.digest);
    }
    ExitCode::SUCCESS
}

fn set_enabled(dir: &Path, name: &str, enabled: bool, as_json: bool) -> ExitCode {
    match PluginStore::new(dir).set_enabled(name, enabled) {
        Ok(row) => {
            if as_json {
                println!("{}", json!(row));
            } else {
                println!(
                    "{} {}",
                    row.name,
                    if enabled { "enabled" } else { "disabled" }
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => failure(error, as_json),
    }
}

fn validate(path: &Path, as_json: bool) -> ExitCode {
    let manifest = match PluginManifest::load(path) {
        Ok(manifest) => manifest,
        Err(error) => return failure(error, as_json),
    };
    let available = command_available(
        manifest
            .entry
            .as_ref()
            .map(|entry| entry.command.as_str())
            .unwrap_or_default(),
    );
    if as_json {
        println!(
            "{}",
            json!({
                "ok": true,
                "name": manifest.name,
                "version": manifest.version,
                "tools": manifest.qualified_tools(),
                "capabilities": manifest.capabilities.summary(),
                "entry_available": available,
            })
        );
    } else {
        println!("{} {} is valid", manifest.name, manifest.version);
        println!("  tools: {}", manifest.qualified_tools().join(", "));
        println!(
            "  capabilities: {}",
            manifest.capabilities.summary().join(" · ")
        );
        if let Some(entry) = &manifest.entry {
            println!(
                "  entry: {} {} ({} on PATH)",
                entry.command,
                entry.args.join(" "),
                if available { "found" } else { "not found" }
            );
        }
    }
    ExitCode::SUCCESS
}

async fn run_tool(dir: &Path, plugin: &str, tool: &str, args: &str, as_json: bool) -> ExitCode {
    let arguments: serde_json::Value = match serde_json::from_str(args) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("rigel: --args is not JSON: {error}");
            return ExitCode::from(1);
        }
    };
    let runtime = PluginRuntime::open(dir);
    match runtime.call(Some(plugin), tool, arguments).await {
        Ok(reply) => {
            if as_json {
                println!(
                    "{}",
                    json!({
                        "ok": true, "plugin": reply.plugin, "tool": reply.tool,
                        "content": reply.content
                    })
                );
            } else {
                println!("{}", reply.text());
            }
            ExitCode::SUCCESS
        }
        Err(error) => failure(error, as_json),
    }
}

fn failure(error: PluginError, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", json!({ "ok": false, "error": error.to_string() }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(error.exit_code())
}

fn skills_failure(error: SkillsError, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", json!({ "ok": false, "error": error.to_string() }));
    }
    eprintln!("rigel: {error}");
    ExitCode::from(error.exit_code())
}

/// The two stores Rigel launches harnesses from: the shared `.agents` directory and Claude's.
pub(crate) fn skill_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return Vec::new();
    };
    vec![
        home.join(".agents").join("skills"),
        home.join(".claude").join("skills"),
    ]
}

/// Whether a command resolves on PATH, without running it. A bare name is looked up; a path is
/// checked directly. This is a hint on the install card, not a proof the tool works.
pub(crate) fn command_available(command: &str) -> bool {
    if command.is_empty() {
        return false;
    }
    let path = Path::new(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string())
            .split(';')
            .map(|part| part.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .any(|dir| {
            extensions.iter().any(|extension| {
                let candidate = dir.join(format!("{command}{extension}"));
                candidate.is_file()
            })
        })
}
