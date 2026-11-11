import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { Menu } from "lucide-react";

import { MoreVertical, SymbolicIcon } from "../icons";

export function MenuButton({
  label = "More",
  id,
  children,
}: {
  label?: string;
  id?: string;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    anchor.current
      ?.querySelector<HTMLButtonElement>("[role='menuitem']")
      ?.focus();
    const onDown = (event: MouseEvent) => {
      if (anchor.current && !anchor.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
      }
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  function navigateMenu(event: ReactKeyboardEvent<HTMLDivElement>) {
    const items = Array.from(
      anchor.current?.querySelectorAll<HTMLButtonElement>(
        "[role='menuitem']",
      ) ?? [],
    );
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    let next: number;
    if (event.key === "ArrowDown") next = (index + 1) % items.length;
    else if (event.key === "ArrowUp")
      next = (index - 1 + items.length) % items.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    else if (event.key === "Tab") {
      setOpen(false);
      trigger.current?.focus();
      return;
    } else return;
    event.preventDefault();
    items[next]?.focus();
  }

  return (
    <div className="menu-anchor" ref={anchor}>
      <button
        type="button"
        id={id}
        ref={trigger}
        className="btn btn-ghost btn-icon"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        title={id === "main-menu-trigger" ? "Main menu (F10)" : label}
        onClick={() => setOpen((value) => !value)}
      >
        {id === "main-menu-trigger" ? (
          <Menu size={18} aria-hidden="true" />
        ) : (
          <MoreVertical size={18} aria-hidden="true" />
        )}
      </button>
      {open && (
        <div
          className="menu-popover"
          onKeyDown={navigateMenu}
          onClick={() => {
            setOpen(false);
            trigger.current?.focus();
          }}
        >
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
