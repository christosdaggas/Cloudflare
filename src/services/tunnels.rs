//! Creating, refreshing and removing tunnels, and the wizard's orchestration.
//!
//! The create flow is the one operation in this application that can leave
//! work half done in three different systems at once — Cloudflare's tunnel
//! store, its DNS, and a machine over SSH — so it is modelled as an explicit
//! sequence of steps whose outcome is reported individually. A wizard that
//! said only "failed" after creating a tunnel would leave the user with an
//! orphan they did not know about.

use secrecy::SecretString;

use crate::cloudflare::error::{CloudflareError, Result};
use crate::cloudflare::{Cancel, CloudflareClient};
use crate::domain::config::OriginRequestUpdate;
use crate::domain::hostname::OriginService;
use crate::domain::tunnel::{Connector, Tunnel, TunnelSummary};
use crate::services::hostnames::{self, PublishRequest, Status, Step, StepResult};
use serde::{Deserialize, Serialize};

/// A tunnel plus the extra facts the list view shows, which the list endpoint
/// does not include.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRow {
    pub tunnel: Tunnel,
    /// `None` until the configuration has been read for this tunnel.
    pub hostname_count: Option<usize>,
    /// Registered hosts associated with this tunnel, by name.
    pub managed_hosts: Vec<String>,
}

impl TunnelRow {
    pub fn new(tunnel: Tunnel) -> Self {
        Self {
            tunnel,
            hostname_count: None,
            managed_hosts: Vec::new(),
        }
    }

    /// The text a search box matches against.
    pub fn search_text(&self) -> String {
        format!(
            "{} {} {} {}",
            self.tunnel.name,
            self.tunnel.id,
            self.tunnel.status.label(),
            self.managed_hosts.join(" ")
        )
        .to_lowercase()
    }
}

/// Lists the tunnels on an account.
pub async fn list(
    client: &CloudflareClient,
    account_id: &str,
    cancel: &Cancel,
) -> Result<Vec<TunnelRow>> {
    let tunnels = client.list_tunnels(account_id, cancel).await?;
    Ok(tunnels.into_iter().map(TunnelRow::new).collect())
}

/// Reads one tunnel's configuration to fill in its hostname count.
///
/// Done per tunnel rather than in the list call because Cloudflare has no
/// endpoint that returns both. The list is shown immediately with the count
/// blank and it fills in as the reads land, which is why `hostname_count` is
/// an `Option` rather than a zero.
pub async fn load_hostname_count(
    client: &CloudflareClient,
    account_id: &str,
    tunnel: &Tunnel,
    cancel: &Cancel,
) -> Option<usize> {
    if !tunnel.config_source.is_remotely_managed() {
        // A locally configured tunnel's ingress lives on its own machine;
        // asking Cloudflare would return an empty document and imply the
        // tunnel serves nothing, which is worse than saying nothing.
        return None;
    }

    client
        .get_configuration(account_id, &tunnel.id, cancel)
        .await
        .ok()
        .map(|(config, _)| config.hostname_count())
}

pub async fn connectors(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    cancel: &Cancel,
) -> Result<Vec<Connector>> {
    client.list_connectors(account_id, tunnel_id, cancel).await
}

pub async fn rename(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    name: &str,
    cancel: &Cancel,
) -> Result<Tunnel> {
    client
        .rename_tunnel(account_id, tunnel_id, name, cancel)
        .await
}

/// What deleting a tunnel will affect, gathered before the confirmation is
/// shown so the dialog can be specific rather than generic.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeleteImpact {
    pub hostnames: Vec<String>,
    pub connector_count: usize,
    pub managed_hosts: Vec<String>,
    /// DNS records that will be left behind pointing at a tunnel that no
    /// longer exists. Visitors get a 1016 error until they are removed.
    pub orphaned_dns: Vec<String>,
}

impl DeleteImpact {
    pub fn is_empty(&self) -> bool {
        self.hostnames.is_empty() && self.connector_count == 0 && self.managed_hosts.is_empty()
    }

