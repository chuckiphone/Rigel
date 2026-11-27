use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Subcommand;
use rigel_core::prompts::PromptLibrary;

#[derive(Subcommand)]
pub(crate) enum PromptCommand {
    /// List the latest version of every prompt.
    List {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// Save a new version of a prompt.
    Save {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        body: String,
    },
    /// Print a prompt, optionally interpolating NAME=VALUE variables.
    Render {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        version: Option<i64>,
        #[arg(long = "var")]
        variables: Vec<String>,
    },
    /// Show line changes between two versions.
    Diff {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        from: i64,
        #[arg(long)]
        to: i64,
    },
    /// Pin an expanded prompt snapshot to an existing transcript turn.
    Pin {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        session: String,
        #[arg(long)]
        turn: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        version: Option<i64>,
        #[arg(long = "var")]
        variables: Vec<String>,
    },
}

pub(crate) async fn run(command: PromptCommand) -> ExitCode {
    let (project, operation) = match command {
        PromptCommand::List { project } => (project, Operation::List),
        PromptCommand::Save {
            project,
            name,
            body,
        } => (project, Operation::Save { name, body }),
        PromptCommand::Render {
            project,
            name,
            version,
            variables,
        } => (
            project,
            Operation::Render {
                name,
                version,
                variables,
            },
        ),
        PromptCommand::Diff {
            project,
            name,
            from,
            to,
        } => (project, Operation::Diff { name, from, to }),
        PromptCommand::Pin {
            project,
            session,
            turn,
            name,
            version,
            variables,
        } => (
            project,
            Operation::Pin {
                session,
                turn,
                name,
                version,
                variables,
            },
        ),
    };
    let library = match PromptLibrary::open(&project).await {
        Ok(library) => library,
        Err(error) => return fail(error),
    };
    match operation {
        Operation::List => match library.list().await {
            Ok(prompts) => {
                for prompt in prompts {
                    println!("{}\t{}", prompt.name, prompt.version);
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(error),
        },
        Operation::Save { name, body } => match library.save(&name, &body).await {
            Ok(prompt) => {
                println!("{}@{}", prompt.name, prompt.version);
                ExitCode::SUCCESS
            }
            Err(error) => fail(error),
        },
        Operation::Render {
            name,
            version,
            variables,
        } => match parse_variables(&variables) {
            Ok(variables) => match library.render(&name, version, &variables).await {
                Ok(rendered) => {
                    println!("{rendered}");
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            },
            Err(error) => fail(error),
        },
        Operation::Diff { name, from, to } => match library.diff(&name, from, to).await {
            Ok(diff) => {
                print!("{diff}");
                ExitCode::SUCCESS
            }
            Err(error) => fail(error),
        },
        Operation::Pin {
            session,
            turn,
            name,
            version,
            variables,
        } => {
            let variables = match parse_variables(&variables) {
                Ok(vars) => vars,
                Err(error) => return fail(error),
            };
            match library
                .pin(&session, &turn, &name, version, &variables)
                .await
            {
                Ok(pin) => {
                    println!("{}@{} pinned to {}", pin.name, pin.version, pin.turn_id);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error),
            }
        }
    }
}

enum Operation {
    List,
    Save {
        name: String,
        body: String,
    },
    Render {
        name: String,
        version: Option<i64>,
        variables: Vec<String>,
    },
    Diff {
        name: String,
        from: i64,
        to: i64,
    },
    Pin {
        session: String,
        turn: String,
        name: String,
        version: Option<i64>,
        variables: Vec<String>,
    },
}

fn parse_variables(values: &[String]) -> Result<BTreeMap<String, String>, String> {
    values
        .iter()
        .map(|value| {
            let (key, value) = value
                .split_once('=')
                .ok_or_else(|| format!("variable must be NAME=VALUE: {value}"))?;
            if key.trim().is_empty() {
                return Err("variable name must not be empty".to_owned());
            }
            Ok((key.trim().to_owned(), value.to_owned()))
        })
        .collect()
}

fn fail(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("rigel: {error}");
    ExitCode::from(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_split_only_on_the_first_equals_sign() {
        let parsed = parse_variables(&["path=a=b.rs".to_owned()]).unwrap();
        assert_eq!(parsed.get("path").map(String::as_str), Some("a=b.rs"));
        assert!(parse_variables(&["missing-separator".to_owned()]).is_err());
    }
}
