//! Auto-generated assets (CNAME).

use std::path::Path;

use crate::config::section::build::assets::FlattenEntry;

/// Extract domain from URL string
///
/// Uses `url` crate for proper parsing to handle edge cases:
/// - Strips port numbers: `example.com:8080` -> `example.com`
/// - Handles auth info: `user:pass@example.com` -> `example.com`
/// - Rejects localhost and IP addresses
///
/// # Returns
/// - `Some(domain)` if valid domain for CNAME
/// - `None` if URL is invalid, localhost, or IP address
fn extract_domain(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;

    // Must be http or https
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }

    // Get host (this strips port and auth automatically)
    let host = parsed.host_str()?;

    // Skip localhost
    if host == "localhost" || host.starts_with("127.") || host == "::1" {
        return None;
    }

    // Skip IP addresses
    if host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }

    Some(host.to_string())
}

/// Check if CNAME should be auto-generated
///
/// Returns the domain name if CNAME should be generated, None otherwise
///
/// # Rules
/// 1. `site.url` must be defined
/// 2. No flatten entry outputs as "CNAME", OR the source file doesn't exist
pub fn should_generate_cname(
    site_url: Option<&str>,
    flatten: &[FlattenEntry],
    site_root: &Path,
) -> Option<String> {
    let url = site_url?;
    let domain = extract_domain(url)?;

    for entry in flatten {
        if entry.output_name() == "CNAME" {
            let src_path = site_root.join(entry.source());
            if src_path.exists() {
                // User provided CNAME file exists, don't auto-generate
                return None;
            }
        }
    }

    Some(domain)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_extract_domain_strips_port() {
        // Port should be stripped
        assert_eq!(
            extract_domain("https://example.com:8080"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_domain("https://example.com:443/path"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_strips_auth() {
        // Auth info should be stripped
        assert_eq!(
            extract_domain("https://user:pass@example.com"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_domain("https://user@example.com"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_strips_path() {
        // Path should be stripped
        assert_eq!(
            extract_domain("https://example.com/path/to/page"),
            Some("example.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_rejects_no_scheme() {
        // URL without scheme should fail (url crate requires scheme)
        assert_eq!(extract_domain("example.com"), None);
    }

    #[test]
    fn test_extract_domain_rejects_invalid_scheme() {
        // Non-http(s) schemes should fail
        assert_eq!(extract_domain("ftp://example.com"), None);
        assert_eq!(extract_domain("file:///path"), None);
    }

    #[test]
    fn test_extract_domain_rejects_localhost() {
        assert_eq!(extract_domain("http://localhost"), None);
        assert_eq!(extract_domain("http://localhost:3000"), None);
        assert_eq!(extract_domain("http://127.0.0.1"), None);
        assert_eq!(extract_domain("http://127.0.0.1:8080"), None);
    }

    #[test]
    fn test_extract_domain_rejects_ip() {
        assert_eq!(extract_domain("http://192.168.1.1"), None);
        assert_eq!(extract_domain("http://10.0.0.1:8080"), None);
    }

    #[test]
    fn test_should_generate_cname_no_url() {
        let flatten: Vec<FlattenEntry> = vec![];
        let tmp = TempDir::new().unwrap();
        assert!(should_generate_cname(None, &flatten, tmp.path()).is_none());
    }

    #[test]
    fn test_should_generate_cname_with_domain() {
        let flatten: Vec<FlattenEntry> = vec![];
        let tmp = TempDir::new().unwrap();
        let result = should_generate_cname(Some("https://example.com"), &flatten, tmp.path());
        assert_eq!(result, Some("example.com".to_string()));
    }

    #[test]
    fn test_should_generate_cname_localhost_skipped() {
        let flatten: Vec<FlattenEntry> = vec![];
        let tmp = TempDir::new().unwrap();
        assert!(
            should_generate_cname(Some("http://localhost:3000"), &flatten, tmp.path()).is_none()
        );
    }

    #[test]
    fn test_should_generate_cname_user_file_exists() {
        let tmp = TempDir::new().unwrap();
        let cname_path = tmp.path().join("assets/CNAME");
        std::fs::create_dir_all(cname_path.parent().unwrap()).unwrap();
        std::fs::write(&cname_path, "user-domain.com").unwrap();

        let flatten = vec![FlattenEntry::Simple("assets/CNAME".into())];
        assert!(should_generate_cname(Some("https://example.com"), &flatten, tmp.path()).is_none());
    }

    #[test]
    fn test_should_generate_cname_user_file_missing() {
        let tmp = TempDir::new().unwrap();
        // User configured CNAME in flatten but file doesn't exist
        let flatten = vec![FlattenEntry::Simple("assets/CNAME".into())];
        let result = should_generate_cname(Some("https://example.com"), &flatten, tmp.path());
        assert_eq!(result, Some("example.com".to_string()));
    }
}
