//! Default values for configuration fields.
//!
//! These functions are used by serde for default deserialization.

// ============================================================================
// Common Defaults
// ============================================================================

pub fn r#true() -> bool {
    true
}

#[allow(unused)]
pub fn r#false() -> bool {
    false
}

// ============================================================================
// [base] Section Defaults
// ============================================================================

pub mod base {
    pub fn url() -> Option<String> {
        None
    }

    pub fn author() -> String {
        "<YOUR_NAME>".into()
    }

    pub fn email() -> String {
        "user@noreply.tola".into()
    }

    pub fn language() -> String {
        "zh-Hans".into()
    }
}

// ============================================================================
// [build] Section Defaults
// ============================================================================

pub mod build {
    use std::path::PathBuf;

    pub fn root() -> Option<PathBuf> {
        None
    }

    pub fn path_prefix() -> PathBuf {
        "".into()
    }

    pub fn content() -> PathBuf {
        "content".into()
    }

    pub fn output() -> PathBuf {
        "public".into()
    }

    pub fn assets() -> PathBuf {
        "assets".into()
    }

    pub fn templates() -> PathBuf {
        "templates".into()
    }

    pub fn utils() -> PathBuf {
        "utils".into()
    }

    pub mod rss {
        use std::path::PathBuf;

        pub fn path() -> PathBuf {
            "feed.xml".into()
        }
    }

    #[allow(unused)]
    pub mod slug {
        use super::super::super::SlugMode;

        pub fn default() -> SlugMode {
            SlugMode::default()
        }

        pub fn no() -> SlugMode {
            SlugMode::No
        }

        pub fn safe() -> SlugMode {
            SlugMode::Safe
        }

        pub fn on() -> SlugMode {
            SlugMode::On
        }
    }

    pub mod typst {
        use super::super::super::ExtractSvgType;

        pub fn command() -> Vec<String> {
            vec!["typst".into()]
        }

        pub mod svg {
            use super::ExtractSvgType;

            pub fn extract_type() -> ExtractSvgType {
                ExtractSvgType::default()
            }

            pub fn inline_max_size() -> String {
                "20KB".into()
            }

            pub fn dpi() -> f32 {
                96.
            }
        }
    }

    pub mod tailwind {
        use std::path::PathBuf;

        pub fn input() -> Option<PathBuf> {
            None
        }

        pub fn command() -> Vec<String> {
            vec!["tailwindcss".into()]
        }
    }
}

// ============================================================================
// [serve] Section Defaults
// ============================================================================

pub mod serve {
    pub fn interface() -> String {
        "127.0.0.1".into()
    }

    pub fn port() -> u16 {
        5277
    }
}

// ============================================================================
// [deploy] Section Defaults
// ============================================================================

pub mod deploy {
    pub fn provider() -> String {
        "github".into()
    }

    pub mod github {
        use std::path::PathBuf;

        pub fn url() -> String {
            "https://github.com/alice/alice.github.io".into()
        }

        pub fn branch() -> String {
            "main".into()
        }

        pub fn token_path() -> Option<PathBuf> {
            None
        }
    }
}
