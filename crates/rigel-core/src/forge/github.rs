use super::{Forge, ForgeError, PullRequestPlan, build_plan};
use crate::session::{Session, TurnTranscript};
use std::{path::Path, process::Command};

pub trait GhRunner: Send + Sync {
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError>;
}
impl<F> GhRunner for F
where
    F: Fn(Option<&Path>, &[&str]) -> Result<String, ForgeError> + Send + Sync,
{
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError> {
        self(project, args)
    }
}
struct SystemGh;
impl GhRunner for SystemGh {
    fn run(&self, project: Option<&Path>, args: &[&str]) -> Result<String, ForgeError> {
        let mut command = Command::new("gh");
        if let Some(project) = project {
            command.current_dir(project);
        }
        command.args(args);
        crate::process::hide(&mut command);
        let output = command.output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ForgeError::GhMissing
            } else {
                ForgeError::Failed(e.to_string())
            }
        })?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if args == ["auth", "status"] {
                Err(ForgeError::NotSignedIn(if msg.is_empty() {
                    "run gh auth login".into()
                } else {
                    msg
                }))
            } else {
                Err(ForgeError::Failed(msg))
            }
        }
    }
}
pub struct GitHubForge {
    runner: Box<dyn GhRunner>,
}
impl Default for GitHubForge {
    fn default() -> Self {
        Self {
            runner: Box::new(SystemGh),
        }
    }
}
impl GitHubForge {
    pub fn with_runner(runner: Box<dyn GhRunner>) -> Self {
        Self { runner }
    }
}
impl Forge for GitHubForge {
    fn check(&self) -> Result<String, ForgeError> {
        self.runner.run(None, &["auth", "status"])?;
        Ok("forge ready".into())
    }
    fn plan(
        &self,
        project: &Path,
        session: &Session,
        turns: &[TurnTranscript],
    ) -> Result<PullRequestPlan, ForgeError> {
        build_plan(project, session, turns)
    }
    fn create(&self, project: &Path, plan: &PullRequestPlan) -> Result<String, ForgeError> {
        self.check()?;
        let output = self.runner.run(
            Some(project),
            &[
                "pr",
                "create",
                "--title",
                &plan.title,
                "--body",
                &plan.body,
                "--base",
                &plan.base,
            ],
        )?;
        Ok(output.lines().last().unwrap_or(&output).to_string())
    }
}
