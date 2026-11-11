// Human wording and semantic classes for every enum the backend sends.
//
// Each status carries three things: a label, an icon name (already in the
// GNOME symbolic vocabulary the Rust side uses) and a semantic class. No
// status is ever communicated by colour alone.

import type {
  ActivityCategory,
  ActivitySeverity,
  ConfigSource,
  DnsState,
  LocalState,
  OriginScheme,
  PermissionState,
  Recovery,
  RemoteServiceState,
  RemoteStatus,
  ServiceAction,
  Step,
  StepStatus,
  TunnelKind,
  TunnelStatus,
  ZoneStatus,
} from "./types";

export type Semantic = "success" | "warning" | "error" | "neutral" | "accent";

export function tunnelStatus(status: TunnelStatus): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (status.kind) {
    case "healthy":
      return { label: "Healthy", tone: "success", icon: "object-select-symbolic" };
    case "degraded":
      return { label: "Degraded", tone: "warning", icon: "dialog-warning-symbolic" };
    case "down":
      return { label: "Down", tone: "error", icon: "dialog-error-symbolic" };
    case "inactive":
      return { label: "Inactive", tone: "neutral", icon: "media-playback-pause-symbolic" };
    case "other":
      return { label: status.data, tone: "neutral", icon: "dialog-question-symbolic" };
  }
}

export function tunnelExplanation(status: TunnelStatus): string {
  switch (status.kind) {
    case "healthy":
      return "All registered connectors have every connection established.";
    case "degraded":
      return "At least one connector is registered but is missing some of its connections to the Cloudflare edge.";
    case "down":
      return "Connectors were registered for this tunnel and are now unreachable.";
    case "inactive":
      return "No connector is registered. Nothing is serving this tunnel yet.";
    case "other":
      return "Cloudflare reported a status this version does not recognise.";
  }
}

/** The most recent of the two connector timestamps, as an ISO string. */
export function tunnelLastActive(tunnel: import("./types").Tunnel): string | null {
  if (tunnel.conns_active_at && tunnel.conns_inactive_at) {
    return tunnel.conns_active_at > tunnel.conns_inactive_at
      ? tunnel.conns_active_at
      : tunnel.conns_inactive_at;
  }
  return tunnel.conns_active_at ?? tunnel.conns_inactive_at ?? null;
}

export function configSource(source: ConfigSource): string {
  switch (source.kind) {
    case "cloudflare":
      return "Remotely managed";
    case "local":
      return "Locally configured";
    case "other":
      return source.data;
  }
}

export function tunnelKind(kind: TunnelKind): string {
  switch (kind.kind) {
    case "cfd":
      return "Cloudflare Tunnel";
    case "warp":
      return "WARP connector";
    case "magic":
      return "Magic WAN";
    case "other":
      return kind.data;
  }
}

export function dnsState(state: DnsState): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (state.kind) {
    case "correct":
      return { label: "DNS ready", tone: "success", icon: "object-select-symbolic" };
    case "not_proxied":
      return { label: "Not proxied", tone: "warning", icon: "dialog-warning-symbolic" };
    case "missing":
      return { label: "No DNS record", tone: "warning", icon: "list-add-symbolic" };
    case "conflicting":
      return { label: "Conflicting record", tone: "error", icon: "dialog-error-symbolic" };
    case "unknown":
      return { label: "DNS not checked", tone: "neutral", icon: "dialog-question-symbolic" };
  }
}

export function dnsDetail(state: DnsState, tunnelName: string): string {
  switch (state.kind) {
    case "correct":
      return `A proxied CNAME points this hostname at “${tunnelName}”.`;
    case "not_proxied":
      return "The CNAME exists but is not proxied, so requests never reach the tunnel. Repairing turns the proxy on.";
    case "missing":
      return "No DNS record points this hostname anywhere. Repairing creates the proxied CNAME.";
    case "conflicting":
      return `An existing ${state.data.record_type} record points at ${state.data.content}. It has to be removed or changed before the tunnel can serve this hostname.`;
    case "unknown":
      return state.data.reason;
  }
}

