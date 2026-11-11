import { useEffect, useRef, useState, type ReactNode } from "react";
import { writeText as clipboardWrite } from "@tauri-apps/plugin-clipboard-manager";

import {
  Check,
  ChevronDown,
  Copy,
  Info,
  Loader2,
  X,
  SymbolicIcon,
} from "../icons";
import type { Semantic } from "../status";
import { useApp } from "../store";

// ── Status ───────────────────────────────────────────────────────────────

export function StatusBadge({
  status,
  large = false,
}: {
  status: { label: string; tone: Semantic; icon: string };
  large?: boolean;
}) {
  return (
    <span
      className={`badge tone-${status.tone}${large ? " badge-lg" : ""}`}
      title={status.label}
    >
      <SymbolicIcon name={status.icon} size={large ? 14 : 12} />
      <span>{status.label}</span>
    </span>
  );
}

export function Badge({
  tone = "neutral",
  icon,
  children,
}: {
  tone?: Semantic;
  icon?: string;
  children: ReactNode;
}) {
  return (
    <span className={`badge tone-${tone}`}>
      {icon && <SymbolicIcon name={icon} size={12} />}
      <span>{children}</span>
    </span>
  );
}

// ── Feedback ─────────────────────────────────────────────────────────────

export function Banner({
  tone = "info",
  icon = "dialog-information-symbolic",
  title,
  children,
  action,
}: {
  tone?: "info" | "warning" | "error";
  icon?: string;
  title?: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div
      className={`banner ${tone}`}
      role={tone === "error" ? "alert" : "status"}
    >
      <SymbolicIcon name={icon} size={17} />
      <div className="banner-main">
        {title && <div className="banner-title">{title}</div>}
        {children && <div className="banner-text">{children}</div>}
      </div>
      {action}
    </div>
  );
}

export function Notice({
  icon = "dialog-information-symbolic",
  children,
}: {
  icon?: string;
  children: ReactNode;
}) {
  return (
    <div className="notice">
      <SymbolicIcon name={icon} size={15} />
      <div>{children}</div>
    </div>
  );
}

export function EmptyState({
  icon = "view-grid-symbolic",
  title,
  description,
  action,
}: {
  icon?: string;
  title: string;
  description?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty-state">
      <div className="empty-icon">
        <SymbolicIcon name={icon} size={26} />
      </div>
      <h3>{title}</h3>
      {description && <p>{description}</p>}
      {action}
    </div>
  );
}

export function LoadingState({ message = "Loading…" }: { message?: string }) {
  return (
    <div className="loading-state">
      <Loader2 size={22} className="spin" />
      <span>{message}</span>
    </div>
  );
}

export function Spinner({ size = 14 }: { size?: number }) {
  return <Loader2 size={size} className="spin" aria-label="Working" />;
}

export function Kbd({ children }: { children: ReactNode }) {
  return <span className="kbd">{children}</span>;
}

export function Property({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="property">
      <div className="property-label">{label}</div>
      <div className={`property-value${mono ? " mono" : ""}`}>{value}</div>
    </div>
  );
}

export function StatCard({
  value,
  label,
  detail,
  icon,
  tone = "accent",
  onClick,
}: {
  value: ReactNode;
  label: string;
  detail?: ReactNode;
  icon: string;
  tone?: Semantic;
  onClick?: () => void;
}) {
  const content = (
    <>
      <div className="stat-top">
        <div className={`stat-icon tone-${tone}`}>
          <SymbolicIcon name={icon} size={16} />
        </div>
        <div className="stat-label">{label}</div>
      </div>
      <div
        className={`stat-value${typeof value === "string" && value.length > 4 ? " stat-value-text" : ""}`}
      >
        {value}
      </div>
      {detail && <div className="stat-detail">{detail}</div>}
    </>
  );

  if (onClick) {
    return (
      <button type="button" className="stat-card clickable" onClick={onClick}>
        {content}
      </button>
    );
  }

  return <div className="stat-card">{content}</div>;
}

