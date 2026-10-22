//! The activity log.
//!
//! These are *this application's* events — the calls it made and what came
//! back. They are deliberately not called an audit log: Cloudflare has one of
//! those, this application does not read it, and presenting a local record as
//! if it were the account's authoritative history would be a lie that matters
//! during an incident.
//!
//! Redaction happens at construction, not at display, so a secret cannot leak
//! by way of a screen that forgot to call the redacting formatter.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActivitySeverity {
    Info,
    Success,
    Warning,
    Error,
}

impl ActivitySeverity {
    pub const ALL: &'static [ActivitySeverity] =
        &[Self::Info, Self::Success, Self::Warning, Self::Error];

    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Success => "Success",
            Self::Warning => "Warning",
            Self::Error => "Error",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Info => "dialog-information-symbolic",
            Self::Success => "object-select-symbolic",
            Self::Warning => "dialog-warning-symbolic",
            Self::Error => "dialog-error-symbolic",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Info => "status-neutral",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityCategory {
    /// A call to the Cloudflare API that is not one of the more specific
    /// categories below.
    Api,
    /// A DNS record created, changed or removed.
    Dns,
    /// A tunnel's ingress configuration written.
    TunnelConfig,
    /// A tunnel created, renamed or deleted.
    Tunnel,
    /// An SSH connection attempt or host-key decision.
    Ssh,
    /// A systemd operation on a registered host.
    RemoteService,
    /// Something the application itself did — settings, credentials, startup.
    App,
}

impl ActivityCategory {
    pub const ALL: &'static [ActivityCategory] = &[
        Self::Api,
        Self::Dns,
        Self::TunnelConfig,
        Self::Tunnel,
        Self::Ssh,
        Self::RemoteService,
        Self::App,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Api => "Cloudflare API",
            Self::Dns => "DNS",
            Self::TunnelConfig => "Tunnel configuration",
            Self::Tunnel => "Tunnel",
            Self::Ssh => "SSH",
            Self::RemoteService => "Remote service",
            Self::App => "Application",
        }
    }

    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Api => "network-transmit-receive-symbolic",
            Self::Dns => "network-server-symbolic",
            Self::TunnelConfig => "document-edit-symbolic",
            Self::Tunnel => "network-vpn-symbolic",
            Self::Ssh => "utilities-terminal-symbolic",
            Self::RemoteService => "system-run-symbolic",
            Self::App => "applications-system-symbolic",
        }
    }
}

/// One line of the activity view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEvent {
    pub id: u64,
    pub at: DateTime<Utc>,
    pub severity: ActivitySeverity,
    pub category: ActivityCategory,
    /// One line, already redacted.
    pub summary: String,
    /// The longer form — a response body, an error chain — already redacted.
    pub detail: Option<String>,
    /// What the event was about: a tunnel name, a hostname, a host.
    pub target: Option<String>,
}

impl ActivityEvent {
    pub fn new(
        severity: ActivitySeverity,
        category: ActivityCategory,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            at: Utc::now(),
            severity,
            category,
            summary: redact(&summary.into()),
            detail: None,
            target: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        self.detail = if detail.trim().is_empty() {
            None
        } else {
            Some(redact(&detail))
        };
        self
    }

    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        let target = target.into();
        self.target = if target.trim().is_empty() {
            None
        } else {
            Some(redact(&target))
        };
        self
    }

    pub fn info(category: ActivityCategory, summary: impl Into<String>) -> Self {
        Self::new(ActivitySeverity::Info, category, summary)
    }

    pub fn success(category: ActivityCategory, summary: impl Into<String>) -> Self {
        Self::new(ActivitySeverity::Success, category, summary)
    }

    pub fn warning(category: ActivityCategory, summary: impl Into<String>) -> Self {
        Self::new(ActivitySeverity::Warning, category, summary)
    }

    pub fn error(category: ActivityCategory, summary: impl Into<String>) -> Self {
        Self::new(ActivitySeverity::Error, category, summary)
    }
}

/// The placeholder a redacted value is replaced with. Visible on purpose: a
/// silently truncated log is worse than one that says something was removed.
pub const REDACTED: &str = "[redacted]";

