//! The wire format, and the conversions into [`crate::domain`].
//!
//! These types exist so that a change in Cloudflare's JSON lands here and
//! nowhere else. Three habits keep them robust, all learned from what the live
//! API actually does rather than from what its schema says:
//!
//! * **Nothing is `deny_unknown_fields` and nearly everything is `Option`.**
//!   Cloudflare returns fields it never declared (`token`, `credentials_file`,
//!   `is_pending_reconnect`, `type` inside `messages`) and omits fields it
//!   declared as required.
//! * **Every enum has a catch-all.** A new tunnel status shipped on a Tuesday
//!   must render as itself, not fail the whole list.
//! * **Timeouts in `originRequest` are seconds.** cloudflared's own source
//!   still carries a comment claiming nanoseconds; its marshaller disagrees,
//!   and the documented defaults (30s / 10s / 90s) match the integers the API
//!   returns.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::hostname::DnsRecord as DomainDnsRecord;
use crate::domain::tunnel::{
    ConfigSource, Connection, Connector, Tunnel, TunnelKind, TunnelStatus,
};
use crate::domain::zone::{Zone, ZoneStatus};
use crate::domain::Account;

// ── Token verification ───────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct TokenStatusDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_on: Option<DateTime<Utc>>,
}

/// What `/user/tokens/verify` established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenStatus {
    pub id: Option<String>,
    /// `active`, `disabled` or `expired`.
    pub status: String,
    pub expires_on: Option<DateTime<Utc>>,
}

impl TokenStatus {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    pub fn caveat(&self) -> Option<String> {
        match self.status.as_str() {
            "active" => match self.expires_on {
                Some(expiry) if expiry - Utc::now() < chrono::TimeDelta::days(14) => Some(format!(
                    "This token expires on {}.",
                    expiry.format("%-d %B %Y")
                )),
                _ => None,
            },
            "disabled" => Some("This token has been disabled in the Cloudflare dashboard.".into()),
            "expired" => Some("This token has expired.".into()),
            other => Some(format!("Cloudflare reports this token as “{other}”.")),
        }
    }
}

impl From<TokenStatusDto> for TokenStatus {
    fn from(dto: TokenStatusDto) -> Self {
        Self {
            id: dto.id,
            status: dto.status.unwrap_or_else(|| "unknown".into()),
            expires_on: dto.expires_on,
        }
    }
}

// ── Accounts ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct AccountDto {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

impl From<AccountDto> for Account {
    fn from(dto: AccountDto) -> Self {
        Self {
            name: dto.name.unwrap_or_else(|| dto.id.clone()),
            id: dto.id,
        }
    }
}

// ── Zones ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ZoneDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub paused: Option<bool>,
    #[serde(default)]
    pub account: Option<ZoneAccountDto>,
    #[serde(default)]
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZoneAccountDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl From<ZoneDto> for Zone {
    fn from(dto: ZoneDto) -> Self {
        let account = dto.account.unwrap_or(ZoneAccountDto {
            id: None,
            name: None,
        });
        Self {
            id: dto.id,
            name: dto.name,
            status: ZoneStatus::from_api(dto.status.as_deref().unwrap_or("")),
            paused: dto.paused.unwrap_or(false),
            account_id: account.id.unwrap_or_default(),
            account_name: account.name.unwrap_or_default(),
            permissions: dto.permissions,
        }
    }
}

// ── Tunnels ──────────────────────────────────────────────────────────────

#[derive(Clone, Deserialize)]
pub struct TunnelDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub account_tag: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conns_active_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub conns_inactive_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub tun_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub remote_config: Option<bool>,
    /// Absent from the create response even when it was sent, so
    /// `remote_config` is the fallback signal.
    #[serde(default)]
    pub config_src: Option<String>,
    #[serde(default)]
    pub connections: Vec<ConnectionDto>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,

    /// Undeclared in Cloudflare's schema, returned by `POST /cfd_tunnel` and by
    /// a `PATCH` that rotated `tunnel_secret`. Handled as a secret: it is
    /// pulled out immediately and never stored on the domain type.
    #[serde(default)]
    pub token: Option<String>,
    /// Also undeclared, also create-only, also a secret.
    #[serde(default)]
    pub credentials_file: Option<serde_json::Value>,
}

