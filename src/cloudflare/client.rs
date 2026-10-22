//! The typed Cloudflare v4 client.
//!
//! Everything that talks to `api.cloudflare.com` goes through here, and
//! nothing here knows about GTK. The base URL is injectable so the test suite
//! can point the whole client at a local mock server; no test needs a real
//! token or a network.
//!
//! ## What this layer is responsible for
//!
//! * Attaching the bearer token, and never letting it into a log or an error.
//! * Turning HTTP status plus the v4 envelope into a [`CloudflareError`] that
//!   names the permission a 403 needs.
//! * Pagination, transparently, with a hard page cap so a server that always
//!   claims one more page cannot spin forever.
//! * Retrying **reads** on a transient failure, with backoff that honours
//!   `retry-after`. Writes and deletes are never retried automatically: a
//!   create that timed out may have succeeded, and the second attempt would
//!   leave two tunnels behind.
//! * Cancellation, so closing a page stops its requests rather than leaving
//!   them to land on a dead widget.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::domain::config::TunnelConfig;
use crate::domain::hostname::DnsRecord;
use crate::domain::tunnel::{Connector, Tunnel};
use crate::domain::zone::Zone;
use crate::domain::Account;

use super::envelope::{Envelope, ResultInfo};
use super::error::{Capability, CloudflareError, Operation, Result};
use super::models::*;

pub const DEFAULT_BASE_URL: &str = "https://api.cloudflare.com/client/v4";

/// Cloudflare caps `per_page` at 50 for zones and accounts and 1000 for
/// tunnels. Fifty is the safe value everywhere.
const PAGE_SIZE: u32 = 50;

/// A backstop against a pagination loop. At 50 per page this is 10 000 items,
/// far more than any account has, and it turns a server bug into a truncated
/// list rather than a hung application.
const MAX_PAGES: u32 = 200;

/// Attempts for an idempotent read, including the first.
const READ_ATTEMPTS: u32 = 3;

/// A cancellation handle.
///
/// Deliberately tiny rather than a dependency: the only thing the UI needs is
/// "stop caring about this request", and dropping the [`CancelGuard`] when a
/// page goes away is the whole protocol.
#[derive(Debug, Clone, Default)]
pub struct Cancel {
    flag: Arc<std::sync::atomic::AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(CloudflareError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Timeouts. Separated so a slow origin does not look like a dead one.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(30),
        }
    }
}

/// The client.
///
/// Cheap to clone — `reqwest::Client` is an `Arc` internally and shares its
/// connection pool, so cloning one per service is correct and pooling is kept.
#[derive(Clone)]
pub struct CloudflareClient {
    http: reqwest::Client,
    base_url: String,
    token: SecretString,
}

impl std::fmt::Debug for CloudflareClient {
    /// Hand-written so that no derive can ever put the token in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudflareClient")
            .field("base_url", &self.base_url)
            .field("token", &"[redacted]")
            .finish()
    }
}

impl CloudflareClient {
    pub fn new(token: SecretString) -> Result<Self> {
        Self::with_base_url(token, DEFAULT_BASE_URL, Timeouts::default())
    }

