import { useMemo, useState } from "react";
import { Copy, RefreshCw, Search, Trash2 } from "lucide-react";

import { api, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import {
  Badge,
  Banner,
  CodeBlock,
  CopyButton,
  EmptyState,
  LoadingState,
  Notice,
  copyText,
} from "../components/ui";
import { useConfirm } from "../components/useConfirm";
import { SymbolicIcon } from "../icons";
import { useApp } from "../store";
import { category, formatActivityTime, severity } from "../status";
import type { ActivityCategory, ActivitySeverity } from "../types";

const SEVERITIES: ActivitySeverity[] = ["info", "success", "warning", "error"];

const CATEGORIES: ActivityCategory[] = [
  "api",
  "dns",
  "tunnel_config",
  "tunnel",
  "ssh",
  "remote_service",
  "app",
];

export function Activity() {
  const state = useApp((store) => store.state);
  const setState = useApp((store) => store.setState);
  const toast = useApp((store) => store.toast);
  const { confirm, dialog: confirmDialog } = useConfirm();

  const [search, setSearch] = useState("");
  const [severityFilter, setSeverityFilter] = useState<"all" | ActivitySeverity>("all");
  const [categoryFilter, setCategoryFilter] = useState<"all" | ActivityCategory>("all");
  const [selected, setSelected] = useState<number | null>(null);
  const [logs, setLogs] = useState<string[] | null>(null);
  const [logsBusy, setLogsBusy] = useState(false);
  const [reportBusy, setReportBusy] = useState(false);

  const activity = state?.activity ?? [];

  const counts = useMemo(() => {
    const result: Record<"all" | ActivitySeverity, number> = {
      all: activity.length,
      info: 0,
      success: 0,
      warning: 0,
      error: 0,
    };
    for (const event of activity) result[event.severity] += 1;
    return result;
  }, [activity]);

  const visible = useMemo(() => {
    const query = search.trim().toLowerCase();
    return activity.filter((event) => {
      if (severityFilter !== "all" && event.severity !== severityFilter) return false;
      if (categoryFilter !== "all" && event.category !== categoryFilter) return false;
      if (!query) return true;
      return `${event.summary} ${event.detail ?? ""} ${event.target ?? ""}`
        .toLowerCase()
        .includes(query);
    });
  }, [activity, search, severityFilter, categoryFilter]);

  async function loadLogs() {
    setLogsBusy(true);
    try {
      setLogs(await api.recentLogLines());
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setLogsBusy(false);
    }
  }

  async function copyReport() {
    setReportBusy(true);
    try {
      const report = await api.diagnosticReport();
      await copyText(report);
      toast("The redacted diagnostic report is on the clipboard.", "success");
    } catch (error) {
      toast(errorText(error), "error");
    } finally {
      setReportBusy(false);
    }
  }

  function clear() {
    confirm({
      title: "Clear the activity log?",
      description:
        "This removes this application's local record. Cloudflare's own audit log is a separate thing and is not affected.",
      body: (
        <Notice>
          When registering, publishing or deleting, the record is often the only local
          note of what happened. Export it first if you need it.
        </Notice>
      ),
      confirmLabel: "Clear",
      destructive: true,
      run: async () => {
        const next = await api.clearActivity();
        setState(next);
        toast("Activity cleared.", "success");
      },
    });
  }

  if (!state) return <LoadingState message="Reading the activity log…" />;

  const event = selected !== null ? activity.find((item) => item.id === selected) : null;

  return (
    <div className="content-inner">
      <Banner tone="info" icon="dialog-information-symbolic" title="This is not Cloudflare's audit log">
        These are the calls this application made and what came back. Cloudflare's own
        audit history is in the dashboard and is deliberately not mirrored here.
      </Banner>

      <div className="activity-toolbar">
        <div className="chips">
          <button
            type="button"
            className={`chip${severityFilter === "all" ? " active" : ""}`}
            onClick={() => setSeverityFilter("all")}
          >
            All <span className="chip-count">{counts.all}</span>
          </button>
          {SEVERITIES.map((item) => (
            <button
              key={item}
              type="button"
              className={`chip${severityFilter === item ? " active" : ""}`}
              onClick={() => setSeverityFilter(item)}
            >
              {severity(item).label} <span className="chip-count">{counts[item]}</span>
            </button>
          ))}
        </div>

        <div className="search">
          <Search size={15} />
          <input
            id="activity-search"
            className="input"
            type="search"
            placeholder="Search the log…"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </div>

        <select
          className="select"
          style={{ width: "auto", minWidth: 170 }}
          value={categoryFilter}
          onChange={(event) =>
            setCategoryFilter(event.target.value as "all" | ActivityCategory)
          }
          aria-label="Filter by category"
        >
          <option value="all">Every category</option>
          {CATEGORIES.map((item) => (
            <option key={item} value={item}>
              {category(item)}
            </option>
          ))}
        </select>

        <div className="row-inline" style={{ marginLeft: "auto" }}>
          <button type="button" className="btn" onClick={() => void copyReport()} disabled={reportBusy}>
            <Copy size={14} /> Copy Report
          </button>
          <button type="button" className="btn btn-danger" onClick={clear} disabled={activity.length === 0}>
            <Trash2 size={14} /> Clear
          </button>
        </div>
      </div>

      {visible.length === 0 ? (
        <div className="card">
          <EmptyState
            icon="document-open-recent-symbolic"
            title={activity.length === 0 ? "Nothing recorded yet" : "No events match"}
            description={
              activity.length === 0
                ? "Requests, host checks, publishes and deletes are recorded here as they happen."
                : "Nothing matches that search and filter. Clear them to see the whole log."
            }
            action={
              activity.length > 0 ? (
                <button
                  type="button"
                  className="btn"
                  onClick={() => {
                    setSearch("");
                    setSeverityFilter("all");
                    setCategoryFilter("all");
                  }}
                >
                  Clear filters
                </button>
              ) : undefined
            }
          />
        </div>
      ) : (
        <div className="group">
          {visible.map((item) => {
            const meta = severity(item.severity);
            return (
              <div
                className="activity-row clickable"
                key={item.id}
                onClick={() => setSelected(item.id)}
                role="button"
                tabIndex={0}
                onKeyDown={(event) => {
                  if (event.key === "Enter") setSelected(item.id);
                }}
              >
                <div className={`activity-icon tone-${meta.tone}`}>
                  <SymbolicIcon name={meta.icon} size={14} />
                </div>
                <div className="activity-main">
                  <div className="activity-summary">{item.summary}</div>
                  <div className="activity-meta">
                    <span>{formatActivityTime(item.at)}</span>
                    <span>·</span>
                    <Badge tone={meta.tone}>{meta.label}</Badge>
                    <span>{category(item.category)}</span>
                    {item.target && (
                      <span className="activity-target">{item.target}</span>
                    )}
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}

      <section className="section">
        <div className="section-heading">
          <h2>Recent log output</h2>
          <p>Everything here was redacted at the sink before it reached the buffer.</p>
        </div>
        <div className="row-inline">
          <button type="button" className="btn" onClick={() => void loadLogs()} disabled={logsBusy}>
            <RefreshCw size={14} className={logsBusy ? "spin" : ""} /> Read the log buffer
          </button>
          {logs !== null && <CopyButton value={logs.join("\n")} label="Copy log" />}
        </div>
        {logs !== null && (
          <CodeBlock
            code={
              logs.length > 0
                ? logs.join("\n")
                : "The log buffer is empty. That is normal at the default level."
            }
            copy={false}
          />
        )}
      </section>

      {event && (
        <Dialog
          title="Activity event"
          description={formatActivityTime(event.at)}
          onClose={() => setSelected(null)}
          footer={
            <button className="btn btn-primary" onClick={() => setSelected(null)}>
              Close
            </button>
          }
        >
          <div className="stack">
            <div className="row-inline spread">
              <Badge
                tone={severity(event.severity).tone}
                icon={severity(event.severity).icon}
              >
                {severity(event.severity).label}
              </Badge>
              <span className="field-hint">{category(event.category)}</span>
            </div>
            <p style={{ margin: 0 }}>{event.summary}</p>
            {event.detail && <CodeBlock code={event.detail} label="Detail" />}
            {event.target && (
              <div className="row-inline spread">
                <span className="field-label">Target</span>
                <span className="mono field-hint">{event.target}</span>
              </div>
            )}
          </div>
        </Dialog>
      )}

      {confirmDialog}
    </div>
  );
}