impl std::fmt::Debug for TunnelDto {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelDto")
            .field("id", &self.id)
            .field("account_tag", &self.account_tag)
            .field("name", &self.name)
            .field("created_at", &self.created_at)
            .field("deleted_at", &self.deleted_at)
            .field("conns_active_at", &self.conns_active_at)
            .field("conns_inactive_at", &self.conns_inactive_at)
            .field("tun_type", &self.tun_type)
            .field("status", &self.status)
            .field("remote_config", &self.remote_config)
            .field("config_src", &self.config_src)
            .field("connections", &self.connections)
            .field("metadata", &self.metadata)
            .field(
                "token",
                &self
                    .token
                    .as_ref()
                    .map(|_| crate::domain::activity::REDACTED),
            )
            .field(
                "credentials_file",
                &self
                    .credentials_file
                    .as_ref()
                    .map(|_| crate::domain::activity::REDACTED),
            )
            .finish()
    }
}

impl TunnelDto {
    /// Whether this is a cloudflared tunnel rather than a WARP connector or a
    /// Magic WAN tunnel sharing the endpoint.
    pub fn is_cloudflared(&self) -> bool {
        matches!(self.tun_type.as_deref(), None | Some("cfd_tunnel"))
    }

    pub fn into_domain(self) -> Tunnel {
        // `config_src` is missing from the create response, so fall back to
        // `remote_config`, which is always present and means the same thing.
        let config_source = match self.config_src.as_deref() {
            Some(value) => ConfigSource::from_api(value),
            None => match self.remote_config {
                Some(true) => ConfigSource::Cloudflare,
                Some(false) => ConfigSource::Local,
                None => ConfigSource::Local,
            },
        };

        let connector_count = self
            .connections
            .iter()
            .filter_map(|connection| connection.client_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            .max(usize::from(!self.connections.is_empty()));

        Tunnel {
            id: self.id.unwrap_or_default(),
            account_tag: self.account_tag.unwrap_or_default(),
            name: self.name.unwrap_or_default(),
            status: TunnelStatus::from_api(self.status.as_deref().unwrap_or("inactive")),
            config_source,
            kind: TunnelKind::from_api(self.tun_type.as_deref().unwrap_or("cfd_tunnel")),
            remote_config: self.remote_config.unwrap_or(false),
            created_at: self.created_at,
            deleted_at: self.deleted_at,
            conns_active_at: self.conns_active_at,
            conns_inactive_at: self.conns_inactive_at,
            connector_count,
            hostname_count: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectionDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub colo_name: Option<String>,
    /// Undeclared in the schema, present in every live response.
    #[serde(default)]
    pub is_pending_reconnect: Option<bool>,
    #[serde(default)]
    pub origin_ip: Option<String>,
    #[serde(default)]
    pub opened_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
}

impl From<ConnectionDto> for Connection {
    fn from(dto: ConnectionDto) -> Self {
        Self {
            id: dto.id.or(dto.uuid).unwrap_or_default(),
            colo_name: dto.colo_name.unwrap_or_default(),
            is_pending_reconnect: dto.is_pending_reconnect.unwrap_or(false),
            origin_ip: dto.origin_ip,
            opened_at: dto.opened_at,
            client_id: dto.client_id,
            client_version: dto.client_version,
        }
    }
}

/// One entry of `GET .../connections` — a `cloudflared` process, with the
/// edge connections it holds open.
#[derive(Debug, Clone, Deserialize)]
pub struct ConnectorDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    #[serde(default)]
    pub run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub config_version: Option<i64>,
    #[serde(default)]
    pub conns: Vec<ConnectionDto>,
}

impl From<ConnectorDto> for Connector {
    fn from(dto: ConnectorDto) -> Self {
        Self {
            id: dto.id.unwrap_or_default(),
            version: dto.version.unwrap_or_default(),
            arch: dto.arch.unwrap_or_default(),
            run_at: dto.run_at,
            features: dto.features,
            config_version: dto.config_version,
            connections: dto.conns.into_iter().map(Connection::from).collect(),
        }
    }
}

/// The body of `POST /accounts/{id}/cfd_tunnel`.
///
/// `config_src` is not optional in practice even though the API treats it so:
/// its default is `"local"`, and a locally configured tunnel silently ignores
/// everything this application writes. It is therefore always sent.
#[derive(Debug, Clone, Serialize)]
pub struct CreateTunnelRequest {
    pub name: String,
    pub config_src: &'static str,
}

impl CreateTunnelRequest {
    pub fn remotely_managed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config_src: "cloudflare",
        }
    }
}

