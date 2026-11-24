//! Skills that ship with Rigel.
//!
//! These are ordinary `SKILL.md` bodies compiled into the binary. They are indexed as
//! `builtin:<slug>` rows so the shelf, the palette, `skills_search` and a session pin all work
//! without a network call, and the same body is what `rigel skills builtin --install` copies
//! into a harness store. Each body names the tool's real install path, its licence where that
//! was verified, and the boundary the agent must not cross.

use super::SkillsError;
use super::frontmatter;
use super::index::{SeedReport, SkillIndex};
use super::scan;
use super::source::SkillRecord;

#[derive(Debug, Clone, Copy)]
pub struct BuiltinSkill {
    /// Stable id, the `<slug>` in `builtin:<slug>`.
    pub slug: &'static str,
    pub name: &'static str,
    pub body: &'static str,
}

impl BuiltinSkill {
    pub fn id(&self) -> String {
        format!("builtin:{}", self.slug)
    }

    pub fn path(&self) -> String {
        format!("skills/builtin/{}/SKILL.md", self.slug)
    }
}

pub const BUILTIN_SKILLS: &[BuiltinSkill] = &[
    BuiltinSkill {
        slug: "skillevaluator",
        name: "skillevaluator",
        body: include_str!("../../../../skills/builtin/skillevaluator/SKILL.md"),
    },
    BuiltinSkill {
        slug: "headroom",
        name: "headroom",
        body: include_str!("../../../../skills/builtin/headroom/SKILL.md"),
    },
    BuiltinSkill {
        slug: "ag-ui",
        name: "ag-ui",
        body: include_str!("../../../../skills/builtin/ag-ui/SKILL.md"),
    },
    BuiltinSkill {
        slug: "mcptoon",
        name: "mcptoon",
        body: include_str!("../../../../skills/builtin/mcptoon/SKILL.md"),
    },
    BuiltinSkill {
        slug: "llmfit",
        name: "llmfit",
        body: include_str!("../../../../skills/builtin/llmfit/SKILL.md"),
    },
    BuiltinSkill {
        slug: "semble",
        name: "semble",
        body: include_str!("../../../../skills/builtin/semble/SKILL.md"),
    },
    BuiltinSkill {
        slug: "cognee-evaluate",
        name: "cognee-evaluate",
        body: include_str!("../../../../skills/builtin/cognee-evaluate/SKILL.md"),
    },
    BuiltinSkill {
        slug: "blender-mcp",
        name: "blender-mcp",
        body: include_str!("../../../../skills/builtin/blender-mcp/SKILL.md"),
    },
    BuiltinSkill {
        slug: "superpowers",
        name: "superpowers",
        body: include_str!("../../../../skills/builtin/superpowers/SKILL.md"),
    },
    BuiltinSkill {
        slug: "untangle-codebase",
        name: "untangle-codebase",
        body: include_str!("../../../../skills/builtin/untangle-codebase/SKILL.md"),
    },
];

pub fn all() -> &'static [BuiltinSkill] {
    BUILTIN_SKILLS
}

pub fn get(slug: &str) -> Option<&'static BuiltinSkill> {
    let slug = slug.strip_prefix("builtin:").unwrap_or(slug);
    BUILTIN_SKILLS.iter().find(|skill| skill.slug == slug)
}

impl SkillIndex {
    /// Write every bundled body into the local index. Idempotent: the body blob is addressed by
    /// SHA-256 and `write_skill` upserts, so a second run changes no row. The static review runs
    /// on the bundled text exactly as it would on a downloaded one; these bodies are reviewed
    /// instructions, not code, and the grade says so.
    pub async fn install_builtin(&self) -> Result<SeedReport, SkillsError> {
        let mut written = 0usize;
        let mut skipped = 0usize;
        for skill in all() {
            let parsed = frontmatter::parse(skill.body);
            let name = parsed.get("name").unwrap_or(skill.name).to_string();
            if name.trim().is_empty() {
                skipped += 1;
                continue;
            }
            let description = parsed.get("description").unwrap_or_default().to_string();
            let sha = self.blobs().put(skill.body.as_bytes())?;
            let capabilities = scan::scan(&parsed.body, &skill.path());
            let grade = scan::grade(&capabilities);
            let record = SkillRecord {
                id: skill.id(),
                name,
                description,
                source: "builtin".to_string(),
                repo: None,
                path: Some(skill.path()),
                url: None,
                body_sha: Some(sha),
                grade: Some(grade.to_string()),
                grade_scope: Some(scan::grade_scope().to_string()),
                license: parsed.get("license").map(ToOwned::to_owned),
                stars: None,
                installs: None,
                tokens: Some(SkillRecord::estimate_tokens(skill.body)),
                capabilities,
                simhash: Some(super::cluster::simhash(skill.body) as i64),
            };
            self.write_skill(&record).await?;
            written += 1;
        }
        Ok(SeedReport {
            rows: all().len(),
            written,
            skipped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_has_a_unique_slug_and_parseable_frontmatter() {
        let mut seen = std::collections::HashSet::new();
        for skill in all() {
            assert!(seen.insert(skill.slug), "duplicate {}", skill.slug);
            let parsed = frontmatter::parse(skill.body);
            assert_eq!(parsed.get("name"), Some(skill.name), "{}", skill.slug);
            let description = parsed.get("description").unwrap_or_default();
            assert!(
                (40..=200).contains(&description.len()),
                "{} has a {} character description",
                skill.slug,
                description.len()
            );
            assert!(parsed.body.trim().len() > 200, "{} is a stub", skill.slug);
        }
    }

    #[test]
    fn ids_and_paths_are_stable() {
        let skill = get("llmfit").unwrap();
        assert_eq!(skill.id(), "builtin:llmfit");
        assert_eq!(skill.path(), "skills/builtin/llmfit/SKILL.md");
        assert!(get("builtin:llmfit").is_some());
        assert!(get("nope").is_none());
    }
}
