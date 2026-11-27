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
        crate::json::print_pretty(&resolution);
        ExitCode::SUCCESS
    } else {
        print!("{}", rigel_core::agent::rules::explain(&resolution));
        ExitCode::SUCCESS
    }
}
