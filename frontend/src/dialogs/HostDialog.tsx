import { useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { KeyRound, Loader2, ShieldCheck } from "lucide-react";

import { api, errorText } from "../api";
import { Dialog } from "../components/Dialog";
import {
  Banner,
  CodeBlock,
  Field,
  Notice,
  Property,
} from "../components/ui";
import { useApp } from "../store";
import type { HostKey, RemoteAuth, RemoteHost, RemoteHostDraft } from "../types";

export function HostDialog({
  host,
  trust,
  hostId,
  onClose,
}: {
  host?: RemoteHost;
  trust?: HostKey;
  hostId?: number;
  onClose: () => void;
}) {
  const state = useApp((store) => store.state);
  const toast = useApp((store) => store.toast);
  const openDialog = useApp((store) => store.openDialog);

  if (trust && hostId !== undefined) {
    return (
      <TrustPrompt
        host={host}
        hostId={hostId}
        trust={trust}
        onClose={onClose}
      />
    );
  }

  return (
    <HostForm
      host={host}
      onClose={onClose}
      onSaved={async (id, passphrase) => {
        if (passphrase) {
          try {
            await api.saveHostPassphrase(id, passphrase);
          } catch (error) {
            toast(errorText(error), "warning");
          }
        }
        toast("Host registered. Reading its key…", "success");
        try {
          const key = await api.probeHostKey(id);
          openDialog({ kind: "host", trust: key, host_id: id, host: undefined });
        } catch (error) {
          toast(
            `The host was saved, but its key could not be read: ${errorText(error)}`,
            "warning",
          );
          onClose();
        }
      }}
      state={state}
    />
  );
}

function HostForm({
  host,
  onClose,
  onSaved,
  state,
}: {
  host?: RemoteHost;
  onClose: () => void;
  onSaved: (id: number, passphrase: string | null) => Promise<void>;
  state: ReturnType<typeof useApp.getState>["state"];
}) {
  const toast = useApp((store) => store.toast);

  const [name, setName] = useState(host?.name ?? "");
  const [address, setAddress] = useState(host?.address ?? "");
  const [port, setPort] = useState(String(host?.port ?? 22));
  const [username, setUsername] = useState(
    host?.username ?? state?.settings.default_ssh_username ?? "",
  );
  const [authKind, setAuthKind] = useState<"agent" | "key_file">(
    host?.auth.kind ?? "agent",
  );
  const [keyPath, setKeyPath] = useState(
    host?.auth.kind === "key_file" ? host.auth.data.path : "",
  );
  const [passphrase, setPassphrase] = useState("");
  const [unit, setUnit] = useState(
    host?.service_unit ?? state?.settings.default_service_unit ?? "cloudflared.service",
  );
  const [tunnelId, setTunnelId] = useState(host?.tunnel_id ?? "");
  const [notes, setNotes] = useState(host?.notes ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const tunnels = state?.tunnels ?? [];
  const hasSavedPassphrase =
    host?.auth.kind === "key_file" && host.auth.data.has_passphrase;

  async function chooseKey() {
    try {
      const selected = await openFileDialog({
        multiple: false,
        directory: false,
        title: "Choose a private key",
      });
      if (typeof selected === "string") setKeyPath(selected);
    } catch (reason) {
      setError(errorText(reason));
    }
  }

  async function save() {
    setBusy(true);
    setError(null);

    const auth: RemoteAuth =
      authKind === "agent"
        ? { kind: "agent" }
        : {
            kind: "key_file",
            data: {
              path: keyPath,
              has_passphrase: Boolean(passphrase) || Boolean(hasSavedPassphrase),
            },
          };

    const draft: RemoteHostDraft = {
      name: name.trim(),
      address: address.trim(),
      port: Number(port) || 0,
      username: username.trim(),
      auth,
      service_unit: unit.trim(),
      tunnel_id: tunnelId || null,
      notes: notes.trim() ? notes.trim() : null,
    };

    try {
      if (host) {
        await api.updateHost({
          ...host,
          ...draft,
        });
        if (passphrase && authKind === "key_file") {
          await api.saveHostPassphrase(host.id, passphrase);
        }
        toast("Host updated.", "success");
        onClose();
      } else {
        const id = await api.saveHost(draft);
        await onSaved(id, passphrase && authKind === "key_file" ? passphrase : null);
      }
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  const canSave =
    name.trim().length > 0 &&
    address.trim().length > 0 &&
    username.trim().length > 0 &&
    unit.trim().length > 0 &&
    (authKind === "agent" || keyPath.trim().length > 0);

  return (
    <Dialog
      title={host ? "Edit host" : "Add a remote host"}
      description={
        host
          ? `${host.username}@${host.address}:${host.port}`
          : "Remote control is optional. A host profile only exists because you add one."
      }
      onClose={onClose}
      wide
      closeOnBackdrop={!busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => void save()}
            disabled={busy || !canSave}
          >
            {busy && <Loader2 size={14} className="spin" />}
            {host ? "Save Changes" : "Add and Verify"}
          </button>
        </>
      }
    >
      <div className="dialog-form">
        {error && (
          <Banner tone="error" icon="dialog-error-symbolic" title="Could not save">
            {error}
          </Banner>
        )}

        <div className="field-row">
          <Field label="Name" htmlFor="host-name" hint="A label you recognise, such as “edge-frankfurt”.">
            <input
              id="host-name"
              className="input"
              value={name}
              autoFocus
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field label="Address" htmlFor="host-address" hint="A hostname or IP address. No user@ here.">
            <input
              id="host-address"
              className="input"
              value={address}
              spellCheck={false}
              onChange={(event) => setAddress(event.target.value)}
            />
          </Field>
        </div>

        <div className="field-row">
          <Field label="SSH user" htmlFor="host-user">
            <input
              id="host-user"
              className="input"
              value={username}
              spellCheck={false}
              onChange={(event) => setUsername(event.target.value)}
            />
          </Field>
          <Field label="Port" htmlFor="host-port">
            <input
              id="host-port"
              className="input"
              inputMode="numeric"
              value={port}
              onChange={(event) => setPort(event.target.value)}
            />
          </Field>
        </div>

        <div className="dialog-section">
          <div className="field-label">Authentication</div>
          <div className="chips">
            <button
              type="button"
              className={`chip${authKind === "agent" ? " active" : ""}`}
              onClick={() => setAuthKind("agent")}
            >
              ssh-agent
            </button>
            <button
              type="button"
              className={`chip${authKind === "key_file" ? " active" : ""}`}
              onClick={() => setAuthKind("key_file")}
            >
              Key file
            </button>
          </div>

          {authKind === "agent" ? (
            <div className="field-hint">
              Uses whatever <span className="mono">ssh-agent</span> is offering. Nothing
              is stored and no key is read from disk.
            </div>
          ) : (
            <div className="stack-sm">
              <Field label="Private key" htmlFor="host-key">
                <div className="row-inline">
                  <input
                    id="host-key"
                    className="input mono"
                    value={keyPath}
                    spellCheck={false}
                    onChange={(event) => setKeyPath(event.target.value)}
                  />
                  <button type="button" className="btn" onClick={() => void chooseKey()}>
                    Browse…
                  </button>
                </div>
              </Field>
              <Field
                label="Passphrase"
                htmlFor="host-passphrase"
                hint={
                  hasSavedPassphrase
                    ? "A passphrase is already saved. Leave this empty to keep it."
                    : "Optional. Stored in the system credential store, never in the database."
                }
              >
                <input
                  id="host-passphrase"
                  className="input"
                  type="password"
                  value={passphrase}
                  autoComplete="off"
                  onChange={(event) => setPassphrase(event.target.value)}
                />
              </Field>
            </div>
          )}
        </div>

        <div className="dialog-section">
          <div className="field-row">
            <Field
              label="Service unit"
              htmlFor="host-unit"
              hint="The systemd unit cloudflared installed."
            >
              <input
                id="host-unit"
                className="input mono"
                value={unit}
                onChange={(event) => setUnit(event.target.value)}
              />
            </Field>
            <Field
              label="Tunnel this host serves"
              htmlFor="host-tunnel"
              hint="Optional. Used for restart warnings and the detail page."
            >
              <select
                id="host-tunnel"
                className="select"
                value={tunnelId}
                onChange={(event) => setTunnelId(event.target.value)}
              >
                <option value="">Not associated</option>
                {tunnels.map((row) => (
                  <option key={row.tunnel.id} value={row.tunnel.id}>
                    {row.tunnel.name}
                  </option>
                ))}
              </select>
            </Field>
          </div>

          <Field label="Notes" htmlFor="host-notes" hint="Optional. Shown in the host list.">
            <textarea
              id="host-notes"
              className="textarea"
              value={notes}
              onChange={(event) => setNotes(event.target.value)}
            />
          </Field>
        </div>

        <Notice icon="security-high-symbolic">
          On the first connection the host key is shown and has to be trusted before
          anything is sent. A key that later changes is a refusal, not a prompt.
        </Notice>
      </div>
    </Dialog>
  );
}

function TrustPrompt({
  host,
  hostId,
  trust,
  onClose,
}: {
  host?: RemoteHost;
  hostId: number;
  trust: HostKey;
  onClose: () => void;
}) {
  const state = useApp((store) => store.state);
  const toast = useApp((store) => store.toast);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const known = host ?? state?.hosts.find((item) => item.id === hostId);

  async function trustAndCheck() {
    setBusy(true);
    setError(null);
    try {
      await api.trustHostKey(hostId, trust.fingerprint);
      try {
        await api.preflightHost(hostId);
        toast("Host trusted and reachable.", "success");
      } catch (reason) {
        toast(
          `The key was trusted, but the checks did not pass: ${errorText(reason)}`,
          "warning",
        );
      }
      onClose();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      title="Trust this host?"
      description={
        known ? `First connection to ${known.name}` : "A host key has to be approved."
      }
      onClose={onClose}
      wide
      closeOnBackdrop={!busy}
      footer={
        <>
          <button className="btn" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={() => void trustAndCheck()}
            disabled={busy}
          >
            {busy ? <Loader2 size={14} className="spin" /> : <ShieldCheck size={14} />}
            Trust and Check
          </button>
        </>
      }
    >
      <div className="stack">
        {error && (
          <Banner tone="error" icon="dialog-error-symbolic" title="Could not trust the host">
            {error}
          </Banner>
        )}

        <p style={{ margin: 0 }}>
          The host presented this key. Compare it with what the server reports before
          trusting it — a fingerprint read over the same channel you are checking is
          the whole point.
        </p>

        <div className="property-grid">
          <Property label="Algorithm" value={trust.algorithm} mono />
          <Property label="Fingerprint" value={trust.fingerprint} mono />
        </div>

        <CodeBlock
          code={`ssh-keyscan -t ${trust.algorithm} ${
            known?.address ?? "the-host"
          } | ssh-keygen -lf -`}
          label="Compare it yourself"
        />

        <Notice icon="dialog-warning-symbolic">
          A key that changes later is a refusal, not a prompt. Nothing has been sent to
          the host yet, and nothing will be until you trust it.
        </Notice>

        <div className="field-hint">
          <KeyRound size={12} style={{ verticalAlign: -1, marginRight: 4 }} />
          The passphrase path is unchanged: an encrypted key needs one in the credential
          store before authentication can succeed.
        </div>
      </div>
    </Dialog>
  );
}