export function dnsIsRepairable(state: DnsState): boolean {
  return state.kind === "missing" || state.kind === "not_proxied";
}

export function zoneStatus(status: ZoneStatus): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (status.kind) {
    case "active":
      return { label: "Active", tone: "success", icon: "object-select-symbolic" };
    case "pending":
      return { label: "Pending nameservers", tone: "warning", icon: "content-loading-symbolic" };
    case "initializing":
      return { label: "Initialising", tone: "warning", icon: "content-loading-symbolic" };
    case "moved":
      return { label: "Moved", tone: "error", icon: "action-unavailable-symbolic" };
    case "deactivated":
      return { label: "Deactivated", tone: "error", icon: "action-unavailable-symbolic" };
    case "other":
      return { label: status.data, tone: "neutral", icon: "dialog-question-symbolic" };
  }
}

export function zoneCaveat(status: ZoneStatus): string | null {
  switch (status.kind) {
    case "active":
      return null;
    case "pending":
      return "This zone is not using Cloudflare's nameservers yet, so a record created here will not resolve until it is.";
    case "initializing":
      return "This zone is still being set up.";
    case "moved":
      return "This zone has been moved away from this account.";
    case "deactivated":
      return "This zone is deactivated and will not serve traffic.";
    case "other":
      return "This zone reports a status this version does not recognise.";
  }
}

export function serviceState(state: RemoteServiceState): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (state.kind) {
    case "active":
      return { label: "Running", tone: "success", icon: "media-playback-start-symbolic" };
    case "inactive":
      return { label: "Stopped", tone: "neutral", icon: "media-playback-stop-symbolic" };
    case "failed":
      return { label: "Failed", tone: "error", icon: "dialog-error-symbolic" };
    case "not_installed":
      return { label: "Not installed", tone: "warning", icon: "package-x-generic-symbolic" };
    case "other":
      return { label: state.data, tone: "neutral", icon: "dialog-question-symbolic" };
  }
}

export function remoteStatus(status: RemoteStatus): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (status.kind) {
    case "unknown":
      return { label: "Not checked", tone: "neutral", icon: "dialog-question-symbolic" };
    case "checking":
      return { label: "Checking…", tone: "neutral", icon: "content-loading-symbolic" };
    case "reachable":
      return serviceState(status.data.service);
    case "unreachable":
      return { label: "Unreachable", tone: "error", icon: "network-offline-symbolic" };
  }
}

export function remoteStatusDetail(status: RemoteStatus): string | null {
  if (status.kind === "unreachable") return status.data.message;
  if (status.kind === "reachable" && status.data.cloudflared_version) {
    return `cloudflared ${status.data.cloudflared_version}`;
  }
  return null;
}

/** The single most useful thing to say about local control, or `null`. */
export function localBlockingReason(local: LocalState): string | null {
  if (!local.systemctl_available) {
    return "systemctl was not found. Local control needs a Linux system with systemd.";
  }
  if (!local.systemd_available) {
    return "This machine is not running systemd, so the cloudflared service cannot be controlled from here.";
  }
  if (!local.cloudflared_version) {
    return "cloudflared is not installed on this machine. Install it from https://pkg.cloudflare.com first.";
  }
  if (!local.unit_present) {
    return "The service unit does not exist on this machine yet. Install the connector first.";
  }
  return null;
}

export function localCanControl(local: LocalState | null): boolean {
  return Boolean(
    local?.systemctl_available && local?.systemd_available && local?.unit_present,
  );
}

export function permissionState(state: PermissionState): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (state) {
    case "unknown":
      return { label: "Not checked", tone: "neutral", icon: "dialog-question-symbolic" };
    case "granted":
      return { label: "Granted", tone: "success", icon: "object-select-symbolic" };
    case "denied":
      return { label: "Missing", tone: "error", icon: "dialog-error-symbolic" };
  }
}

