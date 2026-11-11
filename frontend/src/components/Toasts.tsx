import { X, symbolicIcon } from "../icons";
import { useApp } from "../store";

export function Toasts() {
  const toasts = useApp((store) => store.toasts);
  const dismiss = useApp((store) => store.dismissToast);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-stack" aria-live="polite" aria-atomic="false">
      {toasts.map((toast) => {
        const Icon = symbolicIcon(
          toast.kind === "success"
            ? "object-select-symbolic"
            : toast.kind === "warning"
              ? "dialog-warning-symbolic"
              : toast.kind === "error"
                ? "dialog-error-symbolic"
                : "dialog-information-symbolic",
        );
        return (
          <div className={`toast ${toast.kind}`} key={toast.id} role="status">
            <Icon size={16} />
            <span>{toast.message}</span>
            {toast.action && (
              <button
                type="button"
                className="toast-action"
                onClick={() => {
                  toast.action?.run();
                  dismiss(toast.id);
                }}
              >
                {toast.action.label}
              </button>
            )}
            <button
              type="button"
              className="toast-dismiss"
              aria-label="Dismiss"
              onClick={() => dismiss(toast.id)}
            >
              <X size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