    pub fn with_base_url(
        token: SecretString,
        base_url: impl Into<String>,
        timeouts: Timeouts,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "TunnelManager/",
                env!("CARGO_PKG_VERSION"),
                " (+https://chrisdaggas.com)"
            ))
            .connect_timeout(timeouts.connect)
            .timeout(timeouts.request)
            .build()
            .map_err(|error| CloudflareError::Transport {
                detail: crate::domain::activity::redact(&error.to_string()),
            })?;

        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            token,
        })
    }

    // ── Request plumbing ─────────────────────────────────────────────────

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    /// One HTTP round trip, decoded into an envelope.
    async fn send<T, B>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
        operation: &Operation,
        cancel: &Cancel,
    ) -> Result<Envelope<T>>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        cancel.check()?;

        let mut request = self
            .http
            .request(method.clone(), self.url(path))
            // `bearer_auth` puts the token in a header, never in the URL, so
            // it cannot end up in a redirect, a proxy log or an error string.
            .bearer_auth(self.token.expose_secret());

        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request
            .send()
            .await
            .map_err(|error| CloudflareError::from_reqwest(error, operation.clone()))?;

        cancel.check()?;

        let status = response.status();
        let headers = response.headers().clone();
        let text = response
            .text()
            .await
            .map_err(|error| CloudflareError::from_reqwest(error, operation.clone()))?;

        // Parse first: Cloudflare puts a useful message in the body of a 4xx,
        // and discarding it in favour of the status code loses the only part
        // the user can act on.
        let envelope: Envelope<T> = match serde_json::from_str(&text) {
            Ok(envelope) => envelope,
            Err(error) => {
                // A non-2xx that is not JSON at all — an edge error page, a
                // captive portal — is reported as the status, not as a decode
                // bug in this application.
                if !status.is_success() {
                    return Err(Self::status_error(status, &headers, vec![], operation));
                }
                return Err(CloudflareError::Decode {
                    detail: crate::domain::activity::redact(&error.to_string()),
                    operation: operation.clone(),
                });
            }
        };

        if !status.is_success() {
            return Err(Self::status_error(
                status,
                &headers,
                envelope.errors,
                operation,
            ));
        }
        if !envelope.is_success() {
            return Err(CloudflareError::Api {
                status: status.as_u16(),
                errors: envelope.errors,
                operation: operation.clone(),
            });
        }

        Ok(envelope)
    }

    /// Maps an HTTP status onto the error the UI branches on.
    ///
    /// The status is the authority, not the body's `code`. Cloudflare reuses
    /// code `10000` for "authentication error" *and* for "that field is
    /// required", so branching on it would tell a user to check their token
    /// when they had actually mistyped a hostname.
    fn status_error(
        status: StatusCode,
        headers: &HeaderMap,
        errors: Vec<super::envelope::ApiMessage>,
        operation: &Operation,
    ) -> CloudflareError {
        match status {
            StatusCode::UNAUTHORIZED => CloudflareError::Unauthorized {
                errors,
                operation: operation.clone(),
            },
            StatusCode::FORBIDDEN => CloudflareError::Forbidden {
                errors,
                operation: operation.clone(),
            },
            StatusCode::NOT_FOUND => CloudflareError::NotFound {
                errors,
                operation: operation.clone(),
            },
            StatusCode::TOO_MANY_REQUESTS => CloudflareError::RateLimited {
                retry_after: parse_retry_after(headers),
                operation: operation.clone(),
            },
            other => CloudflareError::Api {
                status: other.as_u16(),
                errors,
                operation: operation.clone(),
            },
        }
    }

    /// A read, retried on a transient failure.
    ///
    /// Only ever used for `GET`. The backoff is `retry-after` when Cloudflare
    /// sent one and 500ms, then 1s otherwise — short enough that a user
    /// watching a spinner does not give up, long enough to clear a blip.
    async fn get<T>(
        &self,
        path: &str,
        query: &[(&str, String)],
        operation: &Operation,
        cancel: &Cancel,
    ) -> Result<Envelope<T>>
    where
        T: DeserializeOwned,
    {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let outcome = self
                .send::<T, ()>(Method::GET, path, query, None, operation, cancel)
                .await;

            match outcome {
                Ok(envelope) => return Ok(envelope),
                Err(error) if attempt < READ_ATTEMPTS && error.is_transient() => {
                    let wait = error
                        .retry_after()
                        .unwrap_or_else(|| Duration::from_millis(500 * u64::from(attempt)));
                    // A rate-limit window is five minutes; waiting that long
                    // inside a spinner is worse than reporting it, so a long
                    // `retry-after` is surfaced rather than slept through.
                    if wait > Duration::from_secs(5) {
                        return Err(error);
                    }
                    tracing::debug!(
                        attempt,
                        wait_ms = wait.as_millis() as u64,
                        "retrying a read after a transient failure"
                    );
                    tokio::time::sleep(wait).await;
                    cancel.check()?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// A write. Never retried — see the module comment.
    async fn write<T, B>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
        operation: &Operation,
        cancel: &Cancel,
    ) -> Result<Envelope<T>>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        self.send(method, path, query, body, operation, cancel)
            .await
    }

    /// Walks every page of a list endpoint.
    async fn get_all<T>(
        &self,
        path: &str,
        base_query: &[(&str, String)],
        operation: &Operation,
        cancel: &Cancel,
    ) -> Result<Vec<T>>
    where
        T: DeserializeOwned,
    {
        let mut items = Vec::new();
        let mut page = 1_u32;

        loop {
            let mut query = base_query.to_vec();
            query.push(("page", page.to_string()));
            query.push(("per_page", PAGE_SIZE.to_string()));

            let envelope: Envelope<Vec<T>> = self.get(path, &query, operation, cancel).await?;
            let batch = envelope.result.unwrap_or_default();
            let batch_len = batch.len();
            items.extend(batch);

            let info = envelope.result_info.unwrap_or(ResultInfo {
                page: Some(page),
                per_page: Some(PAGE_SIZE),
                count: Some(batch_len as u32),
                ..ResultInfo::default()
            });

            if !info.has_more() || batch_len == 0 {
                break;
            }
            if page >= MAX_PAGES {
                return Err(CloudflareError::Truncated {
                    pages: page,
                    operation: operation.clone(),
                });
            }
            page = page.saturating_add(1);
            cancel.check()?;
        }

        Ok(items)
    }

    // ── Endpoints ────────────────────────────────────────────────────────

    /// `GET /user/tokens/verify`
    pub async fn verify_token(&self, cancel: &Cancel) -> Result<TokenStatus> {
        const OP: Operation = Operation::new("verify the API token", Capability::Any);

        let envelope: Envelope<TokenStatusDto> =
            self.get("user/tokens/verify", &[], &OP, cancel).await?;

        // `/user/tokens/verify` is the one endpoint documented as possibly
        // omitting `result`; a 200 with no body still means the token is live.
        Ok(envelope
            .result
            .map(TokenStatus::from)
            .unwrap_or(TokenStatus {
                id: None,
                status: "active".into(),
                expires_on: None,
            }))
    }

    /// `GET /accounts`
    pub async fn list_accounts(&self, cancel: &Cancel) -> Result<Vec<Account>> {
        const OP: Operation = Operation::new("list accounts", Capability::Any);

        let dtos: Vec<AccountDto> = self.get_all("accounts", &[], &OP, cancel).await?;
        Ok(dtos.into_iter().map(Account::from).collect())
    }

    /// `GET /zones`
    pub async fn list_zones(&self, account_id: Option<&str>, cancel: &Cancel) -> Result<Vec<Zone>> {
        const OP: Operation = Operation::new("list domains", Capability::Zones);

        let mut query = Vec::new();
        if let Some(account_id) = account_id {
            query.push(("account.id", account_id.to_string()));
        }

        let dtos: Vec<ZoneDto> = self.get_all("zones", &query, &OP, cancel).await?;
        let mut zones: Vec<Zone> = dtos.into_iter().map(Zone::from).collect();
        zones.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(zones)
    }

    /// `GET /accounts/{account_id}/cfd_tunnel`
    ///
    /// `is_deleted=false` is sent explicitly: omitting it returns deleted
    /// tunnels alongside live ones, which is never what a management list
    /// wants. Non-cloudflared tunnel types — WARP connectors, Magic WAN — are
    /// filtered out here rather than in the UI, because they share the
    /// endpoint but nothing else.
    pub async fn list_tunnels(&self, account_id: &str, cancel: &Cancel) -> Result<Vec<Tunnel>> {
        const OP: Operation = Operation::new("list tunnels", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let dtos: Vec<TunnelDto> = self
            .get_all(
                &format!("accounts/{account_id}/cfd_tunnel"),
                &[("is_deleted", "false".to_string())],
                &OP,
                cancel,
            )
            .await?;

        Ok(dtos
            .into_iter()
            .filter(TunnelDto::is_cloudflared)
            .map(TunnelDto::into_domain)
            .filter(|tunnel| !tunnel.is_deleted())
            .collect())
    }

    /// `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}`
    pub async fn get_tunnel(
        &self,
        account_id: &str,
        tunnel_id: &str,
        cancel: &Cancel,
    ) -> Result<Tunnel> {
        const OP: Operation = Operation::new("read the tunnel", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        let envelope: Envelope<TunnelDto> = self
            .get(
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
                &[],
                &OP,
                cancel,
            )
            .await?;

        envelope
            .result
            .map(TunnelDto::into_domain)
            .ok_or_else(|| CloudflareError::Decode {
                detail: "the tunnel response had no result".into(),
                operation: OP,
            })
    }

    /// `POST /accounts/{account_id}/cfd_tunnel`
    ///
    /// Returns the tunnel and, separately, the run token Cloudflare includes
    /// in the create response. The token is handed back as a
    /// [`SecretString`] so it cannot be printed by accident, and the caller
    /// decides whether to keep it — it is only needed to deploy a connector.
    pub async fn create_tunnel(
        &self,
        account_id: &str,
        name: &str,
        cancel: &Cancel,
    ) -> Result<(Tunnel, Option<SecretString>)> {
        const OP: Operation = Operation::new("create the tunnel", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let name = crate::domain::validation::tunnel_name(name)?;

        let envelope: Envelope<TunnelDto> = self
            .write(
                Method::POST,
                &format!("accounts/{account_id}/cfd_tunnel"),
                &[],
                Some(&CreateTunnelRequest::remotely_managed(name)),
                &OP,
                cancel,
            )
            .await?;

        let dto = envelope.result.ok_or_else(|| CloudflareError::Decode {
            detail: "the create response had no result".into(),
            operation: OP,
        })?;

        let token = dto.token.clone().map(SecretString::from);
        Ok((dto.into_domain(), token))
    }

    /// `PATCH /accounts/{account_id}/cfd_tunnel/{tunnel_id}` — rename only.
    ///
    /// The same endpoint rotates the run token when `tunnel_secret` is sent.
    /// This method deliberately never sends it: a rename that silently
    /// invalidated every running connector's token would be a trap.
    pub async fn rename_tunnel(
        &self,
        account_id: &str,
        tunnel_id: &str,
        name: &str,
        cancel: &Cancel,
    ) -> Result<Tunnel> {
        const OP: Operation = Operation::new("rename the tunnel", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;
        let name = crate::domain::validation::tunnel_name(name)?;

        let envelope: Envelope<TunnelDto> = self
            .write(
                Method::PATCH,
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
                &[],
                Some(&PatchTunnelRequest::rename(name)),
                &OP,
                cancel,
            )
            .await?;

        envelope
            .result
            .map(TunnelDto::into_domain)
            .ok_or_else(|| CloudflareError::Decode {
                detail: "the rename response had no result".into(),
                operation: OP,
            })
    }

    /// `DELETE /accounts/{account_id}/cfd_tunnel/{tunnel_id}`
    pub async fn delete_tunnel(
        &self,
        account_id: &str,
        tunnel_id: &str,
        cancel: &Cancel,
    ) -> Result<()> {
        const OP: Operation = Operation::new("delete the tunnel", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        let _: Envelope<serde_json::Value> = self
            .write(
                Method::DELETE,
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}"),
                &[],
                None::<&()>,
                &OP,
                cancel,
            )
            .await?;

        Ok(())
    }

    /// `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations`
    ///
    /// A tunnel that has never been configured returns a null `config`; that
    /// is an empty configuration, not an error.
    pub async fn get_configuration(
        &self,
        account_id: &str,
        tunnel_id: &str,
        cancel: &Cancel,
    ) -> Result<(TunnelConfig, Option<i64>)> {
        const OP: Operation = Operation::new("read the tunnel configuration", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        let envelope: Envelope<TunnelConfigurationDto> = self
            .get(
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"),
                &[],
                &OP,
                cancel,
            )
            .await?;

        let dto = envelope.result.unwrap_or(TunnelConfigurationDto {
            tunnel_id: None,
            account_id: None,
            version: None,
            created_at: None,
            source: None,
            config: None,
        });

        Ok((dto.config.unwrap_or_default(), dto.version))
    }

    /// `PUT /accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations`
    ///
    /// This replaces the whole document, so the caller must have read the
    /// current one, changed one thing and be writing the rest back untouched.
    /// The configuration is validated first: cloudflared rejects an ingress
    /// list whose last rule has a hostname, and a rejected configuration takes
    /// the tunnel down.
    pub async fn put_configuration(
        &self,
        account_id: &str,
        tunnel_id: &str,
        config: &TunnelConfig,
        cancel: &Cancel,
    ) -> Result<Option<i64>> {
        const OP: Operation = Operation::new("write the tunnel configuration", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;
        config.validate()?;

        let envelope: Envelope<TunnelConfigurationDto> = self
            .write(
                Method::PUT,
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"),
                &[],
                Some(&PutConfigurationRequest { config }),
                &OP,
                cancel,
            )
            .await?;

        Ok(envelope.result.and_then(|dto| dto.version))
    }

    /// `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}/connections`
    pub async fn list_connectors(
        &self,
        account_id: &str,
        tunnel_id: &str,
        cancel: &Cancel,
    ) -> Result<Vec<Connector>> {
        const OP: Operation = Operation::new("read the tunnel's connectors", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        let envelope: Envelope<Vec<ConnectorDto>> = self
            .get(
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/connections"),
                &[],
                &OP,
                cancel,
            )
            .await?;

        Ok(envelope
            .result
            .unwrap_or_default()
            .into_iter()
            .map(Connector::from)
            .collect())
    }

    /// `DELETE /accounts/{account_id}/cfd_tunnel/{tunnel_id}/connections`
    ///
    /// Deregisters a connector at the edge. `client_id` is required by this
    /// method even though the API treats it as optional: omitting it removes
    /// *every* connector, which is a different and far more destructive
    /// operation than the one any button in this application offers.
    pub async fn delete_connector(
        &self,
        account_id: &str,
        tunnel_id: &str,
        client_id: &str,
        cancel: &Cancel,
    ) -> Result<()> {
        const OP: Operation = Operation::new("deregister the connector", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;
        let client_id = crate::domain::validation::connector_id(client_id)?;

        let _: Envelope<serde_json::Value> = self
            .write(
                Method::DELETE,
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/connections"),
                &[("client_id", client_id)],
                None::<&()>,
                &OP,
                cancel,
            )
            .await?;

        Ok(())
    }

    /// `GET /accounts/{account_id}/cfd_tunnel/{tunnel_id}/token`
    ///
    /// The run token. Anyone holding it can run the tunnel, so it is fetched
    /// only when a deployment or a repair actually needs it, returned inside a
    /// [`SecretString`], and never persisted by this layer.
    ///
    /// Note that this endpoint needs a *write* permission group; a read-only
    /// token gets a 403 here even though it can list the tunnel.
    pub async fn get_tunnel_token(
        &self,
        account_id: &str,
        tunnel_id: &str,
        cancel: &Cancel,
    ) -> Result<SecretString> {
        const OP: Operation = Operation::new("fetch the tunnel's run token", Capability::Tunnels);

        let account_id = crate::domain::validation::account_id(account_id)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        // `result` is a bare JSON string here, not an object with a `token`
        // field — a very easy thing to get wrong.
        let envelope: Envelope<String> = self
            .get(
                &format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/token"),
                &[],
                &OP,
                cancel,
            )
            .await?;

        envelope
            .result
            .map(SecretString::from)
            .ok_or_else(|| CloudflareError::Decode {
                detail: "the token response had no result".into(),
                operation: OP,
            })
    }

    /// `GET /zones/{zone_id}/dns_records`, filtered to one exact name.
    pub async fn list_dns_records(
        &self,
        zone_id: &str,
        name: &str,
        cancel: &Cancel,
    ) -> Result<Vec<DnsRecord>> {
        const OP: Operation = Operation::new("read the zone's DNS records", Capability::Dns);

        let zone_id = crate::domain::validation::account_id(zone_id).map_err(|_| {
            CloudflareError::Invalid(crate::domain::validation::ValidationError::new(
                "That zone ID is not valid.",
            ))
        })?;

        // `name.exact` rather than `search`: Cloudflare documents `search` as
        // deliberately unspecified and subject to change, which is not
        // something to build a deletion decision on.
        let dtos: Vec<DnsRecordDto> = self
            .get_all(
                &format!("zones/{zone_id}/dns_records"),
                &[("name.exact", name.to_ascii_lowercase())],
                &OP,
                cancel,
            )
            .await?;

        Ok(dtos
            .into_iter()
            .filter_map(|dto| dto.try_into_domain(&zone_id))
            .collect())
    }

    /// `POST /zones/{zone_id}/dns_records` — the proxied tunnel CNAME.
    pub async fn create_tunnel_cname(
        &self,
        zone_id: &str,
        hostname: &str,
        tunnel_id: &str,
        comment: Option<String>,
        cancel: &Cancel,
    ) -> Result<DnsRecord> {
        const OP: Operation = Operation::new("create the DNS record", Capability::Dns);

        let zone_id = crate::domain::validation::account_id(zone_id).map_err(|_| {
            CloudflareError::Invalid(crate::domain::validation::ValidationError::new(
                "That zone ID is not valid.",
            ))
        })?;
        let hostname = crate::domain::validation::hostname(hostname)?;
        let tunnel_id = crate::domain::validation::tunnel_id(tunnel_id)?;

        let envelope: Envelope<DnsRecordDto> = self
            .write(
                Method::POST,
                &format!("zones/{zone_id}/dns_records"),
                &[],
                Some(&CreateDnsRecordRequest::tunnel_cname(
                    &hostname, &tunnel_id, comment,
                )),
                &OP,
                cancel,
            )
            .await?;

        envelope
            .result
            .and_then(|dto| dto.try_into_domain(&zone_id))
            .ok_or_else(|| CloudflareError::Decode {
                detail: "the DNS create response had no usable result".into(),
                operation: OP,
            })
    }

    /// `PATCH /zones/{zone_id}/dns_records/{record_id}`
    ///
    /// `PATCH`, not `PUT`. `PUT` resets every field the body omits, so
    /// repairing a `proxied` flag with `PUT` would silently discard the
    /// record's comment, tags and TTL.
    pub async fn patch_dns_record(
        &self,
        zone_id: &str,
        record_id: &str,
        patch: PatchDnsRecordRequest,
        cancel: &Cancel,
    ) -> Result<DnsRecord> {
        const OP: Operation = Operation::new("repair the DNS record", Capability::Dns);

        let zone_id = crate::domain::validation::account_id(zone_id).map_err(|_| {
            CloudflareError::Invalid(crate::domain::validation::ValidationError::new(
                "That zone ID is not valid.",
            ))
        })?;
        let record_id = crate::domain::validation::dns_record_id(record_id)?;

        let envelope: Envelope<DnsRecordDto> = self
            .write(
                Method::PATCH,
                &format!("zones/{zone_id}/dns_records/{record_id}"),
                &[],
                Some(&patch),
                &OP,
                cancel,
            )
            .await?;

        envelope
            .result
            .and_then(|dto| dto.try_into_domain(&zone_id))
            .ok_or_else(|| CloudflareError::Decode {
                detail: "the DNS patch response had no usable result".into(),
                operation: OP,
            })
    }

    /// `DELETE /zones/{zone_id}/dns_records/{record_id}`
    ///
    /// Hard delete, unlike a tunnel's soft delete. The caller is responsible
    /// for having established that this record is the tunnel's own — see
    /// [`DnsRecord::is_tunnel_cname`].
    pub async fn delete_dns_record(
        &self,
        zone_id: &str,
        record_id: &str,
        cancel: &Cancel,
    ) -> Result<()> {
        const OP: Operation = Operation::new("remove the DNS record", Capability::Dns);

        let zone_id = crate::domain::validation::account_id(zone_id).map_err(|_| {
            CloudflareError::Invalid(crate::domain::validation::ValidationError::new(
                "That zone ID is not valid.",
            ))
        })?;
        let record_id = crate::domain::validation::dns_record_id(record_id)?;

        let _: Envelope<serde_json::Value> = self
            .write(
                Method::DELETE,
                &format!("zones/{zone_id}/dns_records/{record_id}"),
                &[],
                None::<&()>,
                &OP,
                cancel,
            )
            .await?;

        Ok(())
    }
}

/// Reads `retry-after`, which Cloudflare sends only on a 429.
///
/// The header is case-insensitive per HTTP, and Cloudflare documents it
/// lower-case; `reqwest`'s `HeaderMap` already normalises, so the constant is
/// enough.
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .or_else(|| parse_ratelimit_reset(headers))
}

/// Falls back to the `t` parameter of the structured `Ratelimit` header, which
/// Cloudflare sends on ordinary responses to let a client pace itself.
fn parse_ratelimit_reset(headers: &HeaderMap) -> Option<Duration> {
    let value: &HeaderValue = headers.get("ratelimit")?;
    let value = value.to_str().ok()?;
    value
        .split(';')
        .filter_map(|part| part.trim().strip_prefix("t="))
        .filter_map(|seconds| seconds.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
        .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debug_impl_never_prints_the_token() {
        let client = CloudflareClient::with_base_url(
            SecretString::from("abcdefghijklmnopqrstuvwxyz0123456789"),
            "https://example.test/client/v4",
            Timeouts::default(),
        )
        .unwrap();

        let debug = format!("{client:?}");
        assert!(!debug.contains("abcdefghij"), "got {debug}");
        assert!(debug.contains("[redacted]"));
        assert!(debug.contains("example.test"));
    }

    #[test]
    fn urls_join_without_doubling_the_slash() {
        let client = CloudflareClient::with_base_url(
            SecretString::from("t".repeat(40)),
            "https://example.test/client/v4/",
            Timeouts::default(),
        )
        .unwrap();
        assert_eq!(client.url("zones"), "https://example.test/client/v4/zones");
        assert_eq!(client.url("/zones"), "https://example.test/client/v4/zones");
    }

    #[test]
    fn retry_after_is_read_from_the_header() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("42"));
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(42)));
    }

    #[test]
    fn the_ratelimit_header_is_the_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "ratelimit",
            HeaderValue::from_static("\"default\";r=0;t=30"),
        );
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(30)));
    }

    #[test]
    fn a_missing_retry_after_is_none_rather_than_zero() {
        assert_eq!(parse_retry_after(&HeaderMap::new()), None);
    }

    #[test]
    fn a_malformed_retry_after_is_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("soon"));
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn cancel_is_observable_from_a_clone() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!clone.is_cancelled());
        cancel.cancel();
        assert!(clone.is_cancelled());
        assert!(clone.check().is_err());
    }
}