export function severity(severity: ActivitySeverity): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (severity) {
    case "info":
      return { label: "Info", tone: "neutral", icon: "dialog-information-symbolic" };
    case "success":
      return { label: "Success", tone: "success", icon: "object-select-symbolic" };
    case "warning":
      return { label: "Warning", tone: "warning", icon: "dialog-warning-symbolic" };
    case "error":
      return { label: "Error", tone: "error", icon: "dialog-error-symbolic" };
  }
}

export function category(category: ActivityCategory): string {
  switch (category) {
    case "api":
      return "Cloudflare API";
    case "dns":
      return "DNS";
    case "tunnel_config":
      return "Tunnel configuration";
    case "tunnel":
      return "Tunnel";
    case "ssh":
      return "SSH";
    case "remote_service":
      return "Remote service";
    case "app":
      return "Application";
  }
}

export function stepLabel(step: Step): string {
  switch (step) {
    case "read_configuration":
      return "Read the tunnel's configuration";
    case "check_dns":
      return "Check for conflicting DNS records";
    case "write_configuration":
      return "Update the ingress configuration";
    case "create_dns_record":
      return "Create the proxied CNAME";
    case "repair_dns_record":
      return "Repair the DNS record";
    case "remove_dns_record":
      return "Remove the DNS record";
    case "verify":
      return "Verify the result";
    case "rollback_configuration":
      return "Undo the configuration change";
  }
}

export function stepStatus(status: StepStatus): {
  label: string;
  tone: Semantic;
  icon: string;
} {
  switch (status) {
    case "done":
      return { label: "Done", tone: "success", icon: "object-select-symbolic" };
    case "skipped":
      return { label: "Skipped", tone: "neutral", icon: "media-skip-forward-symbolic" };
    case "failed":
      return { label: "Failed", tone: "error", icon: "dialog-error-symbolic" };
  }
}

/** `Outcome::succeeded()` on the Rust side, recomputed from the steps. */
export function outcomeSucceeded(outcome: {
  steps: { status: StepStatus }[];
}): boolean {
  return outcome.steps.every((step) => step.status !== "failed");
}

/** `Outcome::is_partial()`: some steps worked and a later one did not. */
export function outcomeIsPartial(outcome: {
  steps: { status: StepStatus }[];
}): boolean {
  return (
    !outcomeSucceeded(outcome) &&
    outcome.steps.some((step) => step.status === "done")
  );
}

export function recoveryLabel(recovery: Recovery): string {
  switch (recovery.kind) {
    case "retry":
      return "Try Again";
    case "repair_dns":
      return "Repair DNS";
    case "roll_back_ingress":
      return "Undo the Ingress Change";
    case "resolve_conflict_by_hand":
      return "Open Cloudflare Dashboard";
    case "fix_permissions":
      return "Check the API Token";
  }
}

export function recoveryExplanation(recovery: Recovery): string {
  switch (recovery.kind) {
    case "retry":
      return "Nothing was changed, so this is safe to try again.";
    case "repair_dns":
      return "The tunnel now serves this hostname, but no DNS record points at it. Repairing creates the record and changes nothing else.";
    case "roll_back_ingress":
      return "The tunnel was updated but the DNS record was not created. Undoing removes the rule that was added and leaves every other rule alone.";
    case "resolve_conflict_by_hand":
      return `An existing ${recovery.data.record_type} record points at ${recovery.data.content}. This application will not delete a record it did not create — remove or change it in the Cloudflare dashboard, then publish again.`;
    case "fix_permissions":
      return "The API token is missing a permission this operation needs. Check it on the Connection page.";
  }
}

export function actionLabel(action: ServiceAction): string {
  switch (action) {
    case "start":
      return "Start";
    case "stop":
      return "Stop";
    case "restart":
      return "Restart";
  }
}

export function actionVerb(action: ServiceAction): string {
  switch (action) {
    case "start":
      return "start";
    case "stop":
      return "stop";
    case "restart":
      return "restart";
  }
}

