import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Copy, Minus, Square, X } from "lucide-react";

/**
 * The three window buttons for the custom title bar.
 *
 * The native decorations are off so the sidebar can run to the top of the
 * window and the vertical split is continuous. That makes these buttons, and
 * dragging, the application's responsibility.
 */
export function WindowControls() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const win = getCurrentWindow();
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const refresh = () => {
      void win
        .isMaximized()
        .then((value) => {
          if (!disposed) setMaximized(value);
        })
        .catch(() => undefined);
    };

    refresh();
    void win
      .onResized(refresh)
      .then((remove) => {
        if (disposed) remove();
        else unlisten = remove;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const win = getCurrentWindow();

  return (
    <div className="window-controls" data-no-drag>
      <button
        type="button"
        className="window-control"
        aria-label="Minimize"
        title="Minimize"
        onClick={() => void win.minimize()}
      >
        <Minus size={15} />
      </button>
      <button
        type="button"
        className="window-control"
        aria-label={maximized ? "Restore" : "Maximize"}
        title={maximized ? "Restore" : "Maximize"}
        onClick={() => void win.toggleMaximize()}
      >
        {maximized ? <Copy size={13} /> : <Square size={13} />}
      </button>
      <button
        type="button"
        className="window-control close"
        aria-label="Close"
        title="Close"
        onClick={() => void win.close()}
      >
        <X size={15} />
      </button>
    </div>
  );
}
