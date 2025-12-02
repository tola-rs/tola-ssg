//! XML/HTML processing utilities.

pub mod assets;
pub mod common;
pub mod head;
pub mod link;
pub mod processor;

// Re-export for backward compatibility and ease of use
pub use processor::process_html;
