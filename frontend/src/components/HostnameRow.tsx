import { useState } from "react";
import { Pause, Pencil, Play, Trash2, Wrench } from "lucide-react";

import { Badge, CopyButton, Property, StatusBadge } from "./ui";
import { SymbolicIcon } from "../icons";
import { dnsDetail, dnsIsRepairable, dnsState, originSchemeLabel } from "../status";
import type { OriginRequest, PublicHostname } from "../types";

export function HostnameRow({
  hostname,
  tunnelName,
  busy,
  onEdit,
  onRepair,
  onDisable,
  onEnable,
  onUnpublish,
  showTunnel = false,
}: {
  hostname: PublicHostname;
  tunnelName: string;
  busy: boolean;
  onEdit: () => void;
  onRepair: () => void;
  onDisable: () => void;
  onEnable: () => void;
  onUnpublish: () => void;
  showTunnel?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const dns = dnsState(hostname.dns);
  const overrides = originRequestRows(hostname.origin_request);

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
          <SymbolicIcon name="network-transmit-receive-symbolic" size={16} />
        </div>
        <div className="row-main">
          <div className="row-title">
            <span className="truncate">{displayName(hostname)}</span>
            {hostname.disabled && <Badge tone="neutral">Disabled</Badge>}
          </div>
          <div className="row-subtitle">
            {showTunnel && (
              <>
                <strong style={{ color: "var(--fg-secondary)", fontWeight: 600 }}>
                  {hostname.tunnel_name}
                </strong>
                {" · "}
              </>
            )}
            <span className="mono">{hostname.raw_service}</span>
            {" · "}
            {hostname.zone_name ?? "Zone not on this account"}
          </div>
        </div>
        <StatusBadge status={dns} />
        <div className="row-actions" onClick={(event) => event.stopPropagation()}>
          <button type="button" className="btn btn-sm" onClick={onEdit} disabled={busy}>
            <Pencil size={13} /> Edit
          </button>
          {!hostname.disabled && hostname.zone_id && dnsIsRepairable(hostname.dns) && (
            <button type="button" className="btn btn-sm" onClick={onRepair} disabled={busy}>
              <Wrench size={13} /> Repair
            </button>
          )}
          {hostname.disabled ? (
            <button type="button" className="btn btn-sm" onClick={onEnable} disabled={busy}>
              <Play size={13} /> Enable
            </button>
          ) : (
            <button type="button" className="btn btn-sm" onClick={onDisable} disabled={busy}>
              <Pause size={13} /> Disable
            </button>
          )}
          <button
            type="button"
            className="btn btn-sm btn-danger"
            onClick={onUnpublish}
            disabled={busy}
          >
            <Trash2 size={13} /> Unpublish
          </button>
        </div>
      </div>

      {open && (
        <div style={{ padding: "0 16px 16px" }} className="stack-sm">
          <div className="field-hint">{dnsDetail(hostname.dns, tunnelName)}</div>

          <div className="property-grid">
            <Property label="Origin" value={hostname.raw_service} mono />
            <Property label="Scheme" value={originSchemeLabel(hostname.service.scheme)} />
            <Property label="Zone" value={hostname.zone_name ?? "Not on this account"} />
            <Property
              label="Rule index"
              value={`#${hostname.rule_index + 1} in the ingress list`}
            />
            <Property
              label="Path"
              value={hostname.path ?? "Every path"}
              mono={Boolean(hostname.path)}
            />
            <Property
              label="Origin overrides"
              value={
                overrides.length === 0 ? "cloudflared defaults" : `${overrides.length} set`
              }
            />
          </div>

          {overrides.length > 0 && (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>Origin setting</th>
                    <th>Value</th>
                  </tr>
                </thead>
                <tbody>
                  {overrides.map(([label, value]) => (
                    <tr key={label}>
                      <td>{label}</td>
                      <td className="mono">{value}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          <div className="row-inline">
            <CopyButton value={displayName(hostname)} label="Copy hostname" />
            {hostname.zone_id && <CopyButton value={hostname.zone_id} label="Copy zone ID" />}
          </div>
        </div>
      )}
    </div>
  );
}

export function displayName(hostname: PublicHostname): string {
  return `${hostname.hostname}${hostname.path ?? ""}`;
}

export function originRequestRows(request: OriginRequest | null): [string, string][] {
  if (!request) return [];

  const labels: Record<string, string> = {
    connectTimeout: "Connect timeout",
    tlsTimeout: "TLS timeout",
    tcpKeepAlive: "TCP keep-alive",
    noHappyEyeballs: "No Happy Eyeballs",
    keepAliveConnections: "Keep-alive connections",
    keepAliveTimeout: "Keep-alive timeout",
    httpHostHeader: "HTTP Host header",
    originServerName: "Origin server name",
    caPool: "CA pool",
    noTLSVerify: "Skip TLS verification",
    disableChunkedEncoding: "Disable chunked encoding",
    bastionMode: "Bastion mode",
    proxyAddress: "Proxy address",
    proxyPort: "Proxy port",
    proxyType: "Proxy type",
    http2Origin: "HTTP/2 to origin",
    matchSNItoHost: "Match SNI to Host",
  };

  return Object.entries(request)
    .filter(([, value]) => value !== null && value !== undefined)
    .map(([key, value]) => [labels[key] ?? key, String(value)]);
}
