import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronUp, Plus, Search } from "lucide-react";

import { useApp } from "../store";
import {
  Badge,
  Banner,
  EmptyState,
  LoadingState,
  StatusBadge,
} from "../components/ui";
import {
  configSource,
  formatTime,
  tunnelLastActive,
  tunnelStatus,
} from "../status";

type Filter = "all" | "healthy" | "degraded" | "down" | "inactive";
type SortKey = "name" | "status" | "connectors" | "hostnames" | "last_active";
type Sort = { key: SortKey; desc: boolean };

const STATUS_ORDER: Record<string, number> = {
  down: 0,
  degraded: 1,
  inactive: 2,
  other: 3,
  healthy: 4,
};

const FILTERS: { name: Filter; label: string }[] = [
  { name: "all", label: "All" },
  { name: "healthy", label: "Healthy" },
  { name: "degraded", label: "Degraded" },
  { name: "down", label: "Down" },
  { name: "inactive", label: "Inactive" },
];

export function Tunnels() {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);
  const openDialog = useApp((store) => store.openDialog);
  const pendingFilter = useApp((store) => store.tunnelFilter);
  const setTunnelFilter = useApp((store) => store.setTunnelFilter);

  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [sort, setSort] = useState<Sort>({ key: "status", desc: false });

  useEffect(() => {
    if (pendingFilter) {
      setFilter(pendingFilter as Filter);
      setTunnelFilter(null);
    }
  }, [pendingFilter, setTunnelFilter]);

  const rows = state?.tunnels ?? [];

  const counts = useMemo(() => {
    const result: Record<Filter, number> = {
      all: rows.length,
      healthy: 0,
      degraded: 0,
      down: 0,
      inactive: 0,
    };
    for (const row of rows) {
      switch (row.tunnel.status.kind) {
        case "healthy":
          result.healthy += 1;
          break;
        case "degraded":
          result.degraded += 1;
          break;
        case "down":
          result.down += 1;
          break;
        default:
          result.inactive += 1;
      }
    }
    return result;
  }, [rows]);

  const visible = useMemo(() => {
    const query = search.trim().toLowerCase();
    const filtered = rows.filter((row) => {
      const matchesFilter =
        filter === "all" ||
        (filter === "inactive"
          ? row.tunnel.status.kind === "inactive" ||
            row.tunnel.status.kind === "other"
          : row.tunnel.status.kind === filter);
      if (!matchesFilter) return false;
      if (!query) return true;
      const haystack = [
        row.tunnel.name,
        row.tunnel.id,
        tunnelStatus(row.tunnel.status).label,
        row.managed_hosts.join(" "),
      ]
        .join(" ")
        .toLowerCase();
      return haystack.includes(query);
    });

    const sorted = [...filtered].sort((a, b) => {
      switch (sort.key) {
        case "name":
          return a.tunnel.name.localeCompare(b.tunnel.name);
        case "status":
          return (
            (STATUS_ORDER[a.tunnel.status.kind] ?? 9) -
            (STATUS_ORDER[b.tunnel.status.kind] ?? 9)
          );
        case "connectors":
          return a.tunnel.connector_count - b.tunnel.connector_count;
        case "hostnames":
          return (a.hostname_count ?? -1) - (b.hostname_count ?? -1);
        case "last_active":
          return (tunnelLastActive(a.tunnel) ?? "").localeCompare(
            tunnelLastActive(b.tunnel) ?? "",
          );
      }
    });

    return sort.desc ? sorted.reverse() : sorted;
  }, [rows, search, filter, sort]);

  if (!state) {
    return <LoadingState message="Reading the tunnel list…" />;
  }

  const localCount = rows.filter(
    (row) => row.tunnel.config_source.kind === "local",
  ).length;

  return (
    <div className="content-inner">
      {state.tunnels_error && (
        <Banner
          tone="error"
          icon="dialog-error-symbolic"
          title="The tunnel list could not be loaded"
        >
          {state.tunnels_error}
        </Banner>
      )}

      {localCount > 0 && (
        <Banner
          tone="info"
          icon="dialog-information-symbolic"
          title="Some tunnels are configured locally"
        >
          {localCount} tunnel{localCount === 1 ? " is" : "s are"} driven by a{" "}
          <span className="mono">config.yml</span> on the machine running the
          connector. Their ingress is not edited from here, because cloudflared
          would ignore the write.
        </Banner>
      )}

      {rows.length > 0 && (
        <div className="list-toolbar">
          <div className="chips">
            {FILTERS.map((item) => (
              <button
                key={item.name}
                type="button"
                className={`chip${filter === item.name ? " active" : ""}`}
                aria-pressed={filter === item.name}
                onClick={() => setFilter(item.name)}
              >
                {item.label}
                <span className="chip-count">{counts[item.name]}</span>
              </button>
            ))}
          </div>

          <div className="search">
            <Search size={15} />
            <input
              id="tunnel-search"
              className="input"
              type="search"
              aria-label="Search tunnels"
              placeholder="Search tunnels, IDs or hosts…"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </div>
        </div>
      )}

      {rows.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={
              state.tunnels_error
                ? "network-offline-symbolic"
                : "network-vpn-symbolic"
            }
            title={
              state.tunnels_error
                ? "The tunnel list is unavailable"
                : "No tunnels yet"
            }
            description={
              state.tunnels_error
                ? "The list could not be read. Check the connection and try again."
                : "A tunnel connects a hostname to a machine. Create one here, then publish a hostname so traffic has somewhere to go."
            }
            action={
              state.tunnels_error ? undefined : (
                <button
                  type="button"
                  className="btn btn-primary btn-lg"
                  onClick={() => openDialog({ kind: "create-tunnel" })}
                >
                  <Plus size={16} /> Create a tunnel
                </button>
              )
            }
          />
        </div>
      ) : visible.length === 0 ? (
        <div className="card">
          <EmptyState
            icon="network-transmit-receive-symbolic"
            title="No tunnels match"
            description="Nothing matches that search and filter. Clear them to see every tunnel."
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
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <SortHeader
                  label="Tunnel"
                  column="name"
                  sort={sort}
                  onSort={setSort}
                />
                <SortHeader
                  label="Status"
                  column="status"
                  sort={sort}
                  onSort={setSort}
                />
                <SortHeader
                  label="Connectors"
                  column="connectors"
                  sort={sort}
                  onSort={setSort}
                  className="num hide-sm"
                />
                <SortHeader
                  label="Hostnames"
                  column="hostnames"
                  sort={sort}
                  onSort={setSort}
                  className="num hide-sm"
                />
                <th className="hide-md">Managed hosts</th>
                <SortHeader
                  label="Last active"
                  column="last_active"
                  sort={sort}
                  onSort={setSort}
                  className="hide-md"
                />
              </tr>
            </thead>
            <tbody>
              {visible.map((row) => (
                <tr
                  key={row.tunnel.id}
                  className="clickable"
                  onClick={() =>
                    navigate({ page: "tunnel", id: row.tunnel.id })
                  }
                >
                  <td>
                    <div className="cell-main row-inline">
                      <button
                        type="button"
                        className="table-link truncate"
                        onClick={(event) => {
                          event.stopPropagation();
                          navigate({ page: "tunnel", id: row.tunnel.id });
                        }}
                      >
                        {row.tunnel.name}
                      </button>
                      {row.tunnel.config_source.kind !== "cloudflare" && (
                        <Badge tone="warning">
                          {configSource(row.tunnel.config_source)}
                        </Badge>
                      )}
                    </div>
                    <div className="cell-sub">{row.tunnel.id}</div>
                  </td>
                  <td>
                    <StatusBadge status={tunnelStatus(row.tunnel.status)} />
                  </td>
                  <td className="num hide-sm">{row.tunnel.connector_count}</td>
                  <td className="num hide-sm">{row.hostname_count ?? "—"}</td>
                  <td className="hide-md">
                    {row.managed_hosts.length > 0 ? (
                      <span
                        className="truncate"
                        style={{ maxWidth: 200, display: "inline-block" }}
                      >
                        {row.managed_hosts.join(", ")}
                      </span>
                    ) : (
                      <span className="dimmed">—</span>
                    )}
                  </td>
                  <td className="hide-md">
                    {formatTime(tunnelLastActive(row.tunnel))}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {rows.length > 0 && (
        <div className="list-caption">
          {visible.length} of {rows.length} tunnels
        </div>
      )}
    </div>
  );
}

function SortHeader({
  label,
  column,
  sort,
  onSort,
  className,
}: {
  label: string;
  column: SortKey;
  sort: Sort;
  onSort: (sort: Sort) => void;
  className?: string;
}) {
  const active = sort.key === column;
  return (
    <th
      className={className}
      aria-sort={active ? (sort.desc ? "descending" : "ascending") : "none"}
    >
      <button
        type="button"
        className="btn-link"
        style={{
          color: "inherit",
          fontSize: "inherit",
          fontWeight: "inherit",
          letterSpacing: "inherit",
          textTransform: "inherit",
        }}
        onClick={() =>
          onSort({ key: column, desc: active ? !sort.desc : false })
        }
      >
        {label}
        {active &&
          (sort.desc ? (
            <ChevronDown size={11} style={{ verticalAlign: -1 }} />
          ) : (
            <ChevronUp size={11} style={{ verticalAlign: -1 }} />
          ))}
      </button>
    </th>
  );
}
