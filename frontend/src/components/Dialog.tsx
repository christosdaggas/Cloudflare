import { useEffect, type ReactNode } from "react";

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
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => {
        if (closeOnBackdrop && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className={`dialog${wide ? " wide" : ""}${narrow ? " narrow" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <div className="dialog-header">
          <div className="dialog-header-main">
            <h2>{title}</h2>
            {description && <p>{description}</p>}
          </div>
          <button
            type="button"
            className="btn btn-ghost btn-icon dialog-close"
            onClick={onClose}
            aria-label="Close"
          >
            <X size={16} />
          </button>
        </div>
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-footer">{footer}</div>}
      </div>
    </div>
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
          <button className="btn" onClick={onClose} disabled={busy} autoFocus>
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
