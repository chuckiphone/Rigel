//! Roadmap 66: the project grant and the role/agent permission matrix.
//!
//! A project grant is a ceiling on what any agent in the project may use. It is persisted in
//! `.rigel/permissions.json`; a project with no file keeps the pre-matrix behaviour and grants
//! every tool. A role preset and an optional per-agent override may narrow that ceiling by
//! offering fewer tools, but neither can add one back:
//! [`ProjectPermissions::effective_for_agent`] is always an intersection.
//!
//! The matrix is the same calculation the tool dispatcher uses, so the settings table and the
//! enforcement path cannot disagree about a grant.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::agent::roles;
use crate::agent::tools::Tools;

/// The project-relative file the matrix is stored in, next to the network policy.
pub const PATH: &str = ".rigel/permissions.json";
const VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("permission policy could not be read: {0}")]
    Io(#[from] std::io::Error),
    #[error("permission policy is not valid JSON: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("permission policy version {0} is not supported")]
    Version(u32),
    #[error("`{0}` is not a tool; use the names the dispatcher registers")]
    UnknownTool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectPermissions {
    /// Canonical registry names. An empty list is a real grant: it denies every tool.
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PermissionFile {
    #[serde(default = "version")]
    version: u32,
    #[serde(default)]
    allowed_tools: Vec<String>,
}

fn version() -> u32 {
    VERSION
}

impl ProjectPermissions {
    /// Every registered tool. This is the default when a project has no policy file, and it
    /// preserves the behaviour of every project created before the matrix existed.
    pub fn all() -> Self {
        Self {
            allowed_tools: all_tools(),
        }
    }

    /// A fail-closed policy. Used when a policy file cannot be read during a turn.
    pub fn none() -> Self {
        Self {
            allowed_tools: Vec::new(),
        }
    }

    pub fn load(project_root: &Path) -> Result<Self, PermissionError> {
        let path = project_root.join(PATH);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Self::all()),
            Err(error) => return Err(PermissionError::Io(error)),
        };
        let file: PermissionFile = serde_json::from_str(&text)?;
        if file.version != VERSION {
            return Err(PermissionError::Version(file.version));
        }
        Ok(Self {
            allowed_tools: canonical_tools(&file.allowed_tools)?,
        })
    }

    /// Validate and write the grant. Unknown names are refused rather than silently dropped:
    /// a typo must not quietly remove a tool from every role.
    pub fn save(project_root: &Path, allowed_tools: &[String]) -> Result<Self, PermissionError> {
        let allowed_tools = canonical_tools(allowed_tools)?;
        let path = project_root.join(PATH);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = PermissionFile {
            version: VERSION,
            allowed_tools: allowed_tools.clone(),
        };
        let text = serde_json::to_string_pretty(&file)?;
        fs::write(path, text)?;
        Ok(Self { allowed_tools })
    }

    pub fn as_set(&self) -> HashSet<String> {
        self.allowed_tools.iter().cloned().collect()
    }

    /// What an agent with `role` may use. A role can only narrow the project grant.
    pub fn effective_for_role(&self, role: Option<&str>) -> Vec<String> {
        self.effective_for_agent(role, None)
    }

    /// What an agent with `role` and an optional per-agent override may use. The override is
    /// another filter, never a replacement: neither it nor the role can add a tool the project
    /// withholds.
    pub fn effective_for_agent(
        &self,
        role: Option<&str>,
        overrides: Option<&[String]>,
    ) -> Vec<String> {
        self.effective_for_child(role, overrides, None)
    }

    /// A spawned child is bounded by its parent's effective grant as well as the project, the
    /// child's role and its override. `parent` is captured at spawn time; passing `None` is the
    /// root calculation used by the settings matrix. Every layer is an intersection, so no
    /// argument can widen a grant here.
    pub fn effective_for_child(
        &self,
        role: Option<&str>,
        overrides: Option<&[String]>,
        parent: Option<&HashSet<String>>,
    ) -> Vec<String> {
        let filter = agent_tool_filter(role, overrides)
            .map(|tools| tools.into_iter().collect::<HashSet<_>>());
        let project = self.allowed_tools.iter().cloned().collect::<HashSet<_>>();
        let effective = intersect_grants(&project, parent, filter.as_ref());
        self.allowed_tools
            .iter()
            .filter(|tool| effective.contains(*tool))
            .cloned()
            .collect()
    }

    /// The tools a role asks for that the project grant withholds.
    pub fn blocked_for_role(&self, role: Option<&str>) -> Vec<String> {
        let Some(requested) = roles::tools(role) else {
            return Vec::new();
        };
        requested
            .into_iter()
            .filter(|tool| !self.allowed_tools.iter().any(|allowed| allowed == tool))
            .collect()
    }

    /// The tools this agent would have through its role and the project, but its own override
    /// removes. The settings table shows these as narrowed by the agent, not by the project.
    pub fn narrowed_by_override(
        &self,
        role: Option<&str>,
        overrides: Option<&[String]>,
    ) -> Vec<String> {
        let Some(overrides) = overrides else {
            return Vec::new();
        };
        self.effective_for_role(role)
            .into_iter()
            .filter(|tool| !overrides.iter().any(|candidate| candidate == tool))
            .collect()
    }

    /// The settings table and the dispatcher share this shape.
    pub fn matrix(
        project_root: &Path,
        agents: &[AgentPermissionRow],
    ) -> Result<PermissionMatrix, PermissionError> {
        let permissions = Self::load(project_root)?;
        Ok(permissions.matrix_for(agents))
    }

    pub fn matrix_for(&self, agents: &[AgentPermissionRow]) -> PermissionMatrix {
        let roles = roles::PRESETS
            .iter()
            .map(|preset| {
                let requested = preset.tools.iter().map(|tool| tool.to_string()).collect();
                let effective = self.effective_for_role(Some(preset.id));
                let blocked = self.blocked_for_role(Some(preset.id));
                RoleGrant {
                    id: preset.id.to_string(),
                    title: preset.title.to_string(),
                    requested,
                    effective,
                    blocked_by_project: blocked,
                }
            })
            .collect();
        let agents = agents
            .iter()
            .map(|agent| {
                let preset = roles::preset(agent.role.as_deref());
                let overrides = agent.tool_overrides.as_deref();
                let source = match (preset, overrides.is_some()) {
                    (Some(preset), true) => {
                        format!("role preset `{}` + per-agent override", preset.id)
                    }
                    (Some(preset), false) => format!("role preset `{}`", preset.id),
                    (None, true) => "per-agent override".to_string(),
                    (None, false) => "project grant".to_string(),
                };
                AgentGrant {
                    id: agent.id.clone(),
                    role: agent.role.clone(),
                    role_title: preset.map(|preset| preset.title.to_string()),
                    tool_overrides: agent.tool_overrides.clone(),
                    effective: self.effective_for_agent(agent.role.as_deref(), overrides),
                    narrowed_by_override: self
                        .narrowed_by_override(agent.role.as_deref(), overrides),
                    source,
                }
            })
            .collect();
        PermissionMatrix {
            project_grant: self.allowed_tools.clone(),
            all_tools: all_tools(),
            roles,
            agents,
        }
    }
}

