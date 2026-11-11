import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import {
  ArrowLeftRight,
  Copy,
  FileCode2,
  History,
  LayoutDashboard,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Server,
  Trash2,
  Unplug,
  Waypoints,
} from "lucide-react";

import { api, errorText } from "../api";
import { copyRunToken } from "../actions";
import { Dialog } from "../components/Dialog";
import { HostnameRow, displayName, originRequestRows } from "../components/HostnameRow";
import { OutcomeDialog } from "../components/OutcomeView";
import {
  Badge,
  Banner,
  CodeBlock,
  EmptyState,
  Field,
  LoadingState,
  Notice,
  Property,
  StatusBadge,
} from "../components/ui";
import { useConfirm } from "../components/useConfirm";
import { SymbolicIcon } from "../icons";
import { useApp } from "../store";
import {
  configSource,
  formatActivityTime,
  formatExactTime,
  formatTime,
  severity,
  tunnelExplanation,
  tunnelKind,
  tunnelLastActive,
  tunnelStatus,
} from "../status";
import type {
  Connector,
  DeleteImpact,
  Outcome,
  PublicHostname,
  Tunnel,
  TunnelRow,
} from "../types";

type Tab = "overview" | "hostnames" | "connectors" | "configuration" | "hosts" | "activity";

interface OutcomeState {
  title: string;
  outcome: Outcome;
  hostname: string;
  path: string | null;
  zoneId: string | null;
}

