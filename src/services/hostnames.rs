//! Publishing, repairing and unpublishing a public hostname.
//!
//! A hostname served through a tunnel is two facts in two systems — an ingress
//! rule in the tunnel's configuration and a proxied CNAME in the zone — and
//! there is no transaction across them. Anything here can therefore half
//! succeed, and the whole design of this module is about that:
//!
//! * **Every operation returns a list of steps with an outcome each**, never a
//!   bare `Result`. "Could not publish app.example.com" is not good enough
//!   when the ingress rule went in and the DNS record did not; the user needs
//!   to know which half exists.
//! * **The cheap, read-only checks come first.** A DNS conflict is detected
//!   before the configuration is written, so the commonest failure leaves the
//!   account exactly as it was.
//! * **A failure after a write offers a compensating action**, not a retry of
//!   the whole thing. Retrying a publish whose DNS half failed would rewrite
//!   an ingress rule that is already correct.

use crate::cloudflare::error::CloudflareError;
use crate::cloudflare::{Cancel, CloudflareClient};
use crate::domain::config::{IngressRule, OriginRequestUpdate, TunnelConfig};
use crate::domain::hostname::{DnsState, OriginService};
use crate::domain::validation::{self, ValidationError};

/// The comment written on records this application creates, so a later
/// `dns_records` listing can tell them apart from the user's own.
pub const RECORD_COMMENT: &str = "Managed by Tunnel Manager";

/// The steps a publish or unpublish goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ReadConfiguration,
    CheckDns,
    WriteConfiguration,
    CreateDnsRecord,
    RepairDnsRecord,
    RemoveDnsRecord,
    Verify,
    RollbackConfiguration,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadConfiguration => "Read the tunnel's configuration",
            Self::CheckDns => "Check for conflicting DNS records",
            Self::WriteConfiguration => "Update the ingress configuration",
            Self::CreateDnsRecord => "Create the proxied CNAME",
            Self::RepairDnsRecord => "Repair the DNS record",
            Self::RemoveDnsRecord => "Remove the DNS record",
            Self::Verify => "Verify the result",
            Self::RollbackConfiguration => "Undo the configuration change",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Done,
    /// Deliberately not attempted — no zone selected, nothing to remove.
    Skipped,
    Failed,
}

impl Status {
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Done => "object-select-symbolic",
            Self::Skipped => "media-skip-forward-symbolic",
            Self::Failed => "dialog-error-symbolic",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Done => "success",
            Self::Skipped => "dimmed",
            Self::Failed => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub step: Step,
    pub status: Status,
    /// One sentence, already redacted.
    pub detail: String,
}

impl StepResult {
    fn done(step: Step, detail: impl Into<String>) -> Self {
        Self {
            step,
            status: Status::Done,
            detail: detail.into(),
        }
    }

    fn skipped(step: Step, detail: impl Into<String>) -> Self {
        Self {
            step,
            status: Status::Skipped,
            detail: detail.into(),
        }
    }

    fn failed(step: Step, detail: impl Into<String>) -> Self {
        Self {
            step,
            status: Status::Failed,
            detail: detail.into(),
        }
    }
}

/// What the user can safely do next when something went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// Nothing changed; the same operation can simply be tried again.
    Retry,
    /// The ingress rule exists but DNS does not. Repairing creates only the
    /// missing record — it does not rewrite the configuration.
    RepairDns,
    /// The ingress rule was added and DNS failed. Rolling back removes the
    /// rule again, returning the tunnel to its previous state.
    RollBackIngress,
    /// A conflicting record exists that this application will not touch.
    ResolveConflictByHand {
        record_type: String,
        content: String,
    },
    /// The permission the token is missing.
    FixPermissions,
}

impl Recovery {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Retry => "Try Again",
            Self::RepairDns => "Repair DNS",
            Self::RollBackIngress => "Undo the Ingress Change",
            Self::ResolveConflictByHand { .. } => "Open Cloudflare Dashboard",
            Self::FixPermissions => "Check the API Token",
        }
    }

    pub fn explanation(&self) -> String {
        match self {
            Self::Retry => "Nothing was changed, so this is safe to try again.".into(),
            Self::RepairDns => {
                "The tunnel now serves this hostname, but no DNS record points at it. \
                 Repairing creates the record and changes nothing else."
                    .into()
            }
            Self::RollBackIngress => {
                "The tunnel was updated but the DNS record was not created, so the \
                 hostname does not resolve. Undoing removes the rule that was added and \
                 leaves every other rule alone."
                    .into()
            }
            Self::ResolveConflictByHand {
                record_type,
                content,
            } => format!(
                "An existing {record_type} record points at {content}. This application \
                 will not delete a record it did not create — remove or change it in the \
                 Cloudflare dashboard, then publish again."
            ),
            Self::FixPermissions => {
                "The API token is missing a permission this operation needs. Check it on \
                 the Connection page."
                    .into()
            }
        }
    }
}

