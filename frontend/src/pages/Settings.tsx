import { useEffect, useState } from "react";
import {
  Copy,
  Database,
  ExternalLink,
  Info,
  Loader2,
  Palette,
  Timer,
} from "lucide-react";

import { api, errorText } from "../api";
import { chooseTheme, refreshAll } from "../actions";
import {
  Badge,
  Banner,
  CopyButton,
  LoadingState,
  Notice,
  StatusBadge,
  copyText,
} from "../components/ui";
import { useConfirm } from "../components/useConfirm";
import { useApp } from "../store";
import { permissionState } from "../status";
import type { Settings as SettingsShape } from "../types";

export function Settings() {
  const state = useApp((store) => store.state);
  const bootstrap = useApp((store) => store.bootstrap);
  const setState = useApp((store) => store.setState);
  const toast = useApp((store) => store.toast);
  const navigate = useApp((store) => store.navigate);
  const openDialog = useApp((store) => store.openDialog);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [reportBusy, setReportBusy] = useState(false);

  if (!state) return <LoadingState message="Reading the settings…" />;

  const settings = state.settings;
  const connection = state.connection;
  const connected = connection.kind === "connected";
  const permissionStates = [
    state.capabilities.tunnels,
    state.capabilities.zones,
    state.capabilities.dns,
  ];
  const required = bootstrap?.required_permissions ?? [];

  async function save(patch: Partial<SettingsShape>) {
    if (!state) return;
    try {
      const next = await api.updateSettings({ ...settings, ...patch });
      setState(next);
      toast("Saved.", "success");
    } catch (error) {
      toast(errorText(error), "error");
    }
  }

  function removeToken() {
    confirm({
      title: "Remove the saved API token?",
      description:
        "The token is deleted from the credential store and the account is forgotten. You will be returned to the connection page.",
      body: (
        <Notice>
          Theme, refresh interval, SSH defaults and registered hosts survive: they are
          preferences, not credentials.
        </Notice>
      ),
      confirmLabel: "Disconnect",
      destructive: true,
      run: async () => {
        await api.disconnect();
        toast("Disconnected and removed the saved token.", "success");
      },
    });
  }

  async function changeLogLevel(level: string) {
    try {
      const next = await api.setLogLevel(level);
      setState(next);
      toast("Log level saved.", "success");
    } catch (error) {
      toast(errorText(error), "error");
    }
  }

  async function copyDiagnostics() {
    setReportBusy(true);
    try {
      const report = await api.diagnosticReport();
      await copyText(report);
      toast("The redacted diagnostic report is on the clipboard.", "success");
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setReportBusy(false);
    }
  }

  return (
    <div className="content-inner narrow">
      {connection.kind === "failed" && (
        <Banner tone="error" icon="dialog-error-symbolic" title={connection.data.title}>
          {connection.data.message}
        </Banner>
      )}

      {!state.secrets_available && (
        <Banner
          tone="warning"
          icon="dialog-warning-symbolic"
          title="No credential store is available"
        >
          The token cannot be saved on this machine. The app works for this session, but
          you will paste the token again next time.
        </Banner>
      )}

      {state.storage_problem && (
        <Banner
          tone="warning"
          icon="dialog-warning-symbolic"
          title="Settings are not being saved"
        >
          {state.storage_problem}
        </Banner>
      )}

      <section className="settings-section">
        <h2>Connection</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Cloudflare account</div>
              <div className="settings-row-subtitle">
                {state.account
                  ? `${state.account.name} · ${state.account.id}`
                  : "Not connected"}
              </div>
            </div>
            <div className="settings-row-control">
              <Badge
                tone={connected ? "success" : "error"}
                icon={connected ? "object-select-symbolic" : "dialog-error-symbolic"}
              >
                {connected ? "Connected" : "Not connected"}
              </Badge>
              <Badge tone={settings.token_saved ? "success" : "neutral"}>
                {settings.token_saved ? "Token saved" : "Token not saved"}
              </Badge>
            </div>
          </div>

          <div className="settings-row row-top">
            <div className="settings-row-main">
              <div className="settings-row-title">Token permissions</div>
              <div className="settings-row-subtitle">
                Inferred from the calls that succeeded. An unchecked permission is not a
                missing one.
              </div>
              <div className="stack-sm" style={{ marginTop: 12 }}>
                {required.map((permission, index) => (
                  <div className="row-inline spread" key={permission.dashboard_name}>
                    <div style={{ minWidth: 0 }}>
                      <div className="permission-name">
                        {permission.dashboard_name} · {permission.level}
                      </div>
                      <div className="permission-scope">
                        {permission.scope} · {permission.needed_for}
                      </div>
                    </div>
                    <StatusBadge
                      status={permissionState(
                        permissionStates[index] ?? "unknown",
                      )}
                    />
                  </div>
                ))}
              </div>
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Saved API token</div>
              <div className="settings-row-subtitle">
                Stored in the system credential store, never in a file.
              </div>
            </div>
            <div className="settings-row-control">
              <button
                type="button"
                className="btn btn-danger"
                onClick={removeToken}
                disabled={!settings.token_saved && !connected}
              >
                Remove Saved Token
              </button>
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h2>Account</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Default domain</div>
              <div className="settings-row-subtitle">
                Offered first when publishing a hostname. A zone is never required.
              </div>
            </div>
            <div className="settings-row-control">
              <select
                className="select"
                value={settings.default_zone_id ?? ""}
                onChange={(event) => {
                  const zone = state.zones.find(
                    (item) => item.id === event.target.value,
                  );
                  void save({
                    default_zone_id: zone?.id ?? null,
                    default_zone_name: zone?.name ?? null,
                  });
                }}
              >
                <option value="">No default domain</option>
                {state.zones.map((zone) => (
                  <option key={zone.id} value={zone.id}>
                    {zone.name}
                  </option>
                ))}
              </select>
            </div>
          </div>

          {state.zones.length === 0 && (
            <div className="settings-row">
              <div className="settings-row-main">
                <div className="settings-row-subtitle">
                  No domains are available.{" "}
                  <button
                    type="button"
                    className="btn-link"
                    onClick={() => navigate({ page: "zones" })}
                  >
                    Check the Zones page
                  </button>{" "}
                  — the token may be missing Zone:Read.
                </div>
              </div>
            </div>
          )}
        </div>
      </section>

      <section className="settings-section">
        <h2>Appearance</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title row-inline">
                <Palette size={15} /> Theme
              </div>
              <div className="settings-row-subtitle">
                Follows the system by default, including text scale and high contrast.
              </div>
            </div>
            <div className="settings-row-control">
              <select
                className="select"
                value={settings.theme}
                onChange={(event) => void chooseTheme(event.target.value)}
              >
                <option value="system">Follow the system</option>
                <option value="light">Light</option>
                <option value="dark">Dark</option>
              </select>
              <span
                title="The accent follows the GNOME accent colour"
                style={{
                  width: 18,
                  height: 18,
                  borderRadius: 6,
                  background: "var(--accent)",
                  flex: "0 0 auto",
                }}
              />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title row-inline">
                <Timer size={15} /> Refresh
              </div>
              <div className="settings-row-subtitle">
                How often the tunnel list and domains are reloaded.
              </div>
            </div>
            <div className="settings-row-control">
              <select
                className="select"
                value={String(settings.refresh_interval_secs)}
                onChange={(event) =>
                  void save({ refresh_interval_secs: Number(event.target.value) })
                }
              >
                {(bootstrap?.refresh_intervals ?? []).map(([seconds, label]) => (
                  <option key={seconds} value={seconds}>
                    {label}
                  </option>
                ))}
              </select>
              <button type="button" className="btn" onClick={() => void refreshAll()}>
                Refresh Now
              </button>
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h2>Remote defaults</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Default SSH user</div>
              <div className="settings-row-subtitle">
                Offered when adding a host. A per-host value overrides it.
              </div>
            </div>
            <div className="settings-row-control">
              <TextSetting
                value={settings.default_ssh_username}
                placeholder="deploy"
                onSave={(value) => void save({ default_ssh_username: value })}
                ariaLabel="Default SSH user"
              />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Default SSH port</div>
              <div className="settings-row-subtitle">Usually 22.</div>
            </div>
            <div className="settings-row-control">
              <TextSetting
                value={String(settings.default_ssh_port)}
                inputMode="numeric"
                onSave={(value) => void save({ default_ssh_port: Number(value) })}
                ariaLabel="Default SSH port"
              />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Default service unit</div>
              <div className="settings-row-subtitle">
                The systemd unit cloudflared installs on remote hosts.
              </div>
            </div>
            <div className="settings-row-control">
              <TextSetting
                value={settings.default_service_unit}
                mono
                onSave={(value) => void save({ default_service_unit: value })}
                ariaLabel="Default service unit"
              />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Local service unit</div>
              <div className="settings-row-subtitle">
                The unit this application may control on this machine.
              </div>
            </div>
            <div className="settings-row-control">
              <TextSetting
                value={settings.local_service_unit}
                mono
                onSave={(value) => void save({ local_service_unit: value })}
                ariaLabel="Local service unit"
              />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Local tunnel</div>
              <div className="settings-row-subtitle">
                The tunnel this machine's connector serves. Used for restart warnings.
              </div>
            </div>
            <div className="settings-row-control">
              <select
                className="select"
                value={settings.local_tunnel_id ?? ""}
                onChange={(event) =>
                  void save({
                    local_tunnel_id: event.target.value || null,
                  })
                }
              >
                <option value="">Not associated</option>
                {state.tunnels.map((row) => (
                  <option key={row.tunnel.id} value={row.tunnel.id}>
                    {row.tunnel.name}
                  </option>
                ))}
              </select>
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h2>Data & diagnostics</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title row-inline">
                <Database size={15} /> Local database
              </div>
              <div className="settings-row-subtitle mono">{state.database_path}</div>
              <div className="settings-row-subtitle">
                Schema version {state.schema_version}. Contains no secrets.
              </div>
            </div>
            <div className="settings-row-control">
              <CopyButton value={state.database_path} label="Copy path" />
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Log level</div>
              <div className="settings-row-subtitle">
                Applies immediately. Everything written is redacted first.
              </div>
            </div>
            <div className="settings-row-control">
              <select
                className="select"
                value={settings.log_level}
                onChange={(event) => void changeLogLevel(event.target.value)}
              >
                {(bootstrap?.log_levels ?? []).map(([name, label]) => (
                  <option key={name} value={name}>
                    {label}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">Diagnostic report</div>
              <div className="settings-row-subtitle">
                Settings, counts, recent activity and redacted log lines. No token,
                passphrase or run token.
              </div>
            </div>
            <div className="settings-row-control">
              <button
                type="button"
                className="btn"
                onClick={() => void copyDiagnostics()}
                disabled={reportBusy}
              >
                {reportBusy ? <Loader2 size={14} className="spin" /> : <Copy size={14} />}
                Copy Report
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => navigate({ page: "activity" })}
              >
                Open Activity
              </button>
            </div>
          </div>
        </div>
      </section>

      <section className="settings-section">
        <h2>About</h2>
        <div className="group">
          <div className="settings-row">
            <div className="settings-row-main">
              <div className="settings-row-title">
                {bootstrap?.app_name ?? "Tunnel Manager"} {bootstrap?.version}
              </div>
              <div className="settings-row-subtitle">
                {bootstrap?.platform} ·{" "}
                {bootstrap?.affiliation_notice ??
                  "An unofficial client for the Cloudflare API."}
              </div>
            </div>
            <div className="settings-row-control">
              <button
                type="button"
                className="btn"
                onClick={() => openDialog({ kind: "whats-new" })}
              >
                What's New
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => openDialog({ kind: "shortcuts" })}
              >
                Shortcuts
              </button>
              <button
                type="button"
                className="btn"
                onClick={() => openDialog({ kind: "about" })}
              >
                <Info size={14} /> About
              </button>
            </div>
          </div>
        </div>
      </section>

      <Notice icon="channel-secure-symbolic">
        The API token is filed under{" "}
        <span className="mono">com.chrisdaggas.tunnelmanager</span> in the system
        credential store. Removing it here deletes it there.{" "}
        <button
          type="button"
          className="btn-link"
          onClick={() =>
            void api.openUrl(
              "https://developers.cloudflare.com/fundamentals/api/get-started/create-token/",
            )
          }
        >
          Token documentation <ExternalLink size={11} style={{ verticalAlign: -1 }} />
        </button>
      </Notice>

      {confirmDialog}
    </div>
  );
}

function TextSetting({
  value,
  onSave,
  placeholder,
  mono = false,
  inputMode,
  ariaLabel,
}: {
  value: string;
  onSave: (value: string) => void;
  placeholder?: string;
  mono?: boolean;
  inputMode?: "numeric" | "text";
  ariaLabel: string;
}) {
  const [draft, setDraft] = useState(value);

  useEffect(() => {
    setDraft(value);
  }, [value]);

  return (
    <input
      className={`input${mono ? " mono" : ""}`}
      style={{ width: 190 }}
      value={draft}
      placeholder={placeholder}
      inputMode={inputMode}
      aria-label={ariaLabel}
      spellCheck={false}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={() => {
        if (draft !== value) onSave(draft);
      }}
      onKeyDown={(event) => {
        if (event.key === "Enter") {
          event.currentTarget.blur();
        }
      }}
    />
  );
}