// ── Clipboard ────────────────────────────────────────────────────────────

export async function copyText(text: string): Promise<void> {
  try {
    await clipboardWrite(text);
  } catch {
    // A webview without the clipboard plugin still has the DOM fallback.
    await navigator.clipboard.writeText(text);
  }
}

export function CopyButton({
  value,
  label = "Copy",
  className = "",
  disabled = false,
}: {
  value: string;
  label?: string;
  className?: string;
  disabled?: boolean;
}) {
  const toast = useApp((store) => store.toast);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1600);
    return () => window.clearTimeout(timer);
  }, [copied]);

  return (
    <button
      type="button"
      className={`btn btn-sm ${className}`}
      disabled={disabled}
      onClick={async () => {
        try {
          await copyText(value);
          setCopied(true);
          toast("Copied to the clipboard.", "success");
        } catch (error) {
          toast(`Could not copy: ${String(error)}`, "error");
        }
      }}
      aria-label={label}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
      {label && <span>{label}</span>}
    </button>
  );
}

export function CodeBlock({
  code,
  label,
  copy = true,
}: {
  code: string;
  label?: string;
  copy?: boolean;
}) {
  return (
    <div className="stack-sm">
      {label && (
        <div className="row-inline spread">
          <span className="field-label">{label}</span>
          {copy && <CopyButton value={code} />}
        </div>
      )}
      <div className="code-block-wrap">
        <pre className="code-block">{code}</pre>
        {!label && copy && (
          <CopyButton value={code} label="" className="copy-float" />
        )}
      </div>
    </div>
  );
}

// ── Forms ────────────────────────────────────────────────────────────────

export function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
}: {
  label?: string;
  hint?: ReactNode;
  error?: string | null;
  htmlFor?: string;
  children: ReactNode;
}) {
  return (
    <div className="field">
      {label && (
        <label className="field-label" htmlFor={htmlFor}>
          {label}
        </label>
      )}
      {children}
      {error ? (
        <div className="field-error" role="alert">
          {error}
        </div>
      ) : (
        hint && <div className="field-hint">{hint}</div>
      )}
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      className="switch"
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

// ── Expander ─────────────────────────────────────────────────────────────

export function Expander({
  header,
  children,
  defaultOpen = false,
  actions,
}: {
  header: ReactNode;
  children: ReactNode;
  defaultOpen?: boolean;
  actions?: ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const bodyRef = useRef<HTMLDivElement>(null);

  return (
    <div className="group">
      <div
        className="row"
        style={{ cursor: "pointer" }}
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            setOpen((value) => !value);
          }
        }}
      >
        {header}
        <div
          className="row-actions"
          onClick={(event) => event.stopPropagation()}
        >
          {actions}
          <button
            type="button"
            className="btn btn-ghost btn-icon"
            aria-label={open ? "Collapse" : "Expand"}
            onClick={() => setOpen((value) => !value)}
          >
            <ChevronDown
              size={16}
              style={{
                transform: open ? "rotate(180deg)" : undefined,
                transition: "transform 140ms ease",
              }}
            />
          </button>
        </div>
      </div>
      {open && (
        <div ref={bodyRef} style={{ padding: "0 16px 16px" }}>
          {children}
        </div>
      )}
    </div>
  );
}

// ── Dialog chrome ────────────────────────────────────────────────────────

export function InfoTip({ children }: { children: ReactNode }) {
  return (
    <span title={String(children)}>
      <Info size={13} className="dimmed" />
    </span>
  );
}

export function ClosableNotice({
  children,
  onClose,
}: {
  children: ReactNode;
  onClose: () => void;
}) {
  return (
    <div className="banner info">
      <SymbolicIcon name="dialog-information-symbolic" size={17} />
      <div className="banner-main">{children}</div>
      <button
        className="btn btn-ghost btn-icon"
        onClick={onClose}
        aria-label="Dismiss"
      >
        <X size={15} />
      </button>
    </div>
  );
}
