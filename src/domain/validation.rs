//! Input rules.
//!
//! Everything the user types that will end up in a URL path, a JSON body or —
//! in the remote module — an argument to a command on someone else's machine
//! is checked here first. The remote side never interpolates a value into a
//! shell string, but validating anyway means a malformed value is rejected
//! with a sentence the user can act on rather than by a server 400 pages
//! later.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// A message written for the person who typed the value, not for a log.
    message: String,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

pub type ValidationResult<T = ()> = Result<T, ValidationError>;

/// A Cloudflare account or zone identifier: 32 lowercase hex characters.
pub fn account_id(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter an account ID."));
    }
    if value.len() != 32 || !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ValidationError::new(
            "An account ID is 32 hexadecimal characters. Copy it from the right-hand \
             sidebar of the Cloudflare dashboard.",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

/// A tunnel UUID, as it appears in a URL path.
pub fn tunnel_id(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    let bytes = value.as_bytes();
    let ok = value.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && value
            .chars()
            .enumerate()
            .all(|(i, c)| i == 8 || i == 13 || i == 18 || i == 23 || c.is_ascii_hexdigit());
    if ok {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(ValidationError::new("That is not a tunnel ID."))
    }
}

/// A connector (client) ID, as it appears in a URL query.
///
/// Cloudflare issues these as UUIDs, but the check is deliberately looser
/// than [`tunnel_id`]: a future format must not turn deregistration into a
/// validation error when the server would have accepted it.
pub fn connector_id(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("That connector ID is not valid."));
    }
    if value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ValidationError::new("That connector ID is not valid."));
    }
    Ok(value.to_string())
}

/// A DNS record ID, as it appears in a URL path.
///
/// Cloudflare issues these as 32 hex characters, but the check is
/// deliberately looser than that: the value comes from the server rather
/// than the user, so anything that cannot confuse the URL path is accepted
/// and a future format change cannot turn a repair into a validation error.
pub fn dns_record_id(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ValidationError::new("That DNS record ID is not valid."));
    }
    Ok(value.to_string())
}

/// A tunnel name.
///
/// Cloudflare accepts a fairly wide range here; this is the narrower set that
/// is unambiguous in a DNS-adjacent product and safe as a systemd unit
/// description.
pub fn tunnel_name(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter a name for the tunnel."));
    }
    if value.chars().count() > 64 {
        return Err(ValidationError::new(
            "A tunnel name may be at most 64 characters.",
        ));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' '))
    {
        return Err(ValidationError::new(
            "Use letters, digits, spaces, and the characters - _ . only.",
        ));
    }
    if value.starts_with(['-', '.']) || value.ends_with(['-', '.']) {
        return Err(ValidationError::new(
            "A tunnel name cannot begin or end with - or .",
        ));
    }
    Ok(value.to_string())
}

/// One label of a domain name: the part between two dots.
fn dns_label(label: &str) -> ValidationResult {
    if label.is_empty() {
        return Err(ValidationError::new(
            "A hostname cannot contain an empty label — check for a doubled dot.",
        ));
    }
    if label.len() > 63 {
        return Err(ValidationError::new(
            "Each part of a hostname may be at most 63 characters.",
        ));
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ValidationError::new(
            "A hostname may contain letters, digits, hyphens and underscores only.",
        ));
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(ValidationError::new(
            "No part of a hostname may begin or end with a hyphen.",
        ));
    }
    Ok(())
}

/// A fully qualified public hostname, as it will appear in an ingress rule and
/// as the name of a DNS record.
///
/// A single leading `*` label is allowed — Cloudflare supports wildcard
/// ingress — but the record it needs is the user's to create, so the caller
/// warns rather than silently proceeding.
pub fn hostname(value: &str) -> ValidationResult<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();

    if value.is_empty() {
        return Err(ValidationError::new("Enter a hostname."));
    }
    if value.len() > 253 {
        return Err(ValidationError::new(
            "A hostname may be at most 253 characters.",
        ));
    }
    if !value.contains('.') {
        return Err(ValidationError::new(
            "Enter the full hostname, including the domain — for example \
             app.example.com.",
        ));
    }

    for (index, label) in value.split('.').enumerate() {
        if index == 0 && label == "*" {
            continue;
        }
        dns_label(label)?;
    }

    Ok(value)
}

