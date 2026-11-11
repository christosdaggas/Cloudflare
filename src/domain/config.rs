//! A tunnel's remote configuration, and the read-modify-write rules that keep
//! editing one from destroying the rest of it.
//!
//! `PUT .../configurations` replaces the whole document. There is no way to
//! add a single ingress rule, so every edit in this application is
//! *read the current configuration, change one thing, write it all back*, and
//! the correctness of that hinges entirely on nothing being lost in the
//! middle. Two things protect it:
//!
//! * Every struct here carries a `#[serde(flatten)]` map of the keys this
//!   build does not know about, so a field Cloudflare adds tomorrow survives a
//!   write today.
//! * The catch-all rule is enforced by [`TunnelConfig::normalise`] rather than
//!   by the caller remembering to append one. cloudflared refuses a
//!   configuration whose last rule has a hostname, and a refused configuration
//!   takes the tunnel down.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::domain::hostname::{OriginService, PublicHostname};
use crate::domain::validation::{ValidationError, ValidationResult};

/// What the last ingress rule serves when the application has to invent one.
pub const CATCH_ALL_SERVICE: &str = "http_status:404";

/// The service a *disabled* hostname's rule carries.
///
/// Disabling a hostname removes nothing: the rule stays, its service becomes
/// this, and the origin it served is remembered locally (see
/// `storage::disabled`). The hostname therefore remains visible in the list,
/// and enabling it restores the remembered service exactly.
pub const DISABLED_SERVICE: &str = CATCH_ALL_SERVICE;

/// `originRequest`, the per-rule (or global) origin behaviour.
///
/// The named fields are the ones this application reads or offers to edit.
/// Everything else Cloudflare stores lands in `extra` and is written back
/// untouched — which is the whole point, because a user who configured
/// `caPool` or `access` through the dashboard must not lose it because this
/// application changed a port.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OriginRequest {
    #[serde(rename = "connectTimeout", skip_serializing_if = "Option::is_none")]
    pub connect_timeout: Option<i64>,
    #[serde(rename = "tlsTimeout", skip_serializing_if = "Option::is_none")]
    pub tls_timeout: Option<i64>,
    #[serde(rename = "tcpKeepAlive", skip_serializing_if = "Option::is_none")]
    pub tcp_keep_alive: Option<i64>,
    #[serde(rename = "noHappyEyeballs", skip_serializing_if = "Option::is_none")]
    pub no_happy_eyeballs: Option<bool>,
    #[serde(
        rename = "keepAliveConnections",
        skip_serializing_if = "Option::is_none"
    )]
    pub keep_alive_connections: Option<i64>,
    #[serde(rename = "keepAliveTimeout", skip_serializing_if = "Option::is_none")]
    pub keep_alive_timeout: Option<i64>,
    #[serde(rename = "httpHostHeader", skip_serializing_if = "Option::is_none")]
    pub http_host_header: Option<String>,
    #[serde(rename = "originServerName", skip_serializing_if = "Option::is_none")]
    pub origin_server_name: Option<String>,
    #[serde(rename = "caPool", skip_serializing_if = "Option::is_none")]
    pub ca_pool: Option<String>,
    #[serde(rename = "noTLSVerify", skip_serializing_if = "Option::is_none")]
    pub no_tls_verify: Option<bool>,
    #[serde(
        rename = "disableChunkedEncoding",
        skip_serializing_if = "Option::is_none"
    )]
    pub disable_chunked_encoding: Option<bool>,
    #[serde(rename = "bastionMode", skip_serializing_if = "Option::is_none")]
    pub bastion_mode: Option<bool>,
    #[serde(rename = "proxyAddress", skip_serializing_if = "Option::is_none")]
    pub proxy_address: Option<String>,
    #[serde(rename = "proxyPort", skip_serializing_if = "Option::is_none")]
    pub proxy_port: Option<i64>,
    #[serde(rename = "proxyType", skip_serializing_if = "Option::is_none")]
    pub proxy_type: Option<String>,
    #[serde(rename = "http2Origin", skip_serializing_if = "Option::is_none")]
    pub http2_origin: Option<bool>,
    #[serde(rename = "matchSNItoHost", skip_serializing_if = "Option::is_none")]
    pub match_sni_to_host: Option<bool>,

    /// Everything else, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl OriginRequest {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The fields worth showing on the detail page, as label/value pairs.
    /// Only what is actually set — an empty list means "all defaults".
    pub fn summary(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let mut push = |label: &str, value: Option<String>| {
            if let Some(value) = value {
                rows.push((label.to_string(), value));
            }
        };

        push(
            "Connect timeout",
            self.connect_timeout.map(|v| format!("{v} s")),
        );
        push("TLS timeout", self.tls_timeout.map(|v| format!("{v} s")));
        push(
            "TCP keep-alive",
            self.tcp_keep_alive.map(|v| format!("{v} s")),
        );
        push(
            "Keep-alive connections",
            self.keep_alive_connections.map(|v| v.to_string()),
        );
        push(
            "Keep-alive timeout",
            self.keep_alive_timeout.map(|v| format!("{v} s")),
        );
        push("HTTP Host header", self.http_host_header.clone());
        push("Origin server name", self.origin_server_name.clone());
        push("CA pool", self.ca_pool.clone());
        push("Skip TLS verification", flag(self.no_tls_verify));
        push("HTTP/2 to origin", flag(self.http2_origin));
        push(
            "Disable chunked encoding",
            flag(self.disable_chunked_encoding),
        );
        push("No Happy Eyeballs", flag(self.no_happy_eyeballs));
        push("Match SNI to Host", flag(self.match_sni_to_host));
        push("Bastion mode", flag(self.bastion_mode));
        push("Proxy address", self.proxy_address.clone());
        push("Proxy port", self.proxy_port.map(|v| v.to_string()));
        push("Proxy type", self.proxy_type.clone());

        for (key, value) in &self.extra {
            rows.push((key.clone(), compact_value(value)));
        }

        rows
    }
}

