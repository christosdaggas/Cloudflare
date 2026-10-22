//! Accounts, and what the token is allowed to do on one.

/// A Cloudflare account the token can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub name: String,
}

impl Account {
    /// A short form of the ID for a subtitle: enough to tell two accounts
    /// apart without filling a row with 32 hex characters.
    pub fn short_id(&self) -> String {
        if self.id.len() > 12 {
            format!("{}…{}", &self.id[..6], &self.id[self.id.len() - 4..])
        } else {
            self.id.clone()
        }
    }
}

/// The three permissions this application needs, and whether the token has
/// them.
///
/// `GET /user/tokens/verify` reports only that a token is live, not what it
/// can reach, so the state of each of these is inferred from whether the call
/// that needs it succeeded. That is why every variant has an "unknown" case:
/// claiming a permission is missing when the request simply has not been made
/// yet would be a lie the user cannot act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Unknown,
    Granted,
    Denied,
}

impl PermissionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Not checked",
            Self::Granted => "Granted",
            Self::Denied => "Missing",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Unknown => "dialog-question-symbolic",
            Self::Granted => "object-select-symbolic",
            Self::Denied => "dialog-error-symbolic",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Unknown => "dimmed",
            Self::Granted => "success",
            Self::Denied => "error",
        }
    }
}

/// One row of the permission checklist on the connection page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredPermission {
    /// The name exactly as it appears in the Cloudflare dashboard's token
    /// editor, so the user can find it without translating.
    pub dashboard_name: &'static str,
    pub scope: &'static str,
    pub level: &'static str,
    /// What stops working without it.
    pub needed_for: &'static str,
}

pub const REQUIRED_PERMISSIONS: &[RequiredPermission] = &[
    RequiredPermission {
        dashboard_name: "Cloudflare Tunnel",
        scope: "Account",
        level: "Edit",
        needed_for: "Listing, creating, configuring and deleting tunnels.",
    },
    RequiredPermission {
        dashboard_name: "Zone",
        scope: "Zone",
        level: "Read",
        needed_for: "Listing the domains a hostname can be published under.",
    },
    RequiredPermission {
        dashboard_name: "DNS",
        scope: "Zone",
        level: "Edit",
        needed_for: "Creating and removing the CNAME a public hostname needs.",
    },
];

/// The state of all three, as far as the application has been able to tell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenCapabilities {
    pub tunnels: PermissionStateSlot,
    pub zones: PermissionStateSlot,
    pub dns: PermissionStateSlot,
}

/// `PermissionState` with a `Default` of `Unknown`, so a freshly built
/// capabilities record does not claim anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionStateSlot(pub PermissionState);

impl Default for PermissionStateSlot {
    fn default() -> Self {
        Self(PermissionState::Unknown)
    }
}

impl TokenCapabilities {
    pub fn get(&self, index: usize) -> PermissionState {
        match index {
            0 => self.tunnels.0,
            1 => self.zones.0,
            _ => self.dns.0,
        }
    }

    pub fn set(&mut self, index: usize, state: PermissionState) {
        match index {
            0 => self.tunnels.0 = state,
            1 => self.zones.0 = state,
            _ => self.dns.0 = state,
        }
    }

    /// Whether every Cloudflare feature the application offers is usable.
    pub fn is_complete(&self) -> bool {
        [self.tunnels.0, self.zones.0, self.dns.0]
            .iter()
            .all(|state| *state == PermissionState::Granted)
    }

    /// Whether anything is known to be missing, as opposed to merely
    /// unchecked.
    pub fn has_known_gap(&self) -> bool {
        [self.tunnels.0, self.zones.0, self.dns.0].contains(&PermissionState::Denied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_start_unknown_not_denied() {
        let capabilities = TokenCapabilities::default();
        assert_eq!(capabilities.tunnels.0, PermissionState::Unknown);
        assert!(!capabilities.is_complete());
        assert!(
            !capabilities.has_known_gap(),
            "an unchecked permission is not a missing one"
        );
    }

    #[test]
    fn capabilities_complete_only_when_all_three_are_granted() {
        let mut capabilities = TokenCapabilities::default();
        for index in 0..3 {
            capabilities.set(index, PermissionState::Granted);
        }
        assert!(capabilities.is_complete());

        capabilities.set(2, PermissionState::Denied);
        assert!(!capabilities.is_complete());
        assert!(capabilities.has_known_gap());
    }

    #[test]
    fn the_checklist_matches_the_documented_scopes() {
        assert_eq!(REQUIRED_PERMISSIONS.len(), 3);
        assert_eq!(REQUIRED_PERMISSIONS[0].scope, "Account");
        assert_eq!(REQUIRED_PERMISSIONS[0].dashboard_name, "Cloudflare Tunnel");
        assert_eq!(REQUIRED_PERMISSIONS[1].level, "Read");
        assert_eq!(REQUIRED_PERMISSIONS[2].dashboard_name, "DNS");
    }

    #[test]
    fn short_id_keeps_both_ends() {
        let account = Account {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Acme".into(),
        };
        assert_eq!(account.short_id(), "012345…cdef");
    }
}
