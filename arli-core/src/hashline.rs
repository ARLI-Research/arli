//! Hashline editing — content-hash anchors for reliable file edits.
//!
//! Instead of retyping lines the model wants to change (which leads to
//! whitespace battles and "string not found" errors), each line is
//! identified by a short content hash. The model points at hashes;
//! the tool resolves them to current line numbers.
//!
//! ## How it works
//!
//! 1. Model reads the file via `read`, gets line numbers and hashes
//! 2. Model sends anchors: `[{hash: "a1b2c3d4", line_hint: 42}, ...]`
//! 3. Tool computes current file hashes, matches anchors
//! 4. If all anchors match → apply hunks
//! 5. If any anchor doesn't match → reject with current state (stale file)
//!
//! ## Hash format
//!
//! SHA-256 of the line content (trimmed), first 8 hex characters.
//! Example: line "fn main() {" → SHA-256 → "e3b0c442..." → "e3b0c442"

use sha2::{Digest, Sha256};

/// Compute the hashline anchor for a single line of text.
/// Returns the first 8 hex chars of SHA-256(line.trim_end()).
pub fn hash_line(line: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(line.trim_end().as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..4]) // 4 bytes = 8 hex chars
}

/// Compute hashlines for all lines in a file.
pub fn hash_lines(content: &str) -> Vec<String> {
    content.lines().map(hash_line).collect()
}

/// A hashline anchor — what the model sends to identify a line.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Anchor {
    /// 8-char hex hash of the line content
    pub hash: String,
    /// Optional line number hint (for ambiguity resolution)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_hint: Option<usize>,
}

/// A single hunk to apply.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Hunk {
    /// Anchor hashes identifying lines BEFORE the hunk (context)
    pub before: Vec<Anchor>,
    /// Anchor hashes identifying lines AFTER the hunk (context)
    pub after: Vec<Anchor>,
    /// Lines to remove (empty = insertion only)
    #[serde(default)]
    pub remove: Vec<String>,
    /// Lines to insert (empty = deletion only)
    #[serde(default)]
    pub insert: Vec<String>,
    /// Optional hash of the last line to be removed (for precise location)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hash: Option<String>,
}

/// Result of matching anchors against current file content.
#[derive(Debug)]
pub enum AnchorResult {
    /// All anchors matched, hunks can be applied at these line numbers.
    /// Vector of (start_line, end_line) — 1-indexed, inclusive.
    Matched {
        /// Start line (1-indexed) of the target region
        start: usize,
        /// End line (1-indexed) of the target region
        end: usize,
    },
    /// One or more anchors didn't match — file has changed.
    Stale {
        /// Which anchor hashes failed
        missing: Vec<String>,
        /// Current hashlines for the file (for model to retry)
        current_hashes: Vec<String>,
    },
}

/// Match anchors against current file content.
///
/// Returns matched line ranges for each hunk, or Stale if anchors don't match.
pub fn match_anchors(content: &str, anchors: &[Anchor]) -> AnchorResult {
    let lines: Vec<&str> = content.lines().collect();
    let hashes: Vec<String> = lines.iter().map(|l| hash_line(l)).collect();

    let mut matched_lines: Vec<usize> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    for anchor in anchors {
        let hash = &anchor.hash;

        // Find matching line
        let candidates: Vec<usize> = hashes
            .iter()
            .enumerate()
            .filter(|(_, h)| *h == hash)
            .map(|(i, _)| i + 1) // 1-indexed
            .collect();

        match candidates.len() {
            0 => {
                missing.push(hash.clone());
            }
            1 => {
                matched_lines.push(candidates[0]);
            }
            _ => {
                // Multiple matches — use line_hint to disambiguate
                if let Some(hint) = anchor.line_hint {
                    let closest = candidates
                        .iter()
                        .min_by_key(|&&l| (l as isize - hint as isize).unsigned_abs());
                    if let Some(&line) = closest {
                        matched_lines.push(line);
                    } else {
                        missing.push(hash.clone());
                    }
                } else {
                    // Pick the first one (ambiguous but best effort)
                    matched_lines.push(candidates[0]);
                }
            }
        }
    }

    if !missing.is_empty() {
        AnchorResult::Stale {
            missing,
            current_hashes: hashes,
        }
    } else if matched_lines.is_empty() {
        AnchorResult::Stale {
            missing: vec!["no anchors provided".into()],
            current_hashes: hashes,
        }
    } else {
        let start = *matched_lines.iter().min().unwrap();
        let end = *matched_lines.iter().max().unwrap();
        AnchorResult::Matched { start, end }
    }
}