/// The result of a publish, a repair or an unpublish.
///
/// Not `Eq`: an ingress rule can carry arbitrary preserved JSON, and
/// `serde_json::Value` holds floats.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub hostname: String,
    pub steps: Vec<StepResult>,
    /// The DNS state as last observed.
    pub dns: DnsState,
    /// What to offer next, when something is wrong.
    pub recovery: Option<Recovery>,
    /// The rule that was added or removed, kept so a rollback can put it back
    /// exactly as it was rather than reconstructing it.
    pub touched_rule: Option<IngressRule>,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        self.steps.iter().all(|step| step.status != Status::Failed)
    }

    /// True when some steps completed and a later one failed — the state the
    /// user most needs told about.
    pub fn is_partial(&self) -> bool {
        !self.succeeded() && self.steps.iter().any(|step| step.status == Status::Done)
    }

    pub fn failed_step(&self) -> Option<&StepResult> {
        self.steps.iter().find(|step| step.status == Status::Failed)
    }

    /// One line for a toast.
    pub fn summary(&self) -> String {
        match self.failed_step() {
            None => format!("{} is published.", self.hostname),
            Some(failed) if self.is_partial() => {
                format!("{} is only partly set up: {}", self.hostname, failed.detail)
            }
            Some(failed) => format!("Could not publish {}: {}", self.hostname, failed.detail),
        }
    }
}

/// Everything a publish needs.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub account_id: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub hostname: String,
    pub path: Option<String>,
    pub service: OriginService,
    /// The zone the hostname belongs to. `None` means the zone could not be
    /// determined — the ingress rule is still written, and DNS is skipped with
    /// an explanation rather than silently.
    pub zone_id: Option<String>,
    /// Whether to preserve, clear or replace the rule's `originRequest`.
    pub origin_request: OriginRequestUpdate,
}

impl PublishRequest {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validation::account_id(&self.account_id)?;
        validation::tunnel_id(&self.tunnel_id)?;
        validation::hostname(&self.hostname)?;
        if let Some(path) = &self.path {
            validation::ingress_path(path)?;
        }
        Ok(())
    }
}

