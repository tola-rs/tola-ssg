//! Custom VDOM families for tola-ssg
//!
//! Defines site-specific element families using the `Family` trait.
//!
//! # Usage
//!
//! ```ignore
//! use crate::compiler::family::{TolaSite, Math, MathFamily};
//! use tola_vdom::Element;
//!
//! // Create element with Math family data
//! let elem: Element<TolaSite::Raw> = Element::with_ext(
//!     "span",
//!     TolaSite::RawExt::Math(Math::inline("x^2")),
//! );
//! ```

#![allow(dead_code)]

use tola_vdom::families::{HeadingFamily, LinkFamily, MediaFamily, SvgFamily};
use tola_vdom::vdom::{families, family, processed};

// =============================================================================
// Math Family
// =============================================================================

/// Processed data for Math elements (after KaTeX rendering)
#[processed(Math)]
pub struct MathProcessed {
    /// Rendered HTML (from KaTeX or similar)
    pub html: String,
}

/// Math element family for LaTeX math expressions.
#[family(processed = MathProcessed)]
pub struct Math {
    /// The raw LaTeX formula
    pub formula: String,
    /// Whether this is display math (block) or inline math
    pub display: bool,
}

impl Math {
    /// Create inline math
    pub fn inline(formula: impl Into<String>) -> Self {
        Self {
            formula: formula.into(),
            display: false,
        }
    }

    /// Create display (block) math
    pub fn display(formula: impl Into<String>) -> Self {
        Self {
            formula: formula.into(),
            display: true,
        }
    }
}

// =============================================================================
// Code Family (for syntax highlighting)
// =============================================================================

/// Processed data for Code elements (after syntax highlighting)
#[processed(Code)]
pub struct CodeProcessed {
    /// Syntax-highlighted HTML
    pub html: String,
}

/// Code element family for inline code and code blocks.
#[family(processed = CodeProcessed)]
pub struct Code {
    /// The raw code content
    pub content: String,
    /// Language hint (for code blocks)
    pub language: Option<String>,
    /// Whether this is a code block (true) or inline code (false)
    pub block: bool,
}

impl Code {
    /// Create inline code
    pub fn inline(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            language: None,
            block: false,
        }
    }

    /// Create a code block
    pub fn block(content: impl Into<String>, language: Option<String>) -> Self {
        Self {
            content: content.into(),
            language,
            block: true,
        }
    }
}

// =============================================================================
// Site Phase Definition
// =============================================================================

/// Tola site phase with all families (built-in + custom).
///
/// Generates:
/// - `TolaSite::Raw`, `TolaSite::Indexed`, `TolaSite::Processed` phase types
/// - `TolaSite::RawExt`, `TolaSite::IndexedExt`, `TolaSite::ProcessedExt` extension enums
/// - `TolaSite::identify()`, `TolaSite::index_ext()`, `TolaSite::process_ext()` functions
#[families]
pub struct TolaSite {
    // Built-in families
    link: LinkFamily,
    heading: HeadingFamily,
    svg: SvgFamily,
    media: MediaFamily,
    // Custom families
    math: MathFamily,
    code: CodeFamily,
}

// =============================================================================
// Type Aliases for convenience
// =============================================================================

/// Raw document type
pub type RawDocument = tola_vdom::Document<TolaSite::Raw>;

/// Indexed document type
pub type IndexedDocument = tola_vdom::Document<TolaSite::Indexed>;

/// Processed document type
pub type ProcessedDocument = tola_vdom::Document<TolaSite::Processed>;

/// Raw element type
pub type RawElement = tola_vdom::Element<TolaSite::Raw>;

/// Indexed element type
pub type IndexedElement = tola_vdom::Element<TolaSite::Indexed>;

/// Indexed phase type alias for convenience
pub type Indexed = TolaSite::Indexed;

/// Raw phase type alias for convenience
pub type Raw = TolaSite::Raw;

/// Processed phase type alias for convenience
pub type Processed = TolaSite::Processed;

// =============================================================================
// Cache Type Aliases
// =============================================================================

/// Cache entry for Indexed documents
pub type CacheEntry = tola_vdom::CacheEntry<Indexed>;

/// Non-thread-safe cache for Indexed documents
pub type Cache = tola_vdom::VdomCache<Indexed>;

/// Thread-safe shared cache for Indexed documents
pub type SharedCache = tola_vdom::SharedVdomCache<Indexed>;

/// Patch operation for Indexed phase
pub type PatchOp = tola_vdom::algo::PatchOp<Indexed>;