    /// The sentences the confirmation dialog shows under its heading.
    pub fn warnings(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        if !self.hostnames.is_empty() {
            warnings.push(format!(
                "{} public hostname(s) will stop working: {}.",
                self.hostnames.len(),
                self.hostnames.join(", ")
            ));
        }
        if !self.orphaned_dns.is_empty() {
            warnings.push(format!(
                "{} hostname(s) may have a DNS record left pointing at a tunnel that no longer \
                 exists, and visitors will see a 1016 error until it is removed: {}.",
                self.orphaned_dns.len(),
                self.orphaned_dns.join(", ")
            ));
        }
        if self.connector_count > 0 {
            warnings.push(format!(
                "{} connector(s) are currently registered and will be disconnected.",
                self.connector_count
            ));
        }
        if !self.managed_hosts.is_empty() {
            warnings.push(format!(
                "{} registered host(s) will be detached from this tunnel but not \
                 otherwise changed: {}.",
                self.managed_hosts.len(),
                self.managed_hosts.join(", ")
            ));
        }

        warnings
    }
}

/// Gathers what deleting a tunnel would affect.
pub async fn delete_impact(
    client: &CloudflareClient,
    account_id: &str,
    tunnel: &Tunnel,
    managed_hosts: &[String],
    cancel: &Cancel,
) -> DeleteImpact {
    let mut impact = DeleteImpact {
        managed_hosts: managed_hosts.to_vec(),
        ..DeleteImpact::default()
    };

    if let Ok((config, _)) = client
        .get_configuration(account_id, &tunnel.id, cancel)
        .await
    {
        impact.hostnames = config
            .hostname_rules()
            .filter_map(|(_, rule)| rule.hostname.clone())
            .collect();
        // A published hostname usually has a DNS record, but this was not
        // checked — the zone may be elsewhere — so the dialog words it as
        // "may", not "will". Deleting the tunnel never removes DNS.
        impact.orphaned_dns = impact.hostnames.clone();
    }

    if let Ok(connectors) = client.list_connectors(account_id, &tunnel.id, cancel).await {
        impact.connector_count = connectors.len();
    }

    impact
}

pub async fn delete(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    cancel: &Cancel,
) -> Result<()> {
    client.delete_tunnel(account_id, tunnel_id, cancel).await
}

/// Deregisters one connector at the edge.
///
/// Note what this is not: it does not stop `cloudflared` on the machine. The
/// process will reconnect. Stopping the service is a separate, remote
/// operation, and the UI keeps the two visibly apart.
pub async fn disconnect_connector(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    client_id: &str,
    cancel: &Cancel,
) -> Result<()> {
    client
        .delete_connector(account_id, tunnel_id, client_id, cancel)
        .await
}

// ── The create wizard ────────────────────────────────────────────────────

/// What the wizard collected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRequest {
    pub account_id: String,
    pub name: String,
    /// `None` when the user skipped the hostname step.
    pub hostname: Option<HostnameRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostnameRequest {
    pub hostname: String,
    pub path: Option<String>,
    pub service: OriginService,
    /// `None` when the hostname's zone is not on this account, which is
    /// allowed: the ingress rule is written and DNS is left to the user.
    pub zone_id: Option<String>,
}

/// The plan, as shown on the wizard's review step.
///
/// Deliberately a list of sentences rather than a rendered diff: the user is
/// being asked to approve *changes to their account*, and the most important
/// property of that list is that it is complete and readable.
pub fn review(request: &CreateRequest) -> Vec<String> {
    let mut changes = vec![format!(
        "Create a remotely managed tunnel named “{}”",
        request.name
    )];

    match &request.hostname {
        None => changes.push(
            "No public hostname — the tunnel will be created with an empty \
             configuration and can serve a hostname later"
                .into(),
        ),
        Some(hostname) => {
            changes.push(format!(
                "Add an ingress rule sending {} to {}",
                hostname.hostname,
                hostname.service.to_service_string()
            ));
            changes.push(format!(
                "Add the required catch-all rule ({})",
                crate::domain::config::CATCH_ALL_SERVICE
            ));
            match &hostname.zone_id {
                Some(_) => changes.push(format!(
                    "Create a proxied CNAME for {} pointing at the new tunnel",
                    hostname.hostname
                )),
                None => changes.push(format!(
                    "No DNS record — {} is not in a zone on this account, so its \
                     record has to be created wherever that domain is managed",
                    hostname.hostname
                )),
            }
        }
    }

    changes
}