/// How an ingress edit should handle the rule's `originRequest`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum OriginRequestUpdate {
    /// Leave an existing rule unchanged. A new rule gets no `originRequest`.
    #[default]
    Preserve,
    /// Remove the rule's `originRequest` and use cloudflared's defaults.
    Clear,
    /// Replace the rule's `originRequest` with this value.
    Set(Box<OriginRequest>),
}

fn flag(value: Option<bool>) -> Option<String> {
    value.map(|value| if value { "Yes".into() } else { "No".into() })
}

fn compact_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "—".into(),
        other => other.to_string(),
    }
}

/// `warp-routing`, preserved but not edited here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WarpRouting {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One rule of the ingress list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub service: String,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    pub origin_request: Option<OriginRequest>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl IngressRule {
    /// Whether this rule is a hostname the user has disabled: its service was
    /// rewritten to `http_status:404` and the real origin is remembered
    /// locally. The catch-all rule (no hostname) is never "disabled" — it is
    /// just the catch-all.
    pub fn is_disabled(&self) -> bool {
        self.hostname.is_some() && self.service == DISABLED_SERVICE
    }

    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            ..Self::default()
        }
    }

    pub fn for_hostname(hostname: impl Into<String>, service: impl Into<String>) -> Self {
        Self {
            hostname: Some(hostname.into()),
            service: service.into(),
            ..Self::default()
        }
    }

    /// The rule cloudflared requires at the end of every ingress list.
    pub fn catch_all() -> Self {
        Self::new(CATCH_ALL_SERVICE)
    }

    /// Whether this rule matches everything, which is what makes it a valid
    /// last rule and an invalid anything-else.
    pub fn is_catch_all(&self) -> bool {
        self.hostname.as_deref().unwrap_or_default().is_empty()
            && self.path.as_deref().unwrap_or_default().is_empty()
    }

    /// The key an edit addresses a rule by. Hostnames repeat with different
    /// paths, so the path is part of the identity.
    pub fn key(&self) -> (String, Option<String>) {
        (
            self.hostname
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase(),
            self.path.clone().filter(|path| !path.is_empty()),
        )
    }
}