/// Strips anything that looks like a credential out of a string bound for a
/// log, a diagnostic export or the activity view.
///
/// This is a defence in depth, not the primary control. The primary control is
/// that secrets live in `SecretString` and are never formatted into a message
/// in the first place. This exists because "never" is a property of every call
/// site, and one of them will eventually be wrong.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for (index, line) in text.split_inclusive('\n').enumerate() {
        let _ = index;
        out.push_str(&redact_line(line));
    }

    out
}

fn redact_line(line: &str) -> String {
    // An Authorization header, however it is cased.
    if let Some(position) = find_ignore_case(line, "authorization:") {
        let (head, _) = line.split_at(position + "authorization:".len());
        return format!("{head} {REDACTED}\n");
    }
    if let Some(position) = find_ignore_case(line, "bearer ") {
        let (head, _) = line.split_at(position + "bearer ".len());
        return format!("{head}{REDACTED}\n");
    }

    // A JSON field or key=value pair whose name says it is a secret.
    let mut result = String::with_capacity(line.len());
    let mut rest = line;
    'outer: loop {
        for key in SECRET_KEYS {
            if let Some(position) = find_ignore_case(rest, key) {
                let after = position + key.len();
                let tail = &rest[after..];
                // A bare word like `token` must actually be a key (`:`/`=`
                // follows, optionally through quotes/spaces), otherwise
                // prose such as "run token for …" would redact "for".
                let separator_len = tail
                    .char_indices()
                    .find(|(_, c)| !matches!(c, ':' | '=' | ' ' | '"' | '\''))
                    .map(|(index, _)| index)
                    .unwrap_or(tail.len());
                if !tail[..separator_len].contains([':', '=']) {
                    continue;
                }
                result.push_str(&rest[..after]);
                // Skip the separator: `": "`, `"="`, `":"`.
                let value_start = tail
                    .char_indices()
                    .find(|(_, c)| !matches!(c, ':' | '=' | ' ' | '"' | '\''))
                    .map(|(index, _)| index)
                    .unwrap_or(tail.len());
                result.push_str(&tail[..value_start]);
                let value_end = tail[value_start..]
                    .char_indices()
                    .find(|(_, c)| matches!(c, '"' | '\'' | ',' | '}' | ' ' | '\n' | '\r'))
                    .map(|(index, _)| value_start + index)
                    .unwrap_or(tail.len());
                if value_end > value_start {
                    result.push_str(REDACTED);
                }
                rest = &tail[value_end..];
                continue 'outer;
            }
        }
        result.push_str(rest);
        break;
    }

    // A bare token-shaped run of characters, as a last resort. Cloudflare API
    // tokens are 40 characters of URL-safe base64; tunnel tokens are far
    // longer base64. Anything that long and that uniform in a log line is not
    // prose.
    redact_long_runs(&result)
}

const SECRET_KEYS: &[&str] = &[
    "api_token",
    "apitoken",
    "api-token",
    "api_key",
    "apikey",
    "api-key",
    "auth_key",
    "authkey",
    "auth-key",
    "token",
    "tunnel_secret",
    "tunnelsecret",
    "tunnel-secret",
    "tunnel_token",
    "tunneltoken",
    "tunnel-token",
    "x-auth-key",
    "x-auth-email",
    "authorization",
    "password",
    "passphrase",
    "secret",
    "credentials-file",
    "credentials_file",
    "credentialsfile",
    "private_key",
    "private-key",
    "privatekey",
];

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.to_ascii_lowercase();
    haystack.find(needle)
}