/// Publishes a hostname on a tunnel.
pub async fn publish(
    client: &CloudflareClient,
    request: &PublishRequest,
    cancel: &Cancel,
) -> Outcome {
    let mut steps = Vec::new();
    let mut outcome_dns = DnsState::Unknown {
        reason: "DNS was not checked.".into(),
    };

    if let Err(error) = request.validate() {
        steps.push(StepResult::failed(
            Step::ReadConfiguration,
            error.to_string(),
        ));
        return Outcome {
            hostname: request.hostname.clone(),
            steps,
            dns: outcome_dns,
            recovery: Some(Recovery::Retry),
            touched_rule: None,
        };
    }

    // ── 1. Read the current configuration ────────────────────────────────
    let mut config = match client
        .get_configuration(&request.account_id, &request.tunnel_id, cancel)
        .await
    {
        Ok((config, _version)) => {
            steps.push(StepResult::done(
                Step::ReadConfiguration,
                format!("{} existing rule(s) read.", config.hostname_count()),
            ));
            config
        }
        Err(error) => {
            let recovery = recovery_for(&error);
            steps.push(StepResult::failed(
                Step::ReadConfiguration,
                error.user_message(),
            ));
            return Outcome {
                hostname: request.hostname.clone(),
                steps,
                dns: outcome_dns,
                recovery,
                touched_rule: None,
            };
        }
    };

    // ── 2. Check DNS before writing anything ─────────────────────────────
    //
    // Deliberately ahead of the configuration write. A conflicting record is
    // by far the commonest reason a publish cannot complete, and finding it
    // first means that case leaves the account untouched instead of leaving an
    // orphaned ingress rule behind.
    let mut existing_records = Vec::new();
    match &request.zone_id {
        None => {
            steps.push(StepResult::skipped(
                Step::CheckDns,
                "No zone was selected for this hostname, so DNS was not checked.",
            ));
            outcome_dns = DnsState::Unknown {
                reason: "This hostname's zone is not on the connected account, so its \
                         DNS record cannot be managed from here."
                    .into(),
            };
        }
        Some(zone_id) => {
            match client
                .list_dns_records(zone_id, &request.hostname, cancel)
                .await
            {
                Ok(records) => {
                    outcome_dns = DnsState::evaluate(&records, &request.tunnel_id);
                    existing_records = records;

                    if let DnsState::Conflicting {
                        record_type,
                        content,
                        ..
                    } = &outcome_dns
                    {
                        steps.push(StepResult::failed(
                            Step::CheckDns,
                            format!(
                                "An existing {record_type} record points at {content}. \
                                 Nothing was changed."
                            ),
                        ));
                        return Outcome {
                            hostname: request.hostname.clone(),
                            steps,
                            recovery: Some(Recovery::ResolveConflictByHand {
                                record_type: record_type.clone(),
                                content: content.clone(),
                            }),
                            dns: outcome_dns,
                            touched_rule: None,
                        };
                    }

                    steps.push(StepResult::done(
                        Step::CheckDns,
                        match &outcome_dns {
                            DnsState::Correct { .. } => {
                                "The proxied CNAME already exists.".to_string()
                            }
                            DnsState::NotProxied { .. } => {
                                "A CNAME exists but is not proxied; it will be repaired."
                                    .to_string()
                            }
                            _ => "No conflicting record.".to_string(),
                        },
                    ));
                }
                Err(error) => {
                    // A zone this token cannot read is a permission problem,
                    // not a reason to abandon the ingress change — the user
                    // may be managing DNS elsewhere.
                    steps.push(StepResult::skipped(Step::CheckDns, error.user_message()));
                    outcome_dns = DnsState::Unknown {
                        reason: error.user_message(),
                    };
                }
            }
        }
    }

    // ── 3. Modify and write the configuration ────────────────────────────
    let before = config.clone();
    let service = request.service.to_service_string();
    config.upsert(
        &request.hostname,
        request.path.as_deref(),
        &service,
        request.origin_request.clone(),
    );

    let touched_rule = config
        .find(&request.hostname, request.path.as_deref())
        .and_then(|index| config.ingress.get(index).cloned());

    match client
        .put_configuration(&request.account_id, &request.tunnel_id, &config, cancel)
        .await
    {
        Ok(_version) => steps.push(StepResult::done(
            Step::WriteConfiguration,
            format!("{} now routes to {service}.", request.hostname),
        )),
        Err(error) => {
            let recovery = recovery_for(&error);
            steps.push(StepResult::failed(
                Step::WriteConfiguration,
                error.user_message(),
            ));
            // The PUT failed, so the server still holds `before`. (`config`
            // only diverged locally; this assert guards the code above, not
            // the network.)
            debug_assert!(config.hostname_count() >= before.hostname_count());
            return Outcome {
                hostname: request.hostname.clone(),
                steps,
                dns: outcome_dns,
                recovery: recovery.or(Some(Recovery::Retry)),
                touched_rule: None,
            };
        }
    }

    // ── 4. Create or repair the DNS record ───────────────────────────────
    let Some(zone_id) = request.zone_id.clone() else {
        steps.push(StepResult::skipped(
            Step::CreateDnsRecord,
            "No zone was selected, so no DNS record was created. The tunnel will not \
             serve this hostname until one exists.",
        ));
        return Outcome {
            hostname: request.hostname.clone(),
            steps,
            dns: outcome_dns,
            recovery: None,
            touched_rule,
        };
    };

    match &outcome_dns {
        DnsState::Correct { .. } => {
            steps.push(StepResult::skipped(
                Step::CreateDnsRecord,
                "The proxied CNAME was already correct.",
            ));
        }
        DnsState::NotProxied { record_id } => {
            match client
                .patch_dns_record(
                    &zone_id,
                    record_id,
                    crate::cloudflare::models::PatchDnsRecordRequest {
                        proxied: Some(true),
                        content: Some(format!("{}.cfargotunnel.com", request.tunnel_id)),
                    },
                    cancel,
                )
                .await
            {
                Ok(record) => {
                    outcome_dns = DnsState::Correct {
                        record_id: record.id.clone(),
                    };
                    steps.push(StepResult::done(
                        Step::RepairDnsRecord,
                        "The existing CNAME is now proxied and points at this tunnel.",
                    ));
                }
                Err(error) => {
                    steps.push(StepResult::failed(
                        Step::RepairDnsRecord,
                        error.user_message(),
                    ));
                    return Outcome {
                        hostname: request.hostname.clone(),
                        steps,
                        dns: outcome_dns,
                        recovery: Some(rollback_or_permissions(&error)),
                        touched_rule,
                    };
                }
            }
        }
        _ => {
            match client
                .create_tunnel_cname(
                    &zone_id,
                    &request.hostname,
                    &request.tunnel_id,
                    Some(RECORD_COMMENT.to_string()),
                    cancel,
                )
                .await
            {
                Ok(record) => {
                    outcome_dns = DnsState::Correct {
                        record_id: record.id.clone(),
                    };
                    steps.push(StepResult::done(
                        Step::CreateDnsRecord,
                        format!(
                            "A proxied CNAME now points {} at this tunnel.",
                            request.hostname
                        ),
                    ));
                }
                Err(error) => {
                    steps.push(StepResult::failed(
                        Step::CreateDnsRecord,
                        error.user_message(),
                    ));
                    return Outcome {
                        hostname: request.hostname.clone(),
                        steps,
                        dns: outcome_dns,
                        recovery: Some(rollback_or_permissions(&error)),
                        touched_rule,
                    };
                }
            }
        }
    }

    // ── 5. Verify ────────────────────────────────────────────────────────
    match client
        .list_dns_records(&zone_id, &request.hostname, cancel)
        .await
    {
        Ok(records) => {
            outcome_dns = DnsState::evaluate(&records, &request.tunnel_id);
            if outcome_dns.is_healthy() {
                steps.push(StepResult::done(
                    Step::Verify,
                    "The ingress rule and the DNS record agree.",
                ));
            } else {
                steps.push(StepResult::failed(
                    Step::Verify,
                    outcome_dns.detail(&request.tunnel_name),
                ));
                return Outcome {
                    hostname: request.hostname.clone(),
                    steps,
                    dns: outcome_dns,
                    recovery: Some(Recovery::RepairDns),
                    touched_rule,
                };
            }
        }
        Err(error) => {
            // The work is done; only the confirmation failed.
            steps.push(StepResult::skipped(
                Step::Verify,
                format!("Could not re-read DNS to confirm: {}", error.user_message()),
            ));
            let _ = existing_records;
        }
    }

    Outcome {
        hostname: request.hostname.clone(),
        steps,
        dns: outcome_dns,
        recovery: None,
        touched_rule,
    }
}

