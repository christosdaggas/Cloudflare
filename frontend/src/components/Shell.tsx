import { useEffect, useMemo, useRef, type ReactNode } from "react";
import {
  ArrowLeft,
  ArrowLeftRight,
  Globe,
  History,
  LayoutDashboard,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  Server,
  Settings as SettingsIcon,
  ShieldCheck,
  Waypoints,
  type LucideIcon,
} from "lucide-react";

import { api } from "../api";
import { chooseTheme, quitApplication, refreshAll } from "../actions";
import {
  MenuButton,
  MenuItem,
  MenuLabel,
  MenuSeparator,
} from "../components/Menu";
import { Banner } from "../components/ui";
import { WindowControls } from "./WindowControls";
import { useApp, type PageName, type Route } from "../store";
import type { AppState } from "../types";
import { beginWindowDrag, toggleMaximize } from "../windowChrome";

const NAV: {
  page: PageName;
  label: string;
  icon: LucideIcon;
  group: string;
}[] = [
  {
    page: "dashboard",
    label: "Dashboard",
    icon: LayoutDashboard,
    group: "Workspace",
  },
  { page: "tunnels", label: "Tunnels", icon: Waypoints, group: "Workspace" },
  {
    page: "hostnames",
    label: "Hostnames",
    icon: ArrowLeftRight,
    group: "Workspace",
  },
  { page: "zones", label: "Zones", icon: Globe, group: "Workspace" },
  { page: "hosts", label: "Remote Hosts", icon: Server, group: "Management" },
  { page: "activity", label: "Activity", icon: History, group: "Management" },
];