export function TunnelDetail({ tunnelId }: { tunnelId: string }) {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);
  const openDialog = useApp((store) => store.openDialog);
  const toast = useApp((store) => store.toast);
  const setState = useApp((store) => store.setState);
  const hostnameVersion = useApp((store) => store.hostnameVersion);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const row = state?.tunnels.find((item) => item.tunnel.id === tunnelId);
  const tunnel = row?.tunnel;

  const [tab, setTab] = useState<Tab>("overview");
  const [hostnames, setHostnames] = useState<PublicHostname[] | null>(null);
  const [hostnamesError, setHostnamesError] = useState<string | null>(null);
  const [connectors, setConnectors] = useState<Connector[] | null>(null);
  const [connectorsError, setConnectorsError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<OutcomeState | null>(null);
  const [renameOpen, setRenameOpen] = useState(false);
  const [renameValue, setRenameValue] = useState("");
  const [manual, setManual] = useState<string | null>(null);
  const [selectedEvent, setSelectedEvent] = useState<number | null>(null);

  const reloadHostnames = useCallback(async () => {
    try {
      const list = await api.loadHostnames(tunnelId);
      setHostnames(list);
      setHostnamesError(null);
    } catch (error) {
      setHostnamesError(errorText(error));
    }
  }, [tunnelId]);

  const reloadConnectors = useCallback(async () => {
    try {
      const list = await api.listConnectors(tunnelId);
      setConnectors(list);
      setConnectorsError(null);
    } catch (error) {
      setConnectorsError(errorText(error));
    }
  }, [tunnelId]);

  useEffect(() => {
    let cancelled = false;
    setHostnames(null);
    setHostnamesError(null);
    api
      .loadHostnames(tunnelId)
      .then((list) => {
        if (!cancelled) setHostnames(list);
      })
      .catch((error) => {
        if (!cancelled) setHostnamesError(errorText(error));
      });
    return () => {
      cancelled = true;
    };
  }, [tunnelId, tunnel?.name, hostnameVersion]);

  useEffect(() => {
    let cancelled = false;
    setConnectors(null);
    setConnectorsError(null);
    api
      .listConnectors(tunnelId)
      .then((list) => {
        if (!cancelled) setConnectors(list);
      })
      .catch((error) => {
        if (!cancelled) setConnectorsError(errorText(error));
      });
    return () => {
      cancelled = true;
    };
  }, [tunnelId]);

  const hostsForTunnel = useMemo(
    () => state?.hosts.filter((host) => host.tunnel_id === tunnelId) ?? [],
    [state?.hosts, tunnelId],
  );
  const activityForTunnel = useMemo(
    () =>
      (state?.activity ?? [])
        .filter(
          (event) =>
            event.target === tunnel?.name || event.target === tunnel?.id,
        )
        .slice(0, 30),
    [state?.activity, tunnel?.name, tunnel?.id],
  );

  if (!state) return <LoadingState message="Reading the tunnel…" />;
  if (!row || !tunnel) {
    return (
      <div className="content-inner">
        <div className="card">
          <EmptyState
            icon="dialog-question-symbolic"
            title="Tunnel not found"
            description="This tunnel is no longer in the account's list. It may have been deleted."
            action={
              <button
                type="button"
                className="btn"
                onClick={() => navigate({ page: "tunnels" })}
              >
                Back to tunnels
              </button>
            }
          />
        </div>
      </div>
    );
  }

  const status = tunnelStatus(tunnel.status);
  const lastActive = tunnelLastActive(tunnel);

  async function runHostnameAction(
    title: string,
    hostname: PublicHostname,
    action: () => Promise<Outcome>,
  ) {
    setBusy(title);
    try {
      const result = await action();
      setOutcome({
        title,
        outcome: result,
        hostname: hostname.hostname,
        path: hostname.path,
        zoneId: hostname.zone_id,
      });
      await reloadHostnames();
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  async function askDelete() {
    if (!tunnel) return;
    setBusy("delete");
    try {
      const impact = await api.deleteTunnelImpact(tunnelId);
      confirm({
        title: `Delete “${tunnel.name}”?`,
        description:
          "The tunnel is removed from Cloudflare. DNS records are deliberately left in place.",
        body: <DeleteImpactBody impact={impact} />,
        confirmLabel: "Delete Tunnel",
        destructive: true,
        run: async () => {
          const next = await api.deleteTunnel(tunnelId);
          setState(next);
          toast(`Deleted “${tunnel.name}”.`, "success");
          navigate({ page: "tunnels" });
        },
      });
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  async function saveRename() {
    const name = renameValue.trim();
    if (!name) return;
    setBusy("rename");
    try {
      const next = await api.renameTunnel(tunnelId, name);
      setState(next);
      setRenameOpen(false);
      toast("Tunnel renamed.", "success");
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  async function askDisable(hostname: PublicHostname) {
    const name = displayName(hostname);
    confirm({
      title: `Disable ${name}?`,
      description:
        "The ingress rule stays in the tunnel's configuration, but its service becomes http_status:404. The origin is remembered so Enable restores it exactly.",
      body: (
        <div className="stack-sm">
          <div className="row-inline spread">
            <span className="field-label">Origin</span>
            <span className="mono">{hostname.raw_service}</span>
          </div>
          <Notice>
            Visitors get a 404 while the hostname is disabled. DNS is not touched.
          </Notice>
        </div>
      ),
      confirmLabel: "Disable Hostname",
      run: async () => {
        const result = await api.disableHostname({
          tunnel_id: tunnelId,
          hostname: hostname.hostname,
          path: hostname.path,
          service: hostname.raw_service,
        });
        setOutcome({
          title: "Disable hostname",
          outcome: result,
          hostname: hostname.hostname,
          path: hostname.path,
          zoneId: hostname.zone_id,
        });
        await reloadHostnames();
      },
    });
  }

  async function askUnpublish(hostname: PublicHostname) {
    const name = displayName(hostname);
    confirm({
      title: `Unpublish ${name}?`,
      description:
        "The ingress rule is removed and only the DNS record that points at this tunnel is deleted.",
      body: (
        <div className="stack-sm">
          <div className="row-inline spread">
            <span className="field-label">Origin</span>
            <span className="mono">{hostname.raw_service}</span>
          </div>
          <Notice>
            An A, AAAA or CNAME record belonging to something else is never removed.
          </Notice>
        </div>
      ),
      confirmLabel: "Unpublish",
      destructive: true,
      run: async () => {
        const result = await api.unpublishHostname(
          tunnelId,
          hostname.hostname,
          hostname.path,
          hostname.zone_id,
        );
        setOutcome({
          title: "Unpublish hostname",
          outcome: result,
          hostname: hostname.hostname,
          path: hostname.path,
          zoneId: hostname.zone_id,
        });
        await reloadHostnames();
      },
    });
  }

  function askDisconnect(connector: Connector) {
    confirm({
      title: "Deregister this connector?",
      description: `Cloudflare will remove the connector ${connector.id} from this tunnel.`,
      body: (
        <Notice icon="dialog-warning-symbolic">
          The <span className="mono">cloudflared</span> process on that machine keeps
          running and will reconnect. Stopping the service is a separate, remote
          operation on the Remote Hosts page.
        </Notice>
      ),
      confirmLabel: "Deregister",
      destructive: true,
      run: async () => {
        await api.disconnectConnector(tunnelId, connector.id);
        await reloadConnectors();
      },
    });
  }

  async function showManual() {
    setBusy("manual");
    try {
      setManual(await api.manualSetupInstructions(tunnelId));
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  const tabs: { id: Tab; label: string; icon: ReactNode; count?: number }[] = [
    { id: "overview", label: "Overview", icon: <LayoutDashboard size={15} /> },
    {
      id: "hostnames",
      label: "Hostnames",
      icon: <ArrowLeftRight size={15} />,
      count: hostnames?.length,
    },
    {
      id: "connectors",
      label: "Connectors",
      icon: <Waypoints size={15} />,
      count: connectors?.length,
    },
    { id: "configuration", label: "Configuration", icon: <FileCode2 size={15} /> },
    {
      id: "hosts",
      label: "Hosts",
      icon: <Server size={15} />,
      count: hostsForTunnel.length,
    },
    {
      id: "activity",
      label: "Activity",
      icon: <History size={15} />,
      count: activityForTunnel.length,
    },
  ];

  return (
    <div className="content-inner">
      <div className="detail-head">
        <div className="detail-head-main">
          <div className="detail-title">
            <h1>{tunnel.name}</h1>
            <StatusBadge status={status} large />
            {tunnel.config_source.kind !== "cloudflare" && (
              <Badge tone="warning" icon="dialog-warning-symbolic">
                {configSource(tunnel.config_source)}
              </Badge>
            )}
          </div>
          <div className="detail-subtitle">
            <span className="mono">{tunnel.id}</span>
            <span>·</span>
            <span>{tunnelKind(tunnel.kind)}</span>
            <span>·</span>
            <span>Last active {formatTime(lastActive)}</span>
          </div>
        </div>

        <div className="detail-actions">
          <button
            type="button"
            className="btn"
            onClick={() => void copyRunToken(tunnelId)}
          >
            <Copy size={14} /> Run Token
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => {
              setRenameValue(tunnel.name);
              setRenameOpen(true);
            }}
          >
            <Pencil size={14} /> Rename
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => openDialog({ kind: "reinstall", tunnel_id: tunnelId })}
          >
            <RotateCcw size={14} /> Reinstall
          </button>
          <button
            type="button"
            className="btn btn-danger"
            onClick={() => void askDelete()}
            disabled={busy === "delete"}
          >
            {busy === "delete" ? <Loader2 size={14} className="spin" /> : <Trash2 size={14} />}
            Delete
          </button>
        </div>
      </div>

      <div className="tabs" role="tablist">
        {tabs.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            aria-selected={tab === item.id}
            className={`tab${tab === item.id ? " active" : ""}`}
            onClick={() => setTab(item.id)}
          >
            {item.icon}
            {item.label}
            {item.count !== undefined && <span className="tab-count">{item.count}</span>}
          </button>
        ))}
      </div>

      {tab === "overview" && (
        <div className="stack">
          <Banner
            tone={status.tone === "error" ? "error" : status.tone === "warning" ? "warning" : "info"}
            icon={status.icon}
            title={status.label}
          >
            {tunnelExplanation(tunnel.status)}
          </Banner>

          {tunnel.config_source.kind === "local" && (
            <Banner tone="warning" icon="dialog-warning-symbolic" title="Configured on the connector's machine">
              This tunnel reads its ingress from a <span className="mono">config.yml</span>.
              Writing a remote configuration would be ignored by cloudflared, so hostname
              and ingress editing is disabled here. The tunnel's health still comes from
              Cloudflare.
            </Banner>
          )}

          {tunnel.connector_count === 0 && (
            <Notice icon="dialog-information-symbolic">
              No connector is registered. Copy the run token and run{" "}
              <span className="mono">cloudflared tunnel run</span> somewhere, or deploy a
              connector from Remote Hosts.{" "}
              <button type="button" className="btn-link" onClick={() => openDialog({ kind: "reinstall", tunnel_id: tunnelId })}>
                Reinstall instructions
              </button>
            </Notice>
          )}

          <PropertyGrid row={row} tunnel={tunnel} />
        </div>
      )}

      {tab === "hostnames" && (
        <div className="stack">
          <div className="row-inline spread">
            <div className="field-hint" style={{ maxWidth: 620 }}>
              A public hostname is an ingress rule in this tunnel plus a proxied CNAME in
              its zone. Either can exist without the other; the badge says which half is
              missing.
            </div>
            <div className="row-inline">
              <button type="button" className="btn" onClick={() => void reloadHostnames()}>
                <RefreshCw size={14} /> Refresh
              </button>
              <button
                type="button"
                className="btn btn-primary"
                disabled={!tunnel.remote_config}
                onClick={() => openDialog({ kind: "hostname", tunnel_id: tunnelId })}
              >
                <Plus size={14} /> Publish Hostname
              </button>
            </div>
          </div>

          {hostnamesError && (
            <Banner tone="error" icon="dialog-error-symbolic" title="Could not read the hostnames">
              {hostnamesError}
            </Banner>
          )}

          {hostnames === null ? (
            <LoadingState message="Reading the ingress configuration and DNS…" />
          ) : hostnames.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="network-transmit-receive-symbolic"
                title="No public hostnames"
                description="This tunnel has no ingress rule with a hostname yet. Publish one to send a domain to an origin."
                action={
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => openDialog({ kind: "hostname", tunnel_id: tunnelId })}
                  >
                    <Plus size={15} /> Publish a hostname
                  </button>
                }
              />
            </div>
          ) : (
            <div className="stack-sm">
              {hostnames.map((hostname) => (
                <HostnameRow
                  key={`${hostname.hostname}${hostname.path ?? ""}-${hostname.rule_index}`}
                  hostname={hostname}
                  tunnelName={tunnel.name}
                  busy={busy !== null}
                  onEdit={() =>
                    openDialog({ kind: "hostname", tunnel_id: tunnelId, hostname })
                  }
                  onRepair={() =>
                    void runHostnameAction("Repair DNS", hostname, () =>
                      api.repairDns(hostname.zone_id!, tunnelId, hostname.hostname),
                    )
                  }
                  onDisable={() => void askDisable(hostname)}
                  onEnable={() =>
                    void runHostnameAction("Enable hostname", hostname, () =>
                      api.enableHostname(tunnelId, hostname.hostname, hostname.path),
                    )
                  }
                  onUnpublish={() => void askUnpublish(hostname)}
                />
              ))}
            </div>
          )}
        </div>
      )}

      {tab === "connectors" && (
        <div className="stack">
          <div className="row-inline spread">
            <div className="field-hint" style={{ maxWidth: 620 }}>
              A connector is one <span className="mono">cloudflared</span> process registered
              against this tunnel. Its connections are the four edge links Cloudflare
              reports, which are separate from the systemd state on the machine.
            </div>
            <button type="button" className="btn" onClick={() => void reloadConnectors()}>
              <RefreshCw size={14} /> Refresh
            </button>
          </div>

          {connectorsError && (
            <Banner tone="error" icon="dialog-error-symbolic" title="Could not read the connectors">
              {connectorsError}
            </Banner>
          )}

          {connectors === null ? (
            <LoadingState message="Reading the connectors…" />
          ) : connectors.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="media-playback-pause-symbolic"
                title="No connector is registered"
                description="Nothing is serving this tunnel yet. Run cloudflared with the tunnel's token, or deploy a connector to a registered host."
                action={
                  <button type="button" className="btn" onClick={() => void showManual()}>
                    <FileCode2 size={15} /> Manual setup
                  </button>
                }
              />
            </div>
          ) : (
            <div className="stack-sm">
              {connectors.map((connector) => (
                <ConnectorCard
                  key={connector.id}
                  connector={connector}
                  busy={busy !== null}
                  onDisconnect={() => askDisconnect(connector)}
                />
              ))}
            </div>
          )}
        </div>
      )}

      {tab === "configuration" && (
        <div className="stack">
          <Banner
            tone="info"
            icon="dialog-information-symbolic"
            title="The ingress list is written back whole"
          >
            Every rule this application did not change — including the required catch-all
            and fields it does not understand — is preserved exactly on each write.
          </Banner>

          {tunnel.config_source.kind === "local" ? (
            <Notice icon="dialog-warning-symbolic">
              This tunnel is configured by a <span className="mono">config.yml</span> on the
              connector's machine, so there is no remote configuration to show or edit.
            </Notice>
          ) : hostnames === null ? (
            <LoadingState message="Reading the configuration…" />
          ) : hostnames.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="document-edit-symbolic"
                title="The configuration is empty"
                description="No ingress rule with a hostname exists yet. Publishing one adds it here."
              />
            </div>
          ) : (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Hostname</th>
                    <th>Service</th>
                    <th className="hide-sm">Origin settings</th>
                    <th className="num">Rule</th>
                  </tr>
                </thead>
                <tbody>
                  {hostnames.map((hostname) => (
                    <tr key={`${hostname.hostname}${hostname.path ?? ""}-${hostname.rule_index}`}>
                      <td>
                        <div className="cell-main">
                          {hostname.hostname}
                          {hostname.path ?? ""}
                        </div>
                        <div className="cell-sub">
                          {hostname.zone_name ?? "Zone not on this account"}
                        </div>
                      </td>
                      <td className="mono">{hostname.raw_service}</td>
                      <td className="hide-sm">
                        {originRequestRows(hostname.origin_request).length === 0
                          ? "cloudflared defaults"
                          : `${originRequestRows(hostname.origin_request).length} override(s)`}
                      </td>
                      <td className="num">#{hostname.rule_index + 1}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <Notice>
            The catch-all rule at the end of the list is not shown here. It is required by
            cloudflared and is preserved on every write.
          </Notice>
        </div>
      )}

      {tab === "hosts" && (
        <div className="stack">
          {hostsForTunnel.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="network-server-symbolic"
                title="No registered host serves this tunnel"
                description="Remote control is optional. Add a host profile to start, stop, deploy and inspect cloudflared over SSH."
                action={
                  <div className="row-inline">
                    <button
                      type="button"
                      className="btn"
                      onClick={() => openDialog({ kind: "host" })}
                    >
                      <Plus size={15} /> Add a host
                    </button>
                    <button type="button" className="btn" onClick={() => void showManual()}>
                      <FileCode2 size={15} /> Manual setup
                    </button>
                  </div>
                }
              />
            </div>
          ) : (
            <>
              <div className="group">
                {hostsForTunnel.map((host) => {
                  const statusEntry = state.host_status.find((item) => item.id === host.id);
                  const remote = statusEntry?.status;
                  const tone =
                    remote?.kind === "reachable"
                      ? "success"
                      : remote?.kind === "unreachable"
                        ? "error"
                        : "neutral";
                  const label =
                    remote?.kind === "reachable"
                      ? "Reachable"
                      : remote?.kind === "unreachable"
                        ? "Unreachable"
                        : remote?.kind === "checking"
                          ? "Checking…"
                          : "Not checked";
                  return (
                    <div className="row" key={host.id}>
                      <div className="row-icon">
                        <Server size={16} />
                      </div>
                      <div className="row-main">
                        <div className="row-title">{host.name}</div>
                        <div className="row-subtitle">
                          {host.username}@{host.address}:{host.port} · unit{" "}
                          <span className="mono">{host.service_unit}</span>
                        </div>
                      </div>
                      <Badge tone={tone}>{label}</Badge>
                      <button
                        type="button"
                        className="btn btn-sm"
                        onClick={() => navigate({ page: "hosts" })}
                      >
                        Manage
                      </button>
                    </div>
                  );
                })}
              </div>

              <div className="row-inline">
                <button type="button" className="btn" onClick={() => void showManual()}>
                  <FileCode2 size={15} /> Manual setup
                </button>
                <button
                  type="button"
                  className="btn"
                  onClick={() => openDialog({ kind: "host" })}
                >
                  <Plus size={15} /> Add another host
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {tab === "activity" && (
        <div className="stack">
          {activityForTunnel.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="document-open-recent-symbolic"
                title="Nothing recorded for this tunnel"
                description="Publishing, repairing and deleting are recorded here — and in the full Activity page."
              />
            </div>
          ) : (
            <div className="group">
              {activityForTunnel.map((event) => {
                const meta = severity(event.severity);
                return (
                  <div
                    className="activity-row clickable"
                    key={event.id}
                    onClick={() => setSelectedEvent(event.id)}
                  >
                    <div className={`activity-icon tone-${meta.tone}`}>
                      <SymbolicIcon name={meta.icon} size={14} />
                    </div>
                    <div className="activity-main">
                      <div className="activity-summary">{event.summary}</div>
                      <div className="activity-meta">
                        <span>{formatActivityTime(event.at)}</span>
                        <span>·</span>
                        <span>{meta.label}</span>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {renameOpen && (
        <Dialog
          title="Rename tunnel"
          description={tunnel.name}
          onClose={() => setRenameOpen(false)}
          narrow
          footer={
            <>
              <button className="btn" onClick={() => setRenameOpen(false)} disabled={busy === "rename"}>
                Cancel
              </button>
              <button
                className="btn btn-primary"
                onClick={() => void saveRename()}
                disabled={busy === "rename" || renameValue.trim().length === 0}
              >
                {busy === "rename" && <Loader2 size={14} className="spin" />}
                Rename
              </button>
            </>
          }
        >
          <Field label="New name" htmlFor="rename-tunnel">
            <input
              id="rename-tunnel"
              className="input"
              value={renameValue}
              autoFocus
              onChange={(event) => setRenameValue(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveRename();
              }}
            />
          </Field>
        </Dialog>
      )}

      {manual !== null && (
        <Dialog
          title="Run the connector yourself"
          description="No SSH profile is needed for this route."
          onClose={() => setManual(null)}
          wide
          footer={
            <button className="btn btn-primary" onClick={() => setManual(null)}>
              Close
            </button>
          }
        >
          <div className="stack">
            <CodeBlock code={manual} label="On the machine that should serve this tunnel" />
            <Notice>
              Use <strong>Run Token</strong> above to put the token on the clipboard. It is
              never printed into these instructions.
            </Notice>
          </div>
        </Dialog>
      )}

      {selectedEvent !== null && (() => {
        const event = activityForTunnel.find((item) => item.id === selectedEvent);
        if (!event) return null;
        const meta = severity(event.severity);
        return (
          <Dialog
            title="Activity event"
            description={formatActivityTime(event.at)}
            onClose={() => setSelectedEvent(null)}
            footer={
              <button className="btn btn-primary" onClick={() => setSelectedEvent(null)}>
                Close
              </button>
            }
          >
            <div className="stack">
              <div className="row-inline spread">
                <Badge tone={meta.tone} icon={meta.icon}>
                  {meta.label}
                </Badge>
                {event.target && <span className="mono field-hint">{event.target}</span>}
              </div>
              <p style={{ margin: 0 }}>{event.summary}</p>
              {event.detail && <CodeBlock code={event.detail} label="Detail" />}
            </div>
          </Dialog>
        );
      })()}

      {outcome && (
        <OutcomeDialog
          title={outcome.title}
          outcome={outcome.outcome}
          onClose={() => setOutcome(null)}
          onRepairDns={
            outcome.zoneId
              ? async () => {
                  const result = await api.repairDns(
                    outcome.zoneId!,
                    tunnelId,
                    outcome.hostname,
                  );
                  setOutcome({ ...outcome, title: "Repair DNS", outcome: result });
                  await reloadHostnames();
                }
              : undefined
          }
          onRollback={async () => {
            await api.rollbackIngress(tunnelId, outcome.hostname, outcome.path);
            setOutcome(null);
            await reloadHostnames();
          }}
          onOpenSettings={() => {
            setOutcome(null);
            navigate({ page: "settings" });
          }}
          onOpenDashboard={() => {
            void api.openUrl(
              "https://dash.cloudflare.com/?to=/:account/:zone/dns/records",
            );
          }}
        />
      )}

      {confirmDialog}
    </div>
  );
}

function PropertyGrid({ row, tunnel }: { row: TunnelRow; tunnel: Tunnel }) {
  return (
    <div className="property-grid">
      <Property label="Tunnel ID" value={tunnel.id} mono />
      <Property label="Account tag" value={tunnel.account_tag || "—"} mono />
      <Property label="Kind" value={tunnelKind(tunnel.kind)} />
      <Property label="Configuration" value={configSource(tunnel.config_source)} />
      <Property label="Created" value={formatExactTime(tunnel.created_at)} />
      <Property label="Last active" value={formatExactTime(tunnelLastActive(tunnel))} />
      <Property label="Connectors" value={String(tunnel.connector_count)} />
      <Property
        label="Public hostnames"
        value={row.hostname_count === null ? "Reading…" : String(row.hostname_count)}
      />
      <Property
        label="Managed hosts"
        value={row.managed_hosts.length > 0 ? row.managed_hosts.join(", ") : "None"}
      />
      <Property label="CNAME target" value={`${tunnel.id}.cfargotunnel.com`} mono />
    </div>
  );
}

function DeleteImpactBody({ impact }: { impact: DeleteImpact }) {
  const warnings: string[] = [];
  if (impact.hostnames.length > 0) {
    warnings.push(
      `${impact.hostnames.length} public hostname(s) will stop working: ${impact.hostnames.join(", ")}.`,
    );
  }
  if (impact.orphaned_dns.length > 0) {
    warnings.push(
      `${impact.orphaned_dns.length} hostname(s) may keep a DNS record pointing at a tunnel that no longer exists. Visitors see a 1016 error until it is removed.`,
    );
  }
  if (impact.connector_count > 0) {
    warnings.push(
      `${impact.connector_count} connector(s) are registered and will be disconnected.`,
    );
  }
  if (impact.managed_hosts.length > 0) {
    warnings.push(
      `${impact.managed_hosts.length} registered host(s) will be detached from this tunnel but not otherwise changed: ${impact.managed_hosts.join(", ")}.`,
    );
  }

  if (warnings.length === 0) {
    return <Notice>Nothing is currently attached to this tunnel.</Notice>;
  }

  return (
    <div className="stack-sm">
      {warnings.map((warning) => (
        <Notice key={warning} icon="dialog-warning-symbolic">
          {warning}
        </Notice>
      ))}
    </div>
  );
}

function ConnectorCard({
  connector,
  busy,
  onDisconnect,
}: {
  connector: Connector;
  busy: boolean;
  onDisconnect: () => void;
}) {
  const [open, setOpen] = useState(false);
  const colos = [...new Set(connector.connections.map((item) => item.colo_name).filter(Boolean))].sort();
  const originIp = connector.connections.find((item) => item.origin_ip)?.origin_ip;
  const connected = connector.connections.some((item) => !item.is_pending_reconnect);

  return (
    <div className="group">
      <div
        className="row"
        style={{ cursor: "pointer" }}
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            setOpen((value) => !value);
          }
        }}
      >
        <div className="row-icon">
          <Unplug size={16} />
        </div>
        <div className="row-main">
          <div className="row-title">
            <span className="mono">{connector.id}</span>
            <Badge tone={connected ? "success" : "error"}>
              {connected ? "Connected" : "Not connected"}
            </Badge>
          </div>
          <div className="row-subtitle">
            cloudflared {connector.version || "unknown"} · {connector.arch || "unknown"} ·
            started {formatTime(connector.run_at)}
            {originIp ? ` · origin ${originIp}` : ""}
            {colos.length > 0 ? ` · ${colos.join(", ")}` : ""}
          </div>
        </div>
        <div className="row-actions" onClick={(event) => event.stopPropagation()}>
          <button
            type="button"
            className="btn btn-sm btn-danger"
            onClick={onDisconnect}
            disabled={busy}
          >
            <Trash2 size={13} /> Deregister
          </button>
        </div>
      </div>

      {open && (
        <div style={{ padding: "0 16px 16px" }} className="stack-sm">
          <div className="field-hint">
            {connector.connections.filter((item) => !item.is_pending_reconnect).length} of{" "}
            {connector.connections.length} edge connections established.
          </div>
          {connector.features.length > 0 && (
            <div className="row-inline wrap">
              {connector.features.map((feature) => (
                <Badge tone="neutral" key={feature}>
                  {feature}
                </Badge>
              ))}
            </div>
          )}
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Connection</th>
                  <th>Colo</th>
                  <th className="hide-sm">Origin IP</th>
                  <th>State</th>
                </tr>
              </thead>
              <tbody>
                {connector.connections.map((connection) => (
                  <tr key={connection.id}>
                    <td className="mono">{connection.id}</td>
                    <td>{connection.colo_name || "—"}</td>
                    <td className="mono hide-sm">{connection.origin_ip || "—"}</td>
                    <td>
                      <Badge tone={connection.is_pending_reconnect ? "warning" : "success"}>
                        {connection.is_pending_reconnect ? "Reconnecting" : "Established"}
                      </Badge>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