/// A tunnel's whole remote configuration document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TunnelConfig {
    #[serde(default)]
    pub ingress: Vec<IngressRule>,
    #[serde(rename = "warp-routing", skip_serializing_if = "Option::is_none")]
    pub warp_routing: Option<WarpRouting>,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    pub origin_request: Option<OriginRequest>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What [`TunnelConfig::upsert`] did, so the caller can word the toast
/// honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Added,
    Updated,
}

impl TunnelConfig {
    /// The rules that publish a hostname, with their index in `ingress` — the
    /// index is what an edit or a removal later addresses.
    pub fn hostname_rules(&self) -> impl Iterator<Item = (usize, &IngressRule)> {
        self.ingress
            .iter()
            .enumerate()
            .filter(|(_, rule)| !rule.is_catch_all())
    }

    pub fn hostname_count(&self) -> usize {
        self.hostname_rules().count()
    }

    /// Turns the rules into the model the UI shows, leaving the DNS state
    /// unknown — filling that in needs a zone read the caller may not be able
    /// to make.
    pub fn public_hostnames(
        &self,
        tunnel_id: &str,
        tunnel_name: &str,
        dns_reason: &str,
    ) -> Vec<PublicHostname> {
        self.hostname_rules()
            .map(|(index, rule)| {
                let raw = rule.service.clone();
                let hostname = rule.hostname.clone().unwrap_or_default();
                PublicHostname {
                    service: OriginService::parse(&raw).unwrap_or(OriginService {
                        scheme: crate::domain::hostname::OriginScheme::Http,
                        host: raw.clone(),
                        port: None,
                    }),
                    raw_service: raw,
                    path: rule.path.clone().filter(|path| !path.is_empty()),
                    hostname,
                    tunnel_id: tunnel_id.to_string(),
                    tunnel_name: tunnel_name.to_string(),
                    zone_id: None,
                    zone_name: None,
                    dns: crate::domain::hostname::DnsState::Unknown {
                        reason: dns_reason.to_string(),
                    },
                    origin_request: rule.origin_request.clone(),
                    rule_index: index,
                    disabled: rule.is_disabled(),
                }
            })
            .collect()
    }

    /// Finds a rule by hostname and path.
    pub fn find(&self, hostname: &str, path: Option<&str>) -> Option<usize> {
        let needle = (
            hostname.to_ascii_lowercase(),
            path.map(str::to_string).filter(|path| !path.is_empty()),
        );
        self.ingress
            .iter()
            .position(|rule| !rule.is_catch_all() && rule.key() == needle)
    }

    /// Adds a hostname rule, or updates the service of the one already there.
    ///
    /// An update preserves, clears or replaces the existing rule's
    /// `originRequest` according to `origin_request`. Unknown rule-level keys
    /// are always retained.
    ///
    /// The new rule is inserted *before* the catch-all, never after it —
    /// cloudflared stops at the first matching rule, so a hostname added after
    /// a catch-all would never be reached.
    pub fn upsert(
        &mut self,
        hostname: &str,
        path: Option<&str>,
        service: &str,
        origin_request: OriginRequestUpdate,
    ) -> UpsertOutcome {
        let path = path.map(str::to_string).filter(|path| !path.is_empty());

        if let Some(index) = self.find(hostname, path.as_deref()) {
            let rule = &mut self.ingress[index];
            rule.service = service.to_string();
            match origin_request {
                OriginRequestUpdate::Preserve => {}
                OriginRequestUpdate::Clear => rule.origin_request = None,
                OriginRequestUpdate::Set(origin_request) => {
                    rule.origin_request = Some(*origin_request);
                }
            }
            self.normalise();
            return UpsertOutcome::Updated;
        }

        let rule = IngressRule {
            hostname: Some(hostname.to_ascii_lowercase()),
            path,
            service: service.to_string(),
            origin_request: match origin_request {
                OriginRequestUpdate::Preserve | OriginRequestUpdate::Clear => None,
                OriginRequestUpdate::Set(origin_request) => Some(*origin_request),
            },
            extra: Map::new(),
        };

        let insert_at = self
            .ingress
            .iter()
            .position(IngressRule::is_catch_all)
            .unwrap_or(self.ingress.len());
        self.ingress.insert(insert_at, rule);

        self.normalise();
        UpsertOutcome::Added
    }

