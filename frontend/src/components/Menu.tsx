import { useEffect, useRef, useState, type ReactNode } from "react";

import { MoreVertical, SymbolicIcon } from "../icons";

export function MenuButton({
  label = "More",
  children,
}: {
  label?: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (anchor.current && !anchor.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="menu-anchor" ref={anchor}>
      <button
        type="button"
        className="btn btn-ghost btn-icon"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <MoreVertical size={18} />
      </button>
      {open && (
        <div className="menu-popover" onClick={() => setOpen(false)}>
          <div className="menu" role="menu">
            {children}
          </div>
        </div>
      )}
    </div>
  );
}

export function MenuItem({
  icon,
  label,
  shortcut,
  onClick,
  danger = false,
}: {
  icon?: string;
  label: string;
  shortcut?: string;
  onClick: () => void;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      className="menu-item"
      role="menuitem"
      onClick={onClick}
      style={danger ? { color: "var(--danger)" } : undefined}
    >
      {icon && <SymbolicIcon name={icon} size={15} />}
      <span>{label}</span>
      {shortcut && <span className="menu-shortcut">{shortcut}</span>}
    </button>
  );
}

export function MenuSeparator() {
  return <div className="menu-separator" role="separator" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <div className="menu-label">{children}</div>;
}
