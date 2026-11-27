//! Local skill authoring commands share the core parser and install scanner.

use std::path::Path;
use std::process::ExitCode;

use rigel_core::skills::{SkillSpector, frontmatter, scan, scanner::SkillScanner};

/// Create a starter skill without replacing an existing file.
pub(crate) fn init(path: &Path, name: Option<&str>) -> ExitCode {
    if path.exists() {
        eprintln!("rigel: {} already exists", path.display());
        return ExitCode::from(2);
    }
    let name = name
        .map(str::to_owned)
        .or_else(|| {
            path.parent()
                .and_then(Path::file_name)
                .map(|part| part.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "new-skill".to_string());
    let content = format!(
        "---\nname: {name}\ndescription: Describe what this skill does.\n---\n\n# {name}\n\nExplain when to use this skill and the steps it should follow.\n"
    );
    match std::fs::write(path, content) {
        Ok(()) => {
            println!("created {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rigel: could not create {}: {error}", path.display());
            ExitCode::from(2)
        }
    }
}

/// Run both the indexer's frontmatter interpretation and the install-time security gate.
pub(crate) async fn lint(path: &Path) -> ExitCode {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) => {
            eprintln!("rigel: could not read {}: {error}", path.display());
            return ExitCode::from(2);
        }
    };
    let parsed = frontmatter::parse(&body);
    let fallback_name = path
        .parent()
        .and_then(Path::file_name)
        .map(|part| part.to_string_lossy().into_owned())
        .unwrap_or_else(|| "local-skill".to_string());
    let (name, description) = frontmatter::indexed_metadata(&body, &fallback_name);
    let capabilities = scan::scan(&parsed.body, &path.display().to_string());
    println!("frontmatter: valid ({}; {})", name, description);
    println!(
        "static review: grade {} ({} capability finding(s))",
        scan::grade(&capabilities),
        capabilities.len()
    );

    match scan_as_installer_does(&body).await {
        Ok(verdict) if !verdict.blocked() => {
            println!("install gate: {}", verdict.summary());
            ExitCode::SUCCESS
        }
        Ok(verdict) => {
            eprintln!(
                "rigel: install gate rejected this skill: {}",
                verdict.summary()
            );
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("rigel: install gate could not verify this skill: {error}");
            ExitCode::from(1)
        }
    }
}

async fn scan_as_installer_does(
    body: &str,
) -> Result<rigel_core::skills::ScanVerdict, rigel_core::SkillsError> {
    let directory = std::env::temp_dir().join(format!("rigel-authoring-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).map_err(rigel_core::SkillsError::Io)?;
    let path = directory.join("SKILL.md");
    let result = match std::fs::write(&path, body) {
        Ok(()) => {
            SkillSpector::default()
                .scan(&directory.to_string_lossy())
                .await
        }
        Err(error) => Err(rigel_core::SkillsError::Io(error)),
    };
    let _ = std::fs::remove_dir_all(directory);
    result
}

/// Test is intentionally the same gate the index uses before writing a skill to a harness.
pub(crate) async fn test(path: &Path) -> ExitCode {
    match lint(path).await {
        code if code == ExitCode::SUCCESS => {
            println!("skill passed the install-time load checks");
            ExitCode::SUCCESS
        }
        code => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_projection_matches_the_indexer_fallbacks() {
        assert_eq!(
            frontmatter::indexed_metadata("# body", "fallback"),
            (
                "fallback".to_string(),
                "No description in frontmatter.".to_string()
            )
        );
        assert_eq!(
            frontmatter::indexed_metadata(
                "---\nname: Example\ndescription: A skill\n---\nbody",
                "fallback"
            ),
            ("Example".to_string(), "A skill".to_string())
        );
    }

    #[test]
    fn init_creates_a_core_parseable_skill_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("SKILL.md");
        assert_eq!(init(&file, Some("Example")), ExitCode::SUCCESS);
        let content = std::fs::read_to_string(&file).unwrap();
        let parsed = frontmatter::parse(&content);
        assert_eq!(parsed.get("name"), Some("Example"));
        assert_eq!(
            frontmatter::indexed_metadata(&content, "fallback").0,
            "Example"
        );
        assert_eq!(init(&file, Some("replacement")), ExitCode::from(2));
        assert_eq!(
            frontmatter::parse(&std::fs::read_to_string(file).unwrap()).get("name"),
            Some("Example")
        );
    }
}
