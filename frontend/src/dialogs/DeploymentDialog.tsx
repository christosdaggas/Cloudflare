import { useState } from "react";
import { Loader2, Rocket, ShieldAlert } from "lucide-react";

import { api, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import {
  Badge,
  Banner,
  CodeBlock,
  EmptyState,
  Field,
  Notice,
  Toggle,
} from "../components/ui";
import { useApp } from "../store";
import { serviceState } from "../status";
import type {
  DeploymentOutcome,
  DeploymentPlan,
  RemoteHost,
} from "../types";

export function DeploymentDialog({
  host,
  initialTunnelId,
  onClose,
}: {
  host: RemoteHost;
  initialTunnelId?: string;
  onClose: () => void;
}) {
  const state = useApp((store) => store.state);
  const setState = useApp((store) => store.setState);
  const toast = useApp((store) => store.toast);

  const tunnels = state?.tunnels ?? [];
  const [tunnelId, setTunnelId] = useState(
    initialTunnelId ?? host.tunnel_id ?? tunnels[0]?.tunnel.id ?? "",
  );
  const [plan, setPlan] = useState<DeploymentPlan | null>(null);
  const [confirmedOverwrite, setConfirmedOverwrite] = useState(false);
  const [outcome, setOutcome] = useState<DeploymentOutcome | null>(null);
  const [busy, setBusy] = useState<"plan" | "apply" | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function buildPlan() {
    if (!tunnelId) {
      setError("Choose the tunnel this host should serve.");
      return;
    }
    setBusy("plan");
    setError(null);
    setPlan(null);
    setOutcome(null);
    try {
      setPlan(await api.planDeployment(host.id, tunnelId));
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  async function deploy() {
    if (!plan) return;
    setBusy("apply");
    setError(null);
    try {
      const token = await api.fetchRunToken(plan.tunnel_id);
      const result = await api.applyDeployment({
        host_id: host.id,
        plan,
        token,
        confirmed_overwrite: confirmedOverwrite,
      });
      setOutcome(result);
      if (state) setState(await api.getState());
      toast(
        result.failed
          ? "The deployment did not finish. The steps below say where it stopped."
          : "Connector deployed.",
        result.failed ? "warning" : "success",
      );
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  if (tunnels.length === 0) {
    return (
      <Dialog
        title="Deploy a connector"
        description={host.name}
        onClose={onClose}
        narrow
        footer={
          <button className="btn btn-primary" onClick={onClose}>
            Close
          </button>
        }
      >
        <EmptyState
          icon="network-vpn-symbolic"
          title="No tunnel to deploy"
          description="Create a tunnel first; a connector serves exactly one."
        />
      </Dialog>
    );
  }

  return (
    <Dialog
      title={outcome ? "Deployment result" : "Deploy a connector"}
      description={`${host.name} · ${host.username}@${host.address}:${host.port}`}
      onClose={onClose}
      wide
      closeOnBackdrop={busy === null}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy !== null}>
            {outcome ? "Close" : "Cancel"}
          </button>
          {!plan && !outcome && (
            <button
              className="btn btn-primary"
              onClick={() => void buildPlan()}
              disabled={busy !== null || !tunnelId}
            >
              {busy === "plan" ? <Loader2 size={14} className="spin" /> : <Rocket size={14} />}
              Preview the Plan
            </button>
          )}
          {plan && !outcome && (
            <button
              className="btn btn-primary"
              onClick={() => void deploy()}
              disabled={
                busy !== null ||
                (plan.replaces_existing_unit && !confirmedOverwrite)
              }
            >
              {busy === "apply" ? <Loader2 size={14} className="spin" /> : <Rocket size={14} />}
              Deploy
            </button>
          )}
        </>
      }
    >
      <div className="stack">
        {error && (
          <Banner tone="error" icon="dialog-error-symbolic" title="Could not continue">
            {error}
          </Banner>
        )}

        {outcome ? (
          <>
            <div className="row-inline spread">
              <span className="field-label">Service state</span>
              <Badge
                tone={serviceState(outcome.service_state).tone}
                icon={serviceState(outcome.service_state).icon}
              >
                {serviceState(outcome.service_state).label}
              </Badge>
            </div>
            <div className="steps">
              {outcome.completed.map((step) => (
                <div className="step done" key={step}>
                  <div className="step-marker">✓</div>
                  <div className="step-body">
                    <div className="step-title">{step}</div>
                  </div>
                </div>
              ))}
              {outcome.failed && (
                <div className="step current">
                  <div className="step-marker">!</div>
                  <div className="step-body">
                    <div className="step-title">{outcome.failed[0]}</div>
                    <div className="step-detail">{outcome.failed[1]}</div>
                  </div>
                  <Badge tone="error">Failed</Badge>
                </div>
              )}
            </div>
            {outcome.failed && (
              <Notice icon="dialog-warning-symbolic">
                A partial deployment leaves the steps above in place. Fix the failing
                step and run the deployment again; it is safe to repeat.
              </Notice>
            )}
          </>
        ) : plan ? (
          <>
            <div className="property-grid">
              <div className="property">
                <div className="property-label">Tunnel</div>
                <div className="property-value">{plan.tunnel_name}</div>
              </div>
              <div className="property">
                <div className="property-label">Unit</div>
                <div className="property-value mono">{plan.unit}</div>
              </div>
              <div className="property">
                <div className="property-label">Token file</div>
                <div className="property-value mono">{plan.token_path}</div>
              </div>
              <div className="property">
                <div className="property-label">cloudflared</div>
                <div className="property-value mono">{plan.cloudflared_path}</div>
              </div>
            </div>

            <div className="steps">
              {plan.steps.map((step, index) => (
                <div className="step" key={`${step.description}-${index}`}>
                  <div className="step-marker">{index + 1}</div>
                  <div className="step-body">
                    <div className="step-title">{step.description}</div>
                    <div className="step-detail mono">{step.preview}</div>
                    <div className="row-inline wrap" style={{ marginTop: 6 }}>
                      {step.privileged && (
                        <Badge tone="warning" icon="channel-secure-symbolic">
                          Needs root
                        </Badge>
                      )}
                      {step.overwrites && (
                        <Badge tone="error" icon="dialog-warning-symbolic">
                          Overwrites a file
                        </Badge>
                      )}
                    </div>
                  </div>
                </div>
              ))}
            </div>

            <CodeBlock code={plan.unit_file} label="Unit file that will be written" />

            {plan.replaces_existing_unit && (
              <Banner
                tone="error"
                icon="dialog-warning-symbolic"
                title="An existing unit will be replaced"
              >
                <div className="row-inline spread" style={{ marginTop: 6 }}>
                  <span>
                    The unit at <span className="mono">{plan.unit_path}</span> already
                    exists. Replacing it changes what the service runs.
                  </span>
                  <Toggle
                    checked={confirmedOverwrite}
                    onChange={setConfirmedOverwrite}
                    label="Confirm replacing the existing unit"
                  />
                </div>
              </Banner>
            )}

            <Notice icon="security-high-symbolic">
              The run token is written over the SSH channel's stdin into a root-owned
              file. It never appears on a command line or in a process listing.
            </Notice>
          </>
        ) : (
          <>
            <Field
              label="Tunnel to serve"
              htmlFor="deploy-tunnel"
              hint="One connector serves one tunnel. The plan inspects the host before anything is written."
            >
              <select
                id="deploy-tunnel"
                className="select"
                value={tunnelId}
                onChange={(event) => setTunnelId(event.target.value)}
              >
                {tunnels.map((row) => (
                  <option key={row.tunnel.id} value={row.tunnel.id}>
                    {row.tunnel.name}
                  </option>
                ))}
              </select>
            </Field>

            <div className="notice">
              <ShieldAlert size={16} />
              <div>
                The plan is read-only: it connects, checks for cloudflared, systemd and
                sudo, and shows every command before any of them runs.
              </div>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
