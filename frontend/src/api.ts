import { invoke } from "@tauri-apps/api/core";

import type {
  AppState,
  ApplyDeploymentRequest,
  Bootstrap,
  CommandError,
  Connector,
  CreateOutcome,
  CreateTunnelRequest,
  DisableHostnameRequest,
  DeploymentOutcome,
  DeploymentPlan,
  HostKey,
  HostnameSweep,
  LocalState,
  Outcome,
  Preflight,
  PublicHostname,
  PublishHostnameRequest,
  RemoteHost,
  RemoteHostDraft,
  DeleteImpact,
  ServiceAction,
  ServiceOutcome,
  Settings,
  TokenCheck,
  TransportOutcome,
  TransportProtocol,
  UpdateInfo,
  Zone,
} from "./types";

/**
 * Every Rust command. Argument names are snake_case because the command
 * attribute asks Tauri not to re-case them, which keeps this file a direct
 * mirror of the Rust signatures.
 */
export const api = {
  // State and connection
  getState: () => invoke<AppState>("get_state"),
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  checkToken: (token: string) => invoke<TokenCheck>("check_token", { token }),
  previewZones: (token: string, account_id: string) =>
    invoke<Zone[]>("preview_zones", { token, account_id }),
  connect: (
    token: string,
    account_id: string,
    default_zone_id: string | null,
    remember: boolean,
  ) => invoke<AppState>("connect", { token, account_id, default_zone_id, remember }),
  restoreSession: () => invoke<AppState>("restore_session"),
  disconnect: () => invoke<AppState>("disconnect"),

  // Tunnels
  refreshTunnels: () => invoke<AppState>("refresh_tunnels"),
  reviewTunnel: (request: CreateTunnelRequest) =>
    invoke<string[]>("review_tunnel", { request }),
  createTunnel: (request: CreateTunnelRequest) =>
    invoke<CreateOutcome>("create_tunnel", { request }),
  renameTunnel: (tunnel_id: string, name: string) =>
    invoke<AppState>("rename_tunnel", { tunnel_id, name }),
  deleteTunnelImpact: (tunnel_id: string) =>
    invoke<DeleteImpact>("delete_tunnel_impact", { tunnel_id }),
  deleteTunnel: (tunnel_id: string) =>
    invoke<AppState>("delete_tunnel", { tunnel_id }),
  disconnectConnector: (tunnel_id: string, client_id: string) =>
    invoke<AppState>("disconnect_connector", { tunnel_id, client_id }),
  listConnectors: (tunnel_id: string) =>
    invoke<Connector[]>("list_connectors", { tunnel_id }),
  fetchRunToken: (tunnel_id: string) =>
    invoke<string>("fetch_run_token", { tunnel_id }),

  // Hostnames
  loadHostnames: (tunnel_id: string) =>
    invoke<PublicHostname[]>("load_hostnames", { tunnel_id }),
  loadAllHostnames: () => invoke<HostnameSweep>("load_all_hostnames"),
  publishHostname: (request: PublishHostnameRequest) =>
    invoke<Outcome>("publish_hostname", { request }),
  unpublishHostname: (
    tunnel_id: string,
    hostname: string,
    path: string | null,
    zone_id: string | null,
  ) => invoke<Outcome>("unpublish_hostname", { tunnel_id, hostname, path, zone_id }),
  disableHostname: (request: DisableHostnameRequest) =>
    invoke<Outcome>("disable_hostname", { request }),
  enableHostname: (tunnel_id: string, hostname: string, path: string | null) =>
    invoke<Outcome>("enable_hostname", { tunnel_id, hostname, path }),
  repairDns: (zone_id: string, tunnel_id: string, hostname: string) =>
    invoke<Outcome>("repair_dns", { zone_id, tunnel_id, hostname }),
  rollbackIngress: (tunnel_id: string, hostname: string, path: string | null) =>
    invoke<void>("rollback_ingress", { tunnel_id, hostname, path }),

  // Zones
  refreshZones: () => invoke<AppState>("refresh_zones"),

  // Hosts
  refreshHosts: () => invoke<AppState>("refresh_hosts"),
  saveHost: (draft: RemoteHostDraft) => invoke<number>("save_host", { draft }),
  updateHost: (host: RemoteHost) => invoke<AppState>("update_host", { host }),
  deleteHost: (host_id: number) => invoke<AppState>("delete_host", { host_id }),
  probeHostKey: (host_id: number) => invoke<HostKey>("probe_host_key", { host_id }),
  trustHostKey: (host_id: number, fingerprint: string) =>
    invoke<AppState>("trust_host_key", { host_id, fingerprint }),
  preflightHost: (host_id: number) =>
    invoke<Preflight>("preflight_host", { host_id }),
  hostServiceAction: (host_id: number, action: ServiceAction) =>
    invoke<ServiceOutcome>("host_service_action", { host_id, action }),
  hostLogs: (host_id: number, lines: number) =>
    invoke<string[]>("host_logs", { host_id, lines }),
  readTransport: (host_id: number) =>
    invoke<TransportProtocol | null>("read_transport", { host_id }),
  setTransport: (host_id: number, protocol: TransportProtocol | null) =>
    invoke<TransportOutcome>("set_transport", { host_id, protocol }),
  planDeployment: (host_id: number, tunnel_id: string) =>
    invoke<DeploymentPlan>("plan_deployment", { host_id, tunnel_id }),
  applyDeployment: (request: ApplyDeploymentRequest) =>
    invoke<DeploymentOutcome>("apply_deployment", { request }),
  manualSetupInstructions: (tunnel_id: string) =>
    invoke<string>("manual_setup_instructions", { tunnel_id }),
  sudoersSnippet: (username: string, unit: string) =>
    invoke<string>("sudoers_snippet", { username, unit }),
  saveHostPassphrase: (host_id: number, passphrase: string) =>
    invoke<AppState>("save_host_passphrase", { host_id, passphrase }),
  deleteHostPassphrase: (host_id: number) =>
    invoke<AppState>("delete_host_passphrase", { host_id }),

  // Local connector
  localInspect: (unit: string) => invoke<LocalState>("local_inspect", { unit }),
  localServiceAction: (unit: string, action: ServiceAction) =>
    invoke<ServiceOutcome>("local_service_action", { unit, action }),
  localLogs: (unit: string, lines: number) =>
    invoke<string[]>("local_logs", { unit, lines }),
  localSetTransport: (unit: string, protocol: TransportProtocol | null) =>
    invoke<TransportOutcome>("local_set_transport", { unit, protocol }),

  // Settings and activity
  updateSettings: (settings: Settings) =>
    invoke<AppState>("update_settings", { settings }),
  setLogLevel: (level: string) =>
    invoke<AppState>("set_log_level", { level }),
  diagnosticReport: () => invoke<string>("diagnostic_report"),
  clearActivity: () => invoke<AppState>("clear_activity"),
  recentLogLines: () => invoke<string[]>("recent_log_lines"),
  currentLogLevel: () => invoke<string>("current_log_level"),

  // Application
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  checkForUpdates: () => invoke<UpdateInfo | null>("check_for_updates"),
  setWindowTheme: (theme: string) => invoke<void>("set_window_theme", { theme }),
  systemTheme: () => invoke<string>("system_theme"),
  quit: () => invoke<void>("quit"),
};

/** The message a failed command should show, whatever shape it arrived in. */
export function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const candidate = error as Partial<CommandError>;
    if (typeof candidate.message === "string" && candidate.message.length > 0) {
      return candidate.message;
    }
  }
  return "Something went wrong.";
}

/** The whole structured error, for the cases that branch on `kind`. */
export function commandError(error: unknown): CommandError {
  if (error && typeof error === "object" && "kind" in error) {
    return error as CommandError;
  }
  return {
    kind: "internal",
    title: "Something went wrong",
    message: errorText(error),
  };
}
