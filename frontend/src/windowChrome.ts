import type { MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

// Element includes SVG paths: clicking an icon inside a button must never
// begin a window drag or swallow the control's click.
function isInteractive(target: EventTarget | null): boolean {
  return (
    target instanceof Element &&
    Boolean(
      target.closest(
        "button, a, input, select, textarea, [role='button'], [data-no-drag]",
      ),
    )
  );
}

export function beginWindowDrag(event: MouseEvent<HTMLElement>) {
  if (event.button !== 0 || isInteractive(event.target)) return;
  void getCurrentWindow().startDragging();
}

export function toggleMaximize(event: MouseEvent<HTMLElement>) {
  if (isInteractive(event.target)) return;
  void getCurrentWindow().toggleMaximize();
}
