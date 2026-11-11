import { useEffect, useState } from "react";
import { Loader2 } from "lucide-react";

import { api, errorText } from "../api";
import { copyRunToken } from "../actions";
import { Dialog } from "../components/Dialog";
import { Banner, CodeBlock, LoadingState, Notice } from "../components/ui";
import { useApp } from "../store";

export function ReinstallDialog({
  tunnelId,
  onClose,
}: {
  tunnelId: string;
  onClose: () => void;
}) {
  const state = useApp((store) => store.state);
  const navigate = useApp((store) => store.navigate);

  const [instructions, setInstructions] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const tunnel = state?.tunnels.find((row) => row.tunnel.id === tunnelId)?.tunnel;
  const hosts = state?.hosts.filter((host) => host.tunnel_id === tunnelId) ?? [];

  useEffect(() => {
    let cancelled = false;
    api
      .manualSetupInstructions(tunnelId)
      .then((value) => {
        if (!cancelled) setInstructions(value);
      })
      .catch((reason) => {
        if (!cancelled) setError(errorText(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [tunnelId]);

  return (
    <Dialog
      title="Reinstall a connector"
      description={tunnel?.name ?? tunnelId}
      onClose={onClose}
      wide
      footer={
        <>
          <button
            className="btn"
            onClick={() => {
              navigate({ page: "hosts" });
              onClose();
            }}
          >
            Open Remote Hosts
          </button>
          <button className="btn" onClick={() => void copyRunToken(tunnelId)}>
            Copy Run Token
          </button>
          <button className="btn btn-primary" onClick={onClose}>
            Close
          </button>
        </>
      }
    >
      <div className="stack">
        {error && (
          <Banner tone="error" icon="dialog-error-symbolic" title="Could not build the instructions">
            {error}
          </Banner>
        )}

        <p style={{ margin: 0 }}>
          Use this when the machine that served this tunnel was wiped, or when the
          connector needs to be installed somewhere new. The tunnel itself is unchanged.
        </p>

        {hosts.length > 0 && (
          <div className="dialog-section">
            <div className="field-label">Registered hosts for this tunnel</div>
            <div className="stack-sm">
              {hosts.map((host) => (
                <div className="row-inline spread" key={host.id}>
                  <span>
                    {host.name} ·{" "}
                    <span className="mono">
                      {host.username}@{host.address}
                    </span>
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm"
                    onClick={() => {
                      useApp.getState().openDialog({
                        kind: "deployment",
                        host_id: host.id,
                        tunnel_id: tunnelId,
                      });
                      onClose();
                    }}
                  >
                    Deploy
                  </button>
                </div>
              ))}
            </div>
          </div>
        )}

        {instructions === null ? (
          <LoadingState message="Building the instructions…" />
        ) : (
          <CodeBlock code={instructions} label="On the machine that should serve this tunnel" />
        )}

        <Notice icon="dialog-warning-symbolic">
          A deleted tunnel cannot be reinstalled: its run token is gone and Cloudflare
          will not accept it. Create a new tunnel instead. The DNS records are not
          deleted with the tunnel, so a new one can take the hostnames over.
        </Notice>

        {hosts.length === 0 && (
          <div className="field-hint">
            <Loader2 size={12} style={{ verticalAlign: -1, marginRight: 4 }} />
            No SSH host is registered for this tunnel, so the commands above are the only
            route. Remote control is optional.
          </div>
        )}
      </div>
    </Dialog>
  );
}
