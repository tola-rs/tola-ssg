//! Match relative paths without reading the filesystem.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};

/// Include and exclude patterns relative to a caller's input directory.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePatterns {
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

/// Compiled patterns for relative paths. The base directory is the empty path.
/// Matching an ancestor includes or excludes its descendants.
/// Equality, ordering, and hashing use only the declared patterns.
#[derive(Clone, Debug)]
pub struct FileSelection {
    patterns: FilePatterns,
    include: Vec<Pattern>,
    exclude: Vec<Pattern>,
}

impl FileSelection {
    /// Validate and compile include and exclude patterns.
    pub fn new(patterns: &FilePatterns) -> Result<Self, PatternError> {
        let compile = |key, patterns: &[String]| {
            patterns
                .iter()
                .map(|raw| Pattern::parse(key, raw))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            include: compile("include", &patterns.include)?,
            exclude: compile("exclude", &patterns.exclude)?,
            patterns: patterns.clone(),
        })
    }

    /// The patterns as declared by the author.
    pub fn patterns(&self) -> &FilePatterns {
        &self.patterns
    }

    /// Whether an include pattern matches this path or an ancestor.
    pub fn includes(&self, path: &Path) -> bool {
        self.include.iter().any(|pattern| pattern.contains(path))
    }

    /// Whether an exclude pattern matches this path or an ancestor.
    pub fn excludes(&self, path: &Path) -> bool {
        self.exclude.iter().any(|pattern| pattern.contains(path))
    }

    /// Whether this path is included and not excluded.
    pub fn matches(&self, path: &Path) -> bool {
        self.includes(path) && !self.excludes(path)
    }

    /// Whether a strict descendant could match. An excluded path prunes its subtree.
    pub fn could_match_below(&self, path: &Path) -> bool {
        !self.excludes(path)
            && self
                .include
                .iter()
                .any(|pattern| pattern.could_match_below(path))
    }

    /// Include patterns with no glob syntax, in declaration order.
    pub fn literal_includes(&self) -> impl Iterator<Item = &Path> {
        self.include.iter().filter_map(|pattern| match pattern {
            Pattern::Literal(path) => Some(path.as_path()),
            Pattern::Glob { .. } => None,
        })
    }

    /// Literal includes and the fixed directory prefixes of include globs.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.include
            .iter()
            .map(|pattern| match pattern {
                Pattern::Literal(path) | Pattern::Glob { prefix: path, .. } => path.clone(),
            })
            .collect()
    }
}

impl PartialEq for FileSelection {
    fn eq(&self, other: &Self) -> bool {
        self.patterns == other.patterns
    }
}

impl Eq for FileSelection {}

impl PartialOrd for FileSelection {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileSelection {
    fn cmp(&self, other: &Self) -> Ordering {
        self.patterns.cmp(&other.patterns)
    }
}

impl Hash for FileSelection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.patterns.hash(state);
    }
}

/// A declared pattern that is invalid or can leave its input directory.
#[derive(Debug, thiserror::Error)]
pub struct PatternError {
    key: String,
    pattern: String,
    #[source]
    glob: Option<globset::Error>,
}

impl PatternError {
    /// Qualify the include or exclude key with its configuration location.
    pub fn at(mut self, prefix: &str) -> Self {
        self.key = format!("{prefix}.{}", self.key);
        self
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { key, pattern, glob } = self;
        match glob {
            Some(error) => write!(f, "invalid `{key}` pattern `{pattern}`: {}", error.kind()),
            None => write!(
                f,
                "`{key}` pattern `{pattern}` must stay relative to its input directory"
            ),
        }
    }
}

#[derive(Clone, Debug)]
enum Pattern {
    Literal(PathBuf),
    Glob {
        matcher: Box<GlobMatcher>,
        prefix: PathBuf,
        depth: usize,
    },
}

impl Pattern {
    fn parse(key: &str, raw: &str) -> Result<Self, PatternError> {
        let error = |glob| PatternError {
            key: key.to_owned(),
            pattern: raw.to_owned(),
            glob,
        };
        if raw.is_empty()
            || raw.starts_with('/')
            || raw.contains(['\\', ':', '\0'])
            || raw.split('/').any(|part| part == "..")
        {
            return Err(error(None));
        }

        let parts: Vec<_> = raw
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        let fixed = parts
            .iter()
            .take_while(|part| !part.contains(['*', '?', '[', '{']))
            .count();
        let prefix = parts[..fixed].iter().collect();
        if fixed == parts.len() {
            return Ok(Self::Literal(prefix));
        }

        let pattern = parts.join("/");
        let matcher = GlobBuilder::new(&pattern)
            .literal_separator(true)
            .backslash_escape(false)
            .build()
            .map_err(|cause| error(Some(cause)))?
            .compile_matcher();
        let depth = if pattern.contains("**") {
            usize::MAX
        } else {
            // A character class can match a path separator.
            parts.len() + pattern.matches('[').count()
        };
        Ok(Self::Glob {
            matcher: Box::new(matcher),
            prefix,
            depth,
        })
    }

    fn contains(&self, path: &Path) -> bool {
        match self {
            Self::Literal(literal) => path.starts_with(literal),
            Self::Glob { matcher, .. } => path.ancestors().any(|path| matcher.is_match(path)),
        }
    }

    fn could_match_below(&self, path: &Path) -> bool {
        match self {
            Self::Literal(literal) => literal.starts_with(path) || path.starts_with(literal),
            Self::Glob { prefix, depth, .. } => {
                let shares_prefix = prefix.starts_with(path) || path.starts_with(prefix);
                (shares_prefix && path.components().count() < *depth) || self.contains(path)
            }
        }
    }
}
