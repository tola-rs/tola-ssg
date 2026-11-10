//! Which original inputs a reusable build may consume, and whether it may fetch.

/// Which original inputs a reusable build may consume. Hooks remain trusted host code.
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

    /// The scope `online`, `offline`, or `pure` names, and nothing else.
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
///
/// Does not constrain the external scripts selected by a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkAccess {
    /// Missing resources may be downloaded.
    #[default]
    Allowed,
    /// Only resources already available locally may be used.
    Denied,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name a scope writes is the name it reads back.
    #[test]
    fn scope_names_round_trip() {
        for scope in [InputScope::Online, InputScope::Offline, InputScope::Pure] {
            assert_eq!(InputScope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(InputScope::parse("Online"), None);
    }
}
