// Development-only entry point. Not imported by the app or the production build.
// Official Tauri mocks ensure visual/keyboard tests cannot reach real accounts,
// credentials, services, clipboard contents or remote hosts.
import { isTauri } from "@tauri-apps/api/core";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type {
  AppState,
  Bootstrap,
  PublicHostname,
  Settings,
  TunnelStatus,
} from "../src/types";

if (isTauri()) throw new Error("The design preview must not run inside Tauri.");

const params = new URLSearchParams(location.search);
const theme = params.get("theme") === "light" ? "light" : "dark";
const account = { id: "demo-account", name: "Acme Engineering" };
const capabilities = {
  tunnels: "granted",
  zones: "granted",
  dns: "unknown",
} as const;
const now = Date.now();
const names = [
  "production-web",
  "internal-api",
  "staging-services",
  "media-origin",
  "development",
  "backup-connector",
];
const statuses: TunnelStatus["kind"][] = [
  "healthy",
  "healthy",
  "degraded",
  "down",
  "inactive",
  "healthy",
];
const settings: Settings = {
  account_id: account.id,
  account_name: account.name,
  default_zone_id: "zone-1",
  default_zone_name: "example.com",
  theme,
  refresh_interval_secs: 0,
  log_level: "info",
  default_ssh_username: "deploy",
  default_ssh_port: 22,
  default_service_unit: "cloudflared.service",
  local_service_unit: "cloudflared.service",
  local_tunnel_id: null,
  remote_prompt_dismissed: false,
  token_saved: true,
};
const state: AppState = {
  connection: {
    kind: "connected",
    data: {
      account,
      token: { id: null, status: "active", expires_on: null },
      capabilities,
    },
  },
  account,
  capabilities,
  settings,
  tunnels: names.map((name, i) => ({
    tunnel: {
      id: `00000000-0000-4000-8000-00000000000${i}`,
      account_tag: account.id,
      name,
      status: { kind: statuses[i] } as TunnelStatus,
      config_source: { kind: "cloudflare" },
      kind: { kind: "cfd" },
      remote_config: true,
      created_at: "2026-11-10T10:00:00Z",
      deleted_at: null,
      conns_active_at: new Date(now - i * 3600000).toISOString(),
      conns_inactive_at: null,
      connector_count: i === 4 ? 0 : 1,
      hostname_count: i === 0 ? 3 : 1,
    },
    hostname_count: i === 0 ? 3 : 1,
    managed_hosts: [],
  })),
  zones: ["example.com", "acme.dev", "example.net"].map((name, i) => ({
    id: `zone-${i + 1}`,
    name,
    status: { kind: i === 2 ? "pending" : "active" },
    paused: false,
    account_id: account.id,
    account_name: account.name,
    permissions: ["#dns_records:edit"],
  })),
  hosts: [],
  host_status: [],
  activity: [
    "Connected to Acme Engineering.",
    "Loaded 6 tunnels.",
    "DNS verified for app.example.com.",
  ].map((summary, i) => ({
    id: i + 1,
    at: new Date(now - i * 120000).toISOString(),
    severity: i === 1 ? "info" : "success",
    category: i === 2 ? "dns" : "app",
    summary,
    detail: null,
    target: null,
  })),
  tunnels_error: null,
  busy: [],
  secrets_available: true,
  storage_problem: null,
  database_path: "/demo/tunnel-manager.sqlite3",
  schema_version: 1,
};
const bootstrap: Bootstrap = {
  app_name: "Tunnel Manager",
  app_id: "com.chrisdaggas.tunnelmanager",
  version: "1.0.0",
  tagline: "Manage Cloudflare Tunnels",
  affiliation_notice: "Unofficial Cloudflare client",
  releases: [],
  log_levels: [
    ["info", "Info"],
    ["debug", "Debug"],
  ],
  refresh_intervals: [
    [0, "Manual"],
    [60, "Every minute"],
  ],
  required_permissions: [],
  max_log_lines: 1000,
  default_log_lines: 100,
  platform: "Linux",
  accent: null,
  system_theme: theme,
};

if (params.has("hosts")) {
  state.hosts = ["edge-london", "edge-frankfurt"].map((name, i) => ({
    id: i + 1,
    name,
    address: `192.0.2.${i + 10}`,
    port: 22,
    username: "deploy",
    auth: { kind: "agent" },
    host_key_fingerprint: "SHA256:example-fingerprint-for-preview-only",
    service_unit: "cloudflared.service",
    tunnel_id: state.tunnels[i].tunnel.id,
    created_at: new Date(now).toISOString(),
    last_seen_at: null,
    notes: null,
  }));
  state.host_status = [
    {
      id: 1,
      status: {
        kind: "reachable",
        data: {
          service: { kind: "active" },
          cloudflared_version: "2026.10.0",
          systemd_available: true,
          can_control_service: true,
        },
      },
    },
    { id: 2, status: { kind: "unknown" } },
  ];
}
if (params.get("scenario") === "empty") {
  state.tunnels = [];
  state.zones = [];
  state.activity = [];
}
if (params.get("scenario") === "onboarding")
  state.connection = { kind: "disconnected" };
if (params.get("scenario") === "error")
  state.tunnels_error =
    "The request timed out. Check your connection and refresh to try again.";

const hostnames: PublicHostname[] = state.tunnels.slice(0, 2).map((row, i) => ({
  hostname: i === 0 ? "app.example.com" : "api.example.com",
  path: null,
  service: { scheme: "http", host: "localhost", port: 8080 },
  raw_service: "http://localhost:8080",
  tunnel_id: row.tunnel.id,
  tunnel_name: row.tunnel.name,
  zone_id: "zone-1",
  zone_name: "example.com",
  dns:
    i === 0
      ? { kind: "correct", data: { record_id: "demo-record" } }
      : { kind: "missing" },
  origin_request: null,
  rule_index: 0,
  disabled: false,
}));

mockWindows("main");
mockIPC(
  (command, args) => {
    switch (command) {
      case "bootstrap":
        return bootstrap;
      case "get_state":
      case "refresh_tunnels":
      case "refresh_zones":
      case "refresh_hosts":
        return structuredClone(state);
      case "check_for_updates":
        return null;
      case "set_window_theme":
        return;
      case "system_theme":
        return theme;
      case "plugin:window|is_maximized":
        return false;
      case "plugin:window|start_dragging":
      case "plugin:window|minimize":
      case "plugin:window|toggle_maximize":
      case "plugin:window|close":
        return;
      case "load_all_hostnames":
        return { hostnames, errors: [] };
      case "load_hostnames":
        return hostnames.filter(
          (item) =>
            item.tunnel_id === (args as { tunnel_id: string }).tunnel_id,
        );
      case "list_connectors":
        return [];
      case "local_inspect":
        return {
          systemctl_available: true,
          systemd_available: true,
          cloudflared_version: "2026.10.0",
          unit_present: true,
          service_state: { kind: "active" },
          service_enabled: true,
          protocol: null,
        };
      case "sudoers_snippet":
        return "# Example only: scoped systemctl permissions for the deploy user";
      case "update_settings":
        state.settings = (args as { settings: Settings }).settings;
        return structuredClone(state);
      default:
        throw new Error(`Design preview refused command: ${command}`);
    }
  },
  { shouldMockEvents: true },
);

await import("../src/main");
