import { AboutDialog } from "./AboutDialog";
import { CreateTunnelDialog } from "./CreateTunnelDialog";
import { DeploymentDialog } from "./DeploymentDialog";
import { HostDialog } from "./HostDialog";
import { HostnameDialog } from "./HostnameDialog";
import { ReinstallDialog } from "./ReinstallDialog";
import { ShortcutsDialog } from "./ShortcutsDialog";
import { WhatsNewDialog } from "./WhatsNewDialog";
import { useApp } from "../store";

/** The one place every global dialog is mounted. */
export function DialogHost() {
  const dialog = useApp((store) => store.dialog);
  const close = useApp((store) => store.closeDialog);
  const state = useApp((store) => store.state);

  if (!dialog) return null;

  switch (dialog.kind) {
    case "create-tunnel":
      return <CreateTunnelDialog onClose={close} />;
    case "hostname":
      return (
        <HostnameDialog
          tunnelId={dialog.tunnel_id}
          hostname={dialog.hostname}
          onClose={close}
        />
      );
    case "host":
      return (
        <HostDialog
          host={dialog.host}
          trust={dialog.trust}
          hostId={dialog.host_id}
          onClose={close}
        />
      );
    case "deployment": {
      const host = state?.hosts.find((item) => item.id === dialog.host_id);
      if (!host) return null;
      return (
        <DeploymentDialog
          host={host}
          initialTunnelId={dialog.tunnel_id}
          onClose={close}
        />
      );
    }
    case "reinstall":
      return <ReinstallDialog tunnelId={dialog.tunnel_id} onClose={close} />;
    case "shortcuts":
      return <ShortcutsDialog onClose={close} />;
    case "about":
      return <AboutDialog onClose={close} />;
    case "whats-new":
      return <WhatsNewDialog onClose={close} />;
  }
}