/// The result of running the wizard.
#[derive(Debug, Clone)]
pub struct CreateOutcome {
    pub steps: Vec<StepResult>,
    /// The tunnel, if it was created. Present even when a later step failed —
    /// that is the whole point.
    pub tunnel: Option<Tunnel>,
    /// The run token Cloudflare returned. Held only long enough to deploy a
    /// connector, and never written to storage.
    pub token: Option<SecretString>,
    pub hostname: Option<hostnames::Outcome>,
}

impl CreateOutcome {
    pub fn succeeded(&self) -> bool {
        self.steps.iter().all(|step| step.status != Status::Failed)
    }

    pub fn is_partial(&self) -> bool {
        self.tunnel.is_some() && !self.succeeded()
    }

    /// The instructions to show when SSH deployment was skipped, so the user
    /// can finish by hand.
    ///
    /// Deliberately does not include the token: it is fetched by the button
    /// that copies the command, so it never sits in a label on screen.
    pub fn manual_instructions(&self) -> Option<String> {
        let tunnel = self.tunnel.as_ref()?;
        Some(format!(
            "On the machine that should serve this tunnel:\n\
             \n\
             1. Install cloudflared from Cloudflare's package repository.\n\
             2. Save the tunnel's run token to a root-owned file, mode 0600:\n\
             \x20     sudo install -d -m 0700 /etc/tunnel-manager\n\
             \x20     sudo install -m 0600 /dev/stdin /etc/tunnel-manager/{id}.token\n\
             \x20     (paste the token, then press Ctrl-D)\n\
             3. Run the connector:\n\
             \x20     sudo cloudflared tunnel run --token-file /etc/tunnel-manager/{id}.token\n\
             \n\
             Use “Copy Run Token” to put the token on the clipboard. Treat it like a \
             password — anyone holding it can serve this tunnel.",
            id = tunnel.id
        ))
    }
}

/// Runs the create flow.
///
/// Ordering is chosen so that the irreversible step comes as late as possible
/// and each failure leaves a state the user can understand:
///
/// 1. Create the tunnel. If this fails, nothing exists.
/// 2. If a hostname was asked for, publish it. That is itself a multi-step
///    operation with its own partial-failure handling, and its steps are
///    folded into this outcome rather than summarised away.
///
/// Deployment to a host is *not* here. It is a separate, optional step the
/// wizard offers afterwards, so that a tunnel is never left uncreated because
/// an SSH key was wrong.
pub async fn create(
    client: &CloudflareClient,
    request: &CreateRequest,
    cancel: &Cancel,
) -> CreateOutcome {
    let mut steps = Vec::new();

    let (tunnel, token) = match client
        .create_tunnel(&request.account_id, &request.name, cancel)
        .await
    {
        Ok((tunnel, token)) => {
            steps.push(StepResult {
                step: Step::WriteConfiguration,
                status: Status::Done,
                detail: format!("Created the tunnel “{}”.", tunnel.name),
            });
            (tunnel, token)
        }
        Err(error) => {
            steps.push(StepResult {
                step: Step::WriteConfiguration,
                status: Status::Failed,
                detail: error.user_message(),
            });
            return CreateOutcome {
                steps,
                tunnel: None,
                token: None,
                hostname: None,
            };
        }
    };

    let Some(hostname) = &request.hostname else {
        steps.push(StepResult {
            step: Step::CreateDnsRecord,
            status: Status::Skipped,
            detail: "No public hostname was requested.".into(),
        });
        return CreateOutcome {
            steps,
            tunnel: Some(tunnel),
            token,
            hostname: None,
        };
    };

    let outcome = hostnames::publish(
        client,
        &PublishRequest {
            account_id: request.account_id.clone(),
            tunnel_id: tunnel.id.clone(),
            tunnel_name: tunnel.name.clone(),
            hostname: hostname.hostname.clone(),
            path: hostname.path.clone(),
            service: hostname.service.clone(),
            zone_id: hostname.zone_id.clone(),
            // The create wizard offers no origin settings; the rule is
            // written with cloudflared's defaults and can be refined from
            // the tunnel's detail page afterwards.
            origin_request: OriginRequestUpdate::Preserve,
        },
        cancel,
    )
    .await;

    steps.extend(outcome.steps.iter().cloned());

    if !outcome.succeeded() {
        steps.push(StepResult {
            step: Step::WriteConfiguration,
            status: Status::Skipped,
            detail: format!(
                "The tunnel “{name}” still exists and can be renamed, published to, or deleted from the Tunnels page.",
                name = tunnel.name
            ),
        });
    }

    CreateOutcome {
        steps,
        tunnel: Some(tunnel),
        token,
        hostname: Some(outcome),
    }
}

