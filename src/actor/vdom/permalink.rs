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

        let config = cfg();
        let output_dir = config.paths().output_dir();
        let rel_path = old_url.as_str().trim_matches('/');
        let old_file = if rel_path.is_empty() {
            output_dir.join("index.html")
        } else {
            output_dir.join(rel_path).join("index.html")
        };

        if old_file.exists() {
            if let Err(e) = std::fs::remove_file(&old_file) {
                crate::debug!("vdom"; "failed to remove {}: {}", old_file.display(), e);
                return;
            }
            crate::debug!("vdom"; "removed old output {}", old_file.display());
        }

        // Remove empty parent directory
        if let Some(parent) = old_file.parent()
            && parent.is_dir()
            && std::fs::read_dir(parent)
                .map(|mut e| e.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
    }
}
