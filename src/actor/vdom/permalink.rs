use crate::core::UrlPath;

/// Handles permalink change side effects (old file cleanup)
///
/// Note: Permalink change detection is now done in CompilerActor.
/// This handler only processes side effects.
pub(super) struct PermalinkHandler;

impl PermalinkHandler {
    /// Cleanup old output file.
    pub(super) fn cleanup_old_output(config: &crate::config::SiteConfig, old_url: &UrlPath) {
        crate::reload::compile::cleanup_output_for_url(config, old_url);
    }
}