/// The subdomain part typed into the wizard, which may be empty (meaning the
/// zone apex) or several labels deep.
pub fn subdomain(value: &str, zone: &str) -> ValidationResult<String> {
    let value = value.trim().trim_matches('.').to_ascii_lowercase();
    if value.is_empty() {
        return hostname(zone);
    }
    // Typing the whole thing in is the commonest slip; accept it.
    if value == zone || value.ends_with(&format!(".{zone}")) {
        return hostname(&value);
    }
    hostname(&format!("{value}.{zone}"))
}

/// An HTTP path prefix for an ingress rule.
pub fn ingress_path(value: &str) -> ValidationResult<Option<String>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !value.starts_with('/') {
        return Err(ValidationError::new(
            "A path starts with / — for example /api.",
        ));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ValidationError::new("A path cannot contain spaces."));
    }
    if value.contains("://") || value.contains(['?', '#']) {
        return Err(ValidationError::new(
            "Enter only the path — the origin, query and fragment are separate.",
        ));
    }
    Ok(Some(value.to_string()))
}

/// The host part of an origin service: a hostname, an IPv4 address, or an IPv6
/// address in brackets.
pub fn origin_host(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter the origin host."));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(ValidationError::new(
            "An origin host cannot contain spaces.",
        ));
    }
    if value.starts_with('[') {
        return if value.ends_with(']') && value.len() > 2 {
            Ok(value.to_string())
        } else {
            Err(ValidationError::new(
                "An IPv6 origin must be written in brackets, as [::1].",
            ))
        };
    }
    if value.contains('/') || value.contains('@') || value.contains(':') {
        return Err(ValidationError::new(
            "Enter the host only — the scheme, port and path are separate fields.",
        ));
    }
    // A bare hostname or an IPv4 literal. `localhost` has no dot, so the
    // full-hostname rule above does not apply here.
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        for part in &parts {
            match part.parse::<u8>() {
                Ok(_) => {}
                Err(_) => {
                    return Err(ValidationError::new(
                        "That IPv4 address is not valid — each group is 0 to 255.",
                    ));
                }
            }
        }
        return Ok(value.to_ascii_lowercase());
    }
    for label in &parts {
        dns_label(label)?;
    }
    Ok(value.to_ascii_lowercase())
}

/// An HTTP `Host` header value: a host with an optional port.
pub fn http_host_header(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter the HTTP Host header."));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ValidationError::new(
            "An HTTP Host header cannot contain spaces or control characters.",
        ));
    }
    if value.contains('/') || value.contains('@') {
        return Err(ValidationError::new(
            "Enter a host with an optional port, not a URL.",
        ));
    }

    let (host, port) = if value.starts_with('[') {
        let Some(close) = value.find(']') else {
            return Err(ValidationError::new(
                "An IPv6 Host header must be written as [::1] or [::1]:8443.",
            ));
        };
        let host = &value[..=close];
        let tail = &value[close + 1..];
        let port = if tail.is_empty() {
            None
        } else {
            Some(tail.strip_prefix(':').ok_or_else(|| {
                ValidationError::new("An IPv6 Host header must be written as [::1] or [::1]:8443.")
            })?)
        };
        (host, port)
    } else {
        if value.matches(':').count() > 1 {
            return Err(ValidationError::new(
                "An IPv6 Host header must use brackets, as [::1]:8443.",
            ));
        }
        match value.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (value, None),
        }
    };

    let host = origin_host(host)?;
    match port {
        Some(port) => {
            let port = origin_port(port)?.ok_or_else(|| {
                ValidationError::new("Enter a port after the colon in the Host header.")
            })?;
            Ok(format!("{host}:{port}"))
        }
        None => Ok(host),
    }
}

pub fn origin_port(value: &str) -> ValidationResult<Option<u16>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    match value.parse::<u16>() {
        Ok(0) => Err(ValidationError::new("Port 0 is not a port.")),
        Ok(port) => Ok(Some(port)),
        Err(_) => Err(ValidationError::new("A port is a number from 1 to 65535.")),
    }
}

/// An absolute filesystem path, for a unix socket origin or a private key.
pub fn absolute_path(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter a path."));
    }
    if !value.starts_with('/') {
        return Err(ValidationError::new(
            "Enter an absolute path — starting with / — rather than ~ or a relative path.",
        ));
    }
    if value.contains('\0') {
        return Err(ValidationError::new("That path is not valid."));
    }
    Ok(value.to_string())
}

