//! `rigel rules explain`: which rule file wins and why.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;

#[derive(Subcommand)]
pub enum RulesCommand {
    /// Print the winning rule file, its path and line, and any conflicts.
    Explain {
        #[arg(long, value_name = "PATH", default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

pub fn run(command: RulesCommand) -> ExitCode {
    let RulesCommand::Explain { project, json } = command;
    let resolution = rigel_core::agent::rules::resolve(&project);
    if json {
        match serde_json::to_string_pretty(&resolution) {
            Ok(text) => {
                println!("{text}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("rigel: {error}");
                ExitCode::from(2)
            }
        }
    } else {
        print!("{}", rigel_core::agent::rules::explain(&resolution));
        ExitCode::SUCCESS
    }
}
