//! Skill loader — reads skill definitions from .toml files on disk.
//!
//! Skills live in `~/.arli/skills/` and follow the typed contract format.
//! Each .toml file defines a skill with its parameters, returns, and safety rules.

use crate::error::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A skill definition loaded from disk.
#[derive(Debug, Clone)]
pub struct SkillDef {
    pub name: String,
    pub version: String,
    pub description: String,
    pub system_prompt: String,
    pub directory: PathBuf,
    /// Reference documents (filename, content). Loaded on-demand when skill is activated.
    pub references: Vec<(String, String)>,
    /// Helper scripts (filename, content).
    pub scripts: Vec<(String, String)>,
    /// Rough token count estimate (chars/4).
    pub total_tokens: usize,
}

/// Load all skills from a directory.
///
/// Scans for `<dir>/SKILL.md` (primary) or `<name>.toml` files.
/// By default only loads SKILL.md — references load on-demand via `load_skill_on_activate()`.
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
            if path.extension().is_some_and(|e| e == "toml") {
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

/// Load references and scripts for a skill on activation.
/// Call this when a skill is actually activated (not at discovery time).
pub fn load_skill_on_activate(skill: &mut SkillDef) {
    if skill.references.is_empty() {
        skill.references = load_reference_files(&skill.directory);
    }
    if skill.scripts.is_empty() {
        skill.scripts = load_script_files(&skill.directory);
    }
    // Recalculate tokens now that references are loaded
    skill.total_tokens = estimate_tokens(&skill.system_prompt, &skill.references, &skill.scripts);
}

/// Load reference files from the `references/` subdirectory.
fn load_reference_files(dir: &Path) -> Vec<(String, String)> {
    let refs_dir = dir.join("references");
    if !refs_dir.exists() || !refs_dir.is_dir() {
        return Vec::new();
    }

    let mut refs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&refs_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    refs.push((filename, content));
                }
            }
        }
    }
    refs
}

/// Load script files from the `scripts/` subdirectory.
fn load_script_files(dir: &Path) -> Vec<(String, String)> {
    let scripts_dir = dir.join("scripts");
    if !scripts_dir.exists() || !scripts_dir.is_dir() {
        return Vec::new();
    }

    let mut scripts = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&scripts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let filename = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    scripts.push((filename, content));
                }
            }
        }
    }
    scripts
}

/// Rough token estimate: chars / 4.
fn estimate_tokens(
    system_prompt: &str,
    references: &[(String, String)],
    scripts: &[(String, String)],
) -> usize {
    let mut total_chars = system_prompt.len();
    for (name, content) in references {
        total_chars += name.len() + content.len();
    }
    for (name, content) in scripts {
        total_chars += name.len() + content.len();
    }
    // Rough estimate: 1 token ≈ 4 chars
    (total_chars / 4).max(1)
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
        name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
    }

    let system_prompt = body.trim().to_string();
    let total_tokens = estimate_tokens(&system_prompt, &[], &[]);

    Ok(SkillDef {
        name,
        version,
        description,
        system_prompt,
        directory: dir.to_path_buf(),
        references: Vec::new(),
        scripts: Vec::new(),
        total_tokens,
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

    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let total_tokens = estimate_tokens(&system_prompt, &[], &[]);

    Ok(SkillDef {
        name,
        version,
        description,
        system_prompt,
        directory: dir,
        references: Vec::new(),
        scripts: Vec::new(),
        total_tokens,
    })
}

/// Create a skill from a template — writes SKILL.md + empty references/scripts dirs.
///
/// Takes a name, description, and system_prompt. Creates:
/// ```text
/// {HOME}/.arli/skills/{name}/
///   SKILL.md
///   references/   (empty)
///   scripts/      (empty)
/// ```
pub fn create_skill_from_template(
    name: &str,
    description: &str,
    system_prompt: &str,
) -> Result<SkillDef> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let skills_dir = PathBuf::from(home).join(".arli").join("skills");
    let skill_dir = skills_dir.join(name);

    std::fs::create_dir_all(skill_dir.join("references"))?;
    std::fs::create_dir_all(skill_dir.join("scripts"))?;

    let frontmatter = format!(
        "---\nname: {name}\nversion: \"0.1.0\"\ndescription: {description}\n---\n\n{system_prompt}\n",
        name = name,
        description = description,
        system_prompt = system_prompt,
    );

    let skill_md_path = skill_dir.join("SKILL.md");
    std::fs::write(&skill_md_path, frontmatter)?;

    let total_tokens = estimate_tokens(system_prompt, &[], &[]);

    tracing::info!("Created skill '{}' at {:?}", name, skill_dir);

    Ok(SkillDef {
        name: name.to_string(),
        version: "0.1.0".to_string(),
        description: description.to_string(),
        system_prompt: system_prompt.to_string(),
        directory: skill_dir,
        references: Vec::new(),
        scripts: Vec::new(),
        total_tokens,
    })
}

/// Tool sequence tracker — maps "tool1->tool2->tool3" sequences to occurrence counts.
pub type ToolSequenceTracker = HashMap<String, u32>;