    /// Removes exactly one rule and nothing else.
    ///
    /// Returns the rule that was removed so the caller can offer to put it
    /// back if the DNS half of the operation then fails.
    pub fn remove(&mut self, hostname: &str, path: Option<&str>) -> Option<IngressRule> {
        let index = self.find(hostname, path)?;
        let removed = self.ingress.remove(index);
        self.normalise();
        Some(removed)
    }

    /// Removes the rule at a known index, checking it is the one expected.
    ///
    /// The index comes from a list the user was looking at, which may be stale
    /// by the time they click; the hostname check is what stops a stale index
    /// from unpublishing the wrong thing.
    pub fn remove_at(&mut self, index: usize, expect_hostname: &str) -> Option<IngressRule> {
        let rule = self.ingress.get(index)?;
        if rule.is_catch_all()
            || !rule
                .hostname
                .as_deref()
                .unwrap_or_default()
                .eq_ignore_ascii_case(expect_hostname)
        {
            return None;
        }
        let removed = self.ingress.remove(index);
        self.normalise();
        Some(removed)
    }

    /// Puts a rule back at a given index, for rolling back a half-completed
    /// unpublish.
    pub fn insert_at(&mut self, index: usize, rule: IngressRule) {
        let index = index.min(self.ingress.len());
        self.ingress.insert(index, rule);
        self.normalise();
    }

    /// Makes the ingress list something cloudflared will accept.
    ///
    /// Two invariants, in this order:
    ///
    /// 1. No catch-all rule may appear before the end — everything after one
    ///    is unreachable, and a configuration that silently stops serving half
    ///    its hostnames is worse than one that is rejected. Any catch-all
    ///    found early is lifted out.
    /// 2. The list must end with a catch-all. If the document had one, the
    ///    last one found is reused so a user's chosen fallback service (a
    ///    maintenance page, say) is not replaced with a 404.
    pub fn normalise(&mut self) {
        let mut fallback: Option<IngressRule> = None;
        let mut kept: Vec<IngressRule> = Vec::with_capacity(self.ingress.len());

        for rule in self.ingress.drain(..) {
            if rule.is_catch_all() {
                fallback = Some(rule);
            } else {
                kept.push(rule);
            }
        }

        kept.push(fallback.unwrap_or_else(IngressRule::catch_all));
        self.ingress = kept;
    }