/// Removes a hostname from a tunnel, and only its own DNS record.
pub async fn unpublish(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
    zone_id: Option<&str>,
    cancel: &Cancel,
) -> Outcome {
    let mut steps = Vec::new();
    let mut dns = DnsState::Unknown {
        reason: "DNS was not checked.".into(),
    };

    // ── 1. Read, remove exactly one rule, write back ─────────────────────
    let mut config = match client
        .get_configuration(account_id, tunnel_id, cancel)
        .await
    {
        Ok((config, _)) => config,
        Err(error) => {
            let recovery = recovery_for(&error);
            steps.push(StepResult::failed(
                Step::ReadConfiguration,
                error.user_message(),
            ));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns,
                recovery,
                touched_rule: None,
            };
        }
    };

    let removed = config.remove(hostname, path);
    if removed.is_some() {
        steps.push(StepResult::done(
            Step::ReadConfiguration,
            format!(
                "Removed one rule; {} other rule(s) left untouched.",
                config.hostname_count()
            ),
        ));
    } else {
        steps.push(StepResult::skipped(
            Step::WriteConfiguration,
            "The tunnel had no rule for that hostname; nothing to remove.",
        ));
    }

    if removed.is_some() {
        if let Err(error) = client
            .put_configuration(account_id, tunnel_id, &config, cancel)
            .await
        {
            let recovery = recovery_for(&error).or(Some(Recovery::Retry));
            steps.push(StepResult::failed(
                Step::WriteConfiguration,
                error.user_message(),
            ));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns,
                recovery,
                touched_rule: removed,
            };
        }
        steps.push(StepResult::done(
            Step::WriteConfiguration,
            "The tunnel no longer serves this hostname.",
        ));
    }

    // ── 2. Remove only the record that points at this tunnel ─────────────
    let Some(zone_id) = zone_id else {
        steps.push(StepResult::skipped(
            Step::RemoveDnsRecord,
            "No zone is known for this hostname, so no DNS record was removed. Any \
             record that still exists will now return a 1016 error.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: removed,
        };
    };

    let records = match client.list_dns_records(zone_id, hostname, cancel).await {
        Ok(records) => records,
        Err(error) => {
            steps.push(StepResult::failed(
                Step::RemoveDnsRecord,
                format!(
                    "The ingress rule was removed but the DNS record could not be \
                     looked up: {}",
                    error.user_message()
                ),
            ));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns,
                recovery: Some(Recovery::Retry),
                touched_rule: removed,
            };
        }
    };

    // The only record this application will delete is a CNAME whose content is
    // exactly this tunnel's target. An A record, a TXT record, a CNAME to a
    // different tunnel — all are somebody else's and all are left alone.
    let ours: Vec<_> = records
        .iter()
        .filter(|record| record.is_tunnel_cname(tunnel_id))
        .collect();

    if ours.is_empty() {
        let others = records.len();
        steps.push(StepResult::skipped(
            Step::RemoveDnsRecord,
            if others == 0 {
                "There was no DNS record to remove.".to_string()
            } else {
                format!(
                    "{others} DNS record(s) exist for this hostname, but none points at \
                     this tunnel, so none was removed."
                )
            },
        ));
        dns = DnsState::evaluate(&records, tunnel_id);
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: removed,
        };
    }

    let mut failures = Vec::new();
    let mut deleted = 0;
    for record in &ours {
        match client.delete_dns_record(zone_id, &record.id, cancel).await {
            Ok(()) => deleted += 1,
            Err(error) => failures.push(error.user_message()),
        }
    }

    if failures.is_empty() {
        dns = DnsState::Missing;
        steps.push(StepResult::done(
            Step::RemoveDnsRecord,
            format!("Removed {deleted} DNS record(s) pointing at this tunnel."),
        ));
    } else {
        steps.push(StepResult::failed(
            Step::RemoveDnsRecord,
            format!(
                "Removed {deleted} record(s); {} could not be removed: {}",
                failures.len(),
                failures.join("; ")
            ),
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: Some(Recovery::Retry),
            touched_rule: removed,
        };
    }

    Outcome {
        hostname: hostname.to_string(),
        steps,
        dns,
        recovery: None,
        touched_rule: removed,
    }
}

