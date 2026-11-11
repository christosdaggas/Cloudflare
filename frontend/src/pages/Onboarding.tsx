import { useState, type ReactNode } from "react";
import {
  AlertCircle,
  ArrowRight,
  CheckCircle2,
  Eye,
  EyeOff,
  KeyRound,
  Loader2,
  Lock,
  Server,
  ShieldCheck,
  Waypoints,
} from "lucide-react";

import { api, errorText } from "../api";
import { Badge, Banner, Field, Notice } from "../components/ui";
import { useApp } from "../store";
import type { TokenCheck, Zone } from "../types";

export function Onboarding() {
  const bootstrap = useApp((store) => store.bootstrap);
  const connection = useApp((store) => store.state?.connection);
  const setState = useApp((store) => store.setState);
  const toast = useApp((store) => store.toast);

  const [token, setToken] = useState("");
  const [showToken, setShowToken] = useState(false);
  const [remember, setRemember] = useState(true);
  const [check, setCheck] = useState<TokenCheck | null>(null);
  const [accountId, setAccountId] = useState("");
  const [manualAccount, setManualAccount] = useState("");
  const [zones, setZones] = useState<Zone[]>([]);
  const [zoneId, setZoneId] = useState("");
  const [zoneProblem, setZoneProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState<"check" | "zones" | "connect" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const manual = check !== null && check.accounts.length === 0;
  const account = manual ? manualAccount.trim() : accountId;
  const canConnect = check !== null && account.length > 0 && busy === null;

  async function testToken() {
    if (!token.trim()) {
      setError("Paste an API token first.");
      return;
    }

    setBusy("check");
    setError(null);
    setCheck(null);
    setZones([]);
    setZoneId("");
    setZoneProblem(null);

    try {
      const result = await api.checkToken(token.trim());
      setCheck(result);
      if (result.accounts.length > 0) {
        setAccountId(result.accounts[0].id);
        await loadZones(result.accounts[0].id);
      } else {
        setAccountId("");
      }
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  async function loadZones(forAccount: string) {
    if (!forAccount) return;
    setBusy("zones");
    setZoneProblem(null);
    try {
      const result = await api.previewZones(token.trim(), forAccount);
      setZones(result);
      setZoneId(result[0]?.id ?? "");
    } catch (reason) {
      setZones([]);
      setZoneId("");
      setZoneProblem(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  async function finish() {
    if (!account) {
      setError("Choose an account, or enter its ID.");
      return;
    }

    setBusy("connect");
    setError(null);
    try {
      const result = await api.connect(
        token.trim(),
        account,
        zoneId || null,
        remember,
      );
      setState(result);
      toast("Connected to Cloudflare.", "success");
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="onboarding">
      <section className="onboarding-hero">
        <div className="onboarding-hero-top">
          <div className="onboarding-logo">
            <img src="/app-icon.svg" alt="" />
          </div>
          <div>
            <h1>{bootstrap?.app_name ?? "Tunnel Manager"}</h1>
            <p>{bootstrap?.tagline ?? "Manage remotely configured Cloudflare Tunnels"}</p>
          </div>

          <div className="stack" style={{ gap: 14, marginTop: 10 }}>
            <HeroPoint
              icon={<KeyRound size={17} />}
              title="A scoped token, kept in your keyring"
              body="Only Tunnel:Edit, Zone:Read and DNS:Edit are asked for. The token never goes to a file."
            />
            <HeroPoint
              icon={<Waypoints size={17} />}
              title="Tunnels, hostnames and DNS together"
              body="Publish a hostname with one reviewed click, and see at a glance when the ingress rule and DNS disagree."
            />
            <HeroPoint
              icon={<Server size={17} />}
              title="SSH control is optional"
              body="Everything works without it. When you want it, host keys are pinned and every action is previewed."
            />
          </div>
        </div>

        <div className="onboarding-hero-foot">
          {bootstrap?.affiliation_notice ??
            "An unofficial client for the Cloudflare API. Not affiliated with or endorsed by Cloudflare, Inc."}
        </div>
      </section>

      <section className="onboarding-panel">
        <div className="onboarding-form">
          <div>
            <h2>Connect to Cloudflare</h2>
            <p style={{ margin: "6px 0 0", color: "var(--fg-muted)", fontSize: 13 }}>
              Create a token in the Cloudflare dashboard, then paste it here. It is
              checked before anything is saved.
            </p>
          </div>

          {error && (
            <Banner tone="error" icon="dialog-error-symbolic" title="Could not connect">
              {error}
            </Banner>
          )}

          {!error && connection?.kind === "failed" && (
            <Banner
              tone="error"
              icon="dialog-error-symbolic"
              title={connection.data.title}
            >
              {connection.data.message}
            </Banner>
          )}

          <Field
            label="API token"
            htmlFor="token"
            hint={
              <span>
                Needs <strong>Cloudflare Tunnel → Edit</strong>,{" "}
                <strong>Zone → Read</strong> and <strong>DNS → Edit</strong>.{" "}
                <button
                  type="button"
                  className="btn-link"
                  onClick={() =>
                    void api.openUrl(
                      "https://developers.cloudflare.com/fundamentals/api/get-started/create-token/",
                    )
                  }
                >
                  How to create a scoped token
                </button>
              </span>
            }
          >
            <div style={{ position: "relative" }}>
              <input
                id="token"
                className="input"
                type={showToken ? "text" : "password"}
                value={token}
                placeholder="Paste the token Cloudflare generated"
                autoComplete="off"
                spellCheck={false}
                onChange={(event) => setToken(event.target.value)}
                style={{ paddingRight: 42 }}
              />
              <button
                type="button"
                className="btn btn-ghost btn-icon"
                style={{ position: "absolute", right: 1, top: 1, height: 34 }}
                aria-label={showToken ? "Hide the token" : "Show the token"}
                onClick={() => setShowToken((value) => !value)}
              >
                {showToken ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
            </div>
          </Field>

          <label className="checkbox">
            <input
              type="checkbox"
              checked={remember}
              onChange={(event) => setRemember(event.target.checked)}
            />
            <span>
              Remember this token in the system credential store
              <span className="field-hint" style={{ display: "block" }}>
                Without this, you will paste it again next time.
              </span>
            </span>
          </label>

          <button
            type="button"
            className="btn btn-primary btn-lg btn-wide"
            onClick={() => void testToken()}
            disabled={busy !== null || token.trim().length === 0}
          >
            {busy === "check" ? (
              <Loader2 size={16} className="spin" />
            ) : (
              <ShieldCheck size={16} />
            )}
            {check ? "Check Again" : "Test Connection"}
          </button>

          {check && (
            <>
              <div className="steps">
                <div className="step done">
                  <div className="step-marker">
                    <CheckCircle2 size={14} />
                  </div>
                  <div className="step-body">
                    <div className="step-title">Token accepted</div>
                    <div className="step-detail">
                      Cloudflare reports this token as{" "}
                      <strong>{check.status.status}</strong>
                      {check.status.expires_on
                        ? `, expiring ${new Date(check.status.expires_on).toLocaleDateString()}`
                        : ""}
                      .
                    </div>
                  </div>
                  <Badge tone="success" icon="object-select-symbolic">
                    Active
                  </Badge>
                </div>

                <div className="step current">
                  <div className="step-marker">2</div>
                  <div className="step-body">
                    <div className="step-title">Choose the account</div>
                    {check.accounts.length > 0 ? (
                      <Field htmlFor="account" hint="The zones and tunnels shown belong to this account.">
                        <select
                          id="account"
                          className="select"
                          value={accountId}
                          onChange={(event) => {
                            setAccountId(event.target.value);
                            void loadZones(event.target.value);
                          }}
                        >
                          {check.accounts.map((item) => (
                            <option key={item.id} value={item.id}>
                              {item.name} · {item.id.slice(0, 6)}…{item.id.slice(-4)}
                            </option>
                          ))}
                        </select>
                      </Field>
                    ) : (
                      <div className="stack-sm" style={{ marginTop: 8 }}>
                        <Field
                          label="Account ID"
                          htmlFor="manual-account"
                          hint={
                            check.accounts_unavailable ??
                            "This token is scoped to one account, so its ID has to be entered by hand."
                          }
                        >
                          <div className="row-inline">
                            <input
                              id="manual-account"
                              className="input"
                              value={manualAccount}
                              placeholder="32 hexadecimal characters"
                              spellCheck={false}
                              onChange={(event) => setManualAccount(event.target.value)}
                            />
                            <button
                              type="button"
                              className="btn"
                              disabled={manualAccount.trim().length === 0 || busy !== null}
                              onClick={() => void loadZones(manualAccount.trim())}
                            >
                              {busy === "zones" ? <Loader2 size={15} className="spin" /> : null}
                              Load domains
                            </button>
                          </div>
                        </Field>
                      </div>
                    )}
                  </div>
                </div>

                <div className="step">
                  <div className="step-marker">3</div>
                  <div className="step-body">
                    <div className="step-title">Default domain</div>
                    {zones.length > 0 ? (
                      <Field
                        htmlFor="zone"
                        hint="Offered first when you publish a hostname. You can change it later."
                      >
                        <select
                          id="zone"
                          className="select"
                          value={zoneId}
                          onChange={(event) => setZoneId(event.target.value)}
                        >
                          <option value="">No default domain</option>
                          {zones.map((zone) => (
                            <option key={zone.id} value={zone.id}>
                              {zone.name}
                            </option>
                          ))}
                        </select>
                      </Field>
                    ) : zoneProblem ? (
                      <Notice icon="dialog-warning-symbolic">
                        The domains could not be listed: {zoneProblem} You can still
                        connect; tunnels work without Zone:Read, and hostname publishing
                        can be enabled later.
                      </Notice>
                    ) : (
                      <div className="field-hint" style={{ marginTop: 6 }}>
                        {busy === "zones"
                          ? "Loading the domains this token can see…"
                          : "No domains were found. You can still manage tunnels."}
                      </div>
                    )}
                  </div>
                </div>
              </div>

              <div className="settings-section" style={{ marginTop: 4 }}>
                <h2>
                  <Lock size={13} style={{ marginRight: 6, verticalAlign: -2 }} />
                  What the token is used for
                </h2>
                <div className="group">
                  {(bootstrap?.required_permissions ?? []).map((permission) => (
                    <div className="permission" key={permission.dashboard_name + permission.level}>
                      <div className="row-icon">
                        <ShieldCheck size={16} />
                      </div>
                      <div className="permission-main">
                        <div className="permission-name">
                          {permission.dashboard_name} · {permission.level}
                        </div>
                        <div className="permission-scope">
                          {permission.scope} · {permission.needed_for}
                        </div>
                      </div>
                      <Badge tone="neutral" icon="dialog-question-symbolic">
                        Checked on connect
                      </Badge>
                    </div>
                  ))}
                </div>
              </div>

              <button
                type="button"
                className="btn btn-primary btn-lg btn-wide"
                disabled={!canConnect}
                onClick={() => void finish()}
              >
                {busy === "connect" ? (
                  <Loader2 size={16} className="spin" />
                ) : (
                  <ArrowRight size={16} />
                )}
                {busy === "connect" ? "Connecting…" : "Connect"}
              </button>
            </>
          )}

          <div className="field-hint" style={{ textAlign: "center" }}>
            <AlertCircle size={12} style={{ verticalAlign: -1, marginRight: 4 }} />
            Tunnel Manager is a third-party client. It never sees your Cloudflare
            password.
          </div>
        </div>
      </section>
    </div>
  );
}

function HeroPoint({
  icon,
  title,
  body,
}: {
  icon: ReactNode;
  title: string;
  body: string;
}) {
  return (
    <div className="row-inline" style={{ alignItems: "flex-start", gap: 12 }}>
      <div
        className="row-icon"
        style={{ background: "var(--accent-soft)", color: "var(--accent-text)" }}
      >
        {icon}
      </div>
      <div>
        <div style={{ fontWeight: 650, fontSize: 13 }}>{title}</div>
        <div style={{ fontSize: 12, color: "var(--fg-secondary)", lineHeight: 1.5 }}>
          {body}
        </div>
      </div>
    </div>
  );
}
