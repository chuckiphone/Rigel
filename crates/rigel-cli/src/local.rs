//! `rigel local` — the llama.cpp server behind the `llamacpp` door.
//!
//! The door is a model row; the server is a process. This command is the process half, so a
//! headless run can start the model it is about to use and read the exact launch line and mode
//! without the window. It never downloads or converts a GGUF: the file is the user's, and the
//! flags are the measured profile in [`rigel_core::agent::llamaserver`].

use std::path::Path;
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::agent::llamaserver::{self, Profile, Status};

#[derive(Subcommand)]
pub enum LocalCommand {
    /// The server's state, its launch profile and the mode it would use.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Start the server, or attach to the one already answering.
    Start {
        #[arg(long)]
        json: bool,
    },
    /// Stop the server this machine's record names, after checking the image.
    Stop {
        #[arg(long)]
        json: bool,
    },
    /// Save the binary, the GGUF and the port. With no flags, discover them locally.
    Setup {
        /// The llama-server binary.
        #[arg(long, value_name = "PATH")]
        server: Option<String>,
        /// The GGUF to serve.
        #[arg(long, value_name = "PATH")]
        model: Option<String>,
        /// The loopback port to bind.
        #[arg(long, value_name = "PORT")]
        port: Option<u16>,
    },
}

pub async fn run(dir: &Path, command: Option<LocalCommand>) -> ExitCode {
    match command.unwrap_or(LocalCommand::Status { json: false }) {
        LocalCommand::Status { json } => match llamaserver::status_at(dir).await {
            status if json => print_json(&status),
            status => {
                print_status(&status);
                ExitCode::SUCCESS
            }
        },
        LocalCommand::Start { json } => match llamaserver::start_at(dir).await {
            Ok(status) => {
                if json {
                    print_json(&status)
                } else {
                    print_status(&status);
                    ExitCode::SUCCESS
                }
            }
            Err(error) => fail(&error),
        },
        LocalCommand::Stop { json } => match llamaserver::stop_at(dir) {
            Ok(Some(pid)) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({ "stopped": pid }))
                            .unwrap_or_default()
                    );
                } else {
                    println!("stopped llama-server (PID {pid})");
                }
                ExitCode::SUCCESS
            }
            Ok(None) => {
                if json {
                    println!("{}", serde_json::json!({ "stopped": null }));
                } else {
                    println!("no llama-server started by Rigel is recorded on this machine");
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(&error),
        },
        LocalCommand::Setup {
            server,
            model,
            port,
        } => setup(dir, server, model, port),
    }
}

fn setup(dir: &Path, server: Option<String>, model: Option<String>, port: Option<u16>) -> ExitCode {
    let mut profile = match llamaserver::load_profile_at(dir) {
        Ok(profile) => profile,
        Err(error) => return fail(&error),
    };
    if let Some(server) = server {
        profile.server = server;
    } else if profile.server.trim().is_empty()
        && let Some(found) = llamaserver::discover_server()
    {
        profile.server = found.display().to_string();
    }
    if let Some(model) = model {
        profile.model = model;
    } else if profile.model.trim().is_empty()
        && let Some(found) = llamaserver::discover_model()
    {
        profile.model = found.display().to_string();
    }
    if let Some(port) = port {
        profile.port = port;
    }
    match llamaserver::save_profile_at(dir, &profile) {
        Ok(()) => {
            let path = llamaserver::profile_path_at(dir);
            println!("saved {}", path.display());
            print_profile(&profile);
            ExitCode::SUCCESS
        }
        Err(error) => fail(&error),
    }
}

fn print_json(status: &Status) -> ExitCode {
    match serde_json::to_string_pretty(status) {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("could not encode the status: {error}")),
    }
}

fn print_status(status: &Status) {
    println!("{:<10} {}", "state", status.state);
    println!("{:<10} {}", "base", status.base_url);
    println!(
        "{:<10} {}",
        "server",
        status.server.as_deref().unwrap_or("(not set)")
    );
    match (&status.model, status.model_gb) {
        (Some(model), Some(size)) => println!("{:<10} {model} ({size} GiB)", "model"),
        (Some(model), None) => println!("{:<10} {model}", "model"),
        (None, _) => println!("{:<10} (not set)", "model"),
    }
    match (status.free_ram_gb, status.mode.as_str()) {
        (Some(free), "pinned") => println!("{:<10} pinned in RAM · {free} GiB free", "mode"),
        (Some(free), mode) => println!(
            "{:<10} {mode} · {free} GiB free — below the model plus headroom; expect roughly \
             half the pinned speed",
            "mode"
        ),
        (None, mode) => println!("{:<10} {mode}", "mode"),
    }
    println!(
        "{:<10} {}",
        "pid",
        status
            .pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    println!("{:<10} {}", "detail", status.detail);
}

fn print_profile(profile: &Profile) {
    println!("{:<10} {}", "server", profile.server);
    println!("{:<10} {}", "model", profile.model);
    println!("{:<10} {}", "base", profile.base_url());
    println!(
        "{:<10} {}",
        "flags",
        profile.args(llamaserver::Mode::Pinned).join(" ")
    );
}

fn fail(error: &str) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(2)
}
