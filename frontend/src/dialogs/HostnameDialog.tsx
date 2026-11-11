import { useMemo, useState } from "react";
import { Loader2, Plus, Wrench } from "lucide-react";

import { api, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import { OutcomeSteps } from "../components/OutcomeView";
import {
  Badge,
  Banner,
  CodeBlock,
  Field,
  Notice,
  Toggle,
} from "../components/ui";
import { useApp } from "../store";
import {
  dnsState,
  defaultPort,
  originPrefix,
  originSchemeLabel,
  outcomeSucceeded,
  recoveryExplanation,
  recoveryLabel,
  schemeTarget,
  SELECTABLE_SCHEMES,
  supportsPath,
} from "../status";
import type {
  OriginRequest,
  OriginRequestUpdate,
  OriginScheme,
  Outcome,
  PublicHostname,
  PublishHostnameRequest,
  Zone,
} from "../types";

export function HostnameDialog({
  tunnelId,
  hostname,
  onClose,
}: {
  tunnelId: string | null;
  hostname?: PublicHostname;
  onClose: () => void;
}) {
  const state = useApp((store) => store.state);
  const toast = useApp((store) => store.toast);
  const bumpHostnames = useApp((store) => store.bumpHostnames);

  const tunnels = state?.tunnels ?? [];
  const zones = state?.zones ?? [];

  const [selectedTunnelId, setSelectedTunnelId] = useState(
    hostname?.tunnel_id ?? tunnelId ?? tunnels[0]?.tunnel.id ?? "",
  );
  const [name, setName] = useState(hostname?.hostname ?? "");
  const [path, setPath] = useState(hostname?.path ?? "");
  const [zoneId, setZoneId] = useState(() => {
    if (hostname?.zone_id) return hostname.zone_id;
    const configured = state?.settings.default_zone_id;
    return configured && zones.some((zone) => zone.id === configured) ? configured : "";
  });

  const [scheme, setScheme] = useState<OriginScheme>(
    hostname?.service.scheme ?? "http",
  );
  const [target, setTarget] = useState(hostname?.service.host ?? "localhost");
  const [port, setPort] = useState(
    hostname?.service.port !== null && hostname?.service.port !== undefined
      ? String(hostname.service.port)
      : String(defaultPort(hostname?.service.scheme ?? "http") ?? ""),
  );

  const existing = hostname?.origin_request ?? null;
  const [originMode, setOriginMode] = useState<"preserve" | "set" | "clear">(
    "preserve",
  );
  const [httpHostHeader, setHttpHostHeader] = useState(
    String(existing?.httpHostHeader ?? ""),
  );
  const [originServerName, setOriginServerName] = useState(
    String(existing?.originServerName ?? ""),
  );
  const [noTlsVerify, setNoTlsVerify] = useState(Boolean(existing?.noTLSVerify));
  const [http2Origin, setHttp2Origin] = useState(Boolean(existing?.http2Origin));
  const [disableChunked, setDisableChunked] = useState(
    Boolean(existing?.disableChunkedEncoding),
  );
  const [connectTimeout, setConnectTimeout] = useState(
    existing?.connectTimeout != null ? String(existing.connectTimeout) : "",
  );
  const [tlsTimeout, setTlsTimeout] = useState(
    existing?.tlsTimeout != null ? String(existing.tlsTimeout) : "",
  );
  const [keepAliveTimeout, setKeepAliveTimeout] = useState(
    existing?.keepAliveTimeout != null ? String(existing.keepAliveTimeout) : "",
  );

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const editing = Boolean(hostname);
  const kind = schemeTarget(scheme);
  const portRequired = kind === "host_port" && defaultPort(scheme) === null;
  const servicePreview = previewService(scheme, target, port);

  const selectedTunnel = tunnels.find(
    (row) => row.tunnel.id === selectedTunnelId,
  );

  const bestZone = useMemo(() => bestZoneFor(zones, name), [zones, name]);

  const canPublish =
    busy === false &&
    selectedTunnelId.length > 0 &&
    name.trim().length > 0 &&
    (kind === "none" || target.trim().length > 0) &&
    (!portRequired || port.trim().length > 0);

  function changeScheme(next: OriginScheme) {
    const oldDefault = defaultTarget(scheme);
    if (!target.trim() || target === oldDefault) {
      setTarget(defaultTarget(next));
    }
    setScheme(next);
    const nextDefault = defaultPort(next);
    if (nextDefault !== null) {
      setPort(String(nextDefault));
    } else if (schemeTarget(next) !== "host_port") {
      setPort("");
    }
  }

  function useBestZone() {
    if (bestZone && !zoneId) setZoneId(bestZone.id);
  }

  function buildOriginRequest(): OriginRequestUpdate {
    if (originMode === "preserve") return { kind: "preserve" };
    if (originMode === "clear") return { kind: "clear" };

    const request: OriginRequest = {};
    if (httpHostHeader.trim()) request.httpHostHeader = httpHostHeader.trim();
    if (originServerName.trim()) request.originServerName = originServerName.trim();
    if (noTlsVerify) request.noTLSVerify = true;
    if (http2Origin) request.http2Origin = true;
    if (disableChunked) request.disableChunkedEncoding = true;
    if (connectTimeout.trim()) request.connectTimeout = Number(connectTimeout);
    if (tlsTimeout.trim()) request.tlsTimeout = Number(tlsTimeout);
    if (keepAliveTimeout.trim()) request.keepAliveTimeout = Number(keepAliveTimeout);
    return { kind: "set", data: request };
  }

  async function publish() {
    setBusy(true);
    setError(null);

    const request: PublishHostnameRequest = {
      tunnel_id: selectedTunnelId,
      hostname: name.trim(),
      path: supportsPath(scheme) && path.trim() ? path.trim() : null,
      service: {
        scheme,
        host: target.trim(),
        port:
          kind === "host_port" && port.trim() ? Number(port) : null,
      },
      zone_id: zoneId || null,
      origin_request: buildOriginRequest(),
    };

    try {
      const result = await api.publishHostname(request);
      setOutcome(result);
      bumpHostnames();
      toast(
        outcomeSucceeded(result)
          ? `${request.hostname} is published.`
          : `${request.hostname} needs attention.`,
        outcomeSucceeded(result) ? "success" : "warning",
      );
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  async function repair() {
    if (!zoneId) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.repairDns(zoneId, selectedTunnelId, name.trim());
      setOutcome(result);
      bumpHostnames();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  async function rollback() {
    setBusy(true);
    setError(null);
    try {
      await api.rollbackIngress(selectedTunnelId, name.trim(), path.trim() || null);
      bumpHostnames();
      onClose();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  if (outcome) {
    const dns = dnsState(outcome.dns);
    return (
      <Dialog
        title={outcomeSucceeded(outcome) ? "Hostname published" : "Hostname needs attention"}
        description={outcome.hostname}
        onClose={onClose}
        wide
        footer={
          <>
            <button className="btn" onClick={onClose} disabled={busy}>
              Done
            </button>
            {outcome.recovery?.kind === "repair_dns" && zoneId && (
              <button className="btn" onClick={() => void repair()} disabled={busy}>
                <Wrench size={14} /> Repair DNS
              </button>
            )}
            {outcome.recovery?.kind === "roll_back_ingress" && (
              <button className="btn btn-danger" onClick={() => void rollback()} disabled={busy}>
                Undo the Ingress Change
              </button>
            )}
            {!outcomeSucceeded(outcome) && (
              <button className="btn btn-primary" onClick={() => setOutcome(null)} disabled={busy}>
                Back to the form
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

          <div className="row-inline spread">
            <span className="field-label">DNS</span>
            <Badge tone={dns.tone} icon={dns.icon}>
              {dns.label}
            </Badge>
          </div>

          <OutcomeSteps steps={outcome.steps} />

          {outcome.recovery && (
            <Notice icon="dialog-warning-symbolic">
              <strong>{recoveryLabel(outcome.recovery)}.</strong>{" "}
              {recoveryExplanation(outcome.recovery)}
            </Notice>
          )}

          {outcomeSucceeded(outcome) && (
            <CodeBlock
              code={servicePreview}
              label="Service now written to the ingress rule"
            />
          )}
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog
      title={editing ? "Edit origin" : "Publish a hostname"}
      description={
        editing
          ? `${hostname?.hostname}${hostname?.path ?? ""} · ${hostname?.tunnel_name}`
          : "Adds an ingress rule and, when the zone is known, the proxied CNAME."
      }
      onClose={onClose}
      wide
      closeOnBackdrop={!busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => void publish()}
            disabled={!canPublish}
          >
            {busy ? <Loader2 size={14} className="spin" /> : <Plus size={14} />}
            {editing ? "Save Origin" : "Publish"}
          </button>
        </>
      }
    >
      <div className="dialog-form">
        {error && (
          <Banner tone="error" icon="dialog-error-symbolic" title="Could not publish">
            {error}
          </Banner>
        )}

        {tunnels.length === 0 ? (
          <Banner tone="warning" icon="dialog-warning-symbolic" title="No tunnel to publish to">
            Create a tunnel first; a hostname belongs to exactly one.
          </Banner>
        ) : (
          <>
            {!editing && (
              <Field label="Tunnel" htmlFor="hostname-tunnel">
                <select
                  id="hostname-tunnel"
                  className="select"
                  value={selectedTunnelId}
                  onChange={(event) => setSelectedTunnelId(event.target.value)}
                >
                  {tunnels.map((row) => (
                    <option key={row.tunnel.id} value={row.tunnel.id}>
                      {row.tunnel.name}
                    </option>
                  ))}
                </select>
              </Field>
            )}

            <div className="field-row">
              <Field
                label="Hostname"
                htmlFor="hostname-name"
                hint={
                  editing
                    ? "A hostname is not moved by an edit. Unpublish it and publish it again to move it."
                    : "The full name, such as app.example.com."
                }
              >
                <input
                  id="hostname-name"
                  className="input"
                  value={name}
                  disabled={editing}
                  spellCheck={false}
                  placeholder="app.example.com"
                  onChange={(event) => setName(event.target.value)}
                  onBlur={useBestZone}
                />
              </Field>

              <Field
                label="Zone"
                htmlFor="hostname-zone"
                hint={
                  zones.length === 0
                    ? "No domains are available; the ingress rule is written and DNS is left alone."
                    : editing
                      ? "The zone follows the hostname."
                      : bestZone && bestZone.id !== zoneId
                        ? `Best match: ${bestZone.name}`
                        : "Where the proxied CNAME is created."
                }
              >
                <select
                  id="hostname-zone"
                  className="select"
                  value={zoneId}
                  disabled={editing}
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

            <div className="dialog-section">
              <div className="field-label">Origin service</div>
              <div className="field-row three">
                <Field label="Scheme" htmlFor="hostname-scheme">
                  <select
                    id="hostname-scheme"
                    className="select"
                    value={scheme}
                    onChange={(event) =>
                      changeScheme(event.target.value as OriginScheme)
                    }
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
                    label={
                      kind === "host_port"
                        ? "Host"
                        : kind === "socket_path"
                          ? "Socket path"
                          : "HTTP status"
                    }
                    htmlFor="hostname-target"
                  >
                    <input
                      id="hostname-target"
                      className="input mono"
                      value={target}
                      spellCheck={false}
                      onChange={(event) => setTarget(event.target.value)}
                    />
                  </Field>
                )}

                {kind === "host_port" && (
                  <Field
                    label="Port"
                    htmlFor="hostname-port"
                    hint={portRequired ? "Required for this scheme." : undefined}
                  >
                    <input
                      id="hostname-port"
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
                  htmlFor="hostname-path"
                  hint="Optional. Requests under this path go to this origin; the rest of the hostname is unaffected."
                >
                  <input
                    id="hostname-path"
                    className="input mono"
                    value={path}
                    disabled={editing}
                    spellCheck={false}
                    placeholder="/api"
                    onChange={(event) => setPath(event.target.value)}
                  />
                </Field>
              )}

              <div className="notice">
                <span className="field-label" style={{ marginRight: 6 }}>
                  Writes
                </span>
                <span className="mono selectable">{servicePreview}</span>
              </div>
            </div>

            <div className="dialog-section">
              <div className="row-inline spread">
                <div>
                  <div className="field-label">Origin settings</div>
                  <div className="field-hint">
                    cloudflared's defaults unless overridden. Existing settings are
                    preserved by default.
                  </div>
                </div>
                <div className="chips">
                  <button
                    type="button"
                    className={`chip${originMode === "preserve" ? " active" : ""}`}
                    onClick={() => setOriginMode("preserve")}
                  >
                    Preserve
                  </button>
                  <button
                    type="button"
                    className={`chip${originMode === "set" ? " active" : ""}`}
                    onClick={() => setOriginMode("set")}
                  >
                    Replace
                  </button>
                  <button
                    type="button"
                    className={`chip${originMode === "clear" ? " active" : ""}`}
                    onClick={() => setOriginMode("clear")}
                  >
                    Clear
                  </button>
                </div>
              </div>

              {originMode === "set" && (
                <div className="stack-sm">
                  <div className="field-row">
                    <Field label="HTTP Host header" htmlFor="origin-host-header">
                      <input
                        id="origin-host-header"
                        className="input mono"
                        value={httpHostHeader}
                        onChange={(event) => setHttpHostHeader(event.target.value)}
                      />
                    </Field>
                    <Field label="Origin server name (SNI)" htmlFor="origin-sni">
                      <input
                        id="origin-sni"
                        className="input mono"
                        value={originServerName}
                        onChange={(event) => setOriginServerName(event.target.value)}
                      />
                    </Field>
                  </div>

                  <div className="field-row three">
                    <Field label="Connect timeout (s)" htmlFor="origin-connect">
                      <input
                        id="origin-connect"
                        className="input"
                        inputMode="numeric"
                        value={connectTimeout}
                        onChange={(event) => setConnectTimeout(event.target.value)}
                      />
                    </Field>
                    <Field label="TLS timeout (s)" htmlFor="origin-tls">
                      <input
                        id="origin-tls"
                        className="input"
                        inputMode="numeric"
                        value={tlsTimeout}
                        onChange={(event) => setTlsTimeout(event.target.value)}
                      />
                    </Field>
                    <Field label="Keep-alive timeout (s)" htmlFor="origin-keepalive">
                      <input
                        id="origin-keepalive"
                        className="input"
                        inputMode="numeric"
                        value={keepAliveTimeout}
                        onChange={(event) => setKeepAliveTimeout(event.target.value)}
                      />
                    </Field>
                  </div>

                  <div className="stack-sm">
                    <ToggleRow
                      label="Skip TLS verification"
                      hint="Only for an origin whose certificate cannot be validated."
                      checked={noTlsVerify}
                      onChange={setNoTlsVerify}
                    />
                    <ToggleRow
                      label="HTTP/2 to origin"
                      hint="Use HTTP/2 between cloudflared and the origin."
                      checked={http2Origin}
                      onChange={setHttp2Origin}
                    />
                    <ToggleRow
                      label="Disable chunked encoding"
                      hint="For origins that mishandle chunked transfer encoding."
                      checked={disableChunked}
                      onChange={setDisableChunked}
                    />
                  </div>
                </div>
              )}

              {originMode === "clear" && (
                <Notice icon="dialog-warning-symbolic">
                  The rule's <span className="mono">originRequest</span> is removed and
                  cloudflared's defaults apply.
                </Notice>
              )}
            </div>

            {!zoneId && (
              <Notice icon="dialog-information-symbolic">
                No zone is selected, so only the ingress rule is written. The hostname
                will not resolve until a DNS record points at{" "}
                <span className="mono">
                  {selectedTunnel?.tunnel.id ?? "this tunnel"}.cfargotunnel.com
                </span>
                .
              </Notice>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

function ToggleRow({
  label,
  hint,
  checked,
  onChange,
}: {
  label: string;
  hint: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="row-inline spread">
      <div>
        <div className="field-label">{label}</div>
        <div className="field-hint">{hint}</div>
      </div>
      <Toggle checked={checked} onChange={onChange} label={label} />
    </div>
  );
}

function previewService(scheme: OriginScheme, target: string, port: string): string {
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

function bestZoneFor(zones: Zone[], hostname: string): Zone | null {
  const value = hostname.trim().toLowerCase().replace(/\.$/, "");
  if (!value.includes(".")) return null;

  return (
    zones
      .filter(
        (zone) =>
          value === zone.name.toLowerCase() ||
          value.endsWith(`.${zone.name.toLowerCase()}`),
      )
      .sort((a, b) => b.name.length - a.name.length)[0] ?? null
  );
}

/** What the target field should hold when a scheme is first chosen. */
function defaultTarget(scheme: OriginScheme): string {
  switch (schemeTarget(scheme)) {
    case "none":
      return "";
    case "status_code":
      return "404";
    case "socket_path":
      return "/run/app.sock";
    case "host_port":
      return "localhost";
  }
}