/// A systemd unit name.
///
/// This one is load-bearing for security, not just for tidiness: the name is
/// passed to `systemctl` on a remote machine. The remote module builds its
/// argument vector rather than a shell string, so a space could not inject a
/// second command even if one got through — but a unit name has a defined
/// shape and anything outside it is a mistake worth catching here.
pub fn systemd_unit(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter a service unit name."));
    }
    if value.len() > 128 {
        return Err(ValidationError::new("That unit name is too long."));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@'))
    {
        return Err(ValidationError::new(
            "A unit name may contain letters, digits, and the characters - _ . @ only.",
        ));
    }
    if value.starts_with('.') || value.contains("..") || value.contains('/') {
        return Err(ValidationError::new("That is not a unit name."));
    }
    let value = if value.contains('.') {
        value.to_string()
    } else {
        format!("{value}.service")
    };
    if !value.ends_with(".service") {
        return Err(ValidationError::new(
            "This application only controls .service units.",
        ));
    }
    Ok(value)
}

/// An SSH login name.
pub fn ssh_username(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter the SSH username."));
    }
    if value.len() > 32 {
        return Err(ValidationError::new("That username is too long."));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ValidationError::new(
            "A username may contain letters, digits, and the characters - _ . only.",
        ));
    }
    if value.starts_with('-') {
        return Err(ValidationError::new(
            "A username cannot begin with a hyphen.",
        ));
    }
    Ok(value.to_string())
}

/// The address of an SSH host: a hostname or an IP address, never a
/// `user@host:port` string.
pub fn ssh_address(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter a hostname or IP address."));
    }
    if value.contains('@') {
        return Err(ValidationError::new(
            "Enter the host only — the username is a separate field.",
        ));
    }
    if value.contains(' ') || value.contains('/') {
        return Err(ValidationError::new("That is not a hostname or address."));
    }
    if value.starts_with('[') && value.ends_with(']') {
        return Ok(value.trim_matches(['[', ']']).to_string());
    }
    if value.contains(':') {
        // Bare IPv6 (no port — the port is a separate field). A single
        // colon means host:port, which belongs in the port field.
        return if (value.contains("::") || value.matches(':').count() >= 2)
            && value
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
        {
            Ok(value.to_ascii_lowercase())
        } else {
            Err(ValidationError::new(
                "Enter the host only — the port is a separate field.",
            ))
        };
    }
    for label in value.split('.') {
        dns_label(label)?;
    }
    Ok(value.to_ascii_lowercase())
}

pub fn ssh_port(value: &str) -> ValidationResult<u16> {
    match value.trim() {
        "" => Ok(22),
        other => match other.parse::<u16>() {
            Ok(0) => Err(ValidationError::new("Port 0 is not a port.")),
            Ok(port) => Ok(port),
            Err(_) => Err(ValidationError::new("A port is a number from 1 to 65535.")),
        },
    }
}

/// A friendly label for a saved remote host.
pub fn friendly_name(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Enter a name."));
    }
    if value.chars().count() > 64 {
        return Err(ValidationError::new("Keep the name under 64 characters."));
    }
    if value.chars().any(|c| c.is_control()) {
        return Err(ValidationError::new("That name is not valid."));
    }
    Ok(value.to_string())
}

/// An SSH host-key fingerprint, in the `SHA256:base64` form OpenSSH prints.
pub fn host_key_fingerprint(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    let Some(digest) = value.strip_prefix("SHA256:") else {
        return Err(ValidationError::new(
            "A fingerprint looks like SHA256: followed by 43 characters.",
        ));
    };
    if digest.len() < 40
        || !digest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '='))
    {
        return Err(ValidationError::new("That is not a SHA-256 fingerprint."));
    }
    Ok(value.to_string())
}

