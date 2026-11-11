// The shape of everything the Rust bridge sends across the IPC boundary.
//
// Field names mirror the Rust structs exactly (snake_case). The one deliberate
// exception is `OriginRequest`: its fields are Cloudflare's wire format
// (`connectTimeout`, `noTLSVerify`, …) because that document is written
// straight back to the API.

// ── Enums with an adjacent `kind` tag ────────────────────────────────────

export type TunnelStatus =
  | { kind: "healthy" }
  | { kind: "degraded" }
  | { kind: "down" }
  | { kind: "inactive" }
  | { kind: "other"; data: string };

export type ConfigSource =
  | { kind: "cloudflare" }
  | { kind: "local" }
  | { kind: "other"; data: string };

export type TunnelKind =
  | { kind: "cfd" }
  | { kind: "warp" }
  | { kind: "magic" }
  | { kind: "other"; data: string };

export type ZoneStatus =
  | { kind: "active" }
  | { kind: "pending" }
  | { kind: "initializing" }
  | { kind: "moved" }
  | { kind: "deactivated" }
  | { kind: "other"; data: string };

export type DnsState =
  | { kind: "correct"; data: { record_id: string } }
  | { kind: "not_proxied"; data: { record_id: string } }
  | { kind: "missing" }
  | { kind: "conflicting"; data: { record_id: string; record_type: string; content: string } }
  | { kind: "unknown"; data: { reason: string } };

export type RemoteAuth =
  | { kind: "agent" }
  | { kind: "key_file"; data: { path: string; has_passphrase: boolean } };

export type RemoteServiceState =
  | { kind: "active" }
  | { kind: "inactive" }
  | { kind: "failed" }
  | { kind: "not_installed" }
  | { kind: "other"; data: string };

export type RemoteStatus =
  | { kind: "unknown" }
  | { kind: "checking" }
  | {
      kind: "reachable";
      data: {
        service: RemoteServiceState;
        cloudflared_version: string | null;
        systemd_available: boolean;
        can_control_service: boolean;
      };
    }
  | { kind: "unreachable"; data: { message: string } };

export type ConnectionState =
  | { kind: "disconnected" }
  | { kind: "connecting" }
  | {
      kind: "connected";
      data: { account: Account; token: TokenStatus; capabilities: TokenCapabilities };
    }
  | { kind: "failed"; data: { message: string; title: string } };

export type Recovery =
  | { kind: "retry" }
  | { kind: "repair_dns" }
  | { kind: "roll_back_ingress" }
  | { kind: "resolve_conflict_by_hand"; data: { record_type: string; content: string } }
  | { kind: "fix_permissions" };

export type OriginRequestUpdate =
  | { kind: "preserve" }
  | { kind: "clear" }
  | { kind: "set"; data: OriginRequest };

// ── Unit enums (serde serialises these as strings) ───────────────────────

export type PermissionState = "unknown" | "granted" | "denied";
export type TransportProtocol = "quic" | "http2";
export type ServiceAction = "start" | "stop" | "restart";
export type ActivitySeverity = "info" | "success" | "warning" | "error";
export type ActivityCategory =
  | "api"
  | "dns"
  | "tunnel_config"
  | "tunnel"
  | "ssh"
  | "remote_service"
  | "app";
export type OriginScheme =
  | "http"
  | "https"
  | "tcp"
  | "ssh"
  | "rdp"
  | "unix"
  | "unix_tls"
  | "smb"
  | "http_status"
  | "bastion"
  | "hello_world";
export type TargetKind = "host_port" | "socket_path" | "status_code" | "none";
export type Step =
  | "read_configuration"
  | "check_dns"
  | "write_configuration"
  | "create_dns_record"
  | "repair_dns_record"
  | "remove_dns_record"
  | "verify"
  | "rollback_configuration";
export type StepStatus = "done" | "skipped" | "failed";

// ── Structs ──────────────────────────────────────────────────────────────

export interface Account {
  id: string;
  name: string;
}

export interface TokenStatus {
  id: string | null;
  status: string;
  expires_on: string | null;
}

export interface TokenCapabilities {
  tunnels: PermissionState;
  zones: PermissionState;
  dns: PermissionState;
}

