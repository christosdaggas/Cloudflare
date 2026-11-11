import type { ReactNode } from "react";
import {
  ArrowLeftRight,
  ArrowRight,
  ChevronRight,
  Globe,
  Network,
  Plus,
  Server,
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
import {
  category,
  formatTime,
  remoteStatus,
  severity,
  tunnelStatus,
} from "../status";
import type { AppState, TunnelRow } from "../types";

type FilterName = "healthy" | "degraded" | "down" | "inactive";

export function Dashboard() {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);
  const setTunnelFilter = useApp((store) => store.setTunnelFilter);
  const openDialog = useApp((store) => store.openDialog);

  if (!state) return <LoadingState message="Reading the tunnel list…" />;

  const counts = countStatuses(state.tunnels);
  const attention = counts.degraded + counts.down;
  const hostnames = totalHostnames(state.tunnels);
  const connectors = state.tunnels.reduce(
    (sum, row) => sum + row.tunnel.connector_count,
    0,
  );
  // Show problems first, while keeping a stable order within each status.
  const priority = { down: 0, degraded: 1, inactive: 2, other: 3, healthy: 4 };
  const preview = [...state.tunnels]
    .sort(
      (a, b) => priority[a.tunnel.status.kind] - priority[b.tunnel.status.kind],
    )
    .slice(0, 4);

  function show(status: FilterName) {
    setTunnelFilter(status);
    navigate({ page: "tunnels" });
  }

  return (
    <div className="content-inner">
      {state.tunnels_error && (
        <Banner tone="error" title="Could not load tunnels">
          {state.tunnels_error}
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
      {!state.secrets_available && (
        <Banner
          tone="warning"
          icon="dialog-warning-symbolic"
          title="Credential store unavailable"
        >
          Your token works for this session, but cannot be saved on this
          machine.
        </Banner>
      )}

      {state.tunnels.length === 0 && !state.tunnels_error ? (
        <div className="card">
          <EmptyState
            icon="network-vpn-symbolic"
            title="Your network starts here"
            description="Create a tunnel to connect your services to Cloudflare. You can add hostnames and deploy a connector when you’re ready."
            action={
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => openDialog({ kind: "create-tunnel" })}
              >
                <Plus size={16} aria-hidden="true" /> Create your first tunnel
              </button>
            }
          />
        </div>
      ) : (
        <div className="dashboard-grid">
          <div className="dashboard-column">
            <section className="section" aria-labelledby="health-heading">
              <div className="section-heading">
                <h2 id="health-heading">Tunnel health</h2>
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => navigate({ page: "tunnels" })}
                >
                  View tunnels <ArrowRight size={13} aria-hidden="true" />
                </button>
              </div>
              <div className="card card-body stack">
                <div className="health-overview">
                  <div className="health-total">
                    <strong>{state.tunnels.length}</strong> tunnels
                  </div>
                  <Badge tone="neutral" icon="network-vpn-symbolic">
                    Cloudflare
                  </Badge>
                </div>
                <div className="health-bar" aria-hidden="true">
                  {(["healthy", "degraded", "down", "inactive"] as const).map(
                    (kind) => {
                      const count =
                        counts[kind] + (kind === "inactive" ? counts.other : 0);
                      return (
                        count > 0 && (
                          <span
                            key={kind}
                            className={kind}
                            style={{ flex: count }}
                          />
                        )
                      );
                    },
                  )}
                </div>
                <div className="stat-grid">
                  <StatCard
                    value={counts.healthy}
                    label="Healthy"
                    detail="All edge connections established"
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
                    detail="No active edge connections"
                    icon="dialog-error-symbolic"
                    tone="error"
                    onClick={() => show("down")}
                  />
                  <StatCard
                    value={counts.inactive + counts.other}
                    label="Inactive"
                    detail="Not currently serving traffic"
                    icon="media-playback-pause-symbolic"
                    tone="neutral"
                    onClick={() => show("inactive")}
                  />
                </div>
              </div>
              {attention > 0 && (
                <Banner
                  tone="warning"
                  icon="dialog-warning-symbolic"
                  title={`${attention} tunnel${attention === 1 ? " needs" : "s need"} attention`}
                  action={
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={() =>
                        show(counts.down > 0 ? "down" : "degraded")
                      }
                    >
                      Review
                    </button>
                  }
                >
                  {counts.down > 0 && `${counts.down} down`}
                  {counts.down > 0 && counts.degraded > 0 && " · "}
                  {counts.degraded > 0 && `${counts.degraded} degraded`}. Check
                  connectors on the affected tunnels.
                </Banner>
              )}
            </section>

            <section className="section" aria-labelledby="resources-heading">
              <div className="section-heading">
                <h2 id="resources-heading">Resources</h2>
              </div>
              <div className="group resource-grid">
                <ResourceMetric
                  icon={<ArrowLeftRight size={17} />}
                  value={hostnames ?? "—"}
                  label="Hostnames"
                  onClick={() => navigate({ page: "hostnames" })}
                />
                <ResourceMetric
                  icon={<Network size={17} />}
                  value={connectors}
                  label="Connectors"
                  onClick={() => navigate({ page: "tunnels" })}
                />
                <ResourceMetric
                  icon={<Globe size={17} />}
                  value={state.zones.length}
                  label="Zones"
                  onClick={() => navigate({ page: "zones" })}
                />
              </div>
              {hostnames === null && (
                <div className="field-hint">
                  Hostname counts are still loading.
                </div>
              )}
            </section>

            {preview.length > 0 && (
              <section className="section" aria-labelledby="tunnels-heading">
                <div className="section-heading">
                  <h2 id="tunnels-heading">Your tunnels</h2>
                  <span className="caption">
                    {attention > 0
                      ? "Needs attention first"
                      : `${state.tunnels.length} total`}
                  </span>
                </div>
                <div className="group">
                  {preview.map((row) => (
                    <button
                      type="button"
                      className="row tunnel-preview"
                      key={row.tunnel.id}
                      onClick={() =>
                        navigate({ page: "tunnel", id: row.tunnel.id })
                      }
                    >
                      <div className="row-icon">
                        <Waypoints size={17} aria-hidden="true" />
                      </div>
                      <div className="row-main">
                        <div className="row-title truncate">
                          {row.tunnel.name}
                        </div>
                        <div className="row-subtitle">
                          {row.tunnel.connector_count} connector
                          {row.tunnel.connector_count === 1 ? "" : "s"}
                        </div>
                      </div>
                      <StatusBadge status={tunnelStatus(row.tunnel.status)} />
                      <ChevronRight size={14} aria-hidden="true" />
                    </button>
                  ))}
                </div>
              </section>
            )}
          </div>

          <div className="dashboard-column">
            <section className="section" aria-labelledby="hosts-heading">
              <div className="section-heading">
                <h2 id="hosts-heading">Remote hosts</h2>
                <span className="caption">Optional SSH control</span>
              </div>
              <ReachCard state={state} />
            </section>

            <section className="section" aria-labelledby="actions-heading">
              <div className="section-heading">
                <h2 id="actions-heading">Quick actions</h2>
              </div>
              <div className="group quick-actions">
                <QuickAction
                  icon={<Waypoints size={17} />}
                  title="Create a tunnel"
                  description="Connect a new service to your network"
                  onClick={() => openDialog({ kind: "create-tunnel" })}
                />
                <QuickAction
                  icon={<ArrowLeftRight size={17} />}
                  title="Publish a hostname"
                  description="Route a public hostname to an origin"
                  onClick={() =>
                    openDialog({ kind: "hostname", tunnel_id: null })
                  }
                />
                <QuickAction
                  icon={<Server size={17} />}
                  title="Add a remote host"
                  description="Deploy and manage a connector over SSH"
                  onClick={() => openDialog({ kind: "host" })}
                />
              </div>
            </section>

            <section className="section" aria-labelledby="activity-heading">
              <div className="section-heading">
                <h2 id="activity-heading">Recent activity</h2>
                <button
                  type="button"
                  className="btn-link"
                  onClick={() => navigate({ page: "activity" })}
                >
                  View all <ArrowRight size={13} aria-hidden="true" />
                </button>
              </div>
              <div className="group">
                {state.activity.length === 0 ? (
                  <div className="card-body field-hint">
                    Requests and their outcomes will appear here.
                  </div>
                ) : (
                  state.activity.slice(0, 3).map((event) => {
                    const meta = severity(event.severity);
                    return (
                      <div className="activity-row" key={event.id}>
                        <div className={`activity-icon tone-${meta.tone}`}>
                          <SymbolicIcon name={meta.icon} size={14} />
                        </div>
                        <div className="activity-main">
                          <div className="activity-summary">
                            {event.summary}
                          </div>
                          <div className="activity-meta">
                            <span>{formatTime(event.at)}</span>
                            <span aria-hidden="true">·</span>
                            <span>{category(event.category)}</span>
                          </div>
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </section>
          </div>
        </div>
      )}
    </div>
  );
}

function ReachCard({ state }: { state: AppState }) {
  const openDialog = useApp((store) => store.openDialog);
  const navigate = useApp((store) => store.navigate);

  if (state.hosts.length === 0) {
    return (
      <div className="card reach-empty">
        <div className="empty-icon">
          <Server size={20} aria-hidden="true" />
        </div>
        <div>
          <h3>Bring your connectors closer</h3>
          <p>
            Add a host to deploy cloudflared, control its service and read logs.
            Your tunnels work without this.
          </p>
          <button
            type="button"
            className="btn"
            onClick={() => openDialog({ kind: "host" })}
          >
            <Plus size={15} aria-hidden="true" /> Add a host
          </button>
        </div>
      </div>
    );
  }

  const statuses = new Map(
    state.host_status.map((item) => [item.id, item.status]),
  );
  const reachable = state.hosts.filter(
    (host) => statuses.get(host.id)?.kind === "reachable",
  ).length;
  const unreachable = state.hosts.filter(
    (host) => statuses.get(host.id)?.kind === "unreachable",
  ).length;
  const unchecked = state.hosts.length - reachable - unreachable;

  return (
    <div className="card">
      <div className="card-body stack">
        <div className="row-inline spread">
          <span className="field-label">{state.hosts.length} registered</span>
          <Badge
            tone={unreachable > 0 ? "error" : "neutral"}
            icon="network-server-symbolic"
          >
            {reachable}/{state.hosts.length} reachable
          </Badge>
        </div>
        <div className="reach-bar" aria-hidden="true">
          <div
            className="reach-seg"
            style={{
              width: `${(reachable / state.hosts.length) * 100}%`,
              background: "var(--success)",
            }}
          />
          <div
            className="reach-seg"
            style={{
              width: `${(unreachable / state.hosts.length) * 100}%`,
              background: "var(--danger)",
            }}
          />
        </div>
        <div className="reach-legend">
          <span>{reachable} reachable</span>
          <span>{unreachable} unreachable</span>
          <span>{unchecked} unchecked</span>
        </div>
        {state.hosts.slice(0, 3).map((host) => (
          <div className="row-inline spread" key={host.id}>
            <span className="truncate">{host.name}</span>
            <StatusBadge
              status={remoteStatus(
                statuses.get(host.id) ?? { kind: "unknown" },
              )}
            />
          </div>
        ))}
        <button
          type="button"
          className="btn btn-sm"
          style={{ alignSelf: "flex-start" }}
          onClick={() => navigate({ page: "hosts" })}
        >
          Manage hosts <ArrowRight size={13} aria-hidden="true" />
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
      <div className="quick-action-icon" aria-hidden="true">
        {icon}
      </div>
      <div className="row-main">
        <div className="quick-action-title">{title}</div>
        <div className="quick-action-desc">{description}</div>
      </div>
      <ChevronRight size={15} className="dimmed" aria-hidden="true" />
    </button>
  );
}

function ResourceMetric({
  icon,
  value,
  label,
  onClick,
}: {
  icon: ReactNode;
  value: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button type="button" className="resource-metric" onClick={onClick}>
      <span aria-hidden="true">{icon}</span>
      <strong>{value}</strong>
      <span>{label}</span>
    </button>
  );
}

function countStatuses(rows: TunnelRow[]) {
  const counts = { healthy: 0, degraded: 0, down: 0, inactive: 0, other: 0 };
  for (const row of rows) counts[row.tunnel.status.kind] += 1;
  return counts;
}

function totalHostnames(rows: TunnelRow[]): number | null {
  // A partial sum looks authoritative, so wait until every count is known.
  if (rows.some((row) => row.hostname_count === null)) return null;
  return rows.reduce((sum, row) => sum + (row.hostname_count ?? 0), 0);
}
