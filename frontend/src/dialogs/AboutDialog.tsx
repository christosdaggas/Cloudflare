import { ExternalLink } from "lucide-react";

import { api } from "../api";
import { Dialog } from "../components/Dialog";
import { Notice } from "../components/ui";
import { useApp } from "../store";

const LINKS: { label: string; url: string }[] = [
  {
    label: "Website",
    url: "https://chrisdaggas.com",
  },
  {
    label: "Source and issues",
    url: "https://github.com/christosdaggas/Cloudflare",
  },
  {
    label: "Cloudflare Tunnel documentation",
    url: "https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/",
  },
  {
    label: "How to create a scoped API token",
    url: "https://developers.cloudflare.com/fundamentals/api/get-started/create-token/",
  },
  {
    label: "Contact",
    url: "mailto:info@chrisdaggas.com",
  },
];

export function AboutDialog({ onClose }: { onClose: () => void }) {
  const bootstrap = useApp((store) => store.bootstrap);

  return (
    <Dialog
      title={bootstrap?.app_name ?? "Tunnel Manager"}
      description={`Version ${bootstrap?.version ?? "1.0.0"} · ${bootstrap?.platform ?? "Linux"}`}
      onClose={onClose}
      footer={
        <button className="btn btn-primary" onClick={onClose}>
          Close
        </button>
      }
    >
      <div className="stack">
        <div className="row-inline" style={{ gap: 16 }}>
          <div className="onboarding-logo" style={{ width: 56, height: 56 }}>
            <img src="/app-icon.svg" alt="" />
          </div>
          <div>
            <div style={{ fontWeight: 700, fontSize: 15 }}>
              {bootstrap?.tagline ?? "Manage remotely configured Cloudflare Tunnels"}
            </div>
            <div className="field-hint" style={{ marginTop: 2 }}>
              An unofficial, third-party desktop client. Not affiliated with or endorsed
              by Cloudflare, Inc.
            </div>
          </div>
        </div>

        <div className="group">
          {LINKS.map((link) => (
            <button
              type="button"
              className="row"
              key={link.label}
              style={{ width: "100%", background: "transparent", border: 0, textAlign: "left" }}
              onClick={() => void api.openUrl(link.url)}
            >
              <div className="row-main">
                <div className="row-title" style={{ fontWeight: 500 }}>
                  {link.label}
                </div>
                <div className="row-subtitle mono">{link.url}</div>
              </div>
              <ExternalLink size={14} className="dimmed" />
            </button>
          ))}
        </div>

        <Notice icon="dialog-information-symbolic">
          Your API token is stored in the desktop's credential store, never in a file.
          The application asks only for Cloudflare Tunnel → Edit, Zone → Read and DNS →
          Edit.
        </Notice>

        <div className="field-hint" style={{ textAlign: "center" }}>
          © 2026 Christos A. Daggas · MIT licence
        </div>
      </div>
    </Dialog>
  );
}