export interface RequiredPermission {
  dashboard_name: string;
  scope: string;
  level: string;
  needed_for: string;
}

export interface Connection {
  id: string;
  colo_name: string;
  is_pending_reconnect: boolean;
  origin_ip: string | null;
  opened_at: string | null;
  client_id: string | null;
  client_version: string | null;
}

export interface Connector {
  id: string;
  version: string;
  arch: string;
  run_at: string | null;
  features: string[];
  config_version: number | null;
  connections: Connection[];
}

export interface Tunnel {
  id: string;
  account_tag: string;
  name: string;
  status: TunnelStatus;
  config_source: ConfigSource;
  kind: TunnelKind;
  remote_config: boolean;
  created_at: string | null;
  deleted_at: string | null;
  conns_active_at: string | null;
  conns_inactive_at: string | null;
  connector_count: number;
  hostname_count: number | null;
}

export interface TunnelRow {
  tunnel: Tunnel;
  hostname_count: number | null;
  managed_hosts: string[];
}

export interface Zone {
  id: string;
  name: string;
  status: ZoneStatus;
  paused: boolean;
  account_id: string;
  account_name: string;
  permissions: string[];
}

export interface OriginService {
  scheme: OriginScheme;
  host: string;
  port: number | null;
}

/**
 * `originRequest`. The named fields are Cloudflare's wire names; anything else
 * the account has configured is preserved verbatim and arrives as extra keys.
 */
export interface OriginRequest {
  connectTimeout?: number | null;
  tlsTimeout?: number | null;
  tcpKeepAlive?: number | null;
  noHappyEyeballs?: boolean | null;
  keepAliveConnections?: number | null;
  keepAliveTimeout?: number | null;
  httpHostHeader?: string | null;
  originServerName?: string | null;
  caPool?: string | null;
  noTLSVerify?: boolean | null;
  disableChunkedEncoding?: boolean | null;
  bastionMode?: boolean | null;
  proxyAddress?: string | null;
  proxyPort?: number | null;
  proxyType?: string | null;
  http2Origin?: boolean | null;
  matchSNItoHost?: boolean | null;
  [key: string]: unknown;
}

export interface PublicHostname {
  hostname: string;
  path: string | null;
  service: OriginService;
  raw_service: string;
  tunnel_id: string;
  tunnel_name: string;
  zone_id: string | null;
  zone_name: string | null;
  dns: DnsState;
  origin_request: OriginRequest | null;
  rule_index: number;
  disabled: boolean;
}

export interface RemoteHost {
  id: number;
  name: string;
  address: string;
  port: number;
  username: string;
  auth: RemoteAuth;
  host_key_fingerprint: string | null;
  service_unit: string;
  tunnel_id: string | null;
  created_at: string;
  last_seen_at: string | null;
  notes: string | null;
}

export interface RemoteHostDraft {
  name: string;
  address: string;
  port: number;
  username: string;
  auth: RemoteAuth;
  service_unit: string;
  tunnel_id: string | null;
  notes: string | null;
}

export interface Settings {
  account_id: string | null;
  account_name: string | null;
  default_zone_id: string | null;
  default_zone_name: string | null;
  theme: string;
  refresh_interval_secs: number;
  log_level: string;
  default_ssh_username: string;
  default_ssh_port: number;
  default_service_unit: string;
  local_service_unit: string;
  local_tunnel_id: string | null;
  remote_prompt_dismissed: boolean;
  token_saved: boolean;
}

export interface ActivityEvent {
  id: number;
  at: string;
  severity: ActivitySeverity;
  category: ActivityCategory;
  summary: string;
  detail: string | null;
  target: string | null;
}

export interface HostStatus {
  id: number;
  status: RemoteStatus;
}

export interface AppState {
  connection: ConnectionState;
  account: Account | null;
  zones: Zone[];
  capabilities: TokenCapabilities;
  tunnels: TunnelRow[];
  tunnels_error: string | null;
  hosts: RemoteHost[];
  host_status: HostStatus[];
  activity: ActivityEvent[];
  settings: Settings;
  busy: string[];
  secrets_available: boolean;
  storage_problem: string | null;
  database_path: string;
  schema_version: number;
}

