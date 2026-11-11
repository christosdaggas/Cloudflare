import { useCallback, useEffect, useState } from "react";
import {
  FileCode2,
  Loader2,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  Rocket,
  Server,
  Square,
  Terminal,
  Trash2,
  Wrench,
} from "lucide-react";

import { api, commandError, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import {
  Badge,
  Banner,
  CodeBlock,
  EmptyState,
  LoadingState,
  Notice,
  StatusBadge,
  Toggle,
} from "../components/ui";
import { useConfirm } from "../components/useConfirm";
import { SymbolicIcon } from "../icons";
import { useApp } from "../store";
import {
  actionLabel,
  formatTime,
  localBlockingReason,
  localCanControl,
  remoteStatus,
  remoteStatusDetail,
  serviceOutcomeSucceeded,
  serviceState,
  transportExplanation,
  transportLabel,
} from "../status";
import type {
  LocalState,
  RemoteHost,
  ServiceAction,
  TransportProtocol,
} from "../types";

interface TransportTarget {
  title: string;
  current: TransportProtocol | null;
  apply: (protocol: TransportProtocol | null) => Promise<void>;
}

export function Hosts() {
  const state = useApp((store) => store.state);
  const openDialog = useApp((store) => store.openDialog);
  const toast = useApp((store) => store.toast);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [local, setLocal] = useState<LocalState | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [logs, setLogs] = useState<{ title: string; lines: string[] } | null>(null);
  const [transport, setTransport] = useState<TransportTarget | null>(null);
  const [manual, setManual] = useState<string | null>(null);
  const [snippet, setSnippet] = useState<string | null>(null);

  const unit = state?.settings.local_service_unit ?? "cloudflared.service";
  const tunnels = state?.tunnels ?? [];
  const hosts = state?.hosts ?? [];
  const canControl = localCanControl(local);
  const blockingReason = local ? localBlockingReason(local) : null;

  const loadLocal = useCallback(async () => {
    setLocalError(null);
    try {
      setLocal(await api.localInspect(unit));
    } catch (error) {
      setLocal(null);
      setLocalError(errorText(error));
    }
  }, [unit]);

  useEffect(() => {
    void loadLocal();
  }, [loadLocal]);

  useEffect(() => {
    let cancelled = false;
    const username = state?.settings.default_ssh_username ?? "";
    if (!username) {
      setSnippet(null);
      return;
    }
    api
      .sudoersSnippet(username, state?.settings.default_service_unit ?? unit)
      .then((value) => {
        if (!cancelled) setSnippet(value);
      })
      .catch(() => {
        if (!cancelled) setSnippet(null);
      });
    return () => {
      cancelled = true;
    };
  }, [state?.settings.default_ssh_username, state?.settings.default_service_unit, unit]);

  function askLocal(action: ServiceAction) {
    const run = async () => {
      setBusy(`local-${action}`);
      try {
        const outcome = await api.localServiceAction(unit, action);
        toast(
          `${actionLabel(action)} sent. The service is ${serviceState(
            outcome.state_after,
          ).label.toLowerCase()}.`,
          serviceOutcomeSucceeded(outcome) ? "success" : "warning",
        );
        await loadLocal();
      } catch (error) {
        toast(errorText(error), "error");
      } finally {
        setBusy(null);
      }
    };

    if (action === "start") {
      void run();
      return;
    }

    confirm({
      title: `${actionLabel(action)} the local connector?`,
      description:
        action === "stop"
          ? "If this machine is the only connector serving its tunnel, every hostname on that tunnel goes offline until it starts again."
          : "Restarting interrupts traffic for a few seconds while the connector reconnects.",
      body: (
        <Notice>
          The tunnel itself is not deleted, and its DNS records stay as they are.
        </Notice>
      ),
      confirmLabel: actionLabel(action),
      destructive: action === "stop",
      run,
    });
  }

  async function showLocalLogs() {
    setBusy("local-logs");
    try {
      const lines = await api.localLogs(unit, 100);
      setLogs({ title: `This machine · ${unit}`, lines });
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  async function checkHost(host: RemoteHost) {
    if (!host.host_key_fingerprint) {
      setBusy(`check-${host.id}`);
      try {
        const key = await api.probeHostKey(host.id);
        openDialog({ kind: "host", trust: key, host_id: host.id });
      } catch (error) {
        toast(errorText(error), "error");
      } finally {
        setBusy(null);
      }
      return;
    }

    setBusy(`check-${host.id}`);
    try {
      await api.preflightHost(host.id);
      toast(`Checked “${host.name}”.`, "success");
    } catch (error) {
      const structured = commandError(error);
      if (structured.kind === "host_key_unknown" && structured.data) {
        const key = {
          fingerprint: String(structured.data.fingerprint ?? ""),
          algorithm: String(structured.data.algorithm ?? ""),
        };
        openDialog({ kind: "host", trust: key, host_id: host.id });
      } else {
        toast(structured.message, "error");
      }
    } finally {
      setBusy(null);
    }
  }

  function askHostService(host: RemoteHost, action: ServiceAction) {
    const run = async () => {
      setBusy(`service-${host.id}`);
      try {
        const outcome = await api.hostServiceAction(host.id, action);
        toast(
          `${actionLabel(action)} sent to “${host.name}”. The service is ${serviceState(
            outcome.state_after,
          ).label.toLowerCase()}.`,
          serviceOutcomeSucceeded(outcome) ? "success" : "warning",
        );
      } catch (error) {
        toast(errorText(error), "error");
      } finally {
        setBusy(null);
      }
    };

    if (action === "start") {
      void run();
      return;
    }

    confirm({
      title: `${actionLabel(action)} cloudflared on “${host.name}”?`,
      description:
        action === "stop"
          ? "If this is the only connector serving its tunnel, every hostname on that tunnel goes offline until it starts again."
          : "Restarting interrupts traffic for a few seconds while the connector reconnects.",
      body: (
        <Notice>
          The tunnel and its DNS records are not changed. Stopping a connector is not the
          same as deleting a tunnel.
        </Notice>
      ),
      confirmLabel: actionLabel(action),
      destructive: action === "stop",
      run,
    });
  }

  async function showHostLogs(host: RemoteHost) {
    setBusy(`logs-${host.id}`);
    try {
      const lines = await api.hostLogs(host.id, 100);
      setLogs({ title: `${host.name} · ${host.service_unit}`, lines });
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  async function openHostTransport(host: RemoteHost) {
    setBusy(`transport-${host.id}`);
    try {
      const current = await api.readTransport(host.id);
      setTransport({
        title: host.name,
        current,
        apply: async (protocol) => {
          await api.setTransport(host.id, protocol);
        },
      });
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  function openLocalTransport() {
    setTransport({
      title: "This machine",
      current: local?.protocol ?? null,
      apply: async (protocol) => {
        await api.localSetTransport(unit, protocol);
        await loadLocal();
      },
    });
  }

  function removeHost(host: RemoteHost) {
    confirm({
      title: `Remove “${host.name}”?`,
      description:
        "The host profile and any stored passphrase are deleted. Nothing on the machine is changed and cloudflared keeps running.",
      confirmLabel: "Remove Host",
      destructive: true,
      run: async () => {
        await api.deleteHost(host.id);
      },
    });
  }

  async function showManual() {
    if (tunnels.length === 0) {
      setManual(
        "Create a tunnel first. Its detail page will then have the exact commands for running a connector by hand.",
      );
      return;
    }
    setBusy("manual");
    try {
      setManual(await api.manualSetupInstructions(tunnels[0].tunnel.id));
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  if (!state) return <LoadingState message="Reading the registered hosts…" />;

  return (
    <div className="content-inner">
      <Banner
        tone="info"
        icon="dialog-information-symbolic"
        title="Remote control is optional"
      >
        Everything on the Cloudflare side works without it. A host profile adds SSH
        control: start, stop and restart cloudflared, read its journal, pin its transport
        and deploy a connector. Host keys are pinned, and every privileged action is
        previewed.
      </Banner>

      <section className="section">
        <div className="section-heading">
          <h2>This machine</h2>
          <p>Controlled without SSH, through systemd.</p>
        </div>

        <div className="card">
          <div className="card-header">
            <SymbolicIcon name="applications-system-symbolic" size={16} />
            <h3>Local cloudflared</h3>
            <div style={{ marginLeft: "auto" }} className="row-inline">
              {local && <StatusBadge status={serviceState(local.service_state)} />}
              <button
                type="button"
                className="btn btn-ghost btn-icon"
                onClick={() => void loadLocal()}
                aria-label="Refresh the local connector"
              >
                <RefreshCw size={15} className={busy === "local" ? "spin" : ""} />
              </button>
            </div>
          </div>

          <div className="card-body stack">
            {localError && (
              <Banner tone="warning" icon="dialog-warning-symbolic" title="Local control is not available">
                {localError}
              </Banner>
            )}

            {!local && !localError && <LoadingState message="Inspecting this machine…" />}

            {local && (
              <>
                <div className="property-grid">
                  <div className="property">
                    <div className="property-label">cloudflared</div>
                    <div className="property-value mono">
                      {local.cloudflared_version ?? "Not installed"}
                    </div>
                  </div>
                  <div className="property">
                    <div className="property-label">Service unit</div>
                    <div className="property-value mono">{unit}</div>
                  </div>
                  <div className="property">
                    <div className="property-label">Transport</div>
                    <div className="property-value">{transportLabel(local.protocol)}</div>
                  </div>
                  <div className="property">
                    <div className="property-label">Enabled at boot</div>
                    <div className="property-value">
                      {local.service_enabled === null
                        ? "Unknown"
                        : local.service_enabled
                          ? "Yes"
                          : "No"}
                    </div>
                  </div>
                </div>

                {blockingReason && (
                  <Notice icon="dialog-information-symbolic">{blockingReason}</Notice>
                )}

                <div className="row-inline wrap">
                  {canControl && (
                    <>
                      <button
                        type="button"
                        className="btn"
                        disabled={busy !== null}
                        onClick={() => askLocal("start")}
                      >
                        <Play size={14} /> Start
                      </button>
                      <button
                        type="button"
                        className="btn"
                        disabled={busy !== null}
                        onClick={() => askLocal("stop")}
                      >
                        <Square size={14} /> Stop
                      </button>
                      <button
                        type="button"
                        className="btn"
                        disabled={busy !== null}
                        onClick={() => askLocal("restart")}
                      >
                        <RefreshCw size={14} /> Restart
                      </button>
                    </>
                  )}
                  <button
                    type="button"
                    className="btn"
                    disabled={busy !== null}
                    onClick={() => void showLocalLogs()}
                  >
                    <Terminal size={14} /> Logs
                  </button>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy !== null || !canControl}
                    onClick={openLocalTransport}
                  >
                    <Wrench size={14} /> Transport
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </section>

      <section className="section">
        <div className="section-heading">
          <h2>Registered hosts</h2>
          <div className="row-inline">
            <button type="button" className="btn" onClick={() => void showManual()}>
              <FileCode2 size={14} /> Manual setup
            </button>
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => openDialog({ kind: "host" })}
            >
              <Plus size={14} /> Add Host
            </button>
          </div>
        </div>

        {hosts.length === 0 ? (
          <div className="card">
            <EmptyState
              icon="network-server-symbolic"
              title="No hosts registered"
              description="Add the machines that run cloudflared if you want to control the service, read its logs or deploy a connector from here. The Cloudflare half of the application does not need this."
              action={
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => openDialog({ kind: "host" })}
                >
                  <Plus size={15} /> Add your first host
                </button>
              }
            />
          </div>
        ) : (
          <div className="stack-sm">
            {hosts.map((host) => {
              const statusEntry = state.host_status.find((item) => item.id === host.id);
              const status = remoteStatus(statusEntry?.status ?? { kind: "unknown" });
              const detail = remoteStatusDetail(statusEntry?.status ?? { kind: "unknown" });
              const tunnel = tunnels.find(
                (row) => row.tunnel.id === host.tunnel_id,
              )?.tunnel;
              const trusted = Boolean(host.host_key_fingerprint);
              const hostBusy = busy !== null;

              return (
                <div className="group" key={host.id}>
                  <div className="row">
                    <div className="row-icon">
                      <Server size={16} />
                    </div>
                    <div className="row-main">
                      <div className="row-title">
                        <span>{host.name}</span>
                        {!trusted && (
                          <Badge tone="warning" icon="channel-secure-symbolic">
                            Key not trusted
                          </Badge>
                        )}
                      </div>
                      <div className="row-subtitle">
                        <span className="mono">
                          {host.username}@{host.address}:{host.port}
                        </span>
                        {" · "}
                        {tunnel?.name ?? "No tunnel associated"}
                        {detail ? ` · ${detail}` : ""}
                      </div>
                    </div>
                    <StatusBadge status={status} />
                    <div className="row-actions">
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy}
                        onClick={() => void checkHost(host)}
                      >
                        {busy === `check-${host.id}` ? (
                          <Loader2 size={13} className="spin" />
                        ) : (
                          <RefreshCw size={13} />
                        )}
                        {trusted ? "Check" : "Verify Key"}
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted}
                        onClick={() => askHostService(host, "start")}
                        title={trusted ? "Start the service" : "Trust the host key first"}
                      >
                        <Play size={13} />
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted}
                        onClick={() => askHostService(host, "stop")}
                        title={trusted ? "Stop the service" : "Trust the host key first"}
                      >
                        <Square size={13} />
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted}
                        onClick={() => askHostService(host, "restart")}
                        title={trusted ? "Restart the service" : "Trust the host key first"}
                      >
                        <RefreshCw size={13} />
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted}
                        onClick={() => void showHostLogs(host)}
                      >
                        <Terminal size={13} /> Logs
                      </button>
                    </div>
                  </div>

                  <div style={{ padding: "0 16px 16px" }} className="stack-sm">
                    <div className="property-grid">
                      <div className="property">
                        <div className="property-label">Service unit</div>
                        <div className="property-value mono">{host.service_unit}</div>
                      </div>
                      <div className="property">
                        <div className="property-label">Tunnel</div>
                        <div className="property-value">{tunnel?.name ?? "Not associated"}</div>
                      </div>
                      <div className="property">
                        <div className="property-label">Host key</div>
                        <div className="property-value mono">
                          {host.host_key_fingerprint ?? "Not trusted yet"}
                        </div>
                      </div>
                      <div className="property">
                        <div className="property-label">Last reached</div>
                        <div className="property-value">{formatTime(host.last_seen_at)}</div>
                      </div>
                    </div>

                    {host.notes && (
                      <div className="field-hint">
                        <strong>Notes:</strong> {host.notes}
                      </div>
                    )}

                    <div className="row-inline wrap">
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy}
                        onClick={() => openDialog({ kind: "host", host })}
                      >
                        <Pencil size={13} /> Edit
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted || tunnels.length === 0}
                        onClick={() =>
                          openDialog({
                            kind: "deployment",
                            host_id: host.id,
                            tunnel_id: host.tunnel_id ?? tunnels[0]?.tunnel.id ?? "",
                          })
                        }
                      >
                        <Rocket size={13} /> Deploy Connector
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={hostBusy || !trusted}
                        onClick={() => void openHostTransport(host)}
                      >
                        <Wrench size={13} /> Transport
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm btn-danger"
                        disabled={hostBusy}
                        onClick={() => removeHost(host)}
                      >
                        <Trash2 size={13} /> Remove
                      </button>
                    </div>

                    {!trusted && (
                      <Notice icon="channel-secure-symbolic">
                        Nothing is sent to this host until its key has been read and
                        trusted. Use <strong>Verify Key</strong> to see the fingerprint.
                      </Notice>
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>

      <section className="section">
        <div className="section-heading">
          <h2>Least-privilege sudo</h2>
          <p>Only the commands this application uses, only for one unit.</p>
        </div>
        <div className="card">
          <div className="card-body stack">
            {snippet ? (
              <>
                <CodeBlock code={snippet} label="Sudoers rule" />
                <Notice>
                  Install it with{" "}
                  <span className="mono">
                    sudo visudo -f /etc/sudoers.d/tunnel-manager
                  </span>
                  . It grants no shell and no other unit.
                </Notice>
              </>
            ) : (
              <div className="field-hint">
                Set a default SSH user in Settings to generate a ready-made sudoers rule
                scoped to <span className="mono">{unit}</span>.
              </div>
            )}
          </div>
        </div>
      </section>

      {logs && (
        <Dialog
          title="Recent log output"
          description={logs.title}
          onClose={() => setLogs(null)}
          wide
          footer={
            <>
              <button
                type="button"
                className="btn"
                onClick={() => {
                  void navigator.clipboard.writeText(logs.lines.join("\n"));
                  toast("Log copied to the clipboard.", "success");
                }}
              >
                Copy
              </button>
              <button className="btn btn-primary" onClick={() => setLogs(null)}>
                Close
              </button>
            </>
          }
        >
          <CodeBlock code={logs.lines.join("\n")} />
        </Dialog>
      )}

      {transport && (
        <TransportDialog
          target={transport}
          onClose={() => setTransport(null)}
        />
      )}

      {manual !== null && (
        <Dialog
          title="Manual setup"
          description="No SSH host is needed for this route."
          onClose={() => setManual(null)}
          wide
          footer={
            <button className="btn btn-primary" onClick={() => setManual(null)}>
              Close
            </button>
          }
        >
          <div className="stack">
            <CodeBlock code={manual} />
            <Notice>
              Remote control is optional. The tunnel's health still comes from Cloudflare,
              so a connector started by hand appears on the Tunnels page.
            </Notice>
          </div>
        </Dialog>
      )}

      {confirmDialog}
    </div>
  );
}

function TransportDialog({
  target,
  onClose,
}: {
  target: TransportTarget;
  onClose: () => void;
}) {
  const toast = useApp((store) => store.toast);
  const [protocol, setProtocol] = useState<TransportProtocol | null>(target.current);
  const [understood, setUnderstood] = useState(false);
  const [busy, setBusy] = useState(false);

  async function save() {
    setBusy(true);
    try {
      await target.apply(protocol);
      toast(
        `Transport set to ${transportLabel(protocol)}; the service restarted.`,
        "success",
      );
      onClose();
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      title="Connector transport"
      description={target.title}
      onClose={onClose}
      closeOnBackdrop={!busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => void save()}
            disabled={busy || !understood}
          >
            {busy && <Loader2 size={14} className="spin" />}
            Save and Restart
          </button>
        </>
      }
    >
      <div className="stack">
        <div className="chips">
          {([null, "quic", "http2"] as (TransportProtocol | null)[]).map((option) => (
            <button
              key={option ?? "auto"}
              type="button"
              className={`chip${protocol === option ? " active" : ""}`}
              onClick={() => setProtocol(option)}
            >
              {transportLabel(option)}
            </button>
          ))}
        </div>

        <div className="field-hint">{transportExplanation(protocol)}</div>

        <Notice icon="dialog-warning-symbolic">
          Changing the transport writes (or removes) a systemd drop-in, reloads systemd
          and restarts the unit. Traffic is interrupted for a few seconds.
        </Notice>

        <div className="row-inline spread">
          <span className="field-hint">I understand the service will restart.</span>
          <Toggle
            checked={understood}
            onChange={setUnderstood}
            label="Confirm the restart"
          />
        </div>
      </div>
    </Dialog>
  );
}