/// Apply a set of hunks to file content.
///
/// Returns the new content or an error if anchors don't match.
pub fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut deleted: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut insertions: std::collections::BTreeMap<usize, Vec<String>> =
        std::collections::BTreeMap::new();

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        // Match before anchors
        let all_anchors: Vec<Anchor> = hunk
            .before
            .iter()
            .chain(hunk.after.iter())
            .cloned()
            .collect();
        let result = match_anchors(content, &all_anchors);

        let (start, end) = match result {
            AnchorResult::Matched { start, end } => (start, end),
            AnchorResult::Stale {
                missing,
                current_hashes,
            } => {
                return Err(format!(
                    "Hunk {}: file has changed — anchors not found: {:?}\n\
                     Current hashes: {:?}\n\
                     Re-read the file and retry.",
                    hunk_idx + 1,
                    missing,
                    current_hashes.iter().take(10).collect::<Vec<_>>(),
                ));
            }
        };

        // Mark lines for deletion
        if !hunk.remove.is_empty() {
            if let Some(ref target_hash) = hunk.target_hash {
                // Precise: find the line matching target_hash within range
                for line_num in start..=end {
                    if line_num <= lines.len() && hash_line(&lines[line_num - 1]) == *target_hash {
                        for offset in 0..hunk.remove.len() {
                            deleted.insert(line_num + offset);
                        }
                        break;
                    }
                }
            } else {
                // Remove lines from start to end
                for line_num in start..=end {
                    deleted.insert(line_num);
                }
            }
        }

        // Mark insertions at end position
        if !hunk.insert.is_empty() {
            let insert_at = end + 1;
            insertions
                .entry(insert_at)
                .or_default()
                .extend(hunk.insert.clone());
        }
    }

    // Build new content
    let mut result = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;

        // Insert before this line
        if let Some(insert_lines) = insertions.get(&line_num) {
            for ins in insert_lines {
                result.push(ins.clone());
            }
        }

        // Keep or skip
        if !deleted.contains(&line_num) {
            result.push(line.clone());
        }
    }

    // Insert at end
    let max_line = lines.len() + 1;
    if let Some(insert_lines) = insertions.get(&max_line) {
        for ins in insert_lines {
            result.push(ins.clone());
        }
    }

    Ok(result.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_line_deterministic() {
        let h1 = hash_line("fn main() {");
        let h2 = hash_line("fn main() {");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 8);
    }

    #[test]
    fn test_hash_line_whitespace_insensitive() {
        let h1 = hash_line("  hello  ");
        let h2 = hash_line("  hello");
        // Trimmed: both become "  hello" (trim_end only removes trailing)
        // Actually trim_end removes trailing whitespace: "  hello  " -> "  hello"
        // "  hello" -> "  hello"
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_lines() {
        let content = "line one\nline two\nline three\n";
        let hashes = hash_lines(content);
        assert_eq!(hashes.len(), 3);
        assert_ne!(hashes[0], hashes[1]);
    }

    #[test]
    fn test_match_anchors_exact() {
        let content = "fn main() {\n    let x = 42;\n    println!(\"{}\", x);\n}\n";
        let target_hash = hash_line("    let x = 42;");

        let anchors = vec![Anchor {
            hash: target_hash.clone(),
            line_hint: None,
        }];

        let result = match_anchors(content, &anchors);
        match result {
            AnchorResult::Matched { start, end } => {
                assert_eq!(start, 2); // line 2
                assert_eq!(end, 2);
            }
            AnchorResult::Stale { .. } => panic!("should match"),
        }
    }

    #[test]
    fn test_match_anchors_not_found() {
        let content = "hello\nworld\n";
        let anchors = vec![Anchor {
            hash: "deadbeef".into(),
            line_hint: None,
        }];

        let result = match_anchors(content, &anchors);
        match result {
            AnchorResult::Matched { .. } => panic!("should not match"),
            AnchorResult::Stale { missing, .. } => {
                assert_eq!(missing, vec!["deadbeef"]);
            }
        }
    }

    #[test]
    fn test_match_anchors_range() {
        let content = "line1\nline2\nline3\nline4\nline5\n";
        let h1 = hash_line("line2");
        let h5 = hash_line("line5");

        let anchors = vec![
            Anchor {
                hash: h1,
                line_hint: None,
            },
            Anchor {
                hash: h5,
                line_hint: None,
            },
        ];

        let result = match_anchors(content, &anchors);
        match result {
            AnchorResult::Matched { start, end } => {
                assert_eq!(start, 2);
                assert_eq!(end, 5);
            }
            AnchorResult::Stale { .. } => panic!("should match"),
        }
    }

    #[test]
    fn test_apply_hunks_simple_replace() {
        let content = "fn main() {\n    let x = 42;\n    println!(\"{}\", x);\n}\n";
        let target = hash_line("    let x = 42;");

        let hunks = vec![Hunk {
            before: vec![Anchor {
                hash: target.clone(),
                line_hint: None,
            }],
            after: vec![],
            remove: vec!["    let x = 42;".into()],
            insert: vec!["    let x = 99;".into()],
            target_hash: Some(target),
        }];

        let new_content = apply_hunks(content, &hunks).unwrap();
        assert!(new_content.contains("let x = 99;"));
        assert!(!new_content.contains("let x = 42;"));
        assert!(new_content.contains("fn main() {"));
        assert!(new_content.contains("println!"));
    }

    #[test]
    fn test_apply_hunks_stale_anchor() {
        let content = "line one\nline two\n";
        let hunks = vec![Hunk {
            before: vec![Anchor {
                hash: "deadbeef".into(),
                line_hint: None,
            }],
            after: vec![],
            remove: vec![],
            insert: vec![],
            target_hash: None,
        }];

        let result = apply_hunks(content, &hunks);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("file has changed"));
    }

    #[test]
    fn test_apply_hunks_insert_only() {
        let content = "fn main() {\n}\n";
        let anchor = hash_line("fn main() {");

        let hunks = vec![Hunk {
            before: vec![Anchor {
                hash: anchor,
                line_hint: None,
            }],
            after: vec![],
            remove: vec![],
            insert: vec!["    println!(\"hello\");".into()],
            target_hash: None,
        }];

        let new_content = apply_hunks(content, &hunks).unwrap();
        assert!(new_content.contains("println!(\"hello\");"));
        assert!(new_content.contains("fn main() {"));
    }

    #[test]
    fn test_apply_hunks_delete_only() {
        let content = "keep me\nremove me\nkeep me too\n";
        let target = hash_line("remove me");

        let hunks = vec![Hunk {
            before: vec![Anchor {
                hash: target.clone(),
                line_hint: None,
            }],
            after: vec![],
            remove: vec!["remove me".into()],
            insert: vec![],
            target_hash: Some(target),
        }];

        let new_content = apply_hunks(content, &hunks).unwrap();
        assert!(!new_content.contains("remove me"));
        assert!(new_content.contains("keep me"));
        assert!(new_content.contains("keep me too"));
    }
}