export function Shell({ children }: { children: ReactNode }) {
  const route = useApp((store) => store.route);
  const state = useApp((store) => store.state);
  const bootstrap = useApp((store) => store.bootstrap);
  const update = useApp((store) => store.update);
  const collapsed = useApp((store) => store.sidebarCollapsed);
  const toggleSidebar = useApp((store) => store.toggleSidebar);
  const openDialog = useApp((store) => store.openDialog);

  const meta = useMemo(() => pageMeta(route, state), [route, state]);
  const Icon = meta.icon;
  const contentRef = useRef<HTMLDivElement>(null);

  // A page change starts at the top. Without this the scroll position from
  // the previous page is inherited by the next one.
  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 });
  }, [route]);

  return (
    <div className="app-shell">
      <aside
        id="app-sidebar"
        className={`sidebar${collapsed ? " collapsed" : ""}`}
      >
        <div
          className="sidebar-brand"
          onMouseDown={beginWindowDrag}
          onDoubleClick={toggleMaximize}
        >
          <div className="brand-mark">
            <img src="/app-icon.svg" alt="" />
          </div>
          <div className="brand-text">
            <strong>{bootstrap?.app_name ?? "Tunnel Manager"}</strong>
            <span>Cloudflare tunnels</span>
          </div>
        </div>

        {(["Workspace", "Management"] as const).map((group) => (
          <div key={group}>
            <div className="sidebar-section-label">{group}</div>
            <nav className="nav" aria-label={group}>
              {NAV.filter((item) => item.group === group).map((item) => {
                const ItemIcon = item.icon;
                const active =
                  route.page === item.page ||
                  (item.page === "tunnels" && route.page === "tunnel");
                return (
                  <button
                    key={item.page}
                    type="button"
                    className={`nav-item${active ? " active" : ""}`}
                    onClick={() =>
                      useApp.getState().navigate({ page: item.page })
                    }
                    title={item.label}
                    aria-label={item.label}
                    aria-current={active ? "page" : undefined}
                  >
                    <ItemIcon size={18} aria-hidden="true" />
                    <span className="nav-label">{item.label}</span>
                    {(item.page === "tunnels" || item.page === "zones") &&
                      state && (
                        <span className="nav-count" aria-hidden="true">
                          {item.page === "tunnels"
                            ? state.tunnels.length
                            : state.zones.length}
                        </span>
                      )}
                  </button>
                );
              })}
            </nav>
          </div>
        ))}

        <div className="sidebar-spacer" />

        <div className="sidebar-footer">
          <button
            type="button"
            className={`nav-item${route.page === "settings" ? " active" : ""}`}
            aria-label="Settings"
            aria-current={route.page === "settings" ? "page" : undefined}
            title="Settings"
            onClick={() => useApp.getState().navigate({ page: "settings" })}
          >
            <SettingsIcon size={18} aria-hidden="true" />
            <span className="nav-label">Settings</span>
          </button>
          <button
            type="button"
            className="connection-pill"
            title={`${connectionTitle(state)} · ${connectionSubtitle(state)}`}
            aria-label={`Connection settings: ${connectionTitle(state)}. ${connectionSubtitle(state)}`}
            onClick={() => useApp.getState().navigate({ page: "settings" })}
          >
            {state?.connection.kind === "connected" ? (
              <ShieldCheck
                size={18}
                className="connection-symbol"
                aria-hidden="true"
              />
            ) : (
              <span
                className={`connection-dot ${connectionTone(state)}`}
                aria-hidden="true"
              />
            )}
            <div className="sidebar-footer-text">
              <strong>{connectionTitle(state)}</strong>
              <span>{connectionSubtitle(state)}</span>
            </div>
          </button>
        </div>
      </aside>

      <div className="main">
        <header
          className="headerbar"
          onMouseDown={beginWindowDrag}
          onDoubleClick={toggleMaximize}
        >
          <button
            type="button"
            className="btn btn-ghost btn-icon sidebar-toggle"
            onClick={toggleSidebar}
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            aria-expanded={!collapsed}
            aria-controls="app-sidebar"
            title="Toggle sidebar (F9)"
          >
            {collapsed ? (
              <PanelLeftOpen size={18} />
            ) : (
              <PanelLeftClose size={18} />
            )}
          </button>

          {route.page === "tunnel" && (
            <button
              type="button"
              className="btn btn-ghost btn-icon"
              onClick={() => useApp.getState().navigate({ page: "tunnels" })}
              aria-label="Back to tunnels"
              title="Back to tunnels"
            >
              <ArrowLeft size={18} />
            </button>
          )}

          <div className="header-icon">
            <Icon size={17} />
          </div>
          <div className="header-title">
            <h1>{meta.title}</h1>
            <p>{meta.subtitle}</p>
          </div>

          <div className="header-actions">
            <HeaderActions
              route={route}
              onNew={() => openDialog(newDialogFor(route))}
            />
          </div>

          <MenuButton label="Main menu" id="main-menu-trigger">
            <MenuItem
              icon="preferences-system-symbolic"
              label="Refresh Everything"
              shortcut="Ctrl R"
              onClick={() => void refreshAll()}
            />
            <MenuSeparator />
            <MenuLabel>Appearance</MenuLabel>
            <MenuItem
              label="Follow the system"
              onClick={() => void chooseTheme("system")}
            />
            <MenuItem label="Light" onClick={() => void chooseTheme("light")} />
            <MenuItem label="Dark" onClick={() => void chooseTheme("dark")} />
            <MenuSeparator />
            <MenuItem
              label="Keyboard Shortcuts…"
              shortcut="Ctrl ?"
              onClick={() => openDialog({ kind: "shortcuts" })}
            />
            <MenuItem
              label="What's New…"
              onClick={() => openDialog({ kind: "whats-new" })}
            />
            <MenuItem
              label="About Tunnel Manager…"
              onClick={() => openDialog({ kind: "about" })}
            />
            <MenuSeparator />
            <MenuItem
              label="Quit"
              shortcut="Ctrl Q"
              onClick={() => void quitApplication()}
              danger
            />
          </MenuButton>

          <WindowControls />
        </header>

        <main className="content" ref={contentRef} aria-label={meta.title}>
          {update && (
            <div className="content-inner" style={{ marginBottom: 16 }}>
              <Banner
                tone="info"
                icon="dialog-information-symbolic"
                title={`Tunnel Manager ${update.latest_version} is available`}
                action={
                  <button
                    type="button"
                    className="btn btn-sm"
                    onClick={() => void api.openUrl(update.download_url)}
                  >
                    Download
                  </button>
                }
              >
                A newer release is on GitHub. This version is{" "}
                {bootstrap?.version}.
              </Banner>
            </div>
          )}
          {children}
        </main>
      </div>
    </div>
  );
}