// ── Command payloads ─────────────────────────────────────────────────────

export interface CommandError {
  kind: string;
  title: string;
  message: string;
  data?: Record<string, unknown>;
}

export interface TokenCheck {
  status: TokenStatus;
  accounts: Account[];
  accounts_unavailable: string | null;
  needs_manual_account: boolean;
}

export interface CreateHostnameRequest {
  hostname: string;
  path: string | null;
  service: OriginService;
  zone_id: string | null;
}

export interface CreateTunnelRequest {
  name: string;
  hostname: CreateHostnameRequest | null;
}

export interface PublishHostnameRequest {
  tunnel_id: string;
  hostname: string;
  path: string | null;
  service: OriginService;
  zone_id: string | null;
  origin_request: OriginRequestUpdate;
}

export interface DisableHostnameRequest {
  tunnel_id: string;
  hostname: string;
  path: string | null;
  service: string;
}

export interface ApplyDeploymentRequest {
  host_id: number;
  plan: DeploymentPlan;
  token: string;
  confirmed_overwrite: boolean;
}

export interface StepResult {
  step: Step;
  status: StepStatus;
  detail: string;
}

export interface Outcome {
  hostname: string;
  steps: StepResult[];
  dns: DnsState;
  recovery: Recovery | null;
  touched_rule: unknown | null;
}

export interface CreateOutcome {
  steps: StepResult[];
  tunnel: Tunnel | null;
  token: string | null;
  hostname: Outcome | null;
  succeeded: boolean;
  is_partial: boolean;
  manual_instructions: string | null;
}

export interface HostnameSweep {
  hostnames: PublicHostname[];
  errors: string[];
}

export interface DeleteImpact {
  hostnames: string[];
  connector_count: number;
  managed_hosts: string[];
  orphaned_dns: string[];
}

export interface HostKey {
  fingerprint: string;
  algorithm: string;
}

export interface Preflight {
  reachable: boolean;
  host_key_matched: boolean;
  is_linux: boolean;
  kernel: string | null;
  systemd_available: boolean;
  systemd_version: string | null;
  cloudflared_path: string | null;
  cloudflared_version: string | null;
  supports_token_file: boolean;
  unit_present: boolean;
  service_state: RemoteServiceState;
  service_enabled: boolean | null;
  can_control_service: boolean;
  configured_tunnel_id: string | null;
  notes: string[];
}

export interface ServiceOutcome {
  action: ServiceAction;
  state_before: RemoteServiceState;
  state_after: RemoteServiceState;
  message: string | null;
}

export interface DeploymentStep {
  description: string;
  preview: string;
  privileged: boolean;
  overwrites: boolean;
}

export interface DeploymentPlan {
  tunnel_id: string;
  tunnel_name: string;
  unit: string;
  token_path: string;
  unit_path: string;
  cloudflared_path: string;
  steps: DeploymentStep[];
  unit_file: string;
  replaces_existing_unit: boolean;
}

export interface DeploymentOutcome {
  completed: string[];
  failed: [string, string] | null;
  service_state: RemoteServiceState;
}

export interface TransportOutcome {
  protocol: TransportProtocol | null;
  service_state: RemoteServiceState;
}

export interface LocalState {
  systemctl_available: boolean;
  systemd_available: boolean;
  cloudflared_version: string | null;
  unit_present: boolean;
  service_state: RemoteServiceState;
  service_enabled: boolean | null;
  protocol: TransportProtocol | null;
}

export interface Release {
  version: string;
  date: string;
  changes: string[];
}

export interface Bootstrap {
  app_name: string;
  app_id: string;
  version: string;
  tagline: string;
  affiliation_notice: string;
  releases: Release[];
  log_levels: [string, string][];
  refresh_intervals: [number, string][];
  required_permissions: RequiredPermission[];
  max_log_lines: number;
  default_log_lines: number;
  platform: string;
  accent: string | null;
  system_theme: string;
}

export interface UpdateInfo {
  latest_version: string;
  download_url: string;
  release_name: string;
}
