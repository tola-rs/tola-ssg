//! Input scope and network access rules for reusable builds.

/// Original inputs a reusable build may consume (hooks remain trusted host code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputScope {
    #[default]
    Online,
    Offline,
    Pure,
}

impl InputScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Online => "online",
            Self::Offline => "offline",
            Self::Pure => "pure",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "online" => Some(Self::Online),
            "offline" => Some(Self::Offline),
            "pure" => Some(Self::Pure),
            _ => None,
        }
    }
}

/// Network access for built-in package and remote-icon fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkAccess {
    /// Missing resources may be downloaded.
    #[default]
    Allowed,
    /// Only locally cached or vendored resources may be used.
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_roundtrip() {
        for scope in [InputScope::Online, InputScope::Offline, InputScope::Pure] {
            assert_eq!(InputScope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(InputScope::parse("Online"), None);
    }
}
