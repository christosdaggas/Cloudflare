import { api, errorText } from "./api";
import { copyText } from "./components/ui";
import { useApp } from "./store";
import { applyTheme } from "./theme";

/** Reloads the network-backed lists the shell can see. */
export async function refreshAll(silent = false): Promise<void> {
  const { state, toast } = useApp.getState();
  if (!state || state.connection.kind !== "connected") return;

  try {
    await Promise.all([api.refreshTunnels(), api.refreshZones()]);
    await api.refreshHosts();
    if (!silent) toast("Everything is up to date.", "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
}

/** Saves a theme choice, applies it immediately and asks the window to match. */
export async function chooseTheme(theme: string): Promise<void> {
  const { state, toast } = useApp.getState();
  if (!state) return;

  applyTheme(theme);
  void api.setWindowTheme(theme).catch(() => {
    // The window manager may not support a per-window theme; the web content
    // still changes, which is the important half.
  });

  try {
    await api.updateSettings({ ...state.settings, theme });
  } catch (error) {
    toast(errorText(error), "error");
  }
}

export async function quitApplication(): Promise<void> {
  try {
    await api.quit();
  } catch (error) {
    useApp.getState().toast(errorText(error), "error");
  }
}

/** Copies a tunnel's run token to the clipboard. */
export async function copyRunToken(tunnelId: string): Promise<void> {
  const { toast } = useApp.getState();
  try {
    const token = await api.fetchRunToken(tunnelId);
    await copyText(token);
    toast("The run token is on the clipboard. Treat it like a password.", "success");
  } catch (error) {
    toast(errorText(error), "error");
  }
}