/// Replaces any run of characters that looks like a credential.
///
/// The test is deliberately narrow: at least 32 characters drawn from the
/// base64/URL-safe alphabet, containing an upper-case letter, a lower-case
/// letter and a digit. That is the shape of a Cloudflare API token
/// (`[A-Za-z0-9_-]{40}`) and of a base64 tunnel token, and it is *not* the
/// shape of the identifiers the activity log exists to show — a tunnel UUID, a
/// 32-character account tag and a zone ID are all lower-case hex, so they have
/// no upper-case letter and survive untouched.
///
/// `/` is included because standard base64 uses it and a token containing
/// one would otherwise split into short segments that escape. A filesystem
/// path is excluded by the mixed-case-plus-digit test below: paths are
/// almost never 32+ characters of mixed case plus a digit with no spaces.
fn redact_long_runs(text: &str) -> String {
    const THRESHOLD: usize = 32;

    fn looks_like_a_secret(run: &str) -> bool {
        run.chars().count() >= THRESHOLD
            && run.chars().any(|c| c.is_ascii_uppercase())
            && run.chars().any(|c| c.is_ascii_lowercase())
            && run.chars().any(|c| c.is_ascii_digit())
    }

    let mut out = String::with_capacity(text.len());
    let mut run = String::new();

    let flush = |run: &mut String, out: &mut String| {
        if looks_like_a_secret(run) {
            out.push_str(REDACTED);
        } else {
            out.push_str(run);
        }
        run.clear();
    };

    for character in text.chars() {
        if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_' | '.' | '+' | '=' | '/')
        {
            run.push(character);
        } else {
            flush(&mut run, &mut out);
            out.push(character);
        }
    }
    flush(&mut run, &mut out);

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_an_authorization_header() {
        let redacted = redact("Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345678");
        assert!(!redacted.contains("abcdefghij"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn redacts_a_lowercase_authorization_header() {
        let redacted = redact("authorization: bearer abcdefghijklmnopqrstuvwxyz012345678");
        assert!(!redacted.contains("abcdefghij"));
    }

    #[test]
    fn redacts_a_json_token_field() {
        let redacted =
            redact(r#"{"api_token":"v1.0-abcdefghijklmnop-qrstuvwxyz0123456789","ok":true}"#);
        assert!(!redacted.contains("qrstuvwxyz"));
        assert!(redacted.contains(REDACTED));
        assert!(redacted.contains("\"ok\""), "unrelated fields survive");
    }

    #[test]
    fn redacts_a_tunnel_secret_and_a_tunnel_token() {
        for input in [
            r#"{"tunnel_secret":"AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA="}"#,
            r#"{"tunnel_token":"eyJhIjoiYmJiIiwidCI6ImNjYyIsInMiOiJkZGQifQ=="}"#,
        ] {
            let redacted = redact(input);
            assert!(redacted.contains(REDACTED), "{input}");
            assert!(!redacted.contains("AQIDBAUGBwgJ"), "{input}");
            assert!(!redacted.contains("eyJhIjoiYmJi"), "{input}");
        }
    }

    #[test]
    fn redacts_a_bare_token_shaped_run() {
        let redacted = redact("failed for token AbCd3fGh1jKlMn0pQrStUvWxYz012345678901");
        assert!(!redacted.contains("AbCd3fGh1jKl"));
        assert!(redacted.contains(REDACTED));
    }

    #[test]
    fn leaves_ordinary_prose_alone() {
        let message = "Created tunnel “production web” in account Acme Ltd.";
        assert_eq!(redact(message), message);
    }

    #[test]
    fn leaves_identifiers_that_are_not_secrets_alone() {
        // A tunnel UUID is 36 characters but is not a credential, and the
        // activity log is useless without it.
        let message = "Deleted tunnel f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";
        let redacted = redact(message);
        assert!(
            redacted.contains("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415"),
            "got {redacted}"
        );
    }

    #[test]
    fn leaves_a_hostname_alone() {
        let message = "Published app.example.com to http://localhost:8080";
        assert_eq!(redact(message), message);
    }

    #[test]
    fn an_event_redacts_at_construction() {
        let event = ActivityEvent::error(
            ActivityCategory::Api,
            "request failed: Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456",
        )
        .with_detail(r#"{"api_token":"abcdefghijklmnopqrstuvwxyz0123456789"}"#);

        assert!(!event.summary.contains("abcdefghij"));
        assert!(!event.detail.as_deref().unwrap().contains("abcdefghij"));
    }

    #[test]
    fn every_severity_has_a_distinct_icon() {
        let mut icons: Vec<&str> = ActivitySeverity::ALL
            .iter()
            .map(|severity| severity.icon_name())
            .collect();
        icons.sort_unstable();
        let before = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), before);
    }
}
