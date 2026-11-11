import { useState } from "react";
import {
  ArrowLeft,
  ArrowRight,
  Check,
  Loader2,
  Rocket,
  Waypoints,
} from "lucide-react";

import { api, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import { OutcomeSteps } from "../components/OutcomeView";
import {
  Badge,
  Banner,
  CodeBlock,
  Field,
  Notice,
  StatusBadge,
  Toggle,
} from "../components/ui";
import { useApp } from "../store";
import {
  defaultPort,
  dnsState,
  originPrefix,
  originSchemeLabel,
  schemeTarget,
  SELECTABLE_SCHEMES,
  supportsPath,
} from "../status";
import type {
  CreateOutcome,
  CreateTunnelRequest,
  OriginScheme,
} from "../types";

const STEPS = ["Name", "Hostname", "Review", "Result"];

export function CreateTunnelDialog({ onClose }: { onClose: () => void }) {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);
  const openDialog = useApp((store) => store.openDialog);
  const toast = useApp((store) => store.toast);

  const zones = state?.zones ?? [];
  const hosts = state?.hosts ?? [];

  const [step, setStep] = useState(0);
  const [name, setName] = useState("");
  const [withHostname, setWithHostname] = useState(false);
  const [hostname, setHostname] = useState("");
  const [zoneId, setZoneId] = useState(state?.settings.default_zone_id ?? "");
  const [scheme, setScheme] = useState<OriginScheme>("http");
  const [target, setTarget] = useState("localhost");
  const [port, setPort] = useState("80");
  const [path, setPath] = useState("");
  const [review, setReview] = useState<string[] | null>(null);
  const [outcome, setOutcome] = useState<CreateOutcome | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [deployHostId, setDeployHostId] = useState("");

  const kind = schemeTarget(scheme);
  const hostnameValid =
    !withHostname ||
    (hostname.trim().length > 0 &&
      (kind === "none" || target.trim().length > 0) &&
      (kind !== "host_port" || port.trim().length > 0));

  function request(): CreateTunnelRequest {
    return {
      name: name.trim(),
      hostname: withHostname
        ? {
            hostname: hostname.trim(),
            path: supportsPath(scheme) && path.trim() ? path.trim() : null,
            service: {
              scheme,
              host: target.trim(),
              port: kind === "host_port" && port.trim() ? Number(port) : null,
            },
            zone_id: zoneId || null,
          }
        : null,
    };
  }

  async function toReview() {
    setBusy(true);
    setError(null);
    try {
      setReview(await api.reviewTunnel(request()));
      setStep(2);
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  async function create() {
    setBusy(true);
    setError(null);
    try {
      const result = await api.createTunnel(request());
      setOutcome(result);
      setStep(3);
      toast(
        result.succeeded
          ? `Created “${name.trim()}”.`
          : `“${name.trim()}” was created, but the hostname is only partly set up.`,
        result.succeeded ? "success" : "warning",
      );
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  const tunnel = outcome?.tunnel ?? null;

  return (
    <Dialog
      title="Create a tunnel"
      description="Create a tunnel, optionally add a hostname, then review your changes."
      onClose={onClose}
      wide
      closeOnBackdrop={!busy}
      footer={
        step === 3 ? (
          <>
            {tunnel && (
              <button
                className="btn"
                onClick={() => {
                  navigate({ page: "tunnel", id: tunnel.id });
                  onClose();
                }}
              >
                Open the tunnel
              </button>
            )}
            <button className="btn btn-primary" onClick={onClose}>
              Done
            </button>
          </>
        ) : (
          <>
            {step > 0 && (
              <button
                className="btn"
                onClick={() => setStep((value) => value - 1)}
                disabled={busy}
              >
                <ArrowLeft size={14} /> Back
              </button>
            )}
            <div className="spacer" />
            {step === 0 && (
              <button
                className="btn btn-primary"
                onClick={() => setStep(1)}
                disabled={name.trim().length === 0}
              >
                Continue <ArrowRight size={14} />
              </button>
            )}
            {step === 1 && (
              <button
                className="btn btn-primary"
                onClick={() => void toReview()}
                disabled={busy || !hostnameValid}
              >
                {busy ? (
                  <Loader2 size={14} className="spin" />
                ) : (
                  <ArrowRight size={14} />
                )}
                Review
              </button>
            )}
            {step === 2 && (
              <button
                className="btn btn-primary"
                onClick={() => void create()}
                disabled={busy}
              >
                {busy ? (
                  <Loader2 size={14} className="spin" />
                ) : (
                  <Waypoints size={14} />
                )}
                Create Tunnel
              </button>
            )}
          </>
        )
      }
    >
      <div className="dialog-form">
        <ol className="wizard-progress" aria-label="Creation progress">
          {STEPS.map((label, index) => (
            <li
              key={label}
              className={`wizard-step${
                index === step ? " current" : index < step ? " done" : ""
              }`}
              aria-current={index === step ? "step" : undefined}
            >
              <div className="step-marker">
                {index < step ? <Check size={13} /> : index + 1}
              </div>
              <div className="step-body">
                <div className="step-title">{label}</div>
              </div>
            </li>
          ))}
        </ol>

        {error && (
          <Banner
            tone="error"
            icon="dialog-error-symbolic"
            title="Could not continue"
          >
            {error}
          </Banner>
        )}

        {step === 0 && (
          <>
            <Field
              label="Tunnel name"
              htmlFor="create-name"
              hint="A descriptive name, visible here and in Cloudflare. You can change it later."
            >
              <input
                id="create-name"
                className="input"
                value={name}
                autoFocus
                placeholder="production-web"
                onChange={(event) => setName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && name.trim()) setStep(1);
                }}
              />
            </Field>
            <Notice icon="dialog-information-symbolic">
              Creating a tunnel does not start a connector. The next step can
              publish a hostname; deployment to a machine is offered after the
              tunnel exists.
            </Notice>
          </>
        )}

        {step === 1 && (
          <>
            <div className="row-inline spread">
              <div>
                <div className="field-label">Publish a hostname now</div>
                <div className="field-hint">
                  Optional. The tunnel can be created empty and given hostnames
                  later.
                </div>
              </div>
              <Toggle
                checked={withHostname}
                onChange={setWithHostname}
                label="Publish a hostname now"
              />
            </div>

            {withHostname && (
              <div className="stack-sm">
                <div className="field-row">
                  <Field label="Hostname" htmlFor="create-hostname">
                    <input
                      id="create-hostname"
                      className="input"
                      value={hostname}
                      spellCheck={false}
                      placeholder="app.example.com"
                      onChange={(event) => setHostname(event.target.value)}
                    />
                  </Field>
                  <Field
                    label="Zone"
                    htmlFor="create-zone"
                    hint={
                      zones.length === 0
                        ? "No domains available; DNS will be skipped."
                        : undefined
                    }
                  >
                    <select
                      id="create-zone"
                      className="select"
                      value={zoneId}
                      onChange={(event) => setZoneId(event.target.value)}
                    >
                      <option value="">Not on this account · no DNS</option>
                      {zones.map((zone) => (
                        <option key={zone.id} value={zone.id}>
                          {zone.name}
                        </option>
                      ))}
                    </select>
                  </Field>
                </div>

                <div className="field-row three">
                  <Field label="Scheme" htmlFor="create-scheme">
                    <select
                      id="create-scheme"
                      className="select"
                      value={scheme}
                      onChange={(event) => {
                        const next = event.target.value as OriginScheme;
                        setScheme(next);
                        const nextDefault = defaultPort(next);
                        if (nextDefault !== null) setPort(String(nextDefault));
                        if (schemeTarget(next) === "status_code")
                          setTarget("404");
                        else if (schemeTarget(next) === "socket_path")
                          setTarget("/run/app.sock");
                        else if (
                          schemeTarget(next) === "host_port" &&
                          (target === "404" || target.startsWith("/"))
                        )
                          setTarget("localhost");
                        else if (schemeTarget(next) === "none") setTarget("");
                      }}
                    >
                      {SELECTABLE_SCHEMES.map((item) => (
                        <option key={item} value={item}>
                          {originSchemeLabel(item)}
                        </option>
                      ))}
                    </select>
                  </Field>

                  {kind !== "none" && (
                    <Field
                      label={kind === "socket_path" ? "Socket path" : "Target"}
                      htmlFor="create-target"
                    >
                      <input
                        id="create-target"
                        className="input mono"
                        value={target}
                        onChange={(event) => setTarget(event.target.value)}
                      />
                    </Field>
                  )}

                  {kind === "host_port" && (
                    <Field label="Port" htmlFor="create-port">
                      <input
                        id="create-port"
                        className="input"
                        inputMode="numeric"
                        value={port}
                        onChange={(event) => setPort(event.target.value)}
                      />
                    </Field>
                  )}
                </div>

                {supportsPath(scheme) && (
                  <Field
                    label="Path prefix"
                    htmlFor="create-path"
                    hint="Optional. Requests under this path go to this origin."
                  >
                    <input
                      id="create-path"
                      className="input mono"
                      value={path}
                      placeholder="/api"
                      onChange={(event) => setPath(event.target.value)}
                    />
                  </Field>
                )}

                <div className="notice">
                  <span className="field-label" style={{ marginRight: 6 }}>
                    Writes
                  </span>
                  <span className="mono selectable">
                    {previewService(scheme, target, port)}
                  </span>
                </div>
              </div>
            )}
          </>
        )}

        {step === 2 && (
          <>
            <div className="field-label">What will change</div>
            <div className="steps">
              {(review ?? []).map((change, index) => (
                <div className="step" key={change}>
                  <div className="step-marker">{index + 1}</div>
                  <div className="step-body">
                    <div className="step-title" style={{ fontWeight: 500 }}>
                      {change}
                    </div>
                  </div>
                </div>
              ))}
            </div>
            {review === null && <Loader2 size={18} className="spin" />}
            <Notice icon="dialog-information-symbolic">
              The tunnel is created first. If the hostname half fails, the
              tunnel is still reported and can be fixed from its detail page.
            </Notice>
          </>
        )}

        {step === 3 && outcome && (
          <>
            <Banner
              tone={outcome.succeeded ? "info" : "warning"}
              icon={
                outcome.succeeded
                  ? "object-select-symbolic"
                  : "dialog-warning-symbolic"
              }
              title={
                outcome.succeeded
                  ? `Created “${tunnel?.name ?? name}”`
                  : `“${tunnel?.name ?? name}” was created, but the hostname needs attention`
              }
            >
              {outcome.succeeded
                ? "The tunnel exists on the account. Open it to see its connectors and hostnames."
                : "The tunnel exists and can be renamed, published to, or deleted. The steps below say what failed."}
            </Banner>

            <OutcomeSteps steps={outcome.steps} />

            {outcome.hostname && (
              <div className="row-inline spread">
                <span className="field-label">Hostname DNS</span>
                <StatusBadge status={dnsState(outcome.hostname.dns)} />
              </div>
            )}

            {tunnel && hosts.length > 0 && (
              <div className="dialog-section">
                <div className="field-label">Deploy a connector</div>
                <div className="field-hint">
                  Optional, and separate from creating the tunnel. A plan is
                  shown before anything runs.
                </div>
                <div className="row-inline">
                  <select
                    className="select"
                    value={deployHostId}
                    onChange={(event) => setDeployHostId(event.target.value)}
                  >
                    <option value="">Choose a host…</option>
                    {hosts.map((host) => (
                      <option key={host.id} value={host.id}>
                        {host.name} · {host.address}
                      </option>
                    ))}
                  </select>
                  <button
                    type="button"
                    className="btn"
                    disabled={!deployHostId}
                    onClick={() => {
                      openDialog({
                        kind: "deployment",
                        host_id: Number(deployHostId),
                        tunnel_id: tunnel.id,
                      });
                      onClose();
                    }}
                  >
                    <Rocket size={14} /> Deploy
                  </button>
                </div>
              </div>
            )}

            {tunnel && hosts.length === 0 && outcome.manual_instructions && (
              <div className="dialog-section">
                <div className="row-inline spread">
                  <span className="field-label">
                    Run the connector yourself
                  </span>
                  <Badge tone="neutral">No SSH host registered</Badge>
                </div>
                <CodeBlock code={outcome.manual_instructions} />
              </div>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

function previewService(
  scheme: OriginScheme,
  target: string,
  port: string,
): string {
  const prefix = originPrefix(scheme);
  switch (schemeTarget(scheme)) {
    case "none":
      return prefix;
    case "status_code":
      return `${prefix}${target.trim() || "404"}`;
    case "socket_path":
      return `${prefix}${target.trim() || "/run/app.sock"}`;
    case "host_port":
      return `${prefix}${target.trim() || "localhost"}${
        port.trim() ? `:${port.trim()}` : ""
      }`;
  }
}
