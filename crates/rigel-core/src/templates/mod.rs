//! D152: project-local session templates.
//!
//! A template is the shape of a session before it starts: model, effort, step budget, the roster
//! to spawn and the prompt to leave in the composer. It lives in `.rigel/templates.json`, beside
//! the session database and outside version control, because a model id is machine-specific
//! (N7): committing one would promise a door the reader may not have a key for.
//!
//! The file is replaced atomically (write, then rename), so a crash mid-save leaves the previous
//! list rather than half of a new one.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Enough for a person's habits; far short of a second settings surface.
pub const MAX_TEMPLATES: usize = 40;
const MAX_NAME: usize = 60;

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("session template {0} is not in this project")]
    Unknown(String),
    #[error("a template name is one line, at most {MAX_NAME} characters")]
    InvalidName,
    #[error("this project already has {MAX_TEMPLATES} templates; delete one first")]
    TooMany,
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse templates.json: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("invalid template skill pin: {0}")]
    InvalidPin(String),
}

/// One agent the template spawns. The model is the agent's own (D135), so a template can name a
/// cheap reviewer and a stronger writer without touching the window's default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateAgent {
    pub harness: String,
    pub role: Option<String>,
    pub model: Option<String>,
}

/// Q270: a skill pin travels with the shape. The body hash is kept, not resolved again on
/// create, so a template reproduces the exact version the original session read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateSkill {
    pub skill_id: String,
    pub body_sha: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub max_steps: Option<u16>,
    #[serde(default)]
    pub agents: Vec<TemplateAgent>,
    /// Pinned skill bodies this session should keep. Missing on older template files.
    #[serde(default)]
    pub skills: Vec<TemplateSkill>,
    /// Left in the composer, never sent: starting from a template still needs a Send.
    #[serde(default)]
    pub prompt: String,
    pub created_at: i64,
}

/// The project's templates, read and written as one file.
#[derive(Debug, Clone)]
pub struct SessionTemplates {
    path: PathBuf,
}

impl SessionTemplates {
    pub fn open(project_root: &Path) -> Self {
        Self {
            path: project_root.join(".rigel").join("templates.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<SessionTemplate>, TemplateError> {
        Ok(self.list_v2()?.iter().map(TemplateModel::to_v1).collect())
    }

    /// Load templates in the v2 model; v1 files upgrade through serde defaults.
    pub fn list_v2(&self) -> Result<Vec<TemplateModel>, TemplateError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(TemplateError::Read {
                    path: self.path.display().to_string(),
                    source,
                });
            }
        };
        // `{}` is what an interrupted first save leaves; an empty list is the honest reading.
        if text.trim().is_empty() || text.trim() == "{}" {
            return Ok(Vec::new());
        }
        let templates: Vec<TemplateModel> = serde_json::from_str(&text)?;
        Ok(templates)
    }

    /// Insert or replace by id. An empty id means a new template, named after its first line.
    pub fn save(&self, mut template: SessionTemplate) -> Result<SessionTemplate, TemplateError> {
        let name = template
            .name
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(TemplateError::InvalidName);
        }
        template.name = name;
        template.prompt = template.prompt.trim_end().to_string();
        if template.created_at <= 0 {
            template.created_at = crate::migrate::now_ms();
        }
        if template.id.trim().is_empty() {
            template.id = uuid::Uuid::new_v4().to_string();
        }

        let mut model = TemplateModel::from_v1(&template);
        if !template.id.is_empty()
            && let Some(existing) = self
                .list_v2()?
                .into_iter()
                .find(|row| row.id == template.id)
        {
            model.variables = existing.variables;
            model.slots = existing.slots;
            model.agent_sections = existing.agent_sections;
            model.variable_defs = existing.variable_defs;
        }
        let saved = self.save_v2(model)?;
        template = saved.to_v1();
        Ok(template)
    }

    /// Return the field-level changes that saving this model would apply.
    pub fn preview_save(
        &self,
        template: &TemplateModel,
    ) -> Result<Vec<model::FieldDiff>, TemplateError> {
        let Some(existing) = self
            .list_v2()?
            .into_iter()
            .find(|row| row.id == template.id)
        else {
            return Ok(Vec::new());
        };
        Ok(model::diff(&existing, template))
    }

    /// Insert or replace a v2 template, preserving the existing file format and atomic write.
    pub fn save_v2(&self, mut template: TemplateModel) -> Result<TemplateModel, TemplateError> {
        let name = template
            .name
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(TemplateError::InvalidName);
        }
        template.name = name;
        template.prompt = template.prompt.trim_end().to_string();
        if template.created_at <= 0 {
            template.created_at = crate::migrate::now_ms();
        }
        if template.id.trim().is_empty() {
            template.id = uuid::Uuid::new_v4().to_string();
        }
        // Roadmap 99: the template captures one validated hash per pinned skill. Rejecting an
        // invalid pin here keeps a later session from inheriting a hash that resolves to nothing.
        let pins = skill_pins::SkillPins::try_from_skills(&template.skills)
            .map_err(TemplateError::InvalidPin)?;
        template.skills = pins.skills();

        let mut list = self.list_v2()?;
        match list.iter().position(|row| row.id == template.id) {
            Some(index) => list[index] = template.clone(),
            None => {
                if list.len() >= MAX_TEMPLATES {
                    return Err(TemplateError::TooMany);
                }
                list.push(template.clone());
            }
        }
        self.write(&list)?;
        Ok(template)
    }

    pub fn delete(&self, id: &str) -> Result<(), TemplateError> {
        let mut list = self.list_v2()?;
        let before = list.len();
        list.retain(|row| row.id != id);
        if list.len() == before {
            return Err(TemplateError::Unknown(id.to_string()));
        }
        self.write(&list)
    }

    fn write(&self, list: &[TemplateModel]) -> Result<(), TemplateError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| TemplateError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let text = serde_json::to_string_pretty(list)?;
        let temp = self.path.with_extension("json.tmp");
        fs::write(&temp, text).map_err(|source| TemplateError::Write {
            path: temp.display().to_string(),
            source,
        })?;
        fs::rename(&temp, &self.path).map_err(|source| TemplateError::Write {
            path: self.path.display().to_string(),
            source,
        })
    }
}

pub mod model;
pub mod skill_pins;

pub use skill_pins::{SkillPin, SkillPins, TemplateSkillPin};

pub use model::{FieldDiff, TemplateAgentSection, TemplateModel, TemplateVariable, diff};