/// The one intersection every granted-tool list comes from: project, optional inherited parent
/// grant, and optional role/override filter. A missing filter is a custom role with no override;
/// an empty filter or parent set is a real denial.
pub fn intersect_grants(
    project: &HashSet<String>,
    parent: Option<&HashSet<String>>,
    filter: Option<&HashSet<String>>,
) -> HashSet<String> {
    project
        .iter()
        .filter(|tool| filter.is_none_or(|allowed| allowed.contains(*tool)))
        .filter(|tool| parent.is_none_or(|allowed| allowed.contains(*tool)))
        .cloned()
        .collect()
}

/// The role/override layer on its own, before the project ceiling. `None` means "no role
/// filter" (a custom role with no override), which is how a project with no policy keeps every
/// tool. `Some(empty)` is a real denial.
pub fn agent_tool_filter(role: Option<&str>, overrides: Option<&[String]>) -> Option<Vec<String>> {
    match (roles::tools(role), overrides) {
        (None, None) => None,
        (Some(requested), None) => Some(requested),
        (None, Some(overrides)) => Some(overrides.to_vec()),
        (Some(requested), Some(overrides)) => Some(
            requested
                .into_iter()
                .filter(|tool| overrides.iter().any(|candidate| candidate == tool))
                .collect(),
        ),
    }
}