    /// Checks the document before it is written.
    pub fn validate(&self) -> ValidationResult {
        let Some(last) = self.ingress.last() else {
            return Err(ValidationError::new(
                "An ingress list cannot be empty — it must end with a catch-all rule.",
            ));
        };
        if !last.is_catch_all() {
            return Err(ValidationError::new(
                "The last ingress rule must match every request. Add a rule with no \
                 hostname, normally http_status:404.",
            ));
        }
        if self
            .ingress
            .iter()
            .rev()
            .skip(1)
            .any(IngressRule::is_catch_all)
        {
            return Err(ValidationError::new(
                "A catch-all rule appears before the end of the list, so the rules after \
                 it would never be reached.",
            ));
        }
        for rule in &self.ingress {
            if rule.service.trim().is_empty() {
                return Err(ValidationError::new("Every ingress rule needs a service."));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> TunnelConfig {
        serde_json::from_str(json).expect("configuration should parse")
    }

    #[test]
    fn parses_a_realistic_configuration() {
        let config = parse(
            r#"{
              "ingress": [
                {
                  "hostname": "app.example.com",
                  "service": "http://localhost:8080",
                  "originRequest": { "connectTimeout": 30, "noTLSVerify": true }
                },
                { "service": "http_status:404" }
              ],
              "warp-routing": { "enabled": true }
            }"#,
        );

        assert_eq!(config.ingress.len(), 2);
        assert_eq!(config.hostname_count(), 1);
        assert_eq!(
            config.ingress[0]
                .origin_request
                .as_ref()
                .unwrap()
                .connect_timeout,
            Some(30)
        );
        assert_eq!(config.warp_routing.as_ref().unwrap().enabled, Some(true));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn preserves_unknown_keys_through_a_round_trip() {
        let original = r#"{
          "ingress": [
            {
              "hostname": "app.example.com",
              "service": "http://localhost:8080",
              "somethingNew": { "nested": [1, 2, 3] },
              "originRequest": { "caPool": "/etc/ca.pem", "futureField": "keep me" }
            },
            { "service": "http_status:404" }
          ],
          "warp-routing": { "enabled": false, "futureFlag": true },
          "topLevelFuture": "also kept"
        }"#;

        let config = parse(original);
        let written = serde_json::to_value(&config).unwrap();

        assert_eq!(written["topLevelFuture"], "also kept");
        assert_eq!(written["warp-routing"]["futureFlag"], true);
        assert_eq!(written["ingress"][0]["somethingNew"]["nested"][2], 3);
        assert_eq!(
            written["ingress"][0]["originRequest"]["futureField"],
            "keep me"
        );
        assert_eq!(
            written["ingress"][0]["originRequest"]["caPool"],
            "/etc/ca.pem"
        );
    }

    #[test]
    fn an_edit_preserves_every_other_rule_and_its_origin_request() {
        let mut config = parse(
            r#"{
              "ingress": [
                { "hostname": "a.example.com", "service": "http://localhost:1",
                  "originRequest": { "connectTimeout": 11 } },
                { "hostname": "b.example.com", "service": "http://localhost:2",
                  "originRequest": { "connectTimeout": 22, "unknown": "x" } },
                { "service": "http_status:404" }
              ]
            }"#,
        );

        let outcome = config.upsert(
            "b.example.com",
            None,
            "http://localhost:9999",
            OriginRequestUpdate::Preserve,
        );
        assert_eq!(outcome, UpsertOutcome::Updated);

        assert_eq!(config.ingress.len(), 3);
        assert_eq!(config.ingress[0].service, "http://localhost:1");
        assert_eq!(
            config.ingress[0]
                .origin_request
                .as_ref()
                .unwrap()
                .connect_timeout,
            Some(11)
        );
        assert_eq!(config.ingress[1].service, "http://localhost:9999");
        // The edited rule keeps the origin behaviour it already had.
        let edited = config.ingress[1].origin_request.as_ref().unwrap();
        assert_eq!(edited.connect_timeout, Some(22));
        assert_eq!(edited.extra.get("unknown").unwrap(), "x");
    }

    #[test]
    fn an_edit_can_clear_its_origin_request() {
        let mut config = parse(
            r#"{
              "ingress": [
                { "hostname": "app.example.com", "service": "https://localhost:443",
                  "originRequest": { "noTLSVerify": true } },
                { "service": "http_status:404" }
              ]
            }"#,
        );

        let outcome = config.upsert(
            "app.example.com",
            None,
            "https://localhost:443",
            OriginRequestUpdate::Clear,
        );

        assert_eq!(outcome, UpsertOutcome::Updated);
        assert_eq!(config.ingress[0].origin_request, None);
        let written = serde_json::to_value(config).unwrap();
        assert!(written["ingress"][0].get("originRequest").is_none());
    }