function HeaderActions({ route, onNew }: { route: Route; onNew: () => void }) {
  const busy = useApp((store) => store.state?.busy ?? []);
  const refresh = async () => {
    await refreshAll();
  };

  if (route.page === "dashboard") {
    return (
      <>
        <ActionButton
          icon={
            <RefreshCw
              size={15}
              className={busy.includes("tunnels") ? "spin" : ""}
            />
          }
          label="Refresh"
          onClick={() => void refresh()}
        />
        <button
          type="button"
          className="btn btn-primary"
          onClick={() =>
            useApp.getState().openDialog({ kind: "create-tunnel" })
          }
        >
          <Plus size={15} aria-hidden="true" /> New Tunnel
        </button>
      </>
    );
  }

  if (route.page === "tunnels") {
    return (
      <>
        <ActionButton
          icon={
            <RefreshCw
              size={15}
              className={busy.includes("tunnels") ? "spin" : ""}
            />
          }
          label="Refresh"
          onClick={() => void refresh()}
        />
        <button type="button" className="btn btn-primary" onClick={onNew}>
          <Plus size={15} /> New Tunnel
        </button>
      </>
    );
  }

  if (route.page === "zones") {
    return (
      <ActionButton
        icon={
          <RefreshCw
            size={15}
            className={busy.includes("zones") ? "spin" : ""}
          />
        }
        label="Refresh"
        onClick={() => void refresh()}
      />
    );
  }

  if (route.page === "hostnames") {
    return (
      <>
        <ActionButton
          icon={
            <RefreshCw
              size={15}
              className={busy.includes("tunnels") ? "spin" : ""}
            />
          }
          label="Refresh"
          onClick={() => void refresh()}
        />
        <button type="button" className="btn btn-primary" onClick={onNew}>
          <Plus size={15} /> Publish Hostname
        </button>
      </>
    );
  }

  if (route.page === "hosts") {
    return (
      <>
        <ActionButton
          icon={
            <RefreshCw
              size={15}
              className={busy.includes("hosts") ? "spin" : ""}
            />
          }
          label="Refresh"
          onClick={() => void refresh()}
        />
        <button type="button" className="btn btn-primary" onClick={onNew}>
          <Plus size={15} /> Add Host
        </button>
      </>
    );
  }

  if (route.page === "tunnel") {
    return (
      <ActionButton
        icon={
          <RefreshCw
            size={15}
            className={busy.includes("tunnels") ? "spin" : ""}
          />
        }
        label="Refresh"
        onClick={() => void refresh()}
      />
    );
  }

  return null;
}

function ActionButton({
  icon,
  label,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="btn btn-ghost refresh-button"
      onClick={onClick}
      aria-label={label}
      title={label}
    >
      {icon}
      <span className="refresh-label">{label}</span>
    </button>
  );
}

function newDialogFor(route: Route) {
  switch (route.page) {
    case "tunnels":
      return { kind: "create-tunnel" } as const;
    case "hostnames":
      return { kind: "hostname", tunnel_id: null } as const;
    case "hosts":
      return { kind: "host" } as const;
    default:
      return null;
  }
}

function pageMeta(
  route: Route,
  state: AppState | null,
): { title: string; subtitle: string; icon: LucideIcon } {
  if (route.page === "tunnel") {
    const row = state?.tunnels.find((item) => item.tunnel.id === route.id);
    return {
      title: row?.tunnel.name ?? "Tunnel",
      subtitle: row
        ? "Tunnel details, hostnames and connectors"
        : "Loading tunnel…",
      icon: Waypoints,
    };
  }

  switch (route.page) {
    case "dashboard":
      return {
        title: "Dashboard",
        subtitle: "Your network at a glance",
        icon: LayoutDashboard,
      };
    case "tunnels":
      return {
        title: "Tunnels",
        subtitle: state
          ? `${state.tunnels.length} tunnel${state.tunnels.length === 1 ? "" : "s"} on ${
              state.account?.name ?? "this account"
            }`
          : "Remotely managed tunnels",
        icon: Waypoints,
      };
    case "zones":
      return {
        title: "Zones",
        subtitle: state
          ? `${state.zones.length} domain${state.zones.length === 1 ? "" : "s"} the token can see`
          : "Domains",
        icon: Globe,
      };
    case "hostnames":
      return {
        title: "Hostnames",
        subtitle: "Every public hostname on the account, with its DNS state",
        icon: ArrowLeftRight,
      };
    case "hosts":
      return {
        title: "Remote Hosts",
        subtitle: "Optional SSH control of the machines serving your tunnels",
        icon: Server,
      };
    case "activity":
      return {
        title: "Activity",
        subtitle:
          "This application's local record — not Cloudflare's audit log",
        icon: History,
      };
    case "settings":
      return {
        title: "Settings",
        subtitle: "Connection, appearance, refresh and diagnostics",
        icon: SettingsIcon,
      };
  }
}

function connectionTone(state: AppState | null): string {
  switch (state?.connection.kind) {
    case "connected":
      return "connected";
    case "connecting":
      return "connecting";
    case "failed":
      return "failed";
    default:
      return "";
  }
}

function connectionTitle(state: AppState | null): string {
  switch (state?.connection.kind) {
    case "connected":
      return state?.account?.name ?? "Connected";
    case "connecting":
      return "Connecting…";
    case "failed":
      return "Connection failed";
    default:
      return "Not connected";
  }
}

function connectionSubtitle(state: AppState | null): string {
  switch (state?.connection.kind) {
    case "connected":
      return "Cloudflare connected";
    case "connecting":
      return "Checking the saved token";
    case "failed":
      return "Open Settings to reconnect";
    default:
      return "No API token";
  }
}
