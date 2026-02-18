//! Content processing utilities.

use crate::config::SiteConfig;
use std::fs;

/// Check if the content directory is effectively empty
pub fn is_content_empty(config: &SiteConfig) -> bool {
    let dir = &config.build.content;

    if !dir.exists() {
        return true;
    }

    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(iter) => iter.filter_map(|e| e.ok()).collect(),
        Err(_) => return true,
    };

    if entries.is_empty() {
        return true;
    }

    // Only index.typ exists and it's empty or whitespace-only
    if entries.len() == 1 {
        let entry = &entries[0];
        if entry.file_name() == "index.typ" {
            // Check if file is empty or contains only whitespace
            if let Ok(content) = fs::read_to_string(entry.path()) {
                return content.trim().is_empty();
            }
        }
    }

    false
}

/// Maybe inject hotreload script if content is HTML and ws_port is set
pub fn maybe_inject_hotreload(body: Vec<u8>, content_type: &str, ws_port: Option<u16>) -> Vec<u8> {
    match (content_type.starts_with("text/html"), ws_port) {
        (true, Some(port)) => inject_hotreload_script(&body, port),
        _ => body,
    }
}

/// Inject hotreload script before `</body>` tag
fn inject_hotreload_script(content: &[u8], ws_port: u16) -> Vec<u8> {
    use crate::embed::serve::{HOTRELOAD_JS, HotreloadVars};

    let script = HOTRELOAD_JS.external_tag_with_vars(&HotreloadVars { ws_port });
    let script_bytes = script.as_bytes();

    // Byte pattern for </body> - most generators use lowercase
    const PATTERN: &[u8] = b"</body>";

    // Reverse search for </body> using byte windows
    if let Some(pos) = content
        .windows(PATTERN.len())
        .rposition(|w| w.eq_ignore_ascii_case(PATTERN))
    {
        let mut result = Vec::with_capacity(content.len() + script_bytes.len());
        result.extend_from_slice(&content[..pos]);
        result.extend_from_slice(script_bytes);
        result.extend_from_slice(&content[pos..]);
        return result;
    }

    // No </body> found, append to end (browsers handle this gracefully)
    let mut result = Vec::with_capacity(content.len() + script_bytes.len());
    result.extend_from_slice(content);
    result.extend_from_slice(script_bytes);
    result
}
