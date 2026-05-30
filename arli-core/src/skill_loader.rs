//! Skill loader — reads skill definitions from .toml files on disk.
//!
//! Skills live in `~/.arli/skills/` and follow the typed contract format.
//! Each .toml file defines a skill with its parameters, returns, and safety rules.

use crate::error::Result;
use std::path::{Path, PathBuf};

/// A skill definition loaded from disk.
#[derive(Debug, Clone)]
pub struct SkillDef {
    pub name: String,
    pub version: String,
    pub description: String,
    pub system_prompt: String,
    pub directory: PathBuf,
}

/// Load all skills from a directory.
///
/// Scans for `<dir>/SKILL.md` (primary) or `<name>.toml` files.
pub fn load_skills_from_dir(dir: &Path) -> Result<Vec<SkillDef>> {
    let mut skills = Vec::new();

    if !dir.exists() {
        tracing::info!("Skills directory does not exist: {:?}", dir);
        return Ok(skills);
    }

    // Scan subdirectories for SKILL.md
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.exists() {
                    if let Ok(skill) = parse_skill_md(&skill_md, &path) {
                        skills.push(skill);
                    }
                }
            }
        }
    }

    // Also scan for .toml files directly in the directory
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "toml") {
                if let Ok(skill) = parse_skill_toml(&path) {
                    // Avoid duplicates
                    if !skills.iter().any(|s| s.name == skill.name) {
                        skills.push(skill);
                    }
                }
            }
        }
    }

    tracing::info!("Loaded {} skills from {:?}", skills.len(), dir);
    Ok(skills)
}

/// Parse a SKILL.md file (YAML frontmatter + markdown body).
///
/// Format:
/// ```markdown
/// ---
/// name: my-skill
/// version: "1.0"
/// description: Does something useful
/// ---
///
/// # Skill: My Skill
/// 
/// Instructions for the model...
/// ```
fn parse_skill_md(path: &Path, dir: &Path) -> Result<SkillDef> {
    let content = std::fs::read_to_string(path)?;
    
    let mut name = String::new();
    let mut version = "0.1.0".to_string();
    let mut description = String::new();
    let mut body = String::new();
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;

    for line in content.lines() {
        if line.trim() == "---" {
            if !frontmatter_done {
                if in_frontmatter {
                    frontmatter_done = true;
                } else {
                    in_frontmatter = true;
                }
            }
            continue;
        }

        if in_frontmatter && !frontmatter_done {
            if let Some((key, value)) = line.split_once(':') {
                match key.trim() {
                    "name" => name = value.trim().trim_matches('"').to_string(),
                    "version" => version = value.trim().trim_matches('"').to_string(),
                    "description" => description = value.trim().trim_matches('"').to_string(),
                    _ => {}
                }
            }
        } else if frontmatter_done || !in_frontmatter {
            body.push_str(line);
            body.push('\n');
        }
    }

    if name.is_empty() {
        // Derive name from directory
        name = dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
    }

    Ok(SkillDef {
        name,
        version,
        description,
        system_prompt: body.trim().to_string(),
        directory: dir.to_path_buf(),
    })
}

/// Parse a .toml skill file.
fn parse_skill_toml(path: &Path) -> Result<SkillDef> {
    let content = std::fs::read_to_string(path)?;
    let value: toml::Value = toml::from_str(&content)
        .map_err(|e| crate::error::Error::Tool(format!("Invalid TOML in {:?}: {}", path, e)))?;

    let name = value
        .get("skill")
        .and_then(|v| v.get("name"))
        .or_else(|| value.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let version = value
        .get("skill")
        .and_then(|v| v.get("version"))
        .or_else(|| value.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_string();
    let description = value
        .get("skill")
        .and_then(|v| v.get("description"))
        .or_else(|| value.get("description"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let system_prompt = value
        .get("skill")
        .and_then(|v| v.get("system_prompt").or_else(|| v.get("prompt")))
        .or_else(|| value.get("system_prompt").or_else(|| value.get("prompt")))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(SkillDef {
        name,
        version,
        description,
        system_prompt,
        directory: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_skill_md() {
        let dir = std::env::temp_dir().join("prom-test-skill");
        fs::create_dir_all(&dir).unwrap();

        let skill_md = dir.join("SKILL.md");
        fs::write(&skill_md, "---\nname: test-skill\nversion: \"1.2.0\"\ndescription: A test skill\n---\n\n# Test Skill\n\nDo the thing.\n").unwrap();

        let skill = parse_skill_md(&skill_md, &dir).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.version, "1.2.0");
        assert_eq!(skill.description, "A test skill");
        assert!(skill.system_prompt.contains("Do the thing"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_parse_skill_toml() {
        let dir = std::env::temp_dir();
        let path = dir.join("test-skill.toml");
        fs::write(&path, "[skill]\nname = \"toml-skill\"\nversion = \"2.0\"\ndescription = \"A TOML skill\"\nsystem_prompt = \"Do TOML things.\"\n").unwrap();

        let skill = parse_skill_toml(&path).unwrap();
        assert_eq!(skill.name, "toml-skill");
        assert_eq!(skill.version, "2.0");
        assert!(skill.system_prompt.contains("TOML things"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_skills_from_dir() {
        let dir = std::env::temp_dir().join("prom-test-skills");
        fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("my-skill");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("SKILL.md"), "---\nname: my-skill\n---\n\nBe helpful.\n").unwrap();

        let skills = load_skills_from_dir(&dir).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");

        fs::remove_dir_all(&dir).ok();
    }
}
