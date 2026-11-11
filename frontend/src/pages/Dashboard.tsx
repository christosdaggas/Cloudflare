import { type ReactNode } from "react";
import {
  ArrowLeftRight,
  ArrowRight,
  Globe,
  Plus,
  Server,
  Settings as SettingsIcon,
  Waypoints,
} from "lucide-react";

import { useApp } from "../store";
import {
  Banner,
  Badge,
  EmptyState,
  LoadingState,
  StatCard,
  StatusBadge,
} from "../components/ui";
import { SymbolicIcon } from "../icons";
import { formatTime, remoteStatus, severity } from "../status";
import type { AppState, TunnelRow } from "../types";

type FilterName = "healthy" | "degraded" | "down" | "inactive";

export function Dashboard() {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);
  const setTunnelFilter = useApp((store) => store.setTunnelFilter);
  const openDialog = useApp((store) => store.openDialog);

  if (!state) {
    return <LoadingState message="Reading the tunnel list…" />;
  }

  const counts = countStatuses(state.tunnels);
  const attention = counts.degraded + counts.down;
  const hostnames = totalHostnames(state.tunnels);

  function show(status: FilterName) {
    setTunnelFilter(status);
    navigate({ page: "tunnels" });
  }

  return (
    <div className="content-inner">
      {state.tunnels_error && (
        <Banner tone="error" icon="dialog-error-symbolic" title="The tunnel list could not be loaded">
          {state.tunnels_error}
        </Banner>
      )}

      {state.storage_problem && (
        <Banner tone="warning" icon="dialog-warning-symbolic" title="Settings are not being saved">
          {state.storage_problem}
        </Banner>
      )}

      {!state.secrets_available && (
        <Banner tone="warning" icon="dialog-warning-symbolic" title="No credential store is available">
          The API token cannot be saved on this machine. The app still works for this
          session, but you will paste the token again next time.
        </Banner>
      )}

      {state.tunnels.length === 0 && !state.tunnels_error ? (
        <div className="card">
          <EmptyState
            icon="network-vpn-symbolic"
            title="No tunnels yet"
            description="A tunnel connects a hostname to a machine. Create one here, then publish a hostname so traffic has somewhere to go."
            action={
              <button
                type="button"
                className="btn btn-primary btn-lg"
                onClick={() => openDialog({ kind: "create-tunnel" })}
              >
                <Plus size={16} /> Create your first tunnel
              </button>
            }
          />
        </div>
      ) : (
        <>
          {attention > 0 && (
            <Banner
              tone="warning"
              icon="dialog-warning-symbolic"
              title={`${attention} tunnel${attention === 1 ? "" : "s"} need${attention === 1 ? "s" : ""} attention`}
              action={
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => show(counts.down > 0 ? "down" : "degraded")}
                >
                  Show tunnels
                </button>
              }
            >
              {counts.down > 0 && `${counts.down} down`}
              {counts.down > 0 && counts.degraded > 0 && " · "}
              {counts.degraded > 0 && `${counts.degraded} degraded`}
              {" — a connector is missing some of its edge connections."}
            </Banner>
          )}

          <div className="stat-grid">
            <StatCard
              value={counts.healthy}
              label="Healthy"
              detail="Every connection established"
              icon="object-select-symbolic"
              tone="success"
              onClick={() => show("healthy")}
            />
            <StatCard
              value={counts.degraded}
              label="Degraded"
              detail="Some edge connections missing"
              icon="dialog-warning-symbolic"
              tone="warning"
              onClick={() => show("degraded")}
            />
            <StatCard
              value={counts.down}
              label="Down"
              detail="Connectors registered, unreachable"
              icon="dialog-error-symbolic"
              tone="error"
              onClick={() => show("down")}
            />
            <StatCard
              value={counts.inactive + counts.other}
              label="Inactive"
              detail="Nothing is serving them yet"
              icon="media-playback-pause-symbolic"
              tone="neutral"
              onClick={() => show("inactive")}
            />
          </div>

          <div className="dashboard-grid">
            <section className="section">
              <div className="section-heading">
                <h2>Tunnels at a glance</h2>
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => navigate({ page: "tunnels" })}
                >
                  Open Tunnels <ArrowRight size={12} style={{ verticalAlign: -1 }} />
                </button>
              </div>

              <div className="card">
                <div className="card-body stack-sm">
                  <div className="row-inline spread">
                    <span className="field-label">Published hostnames</span>
                    <span className="numeric" style={{ fontWeight: 700 }}>
                      {hostnames === null ? "—" : hostnames}
                    </span>
                  </div>
                  <div className="row-inline spread">
                    <span className="field-label">Registered connectors</span>
                    <span className="numeric" style={{ fontWeight: 700 }}>
                      {state.tunnels.reduce(
                        (sum, row) => sum + row.tunnel.connector_count,
                        0,
                      )}
                    </span>
                  </div>
                  <div className="row-inline spread">
                    <span className="field-label">Domains available</span>
                    <span className="numeric" style={{ fontWeight: 700 }}>
                      {state.zones.length}
                    </span>
                  </div>
                  {hostnames === null && (
                    <div className="field-hint">
                      Hostname counts appear as each tunnel's configuration is read.
                    </div>
                  )}
                </div>
              </div>

              <div className="card">
                <div className="card-header">
                  <h3>Recent activity</h3>
                  <button
                    type="button"
                    className="btn-link"
                    style={{ marginLeft: "auto" }}
                    onClick={() => navigate({ page: "activity" })}
                  >
                    View all
                  </button>
                </div>
                {state.activity.length === 0 ? (
                  <div className="card-body field-hint">
                    Nothing has happened yet. Requests and their outcomes will appear
                    here.
                  </div>
                ) : (
                  <div>
                    {state.activity.slice(0, 5).map((event) => {
                      const meta = severity(event.severity);
                      return (
                        <div className="activity-row" key={event.id}>
                          <div className={`activity-icon tone-${meta.tone}`}>
                            <SymbolicIcon name={meta.icon} size={14} />
                          </div>
                          <div className="activity-main">
                            <div className="activity-summary">{event.summary}</div>
                            <div className="activity-meta">
                              <span>{formatTime(event.at)}</span>
                              <span>·</span>
                              <span>{event.category.replace("_", " ")}</span>
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </section>

            <section className="section">
              <div className="section-heading">
                <h2>Remote reach</h2>
              </div>
              <ReachCard state={state} />

              <div className="section-heading" style={{ marginTop: 8 }}>
                <h2>Quick actions</h2>
              </div>
              <div className="quick-actions">
                <QuickAction
                  icon={<Waypoints size={17} />}
                  title="Create a tunnel"
                  description="Name it, optionally publish a hostname, then deploy."
                  onClick={() => openDialog({ kind: "create-tunnel" })}
                />
                <QuickAction
                  icon={<ArrowLeftRight size={17} />}
                  title="Publish a hostname"
                  description="Add an ingress rule and its proxied CNAME."
                  onClick={() => openDialog({ kind: "hostname", tunnel_id: null })}
                />
                <QuickAction
                  icon={<Server size={17} />}
                  title="Add a remote host"
                  description="Control cloudflared over SSH with a pinned key."
                  onClick={() => openDialog({ kind: "host" })}
                />
                <QuickAction
                  icon={<SettingsIcon size={17} />}
                  title="Open settings"
                  description="Appearance, refresh interval and diagnostics."
                  onClick={() => navigate({ page: "settings" })}
                />
                <QuickAction
                  icon={<Globe size={17} />}
                  title="Review zones"
                  description="Check that a domain is ready to serve traffic."
                  onClick={() => navigate({ page: "zones" })}
                />
              </div>
            </section>
          </div>
        </>
      )}
    </div>
  );
}

function ReachCard({ state }: { state: AppState }) {
  const openDialog = useApp((store) => store.openDialog);
  const navigate = useApp((store) => store.navigate);

  if (state.hosts.length === 0) {
    return (
      <div className="card">
        <div className="empty-state" style={{ padding: "28px 22px" }}>
          <div className="empty-icon">
            <Server size={24} />
          </div>
          <h3>Remote control is optional</h3>
          <p>
            Everything on the Cloudflare side works without it. Add a host when you
            want to start, stop or deploy cloudflared over SSH.
          </p>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => openDialog({ kind: "host" })}
          >
            <Plus size={15} /> Add a host
          </button>
        </div>
      </div>
    );
  }

  const statuses = new Map(state.host_status.map((item) => [item.id, item.status]));
  let reachable = 0;
  let unreachable = 0;
  let unchecked = 0;
  for (const host of state.hosts) {
    const status = statuses.get(host.id);
    if (status?.kind === "reachable") reachable += 1;
    else if (status?.kind === "unreachable") unreachable += 1;
    else unchecked += 1;
  }

  const total = state.hosts.length;
  const reachablePct = (reachable / total) * 100;
  const unreachablePct = (unreachable / total) * 100;

  return (
    <div className="card">
      <div className="card-body stack">
        <div className="row-inline spread">
          <span className="field-label">Registered hosts</span>
          <Badge tone={unreachable > 0 ? "error" : "neutral"}>
            {reachable}/{total} reachable
          </Badge>
        </div>

        <div className="reach-bar" aria-hidden="true">
          <div
            className="reach-seg"
            style={{ width: `${reachablePct}%`, background: "var(--success)" }}
          />
          <div
            className="reach-seg"
            style={{ width: `${unreachablePct}%`, background: "var(--danger)" }}
          />
        </div>

        <div className="reach-legend">
          <span>
            <i className="reach-dot" style={{ background: "var(--success)" }} />
            {reachable} reachable
          </span>
          <span>
            <i className="reach-dot" style={{ background: "var(--danger)" }} />
            {unreachable} unreachable
          </span>
          <span>
            <i className="reach-dot" style={{ background: "var(--neutral)" }} />
            {unchecked} not checked
          </span>
        </div>

        <div className="stack-sm">
          {state.hosts.slice(0, 4).map((host) => {
            const status = remoteStatus(statuses.get(host.id) ?? { kind: "unknown" });
            return (
              <div className="row-inline spread" key={host.id}>
                <div className="row-inline" style={{ minWidth: 0 }}>
                  <SymbolicIcon name="network-server-symbolic" size={14} />
                  <span className="truncate" style={{ fontSize: 12.5 }}>
                    {host.name}
                  </span>
                </div>
                <StatusBadge status={status} />
              </div>
            );
          })}
        </div>

        <button
          type="button"
          className="btn btn-sm"
          style={{ alignSelf: "flex-start" }}
          onClick={() => navigate({ page: "hosts" })}
        >
          Manage hosts <ArrowRight size={13} />
        </button>
      </div>
    </div>
  );
}

function QuickAction({
  icon,
  title,
  description,
  onClick,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="quick-action" onClick={onClick}>
      <div className="quick-action-icon">{icon}</div>
      <div style={{ minWidth: 0 }}>
        <div className="quick-action-title">{title}</div>
        <div className="quick-action-desc">{description}</div>
      </div>
      <ArrowRight size={15} style={{ marginLeft: "auto", color: "var(--fg-muted)" }} />
    </button>
  );
}

function countStatuses(rows: TunnelRow[]) {
  const counts = { healthy: 0, degraded: 0, down: 0, inactive: 0, other: 0 };
  for (const row of rows) {
    switch (row.tunnel.status.kind) {
      case "healthy":
        counts.healthy += 1;
        break;
      case "degraded":
        counts.degraded += 1;
        break;
      case "down":
        counts.down += 1;
        break;
      case "inactive":
        counts.inactive += 1;
        break;
      default:
        counts.other += 1;
    }
  }
  return counts;
}

function totalHostnames(rows: TunnelRow[]): number | null {
  const known = rows.filter((row) => row.hostname_count !== null);
  if (known.length === 0 && rows.length > 0) return null;
  return known.reduce((sum, row) => sum + (row.hostname_count ?? 0), 0);
}
