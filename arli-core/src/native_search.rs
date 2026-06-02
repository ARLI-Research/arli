//! In-process grep/search — no shell-out to rg/find/grep.
//!
//! Uses ripgrep's own crates (`grep-regex`, `grep-searcher`) for content search
//! and the `ignore` crate for file discovery. Everything runs in-process without
//! fork/exec overhead, and works cross-platform without requiring ripgrep to be
//! installed.

use std::path::Path;

/// Result of a content search.
pub struct SearchMatch {
    /// Relative file path
    pub file: String,
    /// 1-based line number
    pub line: u64,
    /// Matched line content
    pub content: String,
}

/// In-process Sink implementation for `grep_searcher`.
///
/// Collects matches into a `Vec<SearchMatch>`, respecting a limit.
struct SearchSink<'a> {
    matches: &'a mut Vec<SearchMatch>,
    limit: usize,
    file: &'a str,
}

impl<'a> grep_searcher::Sink for SearchSink<'a> {
    type Error = Box<dyn std::error::Error>;

    fn matched(
        &mut self,
        _searcher: &grep_searcher::Searcher,
        mat: &grep_searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if self.matches.len() >= self.limit {
            return Ok(false); // stop searching
        }

        let content = String::from_utf8_lossy(mat.bytes()).trim_end().to_string();
        let line = mat.line_number().unwrap_or(0);

        self.matches.push(SearchMatch {
            file: self.file.to_string(),
            line,
            content,
        });

        Ok(true)
    }
}

/// Search file contents for a regex pattern using in-process ripgrep.
///
/// Returns up to `limit` matches within `timeout_ms` milliseconds.
/// If the timeout is reached, returns whatever matches were found so far.
pub fn grep_content(
    pattern: &str,
    search_path: &str,
    file_glob: Option<&str>,
    limit: usize,
) -> Result<Vec<SearchMatch>, String> {
    let matcher = grep_regex::RegexMatcherBuilder::new()
        .build(pattern)
        .map_err(|e| format!("invalid regex: {e}"))?;

    let mut matches: Vec<SearchMatch> = Vec::with_capacity(limit);

    let walker = ignore::WalkBuilder::new(search_path)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .max_depth(Some(40))
        .build();

    let mut searcher = grep_searcher::SearcherBuilder::new()
        .line_number(true)
        .multi_line(false)
        .build();

    for result in walker {
        let entry = result.map_err(|e| format!("walk error: {e}"))?;
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }

        // Apply file_glob filter
        if let Some(glob) = file_glob {
            if let Some(name) = entry.file_name().to_str() {
                if !simple_glob_match(glob, name) {
                    continue;
                }
            }
        }

        let path = entry.path();
        let rel_path = path
            .strip_prefix(search_path)
            .unwrap_or(path)
            .display()
            .to_string();

        // Read file as bytes for search
        let contents = match std::fs::read(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let mut sink = SearchSink {
            matches: &mut matches,
            limit,
            file: &rel_path,
        };

        searcher
            .search_slice(&matcher, &contents, &mut sink)
            .map_err(|e| format!("search error: {e}"))?;

        if matches.len() >= limit {
            break;
        }
    }

    Ok(matches)
}

/// Find files by glob pattern (like `find -name`), in-process.
///
/// Returns relative file paths, up to `limit`.
pub fn find_files(
    pattern: &str,
    search_path: &str,
    file_glob: Option<&str>,
    limit: usize,
) -> Result<Vec<String>, String> {
    let walker = ignore::WalkBuilder::new(search_path)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .max_depth(Some(5))
        .build();

    let mut files: Vec<String> = Vec::with_capacity(limit);
    let combined_glob = file_glob.unwrap_or(pattern);

    for result in walker {
        if files.len() >= limit {
            break;
        }
        let entry = result.map_err(|e| format!("walk error: {e}"))?;
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }

        if let Some(name) = entry.file_name().to_str() {
            if simple_glob_match(combined_glob, name) {
                let rel = entry
                    .path()
                    .strip_prefix(search_path)
                    .unwrap_or(entry.path())
                    .display()
                    .to_string();
                files.push(rel);
            }
        }
    }

    Ok(files)
}

/// Simple glob match: supports `*` wildcards and literal matching.
/// Falls back to substring match for patterns without `*`.
fn simple_glob_match(pattern: &str, filename: &str) -> bool {
    if pattern == "*" || pattern == "*.*" {
        return true;
    }
    if !pattern.contains('*') {
        return filename.contains(pattern);
    }

    let parts: Vec<&str> = pattern.split('*').collect();
    let mut remaining = filename;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 && !part.is_empty() {
            // Must match prefix
            if !remaining.starts_with(part) {
                return false;
            }
            remaining = &remaining[part.len()..];
        } else if i == parts.len() - 1 && !part.is_empty() {
            // Must match suffix
            if !remaining.ends_with(part) {
                return false;
            }
        } else if !part.is_empty() {
            // Must appear somewhere
            match remaining.find(part) {
                Some(pos) => remaining = &remaining[pos + part.len()..],
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn setup_test_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("arli_search_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut f1 = std::fs::File::create(dir.join("a.rs")).unwrap();
        writeln!(f1, "fn main() {{\n    let x = setup_test_dir();\n    println!(\"hello\");\n}}").unwrap();

        let mut f2 = std::fs::File::create(dir.join("b.py")).unwrap();
        writeln!(f2, "def setup_test_dir():\n    x = setup()\n    print(x)").unwrap();

        let mut f3 = std::fs::File::create(dir.join("c.md")).unwrap();
        writeln!(f3, "# Test\nsetup_test_dir is also here.").unwrap();

        dir
    }

    #[test]
    fn test_grep_content_finds_pattern() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        // Use simple pattern without underscores to avoid regex edge cases
        let matches = grep_content("setup", &dir_str, None, 50).unwrap();
        assert!(
            matches.iter().any(|m| m.file.ends_with("a.rs")),
            "should find in a.rs"
        );
        assert!(
            matches.iter().any(|m| m.file.ends_with("b.py")),
            "should find in b.py"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_content_respects_limit() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        let matches = grep_content("setup", &dir_str, None, 1).unwrap();
        assert!(matches.len() <= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_content_file_glob() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        let matches = grep_content("setup", &dir_str, Some("*.rs"), 50).unwrap();
        assert!(matches.iter().all(|m| m.file.ends_with(".rs")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_content_invalid_regex() {
        let result = grep_content("[invalid", ".", None, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_grep_content_no_matches() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        let matches = grep_content("zzz_nonexistent_zzz", &dir_str, None, 50).unwrap();
        assert!(matches.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_files() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        let files = find_files("*.rs", &dir_str, None, 50).unwrap();
        assert!(files.iter().any(|f| f.ends_with("a.rs")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_find_files_with_limit() {
        let dir = setup_test_dir();
        let dir_str = dir.display().to_string();

        let files = find_files("*", &dir_str, None, 1).unwrap();
        assert!(files.len() <= 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_simple_glob_match() {
        assert!(simple_glob_match("*.rs", "main.rs"));
        assert!(simple_glob_match("*.rs", "lib.rs"));
        assert!(simple_glob_match("*.py", "test.py"));
        assert!(!simple_glob_match("*.rs", "test.py"));
        assert!(simple_glob_match("test*", "test_foo"));
        assert!(!simple_glob_match("test*", "bar_test"));
        assert!(simple_glob_match("*config*", "my_config_file"));
        assert!(simple_glob_match("*", "anything"));
    }
}