    #[test]
    fn a_new_hostname_carries_its_origin_request_and_keeps_it_visible() {
        let mut config = TunnelConfig::default();
        let origin = OriginRequest {
            http_host_header: Some("internal.example.test".into()),
            no_tls_verify: Some(true),
            ..OriginRequest::default()
        };

        assert_eq!(
            config.upsert(
                "app.example.com",
                None,
                "https://localhost:8443",
                OriginRequestUpdate::Set(Box::new(origin.clone()))
            ),
            UpsertOutcome::Added
        );

        let hostnames = config.public_hostnames("tid", "My tunnel", "not checked");
        assert_eq!(hostnames.len(), 1);
        assert_eq!(hostnames[0].origin_request, Some(origin));

        // And a rule without one reports none, meaning cloudflared defaults.
        let mut plain = TunnelConfig::default();
        plain.upsert(
            "plain.example.com",
            None,
            "http://localhost:80",
            OriginRequestUpdate::Preserve,
        );
        let hostnames = plain.public_hostnames("tid", "My tunnel", "not checked");
        assert_eq!(hostnames[0].origin_request, None);
    }

    #[test]
    fn adding_a_hostname_inserts_it_before_the_catch_all() {
        let mut config = parse(
            r#"{"ingress":[{"hostname":"a.example.com","service":"http://localhost:1"},
                           {"service":"http_status:404"}]}"#,
        );

        assert_eq!(
            config.upsert(
                "new.example.com",
                None,
                "https://localhost:8443",
                OriginRequestUpdate::Preserve,
            ),
            UpsertOutcome::Added
        );

        assert_eq!(config.ingress.len(), 3);
        assert_eq!(
            config.ingress[1].hostname.as_deref(),
            Some("new.example.com")
        );
        assert!(config.ingress[2].is_catch_all());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn adding_to_an_empty_configuration_creates_the_catch_all() {
        let mut config = TunnelConfig::default();
        config.upsert(
            "app.example.com",
            None,
            "http://localhost:8080",
            OriginRequestUpdate::Preserve,
        );

        assert_eq!(config.ingress.len(), 2);
        assert_eq!(config.ingress[1].service, CATCH_ALL_SERVICE);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn normalise_keeps_a_custom_fallback_service() {
        let mut config = parse(
            r#"{"ingress":[{"hostname":"a.example.com","service":"http://localhost:1"},
                           {"service":"http://localhost:9/maintenance"}]}"#,
        );
        config.normalise();

        assert_eq!(
            config.ingress.last().unwrap().service,
            "http://localhost:9/maintenance"
        );
        assert!(config.validate().is_ok());
    }

    #[test]
    fn normalise_lifts_a_misplaced_catch_all_to_the_end() {
        let mut config = parse(
            r#"{"ingress":[{"service":"http_status:503"},
                           {"hostname":"a.example.com","service":"http://localhost:1"}]}"#,
        );
        config.normalise();

        assert_eq!(config.ingress.len(), 2);
        assert_eq!(config.ingress[0].hostname.as_deref(), Some("a.example.com"));
        assert_eq!(config.ingress[1].service, "http_status:503");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn hostname_and_path_together_identify_a_rule() {
        let mut config = parse(
            r#"{"ingress":[
                 {"hostname":"a.example.com","path":"/api","service":"http://localhost:1"},
                 {"hostname":"a.example.com","service":"http://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        );

        assert_eq!(
            config.upsert(
                "a.example.com",
                Some("/api"),
                "http://localhost:3",
                OriginRequestUpdate::Preserve,
            ),
            UpsertOutcome::Updated
        );
        assert_eq!(config.ingress[0].service, "http://localhost:3");
        assert_eq!(config.ingress[1].service, "http://localhost:2");
    }

    #[test]
    fn removing_takes_exactly_one_rule() {
        let mut config = parse(
            r#"{"ingress":[
                 {"hostname":"a.example.com","service":"http://localhost:1"},
                 {"hostname":"b.example.com","service":"http://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        );

        let removed = config.remove("a.example.com", None).unwrap();
        assert_eq!(removed.service, "http://localhost:1");
        assert_eq!(config.hostname_count(), 1);
        assert_eq!(config.ingress[0].hostname.as_deref(), Some("b.example.com"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn removing_the_last_hostname_leaves_a_valid_catch_all_only_list() {
        let mut config = parse(
            r#"{"ingress":[{"hostname":"a.example.com","service":"http://localhost:1"},
                           {"service":"http_status:404"}]}"#,
        );

        assert!(config.remove("a.example.com", None).is_some());
        assert_eq!(config.ingress.len(), 1);
        assert!(config.ingress[0].is_catch_all());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn remove_at_refuses_a_stale_index() {
        let mut config = parse(
            r#"{"ingress":[
                 {"hostname":"a.example.com","service":"http://localhost:1"},
                 {"hostname":"b.example.com","service":"http://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        );

        // The user was looking at index 0 being a.example.com; the list has
        // since changed underneath them.
        assert!(config.remove_at(0, "b.example.com").is_none());
        assert_eq!(config.hostname_count(), 2, "nothing removed");
        assert!(config.remove_at(1, "b.example.com").is_some());
    }

    #[test]
    fn remove_at_never_removes_the_catch_all() {
        let mut config = parse(r#"{"ingress":[{"service":"http_status:404"}]}"#);
        assert!(config.remove_at(0, "").is_none());
        assert_eq!(config.ingress.len(), 1);
    }

    #[test]
    fn a_removed_rule_can_be_put_back_exactly() {
        let before = parse(
            r#"{"ingress":[
                 {"hostname":"a.example.com","service":"http://localhost:1",
                  "originRequest":{"connectTimeout":7}},
                 {"hostname":"b.example.com","service":"http://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        );
        let mut after = before.clone();

        let removed = after.remove("a.example.com", None).unwrap();
        after.insert_at(0, removed);

        assert_eq!(after, before, "rollback must restore the document exactly");
    }

    #[test]
    fn validate_rejects_a_list_that_does_not_end_with_a_catch_all() {
        let config =
            parse(r#"{"ingress":[{"hostname":"a.example.com","service":"http://localhost:1"}]}"#);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_an_empty_list() {
        assert!(TunnelConfig::default().validate().is_err());
    }

    #[test]
    fn validate_rejects_an_early_catch_all() {
        let config = TunnelConfig {
            ingress: vec![
                IngressRule::catch_all(),
                IngressRule::for_hostname("a.example.com", "http://localhost:1"),
                IngressRule::catch_all(),
            ],
            ..TunnelConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn public_hostnames_carry_the_rule_index() {
        let config = parse(
            r#"{"ingress":[
                 {"hostname":"a.example.com","service":"http://localhost:1"},
                 {"hostname":"b.example.com","path":"/api","service":"tcp://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        );

        let hostnames = config.public_hostnames("tid", "My tunnel", "not checked");
        assert_eq!(hostnames.len(), 2);
        assert_eq!(hostnames[0].rule_index, 0);
        assert_eq!(hostnames[1].rule_index, 1);
        assert_eq!(hostnames[1].display_name(), "b.example.com/api");
        assert_eq!(
            hostnames[1].service.scheme,
            crate::domain::hostname::OriginScheme::Tcp
        );
    }

    #[test]
    fn an_unparseable_service_is_still_shown_verbatim() {
        let config = parse(
            r#"{"ingress":[{"hostname":"a.example.com","service":"weird://thing"},
                           {"service":"http_status:404"}]}"#,
        );
        let hostnames = config.public_hostnames("tid", "t", "");
        assert_eq!(hostnames[0].raw_service, "weird://thing");
    }

    #[test]
    fn serialising_omits_absent_optional_fields() {
        let mut config = TunnelConfig::default();
        config.upsert(
            "a.example.com",
            None,
            "http://localhost:1",
            OriginRequestUpdate::Preserve,
        );

        let json = serde_json::to_value(&config).unwrap();
        let rule = &json["ingress"][0];
        assert!(
            rule.get("path").is_none(),
            "an absent path must not be written as null"
        );
        assert!(rule.get("originRequest").is_none());
        assert!(json.get("warp-routing").is_none());
        // The catch-all must not carry a hostname key at all.
        assert!(json["ingress"][1].get("hostname").is_none());
    }
}
