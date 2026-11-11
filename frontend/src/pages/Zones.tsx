import { useState } from "react";
import { Globe, Loader2 } from "lucide-react";

import { api, errorText } from "../api";
import {
  Badge,
  Banner,
  EmptyState,
  LoadingState,
  Notice,
  StatCard,
  StatusBadge,
} from "../components/ui";
import { useApp } from "../store";
import { zoneCaveat, zoneStatus } from "../status";
import type { Zone } from "../types";

export function Zones() {
  const state = useApp((store) => store.state);
  const setState = useApp((store) => store.setState);
  const toast = useApp((store) => store.toast);
  const navigate = useApp((store) => store.navigate);
  const [busy, setBusy] = useState<string | null>(null);

  if (!state) return <LoadingState message="Reading the domains…" />;

  const zones = state.zones;
  const defaultId = state.settings.default_zone_id;
  const denied = state.capabilities.zones === "denied";
  const ready = zones.filter((zone) => zone.status.kind === "active").length;
  const defaultZone = zones.find((zone) => zone.id === defaultId);

  async function makeDefault(zone: Zone | null) {
    if (!state) return;
    setBusy(zone?.id ?? "none");
    try {
      const next = await api.updateSettings({
        ...state.settings,
        default_zone_id: zone?.id ?? null,
        default_zone_name: zone?.name ?? null,
      });
      setState(next);
      toast(
        zone
          ? `“${zone.name}” is now the default domain.`
          : "Cleared the default domain.",
        "success",
      );
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="content-inner">
      {denied && (
        <Banner
          tone="warning"
          icon="dialog-warning-symbolic"
          title="The token cannot list domains"
        >
          Publishing a hostname needs <strong>Zone → Read</strong>. Tunnels and
          connectors still work without it.{" "}
          <button
            type="button"
            className="btn-link"
            onClick={() => navigate({ page: "settings" })}
          >
            Check the token
          </button>
        </Banner>
      )}

      {zones.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={denied ? "dialog-error-symbolic" : "web-browser-symbolic"}
            title={denied ? "Domains are not available" : "No domains on this account"}
            description={
              denied
                ? "The API token is missing Zone:Read, so this list cannot be read. A tunnel can still be created and its hostnames managed once the permission is added."
                : "This account has no zones yet. Add a domain in the Cloudflare dashboard, then it will appear here."
            }
            action={
              <button
                type="button"
                className="btn"
                onClick={() =>
                  void api.openUrl("https://dash.cloudflare.com/?to=/:account/domains")
                }
              >
                <Globe size={15} /> Open Cloudflare
              </button>
            }
          />
        </div>
      ) : (
        <>
          <div className="stat-grid">
            <StatCard
              value={zones.length}
              label="Domains"
              detail="Visible to this token"
              icon="web-browser-symbolic"
              tone="accent"
            />
            <StatCard
              value={ready}
              label="Ready to serve"
              detail="Active on Cloudflare's nameservers"
              icon="object-select-symbolic"
              tone="success"
            />
            <StatCard
              value={zones.length - ready}
              label="Not serving"
              detail="Pending, moved or deactivated"
              icon="dialog-warning-symbolic"
              tone={zones.length - ready > 0 ? "warning" : "neutral"}
            />
            <StatCard
              value={defaultZone?.name ?? "None"}
              label="Default domain"
              detail="Offered first when publishing"
              icon="network-transmit-receive-symbolic"
              tone="accent"
            />
          </div>

          <div className="group">
            {zones.map((zone) => {
              const status = zoneStatus(zone.status);
              const caveat = zoneCaveat(zone.status);
              const isDefault = zone.id === defaultId;
              const dnsEdit = zone.permissions.some((permission) =>
                permission.startsWith("#dns_records:edit"),
              );

              return (
                <div className="row row-top" key={zone.id}>
                  <div className={`row-icon tone-${status.tone}`}>
                    <Globe size={16} />
                  </div>
                  <div className="row-main">
                    <div className="row-title">
                      <span>{zone.name}</span>
                      {isDefault && <Badge tone="accent">Default</Badge>}
                      {zone.paused && <Badge tone="warning">Paused</Badge>}
                    </div>
                    <div className="row-subtitle">
                      {zone.account_name || "This account"} ·{" "}
                      <span className="mono">{zone.id}</span>
                    </div>

                    {caveat && (
                      <div
                        className="field-hint"
                        style={{ marginTop: 6, color: "var(--warning)" }}
                      >
                        {caveat}
                      </div>
                    )}

                    <div className="row-inline wrap" style={{ marginTop: 6 }}>
                      <StatusBadge status={status} />
                      <Badge
                        tone={dnsEdit ? "success" : "neutral"}
                        icon={dnsEdit ? "object-select-symbolic" : "dialog-question-symbolic"}
                      >
                        {dnsEdit ? "DNS:Edit" : "DNS scope unknown"}
                      </Badge>
                    </div>
                  </div>

                  <div className="row-actions">
                    {isDefault ? (
                      <Badge tone="accent" icon="object-select-symbolic">
                        Used by default
                      </Badge>
                    ) : (
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={busy !== null}
                        onClick={() => void makeDefault(zone)}
                      >
                        {busy === zone.id && <Loader2 size={13} className="spin" />}
                        Set as default
                      </button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>

          <Notice icon="dialog-information-symbolic">
            A record created in a zone that is not active on Cloudflare's nameservers
            succeeds but does not resolve until the zone is. The badge above says which
            case you are in before you publish.
          </Notice>

          {defaultId && (
            <button
              type="button"
              className="btn btn-sm"
              style={{ alignSelf: "flex-start" }}
              disabled={busy !== null}
              onClick={() => void makeDefault(null)}
            >
              Clear the default domain
            </button>
          )}
        </>
      )}
    </div>
  );
}
