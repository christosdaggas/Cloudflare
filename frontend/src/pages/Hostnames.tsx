import { useCallback, useEffect, useMemo, useState } from "react";
import { Plus, RefreshCw, Search } from "lucide-react";

import { api, errorText } from "../api";
import { HostnameRow, displayName } from "../components/HostnameRow";
import { OutcomeDialog } from "../components/OutcomeView";
import {
  Banner,
  EmptyState,
  LoadingState,
  Notice,
} from "../components/ui";
import { useConfirm } from "../components/useConfirm";
import { useApp } from "../store";
import type { HostnameSweep, Outcome, PublicHostname } from "../types";

type Filter = "all" | "ready" | "attention";

interface OutcomeState {
  title: string;
  outcome: Outcome;
  hostname: string;
  path: string | null;
  zoneId: string | null;
  tunnelId: string;
}

export function Hostnames() {
  const state = useApp((store) => store.state);
  const openDialog = useApp((store) => store.openDialog);
  const navigate = useApp((store) => store.navigate);
  const toast = useApp((store) => store.toast);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [sweep, setSweep] = useState<HostnameSweep | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<OutcomeState | null>(null);

  const load = useCallback(async (quiet = false) => {
    if (!quiet) setLoading(true);
    try {
      const result = await api.loadAllHostnames();
      setSweep(result);
      setError(null);
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  const signature = useMemo(() => {
    const tunnels =
      state?.tunnels.map((row) => `${row.tunnel.id}:${row.tunnel.name}`).join("|") ?? "";
    const zones = state?.zones.map((zone) => zone.id).join(",") ?? "";
    return `${tunnels}::${zones}`;
  }, [state?.tunnels, state?.zones]);

  useEffect(() => {
    void load();
  }, [signature, load]);

  const hostnames = sweep?.hostnames ?? [];

  const counts = useMemo(
    () => ({
      all: hostnames.length,
      ready: hostnames.filter((item) => item.dns.kind === "correct").length,
      attention: hostnames.filter((item) => item.dns.kind !== "correct").length,
    }),
    [hostnames],
  );

  const groups = useMemo(() => {
    const query = search.trim().toLowerCase();
    const visible = hostnames.filter((item) => {
      if (filter === "ready" && item.dns.kind !== "correct") return false;
      if (filter === "attention" && item.dns.kind === "correct") return false;
      if (!query) return true;
      return `${item.hostname}${item.path ?? ""} ${item.tunnel_name} ${item.raw_service} ${
        item.zone_name ?? ""
      }`
        .toLowerCase()
        .includes(query);
    });

    const map = new Map<string, PublicHostname[]>();
    for (const item of visible) {
      const list = map.get(item.tunnel_name) ?? [];
      list.push(item);
      map.set(item.tunnel_name, list);
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [hostnames, search, filter]);

  async function runAction(
    title: string,
    hostname: PublicHostname,
    action: () => Promise<Outcome>,
  ) {
    setBusy(true);
    try {
      const result = await action();
      setOutcome({
        title,
        outcome: result,
        hostname: hostname.hostname,
        path: hostname.path,
        zoneId: hostname.zone_id,
        tunnelId: hostname.tunnel_id,
      });
      await load(true);
    } catch (reason) {
      toast(errorText(reason), "error");
    } finally {
      setBusy(false);
    }
  }

  function askDisable(hostname: PublicHostname) {
    confirm({
      title: `Disable ${displayName(hostname)}?`,
      description:
        "The rule stays in the tunnel's configuration but answers 404. The origin is remembered so Enable restores it exactly.",
      body: (
        <Notice>
          Visitors get a 404 while it is disabled. DNS is not touched.
        </Notice>
      ),
      confirmLabel: "Disable Hostname",
      run: async () => {
        const result = await api.disableHostname({
          tunnel_id: hostname.tunnel_id,
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
          tunnelId: hostname.tunnel_id,
        });
        await load(true);
      },
    });
  }

  function askUnpublish(hostname: PublicHostname) {
    confirm({
      title: `Unpublish ${displayName(hostname)}?`,
      description:
        "The ingress rule is removed and only the DNS record that points at this tunnel is deleted.",
      body: (
        <Notice>
          An A, AAAA or CNAME record belonging to something else is never removed.
        </Notice>
      ),
      confirmLabel: "Unpublish",
      destructive: true,
      run: async () => {
        const result = await api.unpublishHostname(
          hostname.tunnel_id,
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
          tunnelId: hostname.tunnel_id,
        });
        await load(true);
      },
    });
  }

  if (!state) return <LoadingState message="Reading the hostnames…" />;

  return (
    <div className="content-inner">
      {error && (
        <Banner tone="error" icon="dialog-error-symbolic" title="Could not read the hostnames">
          {error}
        </Banner>
      )}

      {sweep && sweep.errors.length > 0 && (
        <Banner
          tone="warning"
          icon="dialog-warning-symbolic"
          title="Some tunnels could not be read"
        >
          <ul style={{ margin: "4px 0 0", paddingLeft: 18 }}>
            {sweep.errors.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </Banner>
      )}

      {state.tunnels.length === 0 ? (
        <div className="card">
          <EmptyState
            icon="network-vpn-symbolic"
            title="Nothing to publish to yet"
            description="A public hostname belongs to a tunnel. Create one first, then come back here to point a domain at an origin."
            action={
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => navigate({ page: "tunnels" })}
              >
                Open Tunnels
              </button>
            }
          />
        </div>
      ) : (
        <>
          <div className="row-inline spread" style={{ flexWrap: "wrap", gap: 10 }}>
            <div className="chips">
              <button
                type="button"
                className={`chip${filter === "all" ? " active" : ""}`}
                onClick={() => setFilter("all")}
              >
                All <span className="chip-count">{counts.all}</span>
              </button>
              <button
                type="button"
                className={`chip${filter === "ready" ? " active" : ""}`}
                onClick={() => setFilter("ready")}
              >
                Ready <span className="chip-count">{counts.ready}</span>
              </button>
              <button
                type="button"
                className={`chip${filter === "attention" ? " active" : ""}`}
                onClick={() => setFilter("attention")}
              >
                Needs attention <span className="chip-count">{counts.attention}</span>
              </button>
            </div>

            <div className="row-inline">
              <div className="search">
                <Search size={15} />
                <input
                  id="hostname-search"
                  className="input"
                  type="search"
                  placeholder="Search hostnames, tunnels or origins…"
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                />
              </div>
              <button
                type="button"
                className="btn"
                onClick={() => void load()}
                disabled={loading}
              >
                <RefreshCw size={14} className={loading ? "spin" : ""} /> Refresh
              </button>
              <button
                type="button"
                className="btn btn-primary"
                onClick={() => openDialog({ kind: "hostname", tunnel_id: null })}
              >
                <Plus size={14} /> Publish
              </button>
            </div>
          </div>

          {loading && hostnames.length === 0 ? (
            <LoadingState message="Sweeping every tunnel's ingress rules and DNS…" />
          ) : hostnames.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="network-transmit-receive-symbolic"
                title="No public hostnames"
                description="No tunnel on this account has an ingress rule with a hostname yet. Publish one to send a domain to an origin."
                action={
                  <button
                    type="button"
                    className="btn btn-primary"
                    onClick={() => openDialog({ kind: "hostname", tunnel_id: null })}
                  >
                    <Plus size={15} /> Publish a hostname
                  </button>
                }
              />
            </div>
          ) : groups.length === 0 ? (
            <div className="card">
              <EmptyState
                icon="document-open-recent-symbolic"
                title="No hostnames match"
                description="Nothing matches that search and filter. Clear them to see every hostname."
                action={
                  <button
                    type="button"
                    className="btn"
                    onClick={() => {
                      setSearch("");
                      setFilter("all");
                    }}
                  >
                    Clear filters
                  </button>
                }
              />
            </div>
          ) : (
            groups.map(([tunnelName, items]) => (
              <section className="section" key={tunnelName}>
                <div className="section-heading">
                  <h2>{tunnelName}</h2>
                  <p>
                    {items.length} hostname{items.length === 1 ? "" : "s"}
                  </p>
                </div>
                <div className="stack-sm">
                  {items.map((hostname) => (
                    <HostnameRow
                      key={`${hostname.tunnel_id}-${hostname.hostname}${
                        hostname.path ?? ""
                      }-${hostname.rule_index}`}
                      hostname={hostname}
                      tunnelName={tunnelName}
                      busy={busy}
                      onEdit={() =>
                        openDialog({
                          kind: "hostname",
                          tunnel_id: hostname.tunnel_id,
                          hostname,
                        })
                      }
                      onRepair={() =>
                        void runAction("Repair DNS", hostname, () =>
                          api.repairDns(
                            hostname.zone_id!,
                            hostname.tunnel_id,
                            hostname.hostname,
                          ),
                        )
                      }
                      onDisable={() => askDisable(hostname)}
                      onEnable={() =>
                        void runAction("Enable hostname", hostname, () =>
                          api.enableHostname(
                            hostname.tunnel_id,
                            hostname.hostname,
                            hostname.path,
                          ),
                        )
                      }
                      onUnpublish={() => askUnpublish(hostname)}
                    />
                  ))}
                </div>
              </section>
            ))
          )}
        </>
      )}

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
                    outcome.tunnelId,
                    outcome.hostname,
                  );
                  setOutcome({ ...outcome, title: "Repair DNS", outcome: result });
                  await load(true);
                }
              : undefined
          }
          onRollback={async () => {
            await api.rollbackIngress(
              outcome.tunnelId,
              outcome.hostname,
              outcome.path,
            );
            setOutcome(null);
            await load(true);
          }}
          onOpenSettings={() => {
            setOutcome(null);
            navigate({ page: "settings" });
          }}
          onOpenDashboard={() =>
            void api.openUrl(
              "https://dash.cloudflare.com/?to=/:account/:zone/dns/records",
            )
          }
        />
      )}

      {confirmDialog}
    </div>
  );
}