/// Analyze tool call history and suggest a skill if a sequence repeats 3+ times.
///
/// Takes a tool sequence tracker (updated after each agent run) and checks if any
/// sequence has reached the threshold. Returns `Some((name, prompt_suggestion))` if so.
pub fn suggest_skill(tracker: &ToolSequenceTracker) -> Option<(String, String)> {
    for (sequence, count) in tracker {
        if *count >= 3 {
            let tools: Vec<&str> = sequence.split("->").collect();
            let name = format!("auto-{}", tools.join("-"));
            let prompt = format!(
                "# Auto-generated skill: {}\n\n\
                 This skill automates a repeated tool sequence:\n\
                 {}\n\n\
                 ## Steps\n\
                 {}\n\n\
                 Use this skill when you need to perform this sequence of operations.",
                name,
                tools.join(" → "),
                tools
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("{}. Run `{}`", i + 1, t))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            return Some((name, prompt));
        }
    }
    None
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
        assert!(skill.total_tokens > 0);
        assert!(skill.references.is_empty());
        assert!(skill.scripts.is_empty());

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
        assert!(skill.total_tokens > 0);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_skills_from_dir() {
        let dir = std::env::temp_dir().join("prom-test-skills");
        fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("my-skill");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("SKILL.md"),
            "---\nname: my-skill\n---\n\nBe helpful.\n",
        )
        .unwrap();

        let skills = load_skills_from_dir(&dir).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_load_skill_with_references_and_scripts() {
        let dir = std::env::temp_dir().join("prom-test-skill-refs");
        fs::create_dir_all(&dir).unwrap();

        let sub = dir.join("ref-skill");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("SKILL.md"),
            "---\nname: ref-skill\n---\n\nSkill with refs.\n",
        )
        .unwrap();

        // Create references/
        let refs_dir = sub.join("references");
        fs::create_dir_all(&refs_dir).unwrap();
        fs::write(refs_dir.join("api.md"), "# API\n\nEndpoint: /v1/foo\n").unwrap();
        fs::write(
            refs_dir.join("endpoints.md"),
            "# Endpoints\n\nGET /health\n",
        )
        .unwrap();

        // Create scripts/
        let scripts_dir = sub.join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();
        fs::write(scripts_dir.join("run.sh"), "#!/bin/bash\necho hello\n").unwrap();

        // Load basic (no refs loaded yet)
        let skills = load_skills_from_dir(&dir).unwrap();
        assert_eq!(skills.len(), 1);
        assert!(skills[0].references.is_empty());
        assert!(skills[0].scripts.is_empty());

        // Activate: now refs load
        let mut skill = skills[0].clone();
        load_skill_on_activate(&mut skill);
        assert_eq!(skill.references.len(), 2);
        assert_eq!(skill.scripts.len(), 1);
        assert!(skill.total_tokens > 0);

        // Verify reference content
        let api_ref = skill
            .references
            .iter()
            .find(|(n, _)| n == "api.md")
            .unwrap();
        assert!(api_ref.1.contains("/v1/foo"));

        // Verify script content
        let run_script = skill.scripts.iter().find(|(n, _)| n == "run.sh").unwrap();
        assert!(run_script.1.contains("echo hello"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_create_skill_from_template() {
        let skill = create_skill_from_template(
            "my-template-skill",
            "A test template skill",
            "# Instructions\n\nDo the template thing.",
        )
        .unwrap();

        assert_eq!(skill.name, "my-template-skill");
        assert_eq!(skill.description, "A test template skill");
        assert!(skill.system_prompt.contains("template thing"));
        assert!(skill.total_tokens > 0);

        // Verify files were created
        let skill_md = skill.directory.join("SKILL.md");
        assert!(skill_md.exists());
        let content = fs::read_to_string(&skill_md).unwrap();
        assert!(content.contains("my-template-skill"));
        assert!(content.contains("Do the template thing"));

        assert!(skill.directory.join("references").exists());
        assert!(skill.directory.join("scripts").exists());

        // Cleanup
        fs::remove_dir_all(&skill.directory).ok();
    }

    #[test]
    fn test_suggest_skill() {
        let mut tracker: ToolSequenceTracker = HashMap::new();

        // Not enough occurrences
        tracker.insert("read_file->patch->terminal".to_string(), 2);
        assert!(suggest_skill(&tracker).is_none());

        // Hit threshold
        tracker.insert("read_file->patch->terminal".to_string(), 3);
        let suggestion = suggest_skill(&tracker).unwrap();
        assert_eq!(suggestion.0, "auto-read_file-patch-terminal");
        assert!(suggestion.1.contains("read_file → patch → terminal"));

        // Multiple entries, one at threshold
        let mut tracker2: ToolSequenceTracker = HashMap::new();
        tracker2.insert("search_files->read_file".to_string(), 1);
        tracker2.insert("terminal->process->terminal".to_string(), 5);
        let suggestion2 = suggest_skill(&tracker2).unwrap();
        assert!(suggestion2.0.contains("terminal-process-terminal"));
    }

    #[test]
    fn test_estimate_tokens() {
        let tokens = estimate_tokens("hello world", &[], &[]);
        assert!(tokens > 0);
        assert_eq!(tokens, 2); // 11 chars / 4 = 2

        let refs = vec![(
            "api.md".to_string(),
            "# API docs\n\nSome content here.".to_string(),
        )];
        let tokens_with_refs = estimate_tokens("hello world", &refs, &[]);
        assert!(tokens_with_refs > tokens);
    }
}