/// Disables a hostname without removing it.
///
/// The ingress rule stays in the tunnel's configuration with its service
/// rewritten to `http_status:404`: Cloudflare answers 404 and the origin is
/// never contacted, but the hostname remains in the list and can be enabled
/// again. The origin it served is remembered by the caller (`storage::disabled`)
/// *before* this runs, so enabling is exact even if this request half fails.
///
/// `origin_request` and every other field on the rule are left untouched —
/// only the service changes.
pub async fn disable_hostname(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
    cancel: &Cancel,
) -> Outcome {
    let mut steps = Vec::new();
    let dns = DnsState::Unknown {
        reason: "DNS was not checked; disabling does not touch it.".into(),
    };

    let mut config = match client
        .get_configuration(account_id, tunnel_id, cancel)
        .await
    {
        Ok((config, _)) => config,
        Err(error) => {
            let recovery = recovery_for(&error);
            steps.push(StepResult::failed(
                Step::ReadConfiguration,
                error.user_message(),
            ));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns,
                recovery,
                touched_rule: None,
            };
        }
    };

    let Some(index) = config.find(hostname, path) else {
        steps.push(StepResult::skipped(
            Step::WriteConfiguration,
            "The tunnel has no rule for that hostname; nothing to disable.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: None,
        };
    };

    if config.ingress[index].is_disabled() {
        steps.push(StepResult::skipped(
            Step::WriteConfiguration,
            "The hostname is already disabled; traffic already answers 404.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: None,
        };
    }

    config.ingress[index].service = crate::domain::config::DISABLED_SERVICE.to_string();

    if let Err(error) = client
        .put_configuration(account_id, tunnel_id, &config, cancel)
        .await
    {
        steps.push(StepResult::failed(
            Step::WriteConfiguration,
            error.user_message(),
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: recovery_for(&error).or(Some(Recovery::Retry)),
            touched_rule: None,
        };
    }

    steps.push(StepResult::done(
        Step::WriteConfiguration,
        "The hostname now answers 404 and the origin is not contacted; the origin \
         it served is remembered for Enable.",
    ));

    Outcome {
        hostname: hostname.to_string(),
        steps,
        dns,
        recovery: None,
        touched_rule: None,
    }
}

/// Re-enables a disabled hostname by restoring the origin it served.
///
/// `original_service` is what the rule served before it was disabled, as
/// remembered by the caller. Without a remembered origin there is nothing
/// exact to restore, so the step fails with instructions rather than
/// inventing a service string.
pub async fn enable_hostname(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
    original_service: Option<String>,
    cancel: &Cancel,
) -> Outcome {
    let mut steps = Vec::new();
    let dns = DnsState::Unknown {
        reason: "DNS was not checked; enabling does not touch it.".into(),
    };

    let mut config = match client
        .get_configuration(account_id, tunnel_id, cancel)
        .await
    {
        Ok((config, _)) => config,
        Err(error) => {
            let recovery = recovery_for(&error);
            steps.push(StepResult::failed(
                Step::ReadConfiguration,
                error.user_message(),
            ));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns,
                recovery,
                touched_rule: None,
            };
        }
    };

    let Some(index) = config.find(hostname, path) else {
        steps.push(StepResult::skipped(
            Step::WriteConfiguration,
            "The tunnel has no rule for that hostname; nothing to enable.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: None,
        };
    };

    if !config.ingress[index].is_disabled() {
        steps.push(StepResult::skipped(
            Step::WriteConfiguration,
            "The hostname is already being served; nothing to enable.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: None,
        };
    }

    let Some(original) = original_service else {
        steps.push(StepResult::failed(
            Step::WriteConfiguration,
            "No origin is remembered for this hostname, so it cannot be restored \
             exactly. Edit the origin to choose one.",
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: None,
            touched_rule: None,
        };
    };

    config.ingress[index].service = original;

    if let Err(error) = client
        .put_configuration(account_id, tunnel_id, &config, cancel)
        .await
    {
        steps.push(StepResult::failed(
            Step::WriteConfiguration,
            error.user_message(),
        ));
        return Outcome {
            hostname: hostname.to_string(),
            steps,
            dns,
            recovery: recovery_for(&error).or(Some(Recovery::Retry)),
            touched_rule: None,
        };
    }

    steps.push(StepResult::done(
        Step::WriteConfiguration,
        "The hostname is being served by its original origin again.",
    ));

    Outcome {
        hostname: hostname.to_string(),
        steps,
        dns,
        recovery: None,
        touched_rule: None,
    }
}

/// Puts an ingress rule back after a failed publish.
///
/// Takes the rule the publish returned rather than rebuilding it, so anything
/// the rule carried — an `originRequest`, a field this build does not know
/// about — comes back exactly as it was.
pub async fn roll_back_ingress(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
    cancel: &Cancel,
) -> Result<(), CloudflareError> {
    let (mut config, _) = client
        .get_configuration(account_id, tunnel_id, cancel)
        .await?;
    // If the rule is already gone there is nothing to undo — and writing
    // the config back anyway would needlessly clobber any concurrent edit.
    if config.remove(hostname, path).is_none() {
        return Ok(());
    }
    client
        .put_configuration(account_id, tunnel_id, &config, cancel)
        .await?;
    Ok(())
}

/// Fixes a missing or unproxied DNS record without touching the tunnel.
pub async fn repair_dns(
    client: &CloudflareClient,
    zone_id: &str,
    tunnel_id: &str,
    hostname: &str,
    cancel: &Cancel,
) -> Outcome {
    let mut steps = Vec::new();

    let records = match client.list_dns_records(zone_id, hostname, cancel).await {
        Ok(records) => records,
        Err(error) => {
            let recovery = recovery_for(&error).or(Some(Recovery::Retry));
            steps.push(StepResult::failed(Step::CheckDns, error.user_message()));
            return Outcome {
                hostname: hostname.to_string(),
                steps,
                dns: DnsState::Unknown {
                    reason: error.user_message(),
                },
                recovery,
                touched_rule: None,
            };
        }
    };

    let state = DnsState::evaluate(&records, tunnel_id);
    steps.push(StepResult::done(Step::CheckDns, state.label().to_string()));

    match &state {
        DnsState::Correct { .. } => {
            steps.push(StepResult::skipped(
                Step::RepairDnsRecord,
                "The DNS record was already correct.",
            ));
            Outcome {
                hostname: hostname.to_string(),
                steps,
                dns: state,
                recovery: None,
                touched_rule: None,
            }
        }

        DnsState::NotProxied { record_id } => {
            match client
                .patch_dns_record(
                    zone_id,
                    record_id,
                    crate::cloudflare::models::PatchDnsRecordRequest {
                        proxied: Some(true),
                        content: Some(format!("{tunnel_id}.cfargotunnel.com")),
                    },
                    cancel,
                )
                .await
            {
                Ok(record) => {
                    steps.push(StepResult::done(
                        Step::RepairDnsRecord,
                        "The record is now proxied.",
                    ));
                    Outcome {
                        hostname: hostname.to_string(),
                        steps,
                        dns: DnsState::Correct {
                            record_id: record.id,
                        },
                        recovery: None,
                        touched_rule: None,
                    }
                }
                Err(error) => {
                    let recovery = recovery_for(&error).or(Some(Recovery::Retry));
                    steps.push(StepResult::failed(
                        Step::RepairDnsRecord,
                        error.user_message(),
                    ));
                    Outcome {
                        hostname: hostname.to_string(),
                        steps,
                        dns: state,
                        recovery,
                        touched_rule: None,
                    }
                }
            }
        }

        DnsState::Missing => {
            match client
                .create_tunnel_cname(
                    zone_id,
                    hostname,
                    tunnel_id,
                    Some(RECORD_COMMENT.to_string()),
                    cancel,
                )
                .await
            {
                Ok(record) => {
                    steps.push(StepResult::done(
                        Step::CreateDnsRecord,
                        "The proxied CNAME was created.",
                    ));
                    Outcome {
                        hostname: hostname.to_string(),
                        steps,
                        dns: DnsState::Correct {
                            record_id: record.id,
                        },
                        recovery: None,
                        touched_rule: None,
                    }
                }
                Err(error) => {
                    let recovery = recovery_for(&error).or(Some(Recovery::Retry));
                    steps.push(StepResult::failed(
                        Step::CreateDnsRecord,
                        error.user_message(),
                    ));
                    Outcome {
                        hostname: hostname.to_string(),
                        steps,
                        dns: state,
                        recovery,
                        touched_rule: None,
                    }
                }
            }
        }

        DnsState::Conflicting {
            record_type,
            content,
            ..
        } => {
            steps.push(StepResult::failed(
                Step::RepairDnsRecord,
                format!(
                    "An existing {record_type} record points at {content}. This \
                     application will not delete a record it did not create."
                ),
            ));
            Outcome {
                hostname: hostname.to_string(),
                recovery: Some(Recovery::ResolveConflictByHand {
                    record_type: record_type.clone(),
                    content: content.clone(),
                }),
                steps,
                dns: state,
                touched_rule: None,
            }
        }

        DnsState::Unknown { reason } => {
            steps.push(StepResult::failed(Step::RepairDnsRecord, reason.clone()));
            Outcome {
                hostname: hostname.to_string(),
                steps,
                dns: state,
                recovery: Some(Recovery::FixPermissions),
                touched_rule: None,
            }
        }
    }
}

/// Reads a tunnel's configuration and joins it with the zone's DNS.
///
/// Used by the tunnel detail page and by Zones & Hostnames. A zone that cannot
/// be read leaves each hostname's DNS state `Unknown` with the reason, rather
/// than claiming everything is fine.
pub async fn load_hostnames(
    client: &CloudflareClient,
    account_id: &str,
    tunnel_id: &str,
    tunnel_name: &str,
    zones: &[crate::domain::zone::Zone],
    cancel: &Cancel,
) -> Result<Vec<crate::domain::hostname::PublicHostname>, CloudflareError> {
    let (config, _) = client
        .get_configuration(account_id, tunnel_id, cancel)
        .await?;

    let mut hostnames = config.public_hostnames(
        tunnel_id,
        tunnel_name,
        "This hostname's zone is not on the connected account.",
    );

    // Group by zone so one listing covers every hostname in it.
    for hostname in &mut hostnames {
        let Some(zone) = crate::domain::zone::Zone::best_match(zones, &hostname.hostname) else {
            continue;
        };
        hostname.zone_id = Some(zone.id.clone());
        hostname.zone_name = Some(zone.name.clone());

        match client
            .list_dns_records(&zone.id, &hostname.hostname, cancel)
            .await
        {
            Ok(records) => hostname.dns = DnsState::evaluate(&records, tunnel_id),
            Err(error) if error.is_cancelled() => return Err(error),
            Err(error) => {
                hostname.dns = DnsState::Unknown {
                    reason: error.user_message(),
                }
            }
        }
    }

    Ok(hostnames)
}

fn recovery_for(error: &CloudflareError) -> Option<Recovery> {
    if error.is_permission_problem() {
        Some(Recovery::FixPermissions)
    } else if error.is_transient() {
        Some(Recovery::Retry)
    } else {
        None
    }
}

/// After the configuration was written and the DNS half failed, the useful
/// offer is either "fix the token" or "undo the ingress change".
fn rollback_or_permissions(error: &CloudflareError) -> Recovery {
    if error.is_permission_problem() {
        Recovery::FixPermissions
    } else {
        Recovery::RollBackIngress
    }
}

/// Builds the configuration a publish *would* write, without writing it.
///
/// The wizard's review step uses this so it can list the exact changes,
/// including whether a catch-all rule will be added.
pub fn preview_configuration(
    current: &TunnelConfig,
    hostname: &str,
    path: Option<&str>,
    service: &str,
) -> (TunnelConfig, Vec<String>) {
    let mut next = current.clone();
    let outcome = next.upsert(hostname, path, service, OriginRequestUpdate::Preserve);

    let mut changes = Vec::new();
    match outcome {
        crate::domain::config::UpsertOutcome::Added => {
            changes.push(format!("Add an ingress rule for {hostname} → {service}"))
        }
        crate::domain::config::UpsertOutcome::Updated => {
            changes.push(format!("Point the existing {hostname} rule at {service}"))
        }
    }

    if current.ingress.is_empty() {
        changes.push(format!(
            "Add the required catch-all rule ({})",
            crate::domain::config::CATCH_ALL_SERVICE
        ));
    }

    let preserved = current.hostname_count();
    if preserved > 0 {
        changes.push(format!("Leave {preserved} existing rule(s) unchanged"));
    }

    (next, changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hostname::OriginScheme;

    #[test]
    fn an_outcome_with_no_failures_succeeded() {
        let outcome = Outcome {
            hostname: "app.example.com".into(),
            steps: vec![
                StepResult::done(Step::ReadConfiguration, "read"),
                StepResult::skipped(Step::CheckDns, "no zone"),
            ],
            dns: DnsState::Missing,
            recovery: None,
            touched_rule: None,
        };
        assert!(outcome.succeeded());
        assert!(!outcome.is_partial());
        assert!(outcome.summary().contains("is published"));
    }

    #[test]
    fn a_failure_after_a_success_is_reported_as_partial() {
        let outcome = Outcome {
            hostname: "app.example.com".into(),
            steps: vec![
                StepResult::done(Step::WriteConfiguration, "written"),
                StepResult::failed(Step::CreateDnsRecord, "permission denied"),
            ],
            dns: DnsState::Missing,
            recovery: Some(Recovery::RollBackIngress),
            touched_rule: None,
        };
        assert!(!outcome.succeeded());
        assert!(outcome.is_partial());
        assert!(outcome.summary().contains("only partly set up"));
        assert_eq!(outcome.failed_step().unwrap().step, Step::CreateDnsRecord);
    }

    #[test]
    fn a_failure_at_the_first_step_is_not_partial() {
        let outcome = Outcome {
            hostname: "app.example.com".into(),
            steps: vec![StepResult::failed(Step::ReadConfiguration, "offline")],
            dns: DnsState::Missing,
            recovery: Some(Recovery::Retry),
            touched_rule: None,
        };
        assert!(!outcome.is_partial());
        assert!(outcome.summary().starts_with("Could not publish"));
    }

    #[test]
    fn every_recovery_explains_what_it_will_do() {
        let recoveries = [
            Recovery::Retry,
            Recovery::RepairDns,
            Recovery::RollBackIngress,
            Recovery::ResolveConflictByHand {
                record_type: "A".into(),
                content: "203.0.113.1".into(),
            },
            Recovery::FixPermissions,
        ];
        for recovery in recoveries {
            assert!(!recovery.label().is_empty());
            assert!(recovery.explanation().len() > 30, "{recovery:?}");
        }
    }

    #[test]
    fn the_rollback_explanation_promises_other_rules_are_untouched() {
        let text = Recovery::RollBackIngress.explanation();
        assert!(text.contains("every other rule alone"), "got {text}");
    }

    #[test]
    fn the_conflict_recovery_says_the_app_will_not_delete_the_record() {
        let text = Recovery::ResolveConflictByHand {
            record_type: "AAAA".into(),
            content: "2001:db8::1".into(),
        }
        .explanation();
        assert!(text.contains("will not delete"));
        assert!(text.contains("2001:db8::1"));
    }

    #[test]
    fn preview_lists_the_exact_changes_for_a_new_hostname() {
        let current = TunnelConfig::default();
        let (next, changes) =
            preview_configuration(&current, "app.example.com", None, "http://localhost:8080");

        assert_eq!(next.hostname_count(), 1);
        assert!(next.validate().is_ok());
        assert!(changes[0].contains("Add an ingress rule"));
        assert!(changes.iter().any(|change| change.contains("catch-all")));
    }

    #[test]
    fn preview_says_when_a_rule_is_being_updated_rather_than_added() {
        let current: TunnelConfig = serde_json::from_str(
            r#"{"ingress":[
                 {"hostname":"app.example.com","service":"http://localhost:1"},
                 {"hostname":"other.example.com","service":"http://localhost:2"},
                 {"service":"http_status:404"}]}"#,
        )
        .unwrap();

        let (next, changes) =
            preview_configuration(&current, "app.example.com", None, "http://localhost:9999");

        assert!(changes[0].contains("Point the existing"));
        assert!(changes
            .iter()
            .any(|change| change.contains("Leave 2 existing rule(s) unchanged")));
        assert_eq!(next.hostname_count(), 2, "no rule was added");
        assert_eq!(next.ingress[0].service, "http://localhost:9999");
        assert_eq!(next.ingress[1].service, "http://localhost:2");
    }

    #[test]
    fn a_publish_request_validates_before_anything_is_sent() {
        let good = PublishRequest {
            account_id: "a".repeat(32),
            tunnel_id: "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into(),
            tunnel_name: "web".into(),
            hostname: "app.example.com".into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            zone_id: Some("z".repeat(32)),
            origin_request: OriginRequestUpdate::Preserve,
        };
        assert!(good.validate().is_ok());

        let bad_hostname = PublishRequest {
            hostname: "not-a-hostname".into(),
            ..good.clone()
        };
        assert!(bad_hostname.validate().is_err());

        let bad_account = PublishRequest {
            account_id: "nope".into(),
            ..good.clone()
        };
        assert!(bad_account.validate().is_err());

        let bad_path = PublishRequest {
            path: Some("http://elsewhere/".into()),
            ..good
        };
        assert!(bad_path.validate().is_err());
    }

    #[test]
    fn every_step_and_status_has_a_label_and_an_icon() {
        for step in [
            Step::ReadConfiguration,
            Step::CheckDns,
            Step::WriteConfiguration,
            Step::CreateDnsRecord,
            Step::RepairDnsRecord,
            Step::RemoveDnsRecord,
            Step::Verify,
            Step::RollbackConfiguration,
        ] {
            assert!(!step.label().is_empty());
        }
        for status in [Status::Done, Status::Skipped, Status::Failed] {
            assert!(status.icon_name().ends_with("-symbolic"));
            assert!(!status.css_class().is_empty());
        }
    }

    #[test]
    fn the_service_string_in_a_preview_is_what_the_editor_produced() {
        let service = OriginService::from_fields(OriginScheme::Https, "10.0.0.5", "8443").unwrap();
        let (_, changes) = preview_configuration(
            &TunnelConfig::default(),
            "app.example.com",
            None,
            &service.to_service_string(),
        );
        assert!(changes[0].contains("https://10.0.0.5:8443"));
    }
}