pub fn all_tools() -> Vec<String> {
    Tools::specs_all()
        .into_iter()
        .map(|spec| spec.name)
        .collect()
}

pub fn canonical_tools(names: &[String]) -> Result<Vec<String>, PermissionError> {
    let known = all_tools();
    let mut requested = HashSet::new();
    for name in names {
        let name = name.trim();
        if !known.iter().any(|candidate| candidate == name) {
            return Err(PermissionError::UnknownTool(name.to_string()));
        }
        requested.insert(name.to_string());
    }
    Ok(known
        .into_iter()
        .filter(|name| requested.contains(name))
        .collect())
}

/// One agent row as the matrix needs it. The override is already stored on the agent; the
/// matrix only computes what the dispatcher will read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPermissionRow {
    pub id: String,
    pub role: Option<String>,
    /// `None` means the role is the only filter; `Some(empty)` denies every tool to this agent.
    pub tool_overrides: Option<Vec<String>>,
}

/// One role's grant as the settings table shows it.
#[derive(Debug, Clone, Serialize)]
pub struct RoleGrant {
    pub id: String,
    pub title: String,
    /// What the preset asks for.
    pub requested: Vec<String>,
    /// The intersection with the project grant.
    pub effective: Vec<String>,
    /// Requested but withheld by the project; these are never offered to the model.
    pub blocked_by_project: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentGrant {
    pub id: String,
    pub role: Option<String>,
    pub role_title: Option<String>,
    /// The agent's explicit narrowing, or `None` when its role alone decides.
    pub tool_overrides: Option<Vec<String>>,
    pub effective: Vec<String>,
    /// Tools the role and project would grant, but this agent's override removes.
    pub narrowed_by_override: Vec<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionMatrix {
    pub project_grant: Vec<String>,
    pub all_tools: Vec<String>,
    pub roles: Vec<RoleGrant>,
    pub agents: Vec<AgentGrant>,
}

impl PermissionMatrix {
    pub fn policy_path(project_root: &Path) -> PathBuf {
        project_root.join(PATH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_policy(root: &Path, text: &str) {
        fs::create_dir_all(root.join(".rigel")).unwrap();
        fs::write(root.join(PATH), text).unwrap();
    }

    fn row(id: &str, role: Option<&str>, overrides: Option<&[&str]>) -> AgentPermissionRow {
        AgentPermissionRow {
            id: id.to_string(),
            role: role.map(str::to_string),
            tool_overrides: overrides
                .map(|tools| tools.iter().map(|tool| tool.to_string()).collect()),
        }
    }

    #[test]
    fn a_missing_policy_grants_every_tool_and_a_role_only_narrows() {
        let tmp = tempfile::tempdir().unwrap();
        let all = ProjectPermissions::load(tmp.path()).unwrap();
        assert_eq!(all.allowed_tools, all_tools());
        assert!(
            all.effective_for_role(Some("reviewer"))
                .contains(&"read_file".to_string())
        );
        assert!(
            !all.effective_for_role(Some("reviewer"))
                .contains(&"edit_file".to_string()),
            "the reviewer preset already withholds writing"
        );
    }

    #[test]
    fn a_role_cannot_widen_a_project_grant() {
        let tmp = tempfile::tempdir().unwrap();
        let saved = ProjectPermissions::save(
            tmp.path(),
            &["read_file".to_string(), "run_command".to_string()],
        )
        .unwrap();
        assert_eq!(
            saved.effective_for_role(Some("reviewer")),
            ["read_file", "run_command"]
        );

        // The implementer asks for edit_file; the project withholds it, so the effective grant
        // does not contain it and the matrix names it as blocked.
        let matrix = saved.matrix_for(&[row("agent-1", Some("implementer"), None)]);
        let implementer = matrix
            .roles
            .iter()
            .find(|role| role.id == "implementer")
            .unwrap();
        assert_eq!(implementer.effective, ["read_file", "run_command"]);
        assert!(
            implementer
                .blocked_by_project
                .contains(&"edit_file".to_string())
        );
        assert!(
            matrix
                .agents
                .iter()
                .all(|agent| !agent.effective.contains(&"edit_file".to_string()))
        );
    }

    #[test]
    fn a_narrower_project_grant_narrows_every_role_further() {
        let tmp = tempfile::tempdir().unwrap();
        let saved = ProjectPermissions::save(tmp.path(), &["read_file".to_string()]).unwrap();
        assert_eq!(
            saved.effective_for_role(Some("reviewer")),
            ["read_file".to_string()]
        );
        assert!(
            saved
                .blocked_for_role(Some("reviewer"))
                .contains(&"run_command".to_string())
        );
        assert_eq!(
            saved.effective_for_role(Some("not-a-preset")),
            ["read_file"]
        );
    }

    #[test]
    fn an_agent_override_only_narrows_its_role_and_project() {
        let tmp = tempfile::tempdir().unwrap();
        // The project withholds write_file even though the implementer role asks for it.
        let saved = ProjectPermissions::save(
            tmp.path(),
            &[
                "read_file".to_string(),
                "list_files".to_string(),
                "edit_file".to_string(),
            ],
        )
        .unwrap();

        // Narrower than the implementer role.
        assert_eq!(
            saved.effective_for_agent(
                Some("implementer"),
                Some(&["read_file".to_string(), "list_files".to_string()])
            ),
            ["read_file", "list_files"]
        );

        // A reviewer cannot add edit_file: the role already withholds it.
        assert_eq!(
            saved.effective_for_agent(Some("reviewer"), Some(&["edit_file".to_string()])),
            Vec::<String>::new()
        );

        // The project still withholds write_file even when an override asks for every tool.
        assert!(
            !saved
                .effective_for_agent(
                    Some("implementer"),
                    Some(&["read_file".to_string(), "write_file".to_string()])
                )
                .contains(&"write_file".to_string())
        );

        // An empty override is a real denial, not "no override".
        assert!(
            saved
                .effective_for_agent(Some("implementer"), Some(&[]))
                .is_empty()
        );

        let matrix = saved.matrix_for(&[row("agent-1", Some("implementer"), Some(&["read_file"]))]);
        let agent = &matrix.agents[0];
        assert_eq!(agent.effective, ["read_file"]);
        assert!(
            agent
                .narrowed_by_override
                .contains(&"edit_file".to_string())
        );
        assert_eq!(
            agent.source,
            "role preset `implementer` + per-agent override"
        );
    }

    #[test]
    fn malformed_and_unknown_policies_do_not_fall_open() {
        let tmp = tempfile::tempdir().unwrap();
        write_policy(tmp.path(), "{not json");
        assert!(ProjectPermissions::load(tmp.path()).is_err());

        write_policy(
            tmp.path(),
            r#"{"version":1,"allowed_tools":["read_file","not_a_tool"]}"#,
        );
        assert!(matches!(
            ProjectPermissions::load(tmp.path()),
            Err(PermissionError::UnknownTool(tool)) if tool == "not_a_tool"
        ));

        write_policy(
            tmp.path(),
            r#"{"version":99,"allowed_tools":["read_file"]}"#,
        );
        assert!(matches!(
            ProjectPermissions::load(tmp.path()),
            Err(PermissionError::Version(99))
        ));
    }

    #[test]
    fn an_empty_grant_is_a_real_denial() {
        let tmp = tempfile::tempdir().unwrap();
        let saved = ProjectPermissions::save(tmp.path(), &[]).unwrap();
        assert!(saved.allowed_tools.is_empty());
        assert!(saved.effective_for_role(Some("reviewer")).is_empty());
        assert_eq!(saved.effective_for_role(None), Vec::<String>::new());
    }
}
