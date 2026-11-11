import { create } from "zustand";

import type { AppState, Bootstrap, HostKey, PublicHostname, RemoteHost, UpdateInfo } from "./types";

export type PageName =
  | "dashboard"
  | "tunnels"
  | "zones"
  | "hostnames"
  | "hosts"
  | "activity"
  | "settings";

export type Route = { page: PageName } | { page: "tunnel"; id: string };

export type DialogState =
  | { kind: "create-tunnel" }
  | { kind: "hostname"; tunnel_id: string | null; hostname?: PublicHostname }
  | { kind: "host"; host?: RemoteHost; trust?: HostKey; host_id?: number }
  | { kind: "deployment"; host_id: number; tunnel_id: string }
  | { kind: "reinstall"; tunnel_id: string }
  | { kind: "shortcuts" }
  | { kind: "about" }
  | { kind: "whats-new" }
  | null;

export interface Toast {
  id: number;
  message: string;
  kind: "info" | "success" | "warning" | "error";
  action?: { label: string; run: () => void };
}

interface AppStore {
  state: AppState | null;
  bootstrap: Bootstrap | null;
  update: UpdateInfo | null;
  systemTheme: "dark" | "light" | "unknown";
  route: Route;
  dialog: DialogState;
  sidebarCollapsed: boolean;
  /** A name filter set by a dashboard card and consumed by the tunnel list. */
  tunnelFilter: string | null;
  /** Bumped after a hostname mutation so pages know to re-read their lists. */
  hostnameVersion: number;
  toasts: Toast[];

  setState: (state: AppState) => void;
  setBootstrap: (bootstrap: Bootstrap) => void;
  setUpdate: (update: UpdateInfo | null) => void;
  setSystemTheme: (theme: string) => void;
  navigate: (route: Route) => void;
  openDialog: (dialog: DialogState) => void;
  closeDialog: () => void;
  setTunnelFilter: (filter: string | null) => void;
  toggleSidebar: () => void;
  bumpHostnames: () => void;
  toast: (
    message: string,
    kind?: Toast["kind"],
    action?: Toast["action"],
  ) => void;
  dismissToast: (id: number) => void;
}

let nextToast = 1;

export const useApp = create<AppStore>((set) => ({
  state: null,
  bootstrap: null,
  update: null,
  systemTheme: "unknown",
  route: { page: "dashboard" },
  dialog: null,
  sidebarCollapsed: localStorage.getItem("tm.sidebar") === "collapsed",
  tunnelFilter: null,
  hostnameVersion: 0,
  toasts: [],

  setState: (state) => set({ state }),
  setBootstrap: (bootstrap) => set({ bootstrap }),
  setUpdate: (update) => set({ update }),
  setSystemTheme: (theme) =>
    set({
      systemTheme:
        theme === "dark" ? "dark" : theme === "light" ? "light" : "unknown",
    }),

  navigate: (route) => set({ route, dialog: null }),
  openDialog: (dialog) => set({ dialog }),
  closeDialog: () => set({ dialog: null }),
  setTunnelFilter: (tunnelFilter) => set({ tunnelFilter }),

  bumpHostnames: () =>
    set((store) => ({ hostnameVersion: store.hostnameVersion + 1 })),

  toggleSidebar: () =>
    set((store) => {
      const sidebarCollapsed = !store.sidebarCollapsed;
      localStorage.setItem("tm.sidebar", sidebarCollapsed ? "collapsed" : "open");
      return { sidebarCollapsed };
    }),

  toast: (message, kind = "info", action) => {
    const id = nextToast++;
    set((store) => ({ toasts: [...store.toasts, { id, message, kind, action }] }));
    // Errors stay a little longer: they are the ones worth reading twice.
    const lifetime = kind === "error" || kind === "warning" ? 9000 : 5000;
    window.setTimeout(() => {
      set((store) => ({ toasts: store.toasts.filter((toast) => toast.id !== id) }));
    }, lifetime);
  },

  dismissToast: (id) =>
    set((store) => ({ toasts: store.toasts.filter((toast) => toast.id !== id) })),
}));

/** The signed-in state, or `null` while the first snapshot is in flight. */
export function useConnected(): boolean {
  return useApp((store) => store.state?.connection.kind === "connected");
}