/// A Cloudflare API token, checked only for shape before it is spent on a
/// network round trip.
///
/// The token itself is never logged or echoed; this returns the trimmed value
/// so the caller can move it straight into a `SecretString`.
pub fn api_token(value: &str) -> ValidationResult<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ValidationError::new("Paste an API token."));
    }
    if value.len() < 32 {
        return Err(ValidationError::new(
            "That looks too short for an API token. A token is about 40 characters.",
        ));
    }
    if value.len() > 256 {
        return Err(ValidationError::new("That is longer than any API token."));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
    {
        return Err(ValidationError::new(
            "An API token contains only letters, digits, and - _ . ~ — check for a \
             stray space or a copied line break.",
        ));
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_accepts_ordinary_names() {
        assert_eq!(hostname("App.Example.COM").unwrap(), "app.example.com");
        assert_eq!(hostname("a.b.c.example.com").unwrap(), "a.b.c.example.com");
        assert_eq!(hostname("app.example.com.").unwrap(), "app.example.com");
    }

    #[test]
    fn hostname_accepts_a_leading_wildcard() {
        assert_eq!(hostname("*.example.com").unwrap(), "*.example.com");
    }

    #[test]
    fn hostname_rejects_a_bare_label() {
        assert!(hostname("app").is_err());
    }

    #[test]
    fn hostname_rejects_a_wildcard_that_is_not_first() {
        assert!(hostname("a.*.example.com").is_err());
    }

    #[test]
    fn hostname_rejects_empty_labels_and_bad_hyphens() {
        assert!(hostname("app..example.com").is_err());
        assert!(hostname("-app.example.com").is_err());
        assert!(hostname("app-.example.com").is_err());
    }

    #[test]
    fn subdomain_composes_with_the_zone() {
        assert_eq!(subdomain("app", "example.com").unwrap(), "app.example.com");
        assert_eq!(subdomain("", "example.com").unwrap(), "example.com");
        assert_eq!(
            subdomain("app.example.com", "example.com").unwrap(),
            "app.example.com"
        );
        assert_eq!(subdomain("a.b", "example.com").unwrap(), "a.b.example.com");
    }

    #[test]
    fn origin_host_rejects_a_pasted_url() {
        assert!(origin_host("http://localhost").is_err());
        assert!(origin_host("localhost:8080").is_err());
        assert_eq!(origin_host("LocalHost").unwrap(), "localhost");
        assert_eq!(origin_host("192.168.1.10").unwrap(), "192.168.1.10");
        assert_eq!(origin_host("[::1]").unwrap(), "[::1]");
    }

    #[test]
    fn http_host_header_accepts_an_optional_port() {
        assert_eq!(
            http_host_header("Example.com:8443").unwrap(),
            "example.com:8443"
        );
        assert_eq!(http_host_header("[::1]:8443").unwrap(), "[::1]:8443");
        assert_eq!(http_host_header("localhost").unwrap(), "localhost");
    }

    #[test]
    fn http_host_header_rejects_urls_and_invalid_ports() {
        assert!(http_host_header("https://example.com").is_err());
        assert!(http_host_header("example.com:0").is_err());
        assert!(http_host_header("example.com:65536").is_err());
        assert!(http_host_header("::1:8443").is_err());
    }

    #[test]
    fn systemd_unit_appends_the_suffix_and_refuses_a_path() {
        assert_eq!(systemd_unit("cloudflared").unwrap(), "cloudflared.service");
        assert_eq!(
            systemd_unit("cloudflared.service").unwrap(),
            "cloudflared.service"
        );
        assert!(systemd_unit("../etc/passwd").is_err());
        assert!(systemd_unit("cloudflared.service; rm -rf /").is_err());
        assert!(systemd_unit("cloudflared.socket").is_err());
    }

    #[test]
    fn api_token_rejects_shell_metacharacters_and_short_values() {
        assert!(api_token("short").is_err());
        assert!(api_token(&"a".repeat(40)).is_ok());
        assert!(api_token(&format!("{}$(id)", "a".repeat(40))).is_err());
    }

    #[test]
    fn api_token_error_never_repeats_the_token() {
        let secret = format!("{}!!", "z".repeat(40));
        let error = api_token(&secret).unwrap_err();
        assert!(!error.message().contains(&secret));
    }

    #[test]
    fn account_id_is_thirty_two_hex_characters() {
        assert!(account_id(&"a".repeat(32)).is_ok());
        assert!(account_id(&"a".repeat(31)).is_err());
        assert!(account_id(&"g".repeat(32)).is_err());
    }

    #[test]
    fn ssh_address_rejects_a_user_at_host() {
        assert!(ssh_address("root@example.com").is_err());
        assert_eq!(ssh_address("Example.COM").unwrap(), "example.com");
        assert_eq!(ssh_address("[2001:db8::1]").unwrap(), "2001:db8::1");
    }

    #[test]
    fn ssh_port_defaults_to_twenty_two() {
        assert_eq!(ssh_port("").unwrap(), 22);
        assert_eq!(ssh_port("2222").unwrap(), 2222);
        assert!(ssh_port("0").is_err());
        assert!(ssh_port("70000").is_err());
    }
}
