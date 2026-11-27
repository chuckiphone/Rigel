use super::{Forge, ForgeError, PullRequestPlan, build_plan};
use crate::session::{Session, TurnTranscript};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct MockForge {
    pub check_result: Result<String, String>,
    pub create_result: String,
}
impl Default for MockForge {
    fn default() -> Self {
        Self {
            check_result: Ok("forge ready".into()),
            create_result: "https://example.test/pr/1".into(),
        }
    }
}
impl Forge for MockForge {
    fn check(&self) -> Result<String, ForgeError> {
        self.check_result.clone().map_err(ForgeError::Failed)
    }
    fn plan(
        &self,
        project: &Path,
        session: &Session,
        turns: &[TurnTranscript],
    ) -> Result<PullRequestPlan, ForgeError> {
        build_plan(project, session, turns)
    }
    fn create(&self, _project: &Path, _plan: &PullRequestPlan) -> Result<String, ForgeError> {
        Ok(self.create_result.clone())
    }
}
