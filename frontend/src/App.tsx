import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";

import { api, errorText } from "./api";
import { quitApplication, refreshAll } from "./actions";
import { DialogHost } from "./dialogs/DialogHost";
import { Shell } from "./components/Shell";
import { Toasts } from "./components/Toasts";
import { LoadingState } from "./components/ui";
import {
  Activity,
  Dashboard,
  Hostnames,
  Hosts,
  Onboarding,
  Settings,
  TunnelDetail,
  Tunnels,
  Zones,
} from "./pages";
import { useApp } from "./store";
import { applyAccent, applyTheme } from "./theme";
import type { AppState } from "./types";

let restoreStarted = false;
let updateChecked = false;

export function App() {
  const state = useApp((store) => store.state);
  const bootstrap = useApp((store) => store.bootstrap);
  const route = useApp((store) => store.route);
  const setState = useApp((store) => store.setState);
  const setBootstrap = useApp((store) => store.setBootstrap);
  const setUpdate = useApp((store) => store.setUpdate);

  // ── Startup ──────────────────────────────────────────────────────────
  //
  // StrictMode deliberately mounts, unmounts and remounts effects in
  // development. The previous version guarded the whole effect with a ref,
  // which meant the remount skipped startup while the first run was already
  // disposed — leaving the splash up forever. The work is restartable now;
  // only the silent reconnect is guarded, because connecting twice would
  // race two sessions against one credential.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      try {
        const boot = await api.bootstrap();
        if (disposed) return;
        setBootstrap(boot);
        applyAccent(boot.accent);
        useApp.getState().setSystemTheme(boot.system_theme);
      } catch (error) {
        if (!disposed) useApp.getState().toast(errorText(error), "error");
      }

      try {
        let snapshot = await api.getState();
        if (disposed) return;
        setState(snapshot);

        // A saved token with a chosen account starts in `connecting`; finish
        // the silent reconnect here rather than blocking the window on it.
        if (snapshot.connection.kind === "connecting" && !restoreStarted) {
          restoreStarted = true;
          snapshot = await api.restoreSession();
          if (!disposed) setState(snapshot);
        }
      } catch (error) {
        if (!disposed) useApp.getState().toast(errorText(error), "error");
      }

      // The update check is deliberately last and silent on failure.
      if (!updateChecked) {
        updateChecked = true;
        void api
          .checkForUpdates()
          .then((update) => {
            if (!disposed) setUpdate(update);
          })
          .catch(() => undefined);
      }
    })();

    listen<AppState>("state-changed", (event) => {
      setState(event.payload);
    })
      .then((remove) => {
        if (disposed) remove();
        else unlisten = remove;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [setBootstrap, setState, setUpdate]);

  // ── Theme and accent ─────────────────────────────────────────────────
  const theme = useApp((store) => store.state?.settings.theme ?? "system");
  const systemTheme = useApp((store) => store.systemTheme);
  const setSystemTheme = useApp((store) => store.setSystemTheme);

  useEffect(() => {
    applyTheme(theme, systemTheme);
    void api.setWindowTheme(theme).catch(() => undefined);

    if (theme !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => setSystemTheme(media.matches ? "dark" : "light");
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [theme, systemTheme, setSystemTheme]);

  // Re-read the desktop preference when the window regains focus, so a theme
  // switch made elsewhere is picked up without a restart.
  useEffect(() => {
    const onFocus = async () => {
      try {
        useApp.getState().setSystemTheme(await api.systemTheme());
      } catch {
        // A session without gsettings keeps the last known value.
      }
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  useEffect(() => {
    applyAccent(bootstrap?.accent ?? null);
  }, [bootstrap?.accent]);

  // ── Automatic refresh ────────────────────────────────────────────────
  const interval = useApp(
    (store) => store.state?.settings.refresh_interval_secs ?? 0,
  );
  const connected = useApp(
    (store) => store.state?.connection.kind === "connected",
  );

  useEffect(() => {
    if (!connected || interval <= 0) return;
    const id = window.setInterval(() => {
      void refreshAll(true);
    }, interval * 1000);
    return () => window.clearInterval(id);
  }, [connected, interval]);

  // ── Keyboard shortcuts ───────────────────────────────────────────────
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const mod = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();

      if (mod && key === "r") {
        event.preventDefault();
        void refreshAll();
      } else if (mod && key === "n") {
        event.preventDefault();
        if (connected) {
          useApp.getState().openDialog({ kind: "create-tunnel" });
        }
      } else if (event.key === "F9") {
        event.preventDefault();
        useApp.getState().toggleSidebar();
      } else if (mod && (event.key === "?" || (event.key === "/" && event.shiftKey))) {
        event.preventDefault();
        useApp.getState().openDialog({ kind: "shortcuts" });
      } else if (mod && key === "q") {
        event.preventDefault();
        void quitApplication();
      } else if (mod && key === "w") {
        event.preventDefault();
        void quitApplication();
      } else if (mod && key === "f") {
        const input = document.querySelector<HTMLInputElement>(
          ".content input[type=search]",
        );
        if (input) {
          event.preventDefault();
          input.focus();
        }
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [connected]);

  if (!state) {
    return (
      <div className="splash">
        <div className="splash-mark">
          <img src="/app-icon.svg" alt="" />
        </div>
        <div className="splash-title">Tunnel Manager</div>
        <LoadingState message="Opening the local database…" />
      </div>
    );
  }

  const onboarding =
    state.connection.kind === "disconnected" || state.connection.kind === "failed";

  if (onboarding) {
    return (
      <>
        <Onboarding />
        <Toasts />
        <DialogHost />
      </>
    );
  }

  return (
    <>
      <Shell>{renderRoute(route)}</Shell>
      <Toasts />
      <DialogHost />
    </>
  );
}

function renderRoute(route: ReturnType<typeof useApp.getState>["route"]) {
  switch (route.page) {
    case "dashboard":
      return <Dashboard />;
    case "tunnels":
      return <Tunnels />;
    case "tunnel":
      return <TunnelDetail tunnelId={route.id} />;
    case "zones":
      return <Zones />;
    case "hostnames":
      return <Hostnames />;
    case "hosts":
      return <Hosts />;
    case "activity":
      return <Activity />;
    case "settings":
      return <Settings />;
  }
}