/// The body of `PATCH /accounts/{id}/cfd_tunnel/{id}`. Only these two fields
/// can be changed.
#[derive(Debug, Clone, Serialize)]
pub struct PatchTunnelRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Base64, at least 32 bytes. Setting it rotates the run token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_secret: Option<String>,
}

impl PatchTunnelRequest {
    pub fn rename(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            tunnel_secret: None,
        }
    }
}

// ── Tunnel configuration ─────────────────────────────────────────────────

/// The result of `GET`/`PUT .../configurations`.
#[derive(Debug, Clone, Deserialize)]
pub struct TunnelConfigurationDto {
    #[serde(default)]
    pub tunnel_id: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    /// Increments on each successful `PUT`. Used to tell whether connectors
    /// have picked the new configuration up yet.
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// Mirrors the tunnel's `config_src`.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub config: Option<crate::domain::config::TunnelConfig>,
}

/// The body of `PUT .../configurations` — one top-level `config` key.
#[derive(Debug, Clone, Serialize)]
pub struct PutConfigurationRequest<'a> {
    pub config: &'a crate::domain::config::TunnelConfig,
}

// ── DNS ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct DnsRecordDto {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub zone_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "type", default)]
    pub record_type: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub proxied: Option<bool>,
    #[serde(default)]
    pub ttl: Option<u32>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl DnsRecordDto {
    /// `None` when the record is missing the fields that make it actionable
    /// (id, type, content). Such a record can never be repaired or deleted
    /// safely, so callers skip it rather than treating it as `Missing` and
    /// writing over it.
    pub fn try_into_domain(self, fallback_zone: &str) -> Option<DomainDnsRecord> {
        if self.id.as_deref().unwrap_or("").is_empty()
            || self.record_type.as_deref().unwrap_or("").is_empty()
            || self.content.as_deref().unwrap_or("").is_empty()
        {
            return None;
        }
        Some(self.into_domain(fallback_zone))
    }

    pub fn into_domain(self, fallback_zone: &str) -> DomainDnsRecord {
        DomainDnsRecord {
            id: self.id.unwrap_or_default(),
            zone_id: self.zone_id.unwrap_or_else(|| fallback_zone.to_string()),
            name: self.name.unwrap_or_default(),
            record_type: self.record_type.unwrap_or_default(),
            content: self.content.unwrap_or_default(),
            proxied: self.proxied.unwrap_or(false),
            ttl: self.ttl.unwrap_or(1),
            comment: self.comment,
        }
    }
}