/** `ServiceOutcome::succeeded()`: did the unit reach the state asked for? */
export function serviceOutcomeSucceeded(outcome: {
  action: ServiceAction;
  state_after: RemoteServiceState;
}): boolean {
  const active = outcome.state_after.kind === "active";
  return outcome.action === "stop" ? !active : active;
}

export function originSchemeLabel(scheme: OriginScheme): string {
  switch (scheme) {
    case "http":
      return "HTTP";
    case "https":
      return "HTTPS";
    case "tcp":
      return "TCP";
    case "ssh":
      return "SSH";
    case "rdp":
      return "RDP";
    case "unix":
      return "Unix socket";
    case "unix_tls":
      return "Unix socket (TLS)";
    case "smb":
      return "SMB";
    case "http_status":
      return "Fixed HTTP status";
    case "bastion":
      return "Bastion";
    case "hello_world":
      return "Built-in test page";
  }
}

/** The prefix an ingress `service` string uses for a scheme. */
export function originPrefix(scheme: OriginScheme): string {
  switch (scheme) {
    case "http":
      return "http://";
    case "https":
      return "https://";
    case "tcp":
      return "tcp://";
    case "ssh":
      return "ssh://";
    case "rdp":
      return "rdp://";
    case "unix":
      return "unix:";
    case "unix_tls":
      return "unix+tls:";
    case "smb":
      return "smb://";
    case "http_status":
      return "http_status:";
    case "bastion":
      return "bastion";
    case "hello_world":
      return "hello_world";
  }
}

export const SELECTABLE_SCHEMES: OriginScheme[] = [
  "http",
  "https",
  "tcp",
  "ssh",
  "rdp",
  "smb",
  "unix",
  "unix_tls",
  "http_status",
];

export function defaultPort(scheme: OriginScheme): number | null {
  switch (scheme) {
    case "http":
      return 80;
    case "https":
      return 443;
    case "ssh":
      return 22;
    case "rdp":
      return 3389;
    case "smb":
      return 445;
    default:
      return null;
  }
}

export function schemeTarget(scheme: OriginScheme): "host_port" | "socket_path" | "status_code" | "none" {
  switch (scheme) {
    case "http":
    case "https":
    case "tcp":
    case "ssh":
    case "rdp":
    case "smb":
      return "host_port";
    case "unix":
    case "unix_tls":
      return "socket_path";
    case "http_status":
      return "status_code";
    case "bastion":
    case "hello_world":
      return "none";
  }
}

export function supportsPath(scheme: OriginScheme): boolean {
  return scheme === "http" || scheme === "https" || scheme === "http_status";
}

export function transportLabel(protocol: "quic" | "http2" | null): string {
  switch (protocol) {
    case "quic":
      return "QUIC";
    case "http2":
      return "HTTP/2";
    default:
      return "Automatic";
  }
}

export function transportExplanation(protocol: "quic" | "http2" | null): string {
  switch (protocol) {
    case "quic":
      return "cloudflared's default. Best throughput on a network that carries long-lived UDP flows well.";
    case "http2":
      return "Runs the tunnel over TCP instead. The fix when UDP flows stall: stalled uploads, requests that never arrive at the origin, intermittent degradation on one connection.";
    default:
      return "Let cloudflared choose. Today that is QUIC with an automatic fallback.";
  }
}

export function plural(count: number, singular: string, pluralForm?: string): string {
  return count === 1 ? singular : pluralForm ?? `${singular}s`;
}

export function formatTime(value: string | null): string {
  if (!value) return "Never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown";

  const seconds = Math.round((Date.now() - date.getTime()) / 1000);
  if (seconds < 45) return "Just now";
  if (seconds < 90) return "A minute ago";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} minutes ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} ${plural(hours, "hour")} ago`;
  const days = Math.round(hours / 24);
  if (days < 14) return `${days} ${plural(days, "day")} ago`;

  return date.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

export function formatExactTime(value: string | null): string {
  if (!value) return "Never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "Unknown";
  return date.toLocaleString();
}

/** `2026-11-11T12:00:00Z` → `11 Nov 2026, 12:00` */
export function formatActivityTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
