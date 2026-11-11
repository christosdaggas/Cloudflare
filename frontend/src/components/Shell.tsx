import {
  useEffect,
  useMemo,
  useRef,
  type MouseEvent,
  type ReactNode,
} from "react";
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
  Waypoints,
  type LucideIcon,
} from "lucide-react";

import { getCurrentWindow } from "@tauri-apps/api/window";

import { api } from "../api";
import { chooseTheme, quitApplication, refreshAll } from "../actions";
import { MenuButton, MenuItem, MenuLabel, MenuSeparator } from "../components/Menu";
import { Banner } from "../components/ui";
import { WindowControls } from "./WindowControls";
import { useApp, type PageName, type Route } from "../store";
import type { AppState } from "../types";

/**
 * The window has no native decorations, so the top bar is also the drag
 * region. Buttons, fields and menus inside it must keep their own clicks.
 */
function isWindowChrome(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    Boolean(
      target.closest(
        "button, a, input, select, textarea, [role='button'], [data-no-drag]",
      ),
    )
  );
}

function beginWindowDrag(event: MouseEvent<HTMLElement>) {
  if (event.button !== 0 || isWindowChrome(event.target)) return;
  void getCurrentWindow().startDragging();
}

function toggleMaximize(event: MouseEvent<HTMLElement>) {
  if (isWindowChrome(event.target)) return;
  void getCurrentWindow().toggleMaximize();
}

const NAV: { page: PageName; label: string; icon: LucideIcon }[] = [
  { page: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { page: "tunnels", label: "Tunnels", icon: Waypoints },
  { page: "zones", label: "Zones", icon: Globe },
  { page: "hostnames", label: "Hostnames", icon: ArrowLeftRight },
  { page: "hosts", label: "Remote Hosts", icon: Server },
  { page: "activity", label: "Activity", icon: History },
  { page: "settings", label: "Settings", icon: SettingsIcon },
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
      <aside className={`sidebar${collapsed ? " collapsed" : ""}`}>
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
            <span>v{bootstrap?.version ?? "1.0.0"}</span>
          </div>
        </div>

        <div className="sidebar-section-label">Workspace</div>
        <nav className="nav" aria-label="Main navigation">
          {NAV.map((item) => {
            const ItemIcon = item.icon;
            const active =
              route.page === item.page ||
              (item.page === "tunnels" && route.page === "tunnel");
            return (
              <button
                key={item.page}
                type="button"
                className={`nav-item${active ? " active" : ""}`}
                onClick={() => useApp.getState().navigate({ page: item.page })}
                title={collapsed ? item.label : undefined}
                aria-current={active ? "page" : undefined}
              >
                <ItemIcon size={18} />
                <span className="nav-label">{item.label}</span>
              </button>
            );
          })}
        </nav>

        <div className="sidebar-spacer" />

        <div className="sidebar-footer">
          <div className="connection-pill">
            <span className={`connection-dot ${connectionTone(state)}`} />
            <div className="sidebar-footer-text">
              <strong>{connectionTitle(state)}</strong>
              <span>{connectionSubtitle(state)}</span>
            </div>
          </div>
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
            className="btn btn-ghost btn-icon"
            onClick={toggleSidebar}
            aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
            title="Toggle sidebar (F9)"
          >
            {collapsed ? <PanelLeftOpen size={18} /> : <PanelLeftClose size={18} />}
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
            <HeaderActions route={route} onNew={() => openDialog(newDialogFor(route))} />
          </div>

          <div className="header-divider" />

          <MenuButton label="Main menu">
            <MenuItem
              icon="preferences-system-symbolic"
              label="Refresh Everything"
              shortcut="Ctrl R"
              onClick={() => void refreshAll()}
            />
            <MenuSeparator />
            <MenuLabel>Appearance</MenuLabel>
            <MenuItem label="Follow the system" onClick={() => void chooseTheme("system")} />
            <MenuItem label="Light" onClick={() => void chooseTheme("light")} />
            <MenuItem label="Dark" onClick={() => void chooseTheme("dark")} />
            <MenuSeparator />
            <MenuItem
              label="Keyboard Shortcuts"
              shortcut="Ctrl ?"
              onClick={() => openDialog({ kind: "shortcuts" })}
            />
            <MenuItem
              label="What's New"
              onClick={() => openDialog({ kind: "whats-new" })}
            />
            <MenuItem
              label="About Tunnel Manager"
              onClick={() => openDialog({ kind: "about" })}
            />
            <MenuSeparator />
            <MenuItem label="Quit" shortcut="Ctrl Q" onClick={() => void quitApplication()} danger />
          </MenuButton>

          <WindowControls />
        </header>

        <div className="content" ref={contentRef}>
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
                A newer release is on GitHub. This version is {bootstrap?.version}.
              </Banner>
            </div>
          )}
          {children}
        </div>
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
      <ActionButton
        icon={<RefreshCw size={15} className={busy.includes("tunnels") ? "spin" : ""} />}
        label="Refresh"
        onClick={() => void refresh()}
      />
    );
  }

  if (route.page === "tunnels") {
    return (
      <>
        <ActionButton
          icon={<RefreshCw size={15} className={busy.includes("tunnels") ? "spin" : ""} />}
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
        icon={<RefreshCw size={15} className={busy.includes("zones") ? "spin" : ""} />}
        label="Refresh"
        onClick={() => void refresh()}
      />
    );
  }

  if (route.page === "hostnames") {
    return (
      <>
        <ActionButton
          icon={<RefreshCw size={15} className={busy.includes("tunnels") ? "spin" : ""} />}
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
          icon={<RefreshCw size={15} className={busy.includes("hosts") ? "spin" : ""} />}
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
        icon={<RefreshCw size={15} className={busy.includes("tunnels") ? "spin" : ""} />}
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
    <button type="button" className="btn" onClick={onClick}>
      {icon}
      <span>{label}</span>
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
      subtitle: row ? "Tunnel details, hostnames and connectors" : "Loading tunnel…",
      icon: Waypoints,
    };
  }

  switch (route.page) {
    case "dashboard":
      return {
        title: "Dashboard",
        subtitle: "Health, reach and the next useful action",
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
        subtitle: state ? `${state.zones.length} domain${state.zones.length === 1 ? "" : "s"} the token can see` : "Domains",
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
        subtitle: "This application's local record — not Cloudflare's audit log",
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
