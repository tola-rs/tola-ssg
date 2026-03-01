use crate::core::UrlPath;

/// Handles permalink change side effects (old file cleanup)
///
/// Note: Permalink change detection is now done in CompilerActor.
/// This handler only processes side effects.
pub(super) struct PermalinkHandler;

impl PermalinkHandler {
    /// Cleanup old output file.
    pub(super) fn cleanup_old_output(old_url: &UrlPath) {
        use crate::config::cfg;
        crate::reload::compile::cleanup_output_for_url(&cfg(), old_url);
    }
}
