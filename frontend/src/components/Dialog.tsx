import { useEffect, useId, useRef, type ReactNode } from "react";

import { Spinner } from "./ui";
import { X } from "../icons";

export function Dialog({
  title,
  description,
  children,
  footer,
  onClose,
  wide = false,
  narrow = false,
  closeOnBackdrop = true,
}: {
  title: string;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  onClose: () => void;
  wide?: boolean;
  narrow?: boolean;
  closeOnBackdrop?: boolean;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const descriptionId = useId();

  // Native modal dialogs make the rest of the webview inert, trap Tab and
  // restore focus to the trigger, without an extra focus-management library.
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const trigger = document.activeElement;
    dialog.showModal();
    dialog
      .querySelector<HTMLElement>(
        "[data-autofocus], [autofocus], input:not([type='hidden']), select, textarea",
      )
      ?.focus();
    return () => {
      dialog.close();
      if (trigger instanceof HTMLElement && trigger.isConnected)
        trigger.focus();
    };
  }, []);

  return (
    <dialog
      ref={dialogRef}
      className="dialog-backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={description ? descriptionId : undefined}
      onCancel={(event) => {
        event.preventDefault();
        if (closeOnBackdrop) onClose();
      }}
      onKeyDown={(event) => {
        if (event.key !== "Tab") return;
        const controls = Array.from(
          event.currentTarget.querySelectorAll<HTMLElement>(
            "button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex]:not([tabindex='-1'])",
          ),
        ).filter((element) => element.getClientRects().length > 0);
        const first = controls[0];
        const last = controls[controls.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first?.focus();
        }
      }}
      onMouseDown={(event) => {
        if (closeOnBackdrop && event.target === event.currentTarget) onClose();
      }}
    >
      <div className={`dialog${wide ? " wide" : ""}${narrow ? " narrow" : ""}`}>
        <div className="dialog-header">
          <div className="dialog-header-main">
            <h2 id={titleId}>{title}</h2>
            {description && <p id={descriptionId}>{description}</p>}
          </div>
          <button
            type="button"
            className="btn btn-ghost btn-icon dialog-close"
            onClick={onClose}
            aria-label="Close"
            disabled={!closeOnBackdrop}
          >
            <X size={16} />
          </button>
        </div>
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-footer">{footer}</div>}
      </div>
    </dialog>
  );
}

export function ConfirmDialog({
  title,
  description,
  children,
  confirmLabel,
  cancelLabel = "Cancel",
  destructive = false,
  busy = false,
  onConfirm,
  onClose,
}: {
  title: string;
  description?: ReactNode;
  children?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog
      title={title}
      description={description}
      onClose={onClose}
      narrow
      closeOnBackdrop={!busy}
      footer={
        <>
          <button
            className="btn"
            onClick={onClose}
            disabled={busy}
            autoFocus
            data-autofocus
          >
            {cancelLabel}
          </button>
          <button
            className={`btn ${destructive ? "btn-danger" : "btn-primary"}`}
            onClick={onConfirm}
            disabled={busy}
          >
            {busy && <Spinner size={14} />}
            {confirmLabel}
          </button>
        </>
      }
    >
      {children}
    </Dialog>
  );
}
