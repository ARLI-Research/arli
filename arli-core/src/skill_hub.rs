//! Skill hub — discover, install, and manage skills.
//!
//! Skills are reusable agent capabilities defined as SKILL.md files.
//! The skill hub indexes local skills and provides search/install commands.
//!
//! Layout:
//!   ~/.arli/skills/
//!     <skill-name>/
//!       SKILL.md    — skill definition with YAML frontmatter
//!       scripts/    — optional helper scripts

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

/// Parsed skill metadata from SKILL.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMeta {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    pub path: PathBuf,
    #[serde(default)]
    pub enabled: bool,
}

/// TOML manifest for installed skills.
#[derive(Debug, Serialize, Deserialize)]
struct SkillsManifest {
    skills: Vec<SkillMeta>,
}

/// Skill hub manager.
pub struct SkillHub {
    skills_dir: PathBuf,
}

impl SkillHub {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self { skills_dir }
    }

    /// Discover all skills in the skills directory.
    pub fn discover(&self) -> anyhow::Result<Vec<SkillMeta>> {
        if !self.skills_dir.exists() {
            return Ok(Vec::new());
        }

        let mut skills = Vec::new();

        for entry in std::fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let skill_md = path.join("SKILL.md");
            if !skill_md.exists() {
                continue;
            }

            let content = match std::fs::read_to_string(&skill_md) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let (frontmatter, _) = parse_frontmatter(&content);
            let name = entry.file_name().to_string_lossy().to_string();

            let description = frontmatter
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("No description")
                .to_string();

            let version = frontmatter
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.1.0")
                .to_string();

            skills.push(SkillMeta {
                name,
                description,
                version,
                path,
                enabled: true,
            });
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }

    /// Search skills by name or description.
    pub fn search(&self, query: &str) -> anyhow::Result<Vec<SkillMeta>> {
        let query = query.to_lowercase();
        let all = self.discover()?;
        Ok(all
            .into_iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&query)
                    || s.description.to_lowercase().contains(&query)
            })
            .collect())
    }

    /// Enable/disable a skill.
    pub fn set_enabled(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        let skill_dir = self.skills_dir.join(name);
        if !skill_dir.exists() {
            anyhow::bail!("Skill '{}' not found", name);
        }

        let manifest_path = self.skills_dir.join("manifest.toml");
        let mut manifest = if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path)?;
            toml::from_str::<SkillsManifest>(&content).unwrap_or(SkillsManifest { skills: vec![] })
        } else {
            SkillsManifest { skills: vec![] }
        };

        if let Some(skill) = manifest.skills.iter_mut().find(|s| s.name == name) {
            skill.enabled = enabled;
        } else {
            manifest.skills.push(SkillMeta {
                name: name.to_string(),
                description: String::new(),
                version: String::new(),
                path: skill_dir,
                enabled,
            });
        }

        std::fs::write(&manifest_path, toml::to_string_pretty(&manifest)?)?;
        info!("Skill '{}' enabled={}", name, enabled);
        Ok(())
    }

    /// Get the skills directory path.
    pub fn dir(&self) -> &PathBuf {
        &self.skills_dir
    }
}

/// Parse YAML frontmatter from a SKILL.md file.
/// Frontmatter is delimited by `---` at start.
fn parse_frontmatter(content: &str) -> (toml::Value, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (toml::Value::Table(toml::Table::new()), content);
    }

    let rest = &trimmed[3..]; // skip opening ---
    if let Some(end) = rest.find("\n---") {
        let fm = &rest[..end];
        let body = &rest[end + 4..];
        let parsed = toml::from_str(fm).unwrap_or(toml::Value::Table(toml::Table::new()));
        (parsed, body)
    } else {
        (toml::Value::Table(toml::Table::new()), content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter() {
        let content = "---\nname = \"test-skill\"\ndescription = \"A test\"\n---\n\n# Body text";
        let (fm, body) = parse_frontmatter(content);
        assert_eq!(fm.get("name").and_then(|v| v.as_str()), Some("test-skill"));
        assert!(body.contains("# Body text"));
    }

    #[test]
    fn test_discover_empty_dir() {
        let dir = std::env::temp_dir().join("arli-test-skills");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let hub = SkillHub::new(dir.clone());
        let skills = hub.discover().unwrap();
        assert!(skills.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_skill() {
        let dir = std::env::temp_dir().join("arli-test-skills-2");
        let _ = std::fs::remove_dir_all(&dir);

        let skill_dir = dir.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription = \"My test skill\"\nversion = \"1.0\"\n---\n\nSkill body.",
        )
        .unwrap();

        let hub = SkillHub::new(dir.clone());
        let skills = hub.discover().unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].description, "My test skill");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search() {
        let dir = std::env::temp_dir().join("arli-test-skills-3");
        let _ = std::fs::remove_dir_all(&dir);

        let skill_dir = dir.join("web-scraper");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription = \"Scrapes web pages\"\n---\n\n.",
        )
        .unwrap();

        let hub = SkillHub::new(dir.clone());
        let results = hub.search("scrape").unwrap();
        assert_eq!(results.len(), 1);

        let results = hub.search("nonexistent").unwrap();
        assert!(results.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