/// Fetches a tunnel's run token, for a deployment or for the clipboard.
///
/// Kept out of every other flow on purpose. The token is the credential that
/// lets anything serve the tunnel, so it is fetched at the moment it is needed
/// and dropped straight afterwards, rather than being carried around in
/// application state.
pub async fn fetch_run_token(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    cancel: &Cancel,
) -> Result<SecretString> {
    client
        .get_tunnel_token(account_id, tunnel_id, cancel)
        .await
        .map_err(|error| match error {
            // This endpoint needs a *write* permission group. A read-only
            // token lists the tunnel happily and is refused here, which is
            // confusing unless it is said out loud.
            CloudflareError::Forbidden { errors, .. } => CloudflareError::Forbidden {
                errors,
                operation: crate::cloudflare::Operation::new(
                    "fetch the tunnel's run token — this needs Cloudflare Tunnel: Edit, \
                     not Read",
                    crate::cloudflare::Capability::Tunnels,
                ),
            },
            other => other,
        })
}

/// The dashboard's counters.
pub fn summarise(rows: &[TunnelRow]) -> TunnelSummary {
    let tunnels: Vec<Tunnel> = rows.iter().map(|row| row.tunnel.clone()).collect();
    TunnelSummary::of(&tunnels)
}

/// Total published hostnames across every tunnel whose configuration has been
/// read. Tunnels not yet read are excluded rather than counted as zero.
pub fn total_hostnames(rows: &[TunnelRow]) -> Option<usize> {
    let known: Vec<usize> = rows.iter().filter_map(|row| row.hostname_count).collect();
    if known.is_empty() && !rows.is_empty() {
        None
    } else {
        Some(known.iter().sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tunnel::{ConfigSource, TunnelKind, TunnelStatus};

    fn tunnel(name: &str, status: TunnelStatus) -> Tunnel {
        Tunnel {
            id: "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into(),
            account_tag: "a".repeat(32),
            name: name.into(),
            status,
            config_source: ConfigSource::Cloudflare,
            kind: TunnelKind::Cfd,
            remote_config: true,
            created_at: None,
            deleted_at: None,
            conns_active_at: None,
            conns_inactive_at: None,
            connector_count: 0,
            hostname_count: None,
        }
    }

    #[test]
    fn review_of_a_tunnel_with_no_hostname_says_so_explicitly() {
        let changes = review(&CreateRequest {
            account_id: "a".repeat(32),
            name: "web".into(),
            hostname: None,
        });

        assert_eq!(changes.len(), 2);
        assert!(changes[0].contains("remotely managed"));
        assert!(changes[1].contains("No public hostname"));
        assert!(
            !changes.iter().any(|change| change.contains("CNAME")),
            "nothing about DNS when no hostname was asked for"
        );
    }

    #[test]
    fn review_of_a_tunnel_with_a_hostname_lists_all_four_changes() {
        let changes = review(&CreateRequest {
            account_id: "a".repeat(32),
            name: "web".into(),
            hostname: Some(HostnameRequest {
                hostname: "app.example.com".into(),
                path: None,
                service: OriginService::http("localhost", 8080),
                zone_id: Some("z".repeat(32)),
            }),
        });

        assert!(changes[0].contains("Create a remotely managed tunnel"));
        assert!(changes[1].contains("app.example.com"));
        assert!(changes[1].contains("http://localhost:8080"));
        assert!(changes[2].contains("catch-all"));
        assert!(changes[3].contains("proxied CNAME"));
    }

    #[test]
    fn review_warns_when_the_hostname_is_not_in_a_zone_on_this_account() {
        let changes = review(&CreateRequest {
            account_id: "a".repeat(32),
            name: "web".into(),
            hostname: Some(HostnameRequest {
                hostname: "app.elsewhere.test".into(),
                path: None,
                service: OriginService::http("localhost", 8080),
                zone_id: None,
            }),
        });

        let dns = changes.last().unwrap();
        assert!(dns.contains("No DNS record"), "got {dns}");
        assert!(dns.contains("wherever that domain is managed"));
    }

    #[test]
    fn delete_impact_says_nothing_when_there_is_nothing_to_say() {
        let impact = DeleteImpact::default();
        assert!(impact.is_empty());
        assert!(impact.warnings().is_empty());
    }

    #[test]
    fn delete_impact_warns_about_orphaned_dns_and_names_the_error() {
        let impact = DeleteImpact {
            hostnames: vec!["app.example.com".into()],
            orphaned_dns: vec!["app.example.com".into()],
            connector_count: 2,
            managed_hosts: vec!["edge-1".into()],
        };

        let warnings = impact.warnings();
        assert!(!impact.is_empty());
        assert!(warnings.iter().any(|w| w.contains("1016")));
        assert!(warnings.iter().any(|w| w.contains("2 connector(s)")));
        assert!(warnings
            .iter()
            .any(|w| w.contains("detached from this tunnel but not otherwise changed")));
    }

    #[test]
    fn a_create_outcome_that_made_a_tunnel_but_failed_later_is_partial() {
        let outcome = CreateOutcome {
            steps: vec![
                StepResult {
                    step: Step::WriteConfiguration,
                    status: Status::Done,
                    detail: "created".into(),
                },
                StepResult {
                    step: Step::CreateDnsRecord,
                    status: Status::Failed,
                    detail: "denied".into(),
                },
            ],
            tunnel: Some(tunnel("web", TunnelStatus::Inactive)),
            token: None,
            hostname: None,
        };
        assert!(!outcome.succeeded());
        assert!(outcome.is_partial());
    }

    #[test]
    fn a_create_outcome_that_never_made_a_tunnel_is_not_partial() {
        let outcome = CreateOutcome {
            steps: vec![StepResult {
                step: Step::WriteConfiguration,
                status: Status::Failed,
                detail: "denied".into(),
            }],
            tunnel: None,
            token: None,
            hostname: None,
        };
        assert!(!outcome.is_partial());
        assert!(outcome.manual_instructions().is_none());
    }

    #[test]
    fn manual_instructions_use_the_token_file_and_never_print_a_token() {
        let outcome = CreateOutcome {
            steps: vec![],
            tunnel: Some(tunnel("web", TunnelStatus::Inactive)),
            token: Some(SecretString::from("eyJhIjoieCJ9")),
            hostname: None,
        };

        let text = outcome.manual_instructions().unwrap();
        assert!(text.contains("--token-file"));
        assert!(text.contains("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415.token"));
        assert!(text.contains("0600"));
        assert!(
            !text.contains("eyJhIjoieCJ9"),
            "the token must never be printed into on-screen instructions"
        );
        assert!(text.contains("Copy Run Token"));
    }

    #[test]
    fn a_row_is_searchable_by_name_id_status_and_host() {
        let mut row = TunnelRow::new(tunnel("Production Web", TunnelStatus::Degraded));
        row.managed_hosts = vec!["edge-frankfurt".into()];

        let text = row.search_text();
        assert!(text.contains("production web"));
        assert!(text.contains("f70ff985"));
        assert!(text.contains("degraded"));
        assert!(text.contains("edge-frankfurt"));
    }

    #[test]
    fn total_hostnames_is_unknown_until_something_has_been_read() {
        let rows = vec![TunnelRow::new(tunnel("a", TunnelStatus::Healthy))];
        assert_eq!(total_hostnames(&rows), None, "0 would be a lie here");

        let mut counted = rows.clone();
        counted[0].hostname_count = Some(3);
        assert_eq!(total_hostnames(&counted), Some(3));

        assert_eq!(total_hostnames(&[]), Some(0), "an empty account has none");
    }

    #[test]
    fn the_summary_matches_the_rows() {
        let rows = vec![
            TunnelRow::new(tunnel("a", TunnelStatus::Healthy)),
            TunnelRow::new(tunnel("b", TunnelStatus::Down)),
            TunnelRow::new(tunnel("c", TunnelStatus::Inactive)),
        ];
        let summary = summarise(&rows);
        assert_eq!(summary.healthy, 1);
        assert_eq!(summary.down, 1);
        assert_eq!(summary.inactive, 1);
        assert!(summary.needs_attention());
    }
}
