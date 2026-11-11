import { useState, type ReactNode } from "react";

import { errorText } from "../api";
import { useApp } from "../store";
import { ConfirmDialog } from "./Dialog";

export interface ConfirmOptions {
  title: string;
  description?: ReactNode;
  body?: ReactNode;
  confirmLabel: string;
  destructive?: boolean;
  run: () => Promise<void> | void;
}

/**
 * A destructive confirmation, with the safe action focused by default.
 *
 * The hook owns the busy flag so a command that throws leaves the dialog open
 * and usable rather than closing on a failure the user has not read.
 */
export function useConfirm() {
  const [options, setOptions] = useState<ConfirmOptions | null>(null);
  const [busy, setBusy] = useState(false);

  const dialog = options ? (
    <ConfirmDialog
      title={options.title}
      description={options.description}
      confirmLabel={options.confirmLabel}
      destructive={options.destructive}
      busy={busy}
      onClose={() => {
        if (!busy) setOptions(null);
      }}
      onConfirm={async () => {
        setBusy(true);
        try {
          await options.run();
          setOptions(null);
        } catch (error) {
          useApp.getState().toast(errorText(error), "error");
        } finally {
          setBusy(false);
        }
      }}
    >
      {options.body}
    </ConfirmDialog>
  ) : null;

  return { confirm: setOptions, dialog };
}