/// TTL `1` is Cloudflare's sentinel for "automatic", which is what a proxied
/// record gets regardless.
pub const TTL_AUTOMATIC: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct CreateDnsRecordRequest {
    #[serde(rename = "type")]
    pub record_type: &'static str,
    pub name: String,
    pub content: String,
    pub proxied: bool,
    pub ttl: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl CreateDnsRecordRequest {
    /// The proxied CNAME a public hostname needs.
    ///
    /// `proxied: true` is not a preference: `cfargotunnel.com` does not
    /// resolve publicly, so a grey-clouded record simply fails for visitors.
    pub fn tunnel_cname(hostname: &str, tunnel_id: &str, comment: Option<String>) -> Self {
        Self {
            record_type: "CNAME",
            name: hostname.to_string(),
            content: format!("{tunnel_id}.cfargotunnel.com"),
            proxied: true,
            ttl: TTL_AUTOMATIC,
            comment,
        }
    }
}

/// The body of `PATCH /zones/{id}/dns_records/{id}`.
///
/// `PATCH`, never `PUT`: `PUT` resets every field the body omits, so repairing
/// a record's `proxied` flag with `PUT` would silently wipe its comment, tags
/// and TTL.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PatchDnsRecordRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxied: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_tunnel_object() {
        let dto: TunnelDto = serde_json::from_str(
            r#"{
              "id": "c1744f8b-faa1-48a4-9e5c-02ac921467fa",
              "account_tag": "699d98642c564d2e855e9661899b7252",
              "created_at": "2025-02-18T22:41:43.534395Z",
              "deleted_at": null,
              "name": "example-tunnel",
              "connections": [],
              "conns_active_at": null,
              "conns_inactive_at": "2025-02-18T22:41:43.534395Z",
              "tun_type": "cfd_tunnel",
              "metadata": {},
              "status": "inactive",
              "remote_config": true
            }"#,
        )
        .unwrap();

        let tunnel = dto.into_domain();
        assert_eq!(tunnel.name, "example-tunnel");
        assert_eq!(tunnel.status, TunnelStatus::Inactive);
        assert_eq!(tunnel.config_source, ConfigSource::Cloudflare);
        assert_eq!(tunnel.kind, TunnelKind::Cfd);
        assert_eq!(tunnel.connector_count, 0);
        assert!(!tunnel.is_deleted());
    }

    #[test]
    fn a_create_response_without_config_src_is_read_from_remote_config() {
        // The create response omits config_src even when it was sent.
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"a","name":"t","status":"inactive","remote_config":true,
                "token":"eyJhIjoiNWFiNGU5Wg=="}"#,
        )
        .unwrap();
        assert!(dto.token.is_some(), "the token is captured, not dropped");
        assert_eq!(
            dto.clone().into_domain().config_source,
            ConfigSource::Cloudflare
        );

        let local: TunnelDto =
            serde_json::from_str(r#"{"id":"a","name":"t","remote_config":false}"#).unwrap();
        assert_eq!(local.into_domain().config_source, ConfigSource::Local);
    }

    #[test]
    fn config_src_wins_over_remote_config_when_both_are_present() {
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"a","name":"t","remote_config":false,"config_src":"cloudflare"}"#,
        )
        .unwrap();
        assert_eq!(dto.into_domain().config_source, ConfigSource::Cloudflare);
    }

    #[test]
    fn an_unknown_status_survives_verbatim() {
        let dto: TunnelDto =
            serde_json::from_str(r#"{"id":"a","name":"t","status":"reticulating"}"#).unwrap();
        assert_eq!(
            dto.into_domain().status,
            TunnelStatus::Other("reticulating".into())
        );
    }

    #[test]
    fn a_tunnel_with_undeclared_fields_still_parses() {
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"a","name":"t","brand_new":"value","connections":[
                 {"id":"c","colo_name":"ATH","is_pending_reconnect":false,
                  "client_id":"cid","also_new":1}]}"#,
        )
        .unwrap();
        assert_eq!(dto.connections.len(), 1);
        assert_eq!(dto.into_domain().connector_count, 1);
    }

    #[test]
    fn connector_count_is_distinct_client_ids_not_connections() {
        // Four edge connections from one connector is one machine, not four.
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"a","name":"t","connections":[
                 {"id":"1","client_id":"same"},{"id":"2","client_id":"same"},
                 {"id":"3","client_id":"same"},{"id":"4","client_id":"same"}]}"#,
        )
        .unwrap();
        assert_eq!(dto.into_domain().connector_count, 1);

        let two: TunnelDto = serde_json::from_str(
            r#"{"id":"a","name":"t","connections":[
                 {"id":"1","client_id":"a"},{"id":"2","client_id":"b"}]}"#,
        )
        .unwrap();
        assert_eq!(two.into_domain().connector_count, 2);
    }

    #[test]
    fn non_cloudflared_tunnels_are_identifiable() {
        let warp: TunnelDto =
            serde_json::from_str(r#"{"id":"a","tun_type":"warp_connector"}"#).unwrap();
        assert!(!warp.is_cloudflared());

        let cfd: TunnelDto = serde_json::from_str(r#"{"id":"a","tun_type":"cfd_tunnel"}"#).unwrap();
        assert!(cfd.is_cloudflared());

        let unset: TunnelDto = serde_json::from_str(r#"{"id":"a"}"#).unwrap();
        assert!(unset.is_cloudflared(), "assume cfd when unstated");
    }

    #[test]
    fn parses_a_connector_with_its_edge_connections() {
        let dto: ConnectorDto = serde_json::from_str(
            r#"{
              "id": "1bedc50d-42b3-473c-b108-ff3d10c0d925",
              "features": ["ha-origin"],
              "version": "2022.7.1",
              "arch": "linux_amd64",
              "run_at": "2009-11-10T23:00:00Z",
              "config_version": 3,
              "conns": [
                {"colo_name":"DFW","uuid":"u1","id":"u1","is_pending_reconnect":false,
                 "origin_ip":"10.1.0.137","opened_at":"2021-01-25T18:22:34.317854Z",
                 "client_id":"c1","client_version":"2022.7.1"}
              ]
            }"#,
        )
        .unwrap();

        let connector = Connector::from(dto);
        assert_eq!(connector.version, "2022.7.1");
        assert_eq!(connector.config_version, Some(3));
        assert_eq!(connector.established_count(), 1);
        assert_eq!(connector.colos(), vec!["DFW".to_string()]);
        assert_eq!(connector.origin_ip(), Some("10.1.0.137"));
    }

    #[test]
    fn a_connection_uses_uuid_when_id_is_absent() {
        let dto: ConnectionDto = serde_json::from_str(r#"{"uuid":"only-uuid"}"#).unwrap();
        assert_eq!(Connection::from(dto).id, "only-uuid");
    }

    #[test]
    fn create_always_asks_for_a_remotely_managed_tunnel() {
        let body =
            serde_json::to_value(CreateTunnelRequest::remotely_managed("api-tunnel")).unwrap();
        assert_eq!(body["name"], "api-tunnel");
        assert_eq!(
            body["config_src"], "cloudflare",
            "the API default is `local`, which would ignore every configuration write"
        );
        assert!(
            body.get("tunnel_secret").is_none(),
            "a remotely managed tunnel needs no secret in the body"
        );
    }

    #[test]
    fn a_rename_patch_sends_only_the_name() {
        let body = serde_json::to_value(PatchTunnelRequest::rename("new name")).unwrap();
        assert_eq!(body["name"], "new name");
        assert!(
            body.get("tunnel_secret").is_none(),
            "renaming must not rotate the run token"
        );
    }

    #[test]
    fn the_tunnel_cname_body_is_proxied_with_automatic_ttl() {
        let body = serde_json::to_value(CreateDnsRecordRequest::tunnel_cname(
            "app.example.com",
            "c1744f8b-faa1-48a4-9e5c-02ac921467fa",
            Some("Managed by Tunnel Manager".into()),
        ))
        .unwrap();

        assert_eq!(body["type"], "CNAME");
        assert_eq!(body["name"], "app.example.com");
        assert_eq!(
            body["content"],
            "c1744f8b-faa1-48a4-9e5c-02ac921467fa.cfargotunnel.com"
        );
        assert_eq!(body["proxied"], true);
        assert_eq!(body["ttl"], 1);
        assert_eq!(body["comment"], "Managed by Tunnel Manager");
    }

    #[test]
    fn a_dns_patch_sends_only_what_changed() {
        let body = serde_json::to_value(PatchDnsRecordRequest {
            proxied: Some(true),
            ..PatchDnsRecordRequest::default()
        })
        .unwrap();
        assert_eq!(body["proxied"], true);
        assert!(
            body.get("content").is_none(),
            "PATCH must not resend fields it is not changing"
        );
    }

    #[test]
    fn parses_the_configuration_envelope_result() {
        let dto: TunnelConfigurationDto = serde_json::from_str(
            r#"{
              "tunnel_id": "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415",
              "account_id": "023e105f4ecef8ad9ca31a8372d0c353",
              "version": 3,
              "created_at": "2014-01-01T05:20:00.12345Z",
              "source": "cloudflare",
              "config": {
                "ingress": [
                  {"hostname":"app.example.com","service":"http://localhost:8001","originRequest":{}},
                  {"service":"http_status:404"}
                ],
                "warp-routing": {"enabled": false},
                "originRequest": {}
              }
            }"#,
        )
        .unwrap();

        assert_eq!(dto.version, Some(3));
        assert_eq!(dto.source.as_deref(), Some("cloudflare"));
        let config = dto.config.unwrap();
        assert_eq!(config.hostname_count(), 1);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn a_tunnel_with_no_configuration_yet_parses_as_none() {
        let dto: TunnelConfigurationDto =
            serde_json::from_str(r#"{"tunnel_id":"t","version":0,"config":null}"#).unwrap();
        assert!(dto.config.is_none());
    }

    #[test]
    fn origin_request_timeouts_round_trip_as_seconds() {
        let dto: TunnelConfigurationDto = serde_json::from_str(
            r#"{"config":{"ingress":[{"hostname":"a.example.com",
                 "service":"http://localhost:1",
                 "originRequest":{"connectTimeout":30,"tlsTimeout":10,
                                  "keepAliveTimeout":90,"tcpKeepAlive":30}},
                 {"service":"http_status:404"}]}}"#,
        )
        .unwrap();

        let origin = dto.config.unwrap().ingress[0]
            .origin_request
            .clone()
            .unwrap();
        assert_eq!(origin.connect_timeout, Some(30), "seconds, not nanoseconds");
        assert_eq!(origin.keep_alive_timeout, Some(90));
    }

    #[test]
    fn token_status_flags_an_imminent_expiry() {
        let soon = TokenStatus {
            id: Some("x".into()),
            status: "active".into(),
            expires_on: Some(Utc::now() + chrono::TimeDelta::days(3)),
        };
        assert!(soon.is_active());
        assert!(soon.caveat().unwrap().contains("expires"));

        let far = TokenStatus {
            expires_on: Some(Utc::now() + chrono::TimeDelta::days(300)),
            ..soon.clone()
        };
        assert!(far.caveat().is_none());

        let expired = TokenStatus {
            status: "expired".into(),
            ..soon
        };
        assert!(!expired.is_active());
        assert!(expired.caveat().is_some());
    }

    #[test]
    fn zone_permissions_are_carried_through() {
        // A doubled hash on the raw string: the permission values themselves
        // start with `#`, which would otherwise close an `r#"…"#`.
        let dto: ZoneDto = serde_json::from_str(
            r##"{"id":"z","name":"example.com","status":"active","paused":false,
                 "account":{"id":"a","name":"Acme"},
                 "permissions":["#zone:read","#dns_records:edit"]}"##,
        )
        .unwrap();
        let zone = Zone::from(dto);
        assert_eq!(zone.account_name, "Acme");
        assert!(zone.has_dns_edit());
        assert!(zone.status.can_serve());
    }

    #[test]
    fn a_zone_without_dns_edit_is_detected() {
        let dto: ZoneDto = serde_json::from_str(
            r##"{"id":"z","name":"example.com","status":"active","permissions":["#zone:read"]}"##,
        )
        .unwrap();
        assert!(!Zone::from(dto).has_dns_edit());
    }

    #[test]
    fn a_zone_that_reports_no_permissions_is_not_assumed_to_be_denied() {
        let dto: ZoneDto =
            serde_json::from_str(r#"{"id":"z","name":"example.com","status":"active"}"#).unwrap();
        assert!(
            Zone::from(dto).has_dns_edit(),
            "an absent permission list is unknown, not a refusal"
        );
    }

    #[test]
    fn a_dns_record_falls_back_to_the_zone_it_was_fetched_from() {
        let dto: DnsRecordDto = serde_json::from_str(
            r#"{"id":"r","name":"app.example.com","type":"CNAME",
                "content":"tid.cfargotunnel.com","proxied":true,"ttl":1}"#,
        )
        .unwrap();
        let record = dto.into_domain("zone-abc");
        assert_eq!(record.zone_id, "zone-abc");
        assert!(record.is_tunnel_cname("tid"));
    }
}
