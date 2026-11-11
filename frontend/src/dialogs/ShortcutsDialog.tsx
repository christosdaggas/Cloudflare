import { Dialog } from "../components/Dialog";
import { Kbd } from "../components/ui";

const SHORTCUTS: { group: string; items: { label: string; keys: string[] }[] }[] = [
  {
    group: "General",
    items: [
      { label: "Show keyboard shortcuts", keys: ["Ctrl", "?"] },
      { label: "Quit", keys: ["Ctrl", "Q"] },
      { label: "Close the window", keys: ["Ctrl", "W"] },
    ],
  },
  {
    group: "Navigation",
    items: [
      { label: "Collapse or expand the sidebar", keys: ["F9"] },
      { label: "Focus the search field", keys: ["Ctrl", "F"] },
      { label: "Refresh everything", keys: ["Ctrl", "R"] },
    ],
  },
  {
    group: "Actions",
    items: [
      { label: "Create a tunnel", keys: ["Ctrl", "N"] },
      { label: "Close a dialog or dismiss a menu", keys: ["Esc"] },
    ],
  },
];

export function ShortcutsDialog({ onClose }: { onClose: () => void }) {
  return (
    <Dialog
      title="Keyboard shortcuts"
      description="Every action in this window has a keyboard route."
      onClose={onClose}
      footer={
        <button className="btn btn-primary" onClick={onClose}>
          Close
        </button>
      }
    >
      <div className="stack">
        {SHORTCUTS.map((section) => (
          <div className="dialog-section" key={section.group}>
            <div className="field-label">{section.group}</div>
            <div className="group">
              {section.items.map((item) => (
                <div className="row" key={item.label}>
                  <div className="row-main">
                    <div className="row-title" style={{ fontWeight: 500 }}>
                      {item.label}
                    </div>
                  </div>
                  <div className="row-actions">
                    {item.keys.map((key) => (
                      <Kbd key={key}>{key}</Kbd>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    </Dialog>
  );
}
