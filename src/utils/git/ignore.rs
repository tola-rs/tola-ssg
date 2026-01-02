use gix::{bstr::ByteSlice, glob::wildmatch};

// Constants for gix::ignore::search::pattern::Mode (which is private)
// See: https://github.com/Byron/gitoxide/blob/main/gix-ignore/src/search/pattern.rs
const MODE_NO_SUB_DIR: u32 = 1 << 0; // Pattern has no internal slash (matches basename unless absolute)
// const MODE_ENDS_WITH: u32 = 1 << 1;    // Pattern ends with something (not used here directly)
const MODE_MUST_MATCH_DIR: u32 = 1 << 2; // Pattern ends with slash (must match directory)
const MODE_NEGATIVE: u32 = 1 << 3; // Pattern starts with ! (negation)
const MODE_ABSOLUTE: u32 = 1 << 4; // Pattern starts with / (rooted at gitignore location)

/// Matches paths against .gitignore patterns.
///
/// This struct handles the complexity of gitignore rules, including:
/// - Pattern negation (!)
/// - Directory-only matches (ending with /)
/// - Absolute paths (starting with /)
/// - Basename vs path-relative matching
pub struct IgnoreMatcher {
    // Store (pattern_text, mode_bits)
    patterns: Vec<(gix::bstr::BString, u32)>,
}

impl IgnoreMatcher {
    /// Parse gitignore bytes into patterns
    pub fn new(gitignore: &[u8]) -> Self {
        let patterns: Vec<(gix::bstr::BString, u32)> = gix::ignore::parse(gitignore)
            .map(|(pattern, _, _)| (pattern.text, pattern.mode.bits()))
            .collect();
        Self { patterns }
    }

    /// Check if a path matches any ignore pattern
    ///
    /// Implements git's ignore logic:
    /// - Iterates patterns in order (last match wins)
    /// - Handles negation (!)
    /// - Handles directory-only patterns (ending in /)
    /// - Handles basename vs path-relative matching
    pub fn matches(&self, path: &str, is_dir: bool) -> bool {
        let mut is_ignored = false;
        for (text, mode) in &self.patterns {
            // If pattern must match a directory but path is not a directory, skip
            // e.g. "build/" should not match a file named "build"
            if (mode & MODE_MUST_MATCH_DIR != 0) && !is_dir {
                continue;
            }

            let mut match_path = path;
            let text_bytes = text.as_bstr();

            let is_absolute = mode & MODE_ABSOLUTE != 0;
            let has_internal_slash = mode & MODE_NO_SUB_DIR == 0;

            // If pattern is not absolute and has no internal slash, it matches against the basename.
            // Example: "*.log" matches "src/error.log" (basename "error.log").
            // Example: "/root.log" matches "root.log" but NOT "src/root.log".
            // Example: "target/debug" has slash, so it matches path-relatively.
            if !has_internal_slash && !is_absolute {
                match_path = path.rsplit_once('/').map_or(match_path, |(_, name)| name);
            }

            let is_match = wildmatch(
                text_bytes,
                match_path.into(),
                wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
            );

            if is_match {
                // If it's a negative match (starts with !), we un-ignore it.
                // Otherwise, we ignore it.
                is_ignored = mode & MODE_NEGATIVE == 0;
            }
        }
        is_ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gix_parse_behavior() {
        let gitignore = b"/root_only\nsub/dir\n*.log\ntemp/";
        for (pattern, _, _) in gix::ignore::parse(gitignore) {
            println!(
                "Pattern: {:?}, Mode: {:?} (bits: {:b})",
                pattern.text,
                pattern.mode,
                pattern.mode.bits()
            );
        }
    }

    #[test]
    fn test_ignore_matcher() {
        // Note: We use "target/**" to match nested files with simple wildmatch
        let gitignore = b"target/**\n*.log\n.DS_Store\n!important.log\nbuild/\n/root_only";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("target/debug/tola", false));
        assert!(matcher.matches("error.log", false));
        assert!(matcher.matches(".DS_Store", false));
        assert!(!matcher.matches("important.log", false));

        assert!(matcher.matches("build", true)); // Should match directory
        assert!(!matcher.matches("build", false)); // Should NOT match file named build

        // Absolute path test
        assert!(matcher.matches("root_only", false)); // Matches at root
        assert!(!matcher.matches("src/root_only", false)); // Should NOT match in subdir

        assert!(!matcher.matches("src/main.rs", false));
        assert!(!matcher.matches("README.md", false));
    }

    #[test]
    fn test_ignore_matcher_edge_cases() {
        // Note: Indentation in string literal is part of the content!
        // We should use a string without leading spaces for testing parsing.
        let gitignore = b"# This is a comment
*.tmp
!important.tmp
/TODO
docs/*.md";
        let matcher = IgnoreMatcher::new(gitignore);

        // Comments
        assert!(!matcher.matches("# This is a comment", false));

        // Simple wildcard
        assert!(matcher.matches("file.tmp", false));
        assert!(matcher.matches("dir/file.tmp", false));

        // Negation
        assert!(!matcher.matches("important.tmp", false));

        // Anchored
        assert!(matcher.matches("TODO", false));
        assert!(!matcher.matches("src/TODO", false));

        // Nested wildcard
        assert!(matcher.matches("docs/intro.md", false));
        assert!(!matcher.matches("docs/other/intro.md", false));
    }

    #[test]
    fn test_ignore_matcher_precedence() {
        let gitignore = b"*.log\n!important.log\nimportant.log";
        // Last one wins.
        let matcher = IgnoreMatcher::new(gitignore);
        assert!(matcher.matches("important.log", false));
    }

    #[test]
    fn test_ignore_matcher_doublestar() {
        let gitignore = b"**/temp";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("temp", false));
        assert!(matcher.matches("src/temp", false));
        assert!(matcher.matches("a/b/c/temp", false));

        assert!(!matcher.matches("temp/foo", false));
    }

    #[test]
    fn test_ignore_matcher_whitespace() {
        // Trailing spaces are ignored unless escaped
        let gitignore = b"*.txt   \n*.rs\\ ";
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("file.txt", false));
        assert!(matcher.matches("file.rs ", false));
        assert!(!matcher.matches("file.rs", false));
    }

    #[test]
    fn test_ignore_matcher_unicode() {
        let gitignore = "🧪/*.log\n!❗.log".as_bytes();
        let matcher = IgnoreMatcher::new(gitignore);

        assert!(matcher.matches("🧪/error.log", false));
        assert!(!matcher.matches("🧪/❗.log", false));
        assert!(!matcher.matches("🤷/error.log", false));
    }
}
