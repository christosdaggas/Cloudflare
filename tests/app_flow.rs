//! The application's state flow, end to end.
//!
//! Everything here runs against three fakes and nothing else: a `wiremock`
//! server on `127.0.0.1` standing in for `api.cloudflare.com`, a
//! [`MockController`] standing in for SSH, and a SQLite file in a temporary
//! directory. No credential, no display and no network is needed, which is the
//! point — the whole first-run journey is a thing that can be regression
//! tested rather than a thing that has to be clicked through.
//!
//! The two tests that matter most are the pair at the top: the same journey
//! with the optional steps taken and with them skipped. Both optional steps —
//! a public hostname and a remote host — are genuinely optional, and the
//! second test proves it by asserting that no DNS endpoint and no remote
//! method was reached at all.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use tunnel_manager::cloudflare::error::CloudflareError;
use tunnel_manager::cloudflare::{Cancel, CloudflareClient, Timeouts};
use tunnel_manager::domain::account::PermissionState;
use tunnel_manager::domain::activity::{redact, ActivityCategory, ActivityEvent};
use tunnel_manager::domain::config::{OriginRequestUpdate, TunnelConfig};
use tunnel_manager::domain::host::{
    RemoteAuth, RemoteHost, RemoteHostDraft, RemoteServiceState, RemoteStatus,
};
use tunnel_manager::domain::hostname::{DnsState, OriginService};
use tunnel_manager::domain::tunnel::TunnelStatus;
use tunnel_manager::domain::{validation, Account};
use tunnel_manager::remote::commands::{RemoteCommand, ServiceAction, MAX_LOG_LINES};
use tunnel_manager::remote::mock::MockCall;
use tunnel_manager::remote::{MockController, MockScript, RemoteConnectorController, RemoteError};
use tunnel_manager::services::hostnames::{self, PublishRequest, Status, Step};
use tunnel_manager::services::hosts::{self, TrustDecision};
use tunnel_manager::services::session::{self, ConnectionState, Session};
use tunnel_manager::services::tunnels::{self, CreateRequest, HostnameRequest};
use tunnel_manager::storage::{activity, db, hosts as host_store, settings, Settings};

// ── Fixtures ─────────────────────────────────────────────────────────────

const ACCOUNT_ID: &str = "8f2c4b1d9e6a70335c1d2e4f6a8b0c1d";
const ZONE_ID: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f809";
const TUNNEL_ID: &str = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";
const RECORD_ID: &str = "372e67954025e0ba6aaa6d586b9e0b59";
const HOSTNAME: &str = "app.example.com";

/// A stand-in run token, shaped the way a real credential is: long, mixed
/// case, digits. The redactor only catches a bare secret that looks like this,
/// so a placeholder such as "secret" would make the leak tests pass for the
/// wrong reason.
const RUN_TOKEN: &str = "Zx7Kq2mNp4Rt8Vw1Ay5Bc9De3Fg6Hj0KlMnOpQr";

/// The API token every client in this file is built with.
const API_TOKEN_LEN: usize = 40;

fn api_token() -> SecretString {
    SecretString::from("t".repeat(API_TOKEN_LEN))
}

fn client(server: &MockServer) -> CloudflareClient {
    CloudflareClient::with_base_url(
        api_token(),
        format!("{}/client/v4", server.uri()),
        Timeouts::default(),
    )
    .expect("the client builds")
}

fn ok(result: Value) -> Value {
    json!({"success": true, "errors": [], "messages": [], "result": result})
}

/// A single-page list envelope, with the `result_info` block Cloudflare sends
/// on `/zones` and omits on `/accounts`.
fn ok_page(result: Value) -> Value {
    let count = result.as_array().map(Vec::len).unwrap_or(0);
    json!({
        "success": true,
        "errors": [],
        "messages": [],
        "result": result,
        "result_info": {
            "page": 1, "per_page": 50, "count": count,
            "total_count": count, "total_pages": 1
        }
    })
}

fn api_failure(code: i64, message: &str) -> Value {
    json!({
        "success": false,
        "errors": [{"code": code, "message": message}],
        "messages": [],
        "result": null
    })
}

/// Answers with each body in turn, repeating the last once the list runs out.
///
/// One endpoint has to answer differently before and after a write — the DNS
/// listing is empty when a publish checks it and populated when the same
/// publish verifies it — and doing that with a counter rather than with two
/// mounted mocks keeps the test independent of wiremock's matching order.
fn in_sequence(bodies: Vec<Value>) -> impl Respond {
    let calls = Arc::new(AtomicUsize::new(0));
    move |_request: &Request| {
        let index = calls
            .fetch_add(1, Ordering::SeqCst)
            .min(bodies.len().saturating_sub(1));
        ResponseTemplate::new(200).set_body_json(bodies[index].clone())
    }
}

fn tunnel_json(status: &str, connections: Value) -> Value {
    json!({
        "id": TUNNEL_ID,
        "account_tag": ACCOUNT_ID,
        "name": "web",
        "created_at": "2026-08-22T09:00:00Z",
        "deleted_at": null,
        "conns_active_at": "2026-08-22T09:01:00Z",
        "conns_inactive_at": null,
        "tun_type": "cfd_tunnel",
        "status": status,
        "remote_config": true,
        "config_src": "cloudflare",
        "connections": connections
    })
}

fn connection_json() -> Value {
    json!([{
        "id": "1f6a5b3c-0d2e-4f81-9a7b-6c5d4e3f2a1b",
        "client_id": "3c2b1a09-8f7e-6d5c-4b3a-2f1e0d9c8b7a",
        "colo_name": "fra01",
        "origin_ip": "203.0.113.7",
        "opened_at": "2026-08-22T09:01:00Z"
    }])
}

fn dns_record_json() -> Value {
    json!({
        "id": RECORD_ID,
        "zone_id": ZONE_ID,
        "name": HOSTNAME,
        "type": "CNAME",
        "content": format!("{TUNNEL_ID}.cfargotunnel.com"),
        "proxied": true,
        "ttl": 1,
        "comment": hostnames::RECORD_COMMENT
    })
}

fn dns_path() -> String {
    format!("/client/v4/zones/{ZONE_ID}/dns_records")
}

fn tunnel_path() -> String {
    format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")
}

// ── Mount helpers ────────────────────────────────────────────────────────

async fn mount_verify(server: &MockServer, status: &str) {
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({
            "id": "ed17574386854bf78a67040be0a770b0",
            "status": status
        }))))
        .mount(server)
        .await;
}

async fn mount_accounts(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(json!([
            {"id": ACCOUNT_ID, "name": "Acme Ltd"}
        ]))))
        .mount(server)
        .await;
}

async fn mount_zones(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/client/v4/zones"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(json!([
            {
                "id": ZONE_ID,
                "name": "example.com",
                "status": "active",
                "paused": false,
                "account": {"id": ACCOUNT_ID, "name": "Acme Ltd"},
                "permissions": ["#zone:read", "#dns_records:edit"]
            }
        ]))))
        .mount(server)
        .await;
}

/// The tunnel list: empty on the first call, then one healthy tunnel. That is
/// the shape of the real journey — an account with nothing in it, and the same
/// account after a tunnel has been created and a connector has registered.
async fn mount_tunnel_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(tunnel_path()))
        .respond_with(in_sequence(vec![
            ok_page(json!([])),
            ok_page(json!([tunnel_json("healthy", connection_json())])),
        ]))
        .mount(server)
        .await;
}

async fn mount_create_tunnel(server: &MockServer) {
    let mut created = tunnel_json("inactive", json!([]));
    created["config_src"] = Value::Null;
    created["conns_active_at"] = Value::Null;
    created["token"] = json!(RUN_TOKEN);

    Mock::given(method("POST"))
        .and(path(tunnel_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(created)))
        .mount(server)
        .await;
}

async fn mount_configuration(server: &MockServer) {
    let config_path = format!("{}/{TUNNEL_ID}/configurations", tunnel_path());

    Mock::given(method("GET"))
        .and(path(config_path.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({
            "tunnel_id": TUNNEL_ID,
            "version": 1,
            "config": {"ingress": []}
        }))))
        .mount(server)
        .await;

    Mock::given(method("PUT"))
        .and(path(config_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({
            "tunnel_id": TUNNEL_ID,
            "version": 2
        }))))
        .mount(server)
        .await;
}

async fn mount_dns(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(dns_path()))
        .respond_with(in_sequence(vec![
            ok_page(json!([])),
            ok_page(json!([dns_record_json()])),
        ]))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path(dns_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(dns_record_json())))
        .mount(server)
        .await;
}

async fn mount_run_token(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("{}/{TUNNEL_ID}/token", tunnel_path())))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!(RUN_TOKEN))))
        .mount(server)
        .await;
}

/// Every endpoint the happy path touches.
async fn mount_everything(server: &MockServer) {
    mount_verify(server, "active").await;
    mount_accounts(server).await;
    mount_zones(server).await;
    mount_tunnel_list(server).await;
    mount_create_tunnel(server).await;
    mount_configuration(server).await;
    mount_dns(server).await;
    mount_run_token(server).await;
}

// ── Storage helpers ──────────────────────────────────────────────────────

fn open_temp_db(directory: &tempfile::TempDir) -> rusqlite::Connection {
    db::open(&directory.path().join("tunnel-manager.sqlite3")).expect("the database opens")
}

fn draft() -> RemoteHostDraft {
    RemoteHostDraft {
        name: "edge-frankfurt".into(),
        address: "edge.example.test".into(),
        port: 22,
        username: "deploy".into(),
        auth: RemoteAuth::Agent,
        service_unit: "cloudflared".into(),
        tunnel_id: Some(TUNNEL_ID.into()),
        notes: None,
    }
}

/// Every TEXT value in every table, one per line.
///
/// The leak tests grep this rather than naming the columns they expect a
/// secret in: a column added tomorrow is covered without anyone remembering to
/// extend the test, which is the only version of this check worth having.
fn dump_text_columns(connection: &rusqlite::Connection) -> String {
    let tables: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    let mut dump = String::new();
    for table in tables {
        let mut statement = connection
            .prepare(&format!("SELECT * FROM \"{table}\""))
            .unwrap();
        let columns = statement.column_count();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for index in 0..columns {
                if let Ok(Some(text)) = row.get::<_, Option<String>>(index) {
                    dump.push_str(&text);
                    dump.push('\n');
                }
            }
        }
    }
    dump
}

fn assert_no_secret_in(dump: &str) {
    assert!(
        !dump.contains(RUN_TOKEN),
        "a run token reached the database"
    );
    assert!(
        !dump.contains(&"t".repeat(API_TOKEN_LEN)),
        "an API token reached the database"
    );
}

// ── The headline test ────────────────────────────────────────────────────

/// The whole first-run journey, with both optional steps taken.
///
/// Written as one test on purpose. Each stage's assertions are about the state
/// the *previous* stage left behind, and splitting them up would lose exactly
/// the property being checked: that the pieces compose.
#[tokio::test]
async fn the_whole_journey_from_a_token_to_a_deployed_connector() {
    let server = MockServer::start().await;
    mount_everything(&server).await;

    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);
    let cancel = Cancel::new();
    let client = client(&server);

    // ── 1. The token verifies and discovery finds the account ────────────
    let check = session::check_token(&client, &cancel).await.unwrap();
    assert!(check.status.is_active());
    assert!(
        check.status.caveat().is_none(),
        "a live token with no expiry"
    );
    assert!(!check.needs_manual_account(), "discovery found an account");
    assert_eq!(check.accounts.len(), 1);
    assert!(check.accounts_unavailable.is_none());

    // ── 2. Selecting the account opens a session ─────────────────────────
    let account = check.accounts[0].clone();
    assert_eq!(account.name, "Acme Ltd");
    let mut session = Session::new(client.clone(), account.clone());
    assert!(
        !session.can_publish_hostnames(),
        "no zone has been loaded yet, so nothing can be published"
    );

    // ── 3. Zones load, so a hostname can be offered ──────────────────────
    assert_eq!(
        session.load_zones(&cancel).await,
        None,
        "no failure message"
    );
    assert_eq!(session.zones.len(), 1);
    assert_eq!(session.capabilities.zones.0, PermissionState::Granted);
    assert_eq!(
        session.capabilities.dns.0,
        PermissionState::Granted,
        "the zone listing reported #dns_records:edit"
    );
    assert!(session.can_publish_hostnames());

    // ── 4. Choosing the zone, and remembering both choices ───────────────
    let zone = session
        .zone_for(HOSTNAME)
        .expect("example.com covers it")
        .clone();
    session.default_zone_id = Some(zone.id.clone());
    assert_eq!(session.default_zone().unwrap().name, "example.com");

    let stored = Settings {
        account_id: Some(account.id.clone()),
        account_name: Some(account.name.clone()),
        default_zone_id: Some(zone.id.clone()),
        default_zone_name: Some(zone.name.clone()),
        token_saved: true,
        ..Settings::default()
    };
    settings::save(&connection, &stored).unwrap();
    assert!(settings::load(&connection).unwrap().is_configured());

    // ── 5. The account starts out empty ──────────────────────────────────
    let before = tunnels::list(&client, &account.id, &cancel).await.unwrap();
    assert!(before.is_empty());
    assert_eq!(tunnels::total_hostnames(&before), Some(0));

    // ── 6. The wizard's review, then the create ──────────────────────────
    let request = CreateRequest {
        account_id: account.id.clone(),
        name: "web".into(),
        hostname: Some(HostnameRequest {
            hostname: HOSTNAME.into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            zone_id: Some(zone.id.clone()),
        }),
    };
    let review = tunnels::review(&request);
    assert!(review.iter().any(|line| line.contains("proxied CNAME")));

    let outcome = tunnels::create(&client, &request, &cancel).await;
    assert!(outcome.succeeded(), "steps: {:?}", outcome.steps);
    assert!(!outcome.is_partial());

    let tunnel = outcome.tunnel.clone().expect("the tunnel was created");
    assert_eq!(tunnel.id, TUNNEL_ID);
    assert!(tunnel.config_source.is_remotely_managed());
    assert_eq!(
        outcome.token.as_ref().map(|token| token.expose_secret()),
        Some(RUN_TOKEN),
        "the create response's token is carried in memory only"
    );

    // ── 7. The hostname is published in both systems ─────────────────────
    let published = outcome.hostname.clone().expect("a hostname was requested");
    assert!(published.succeeded(), "steps: {:?}", published.steps);
    assert!(matches!(published.dns, DnsState::Correct { .. }));
    assert!(published.recovery.is_none());
    assert!(published
        .steps
        .iter()
        .any(|step| step.step == Step::CreateDnsRecord && step.status == Status::Done));
    assert!(published
        .steps
        .iter()
        .any(|step| step.step == Step::Verify && step.status == Status::Done));

    let written = written_configuration(&server).await;
    assert_eq!(written.hostname_count(), 1);
    assert!(
        written.ingress.last().unwrap().is_catch_all(),
        "cloudflared refuses a list whose last rule has a hostname"
    );

    activity::record(
        &connection,
        &ActivityEvent::success(ActivityCategory::Tunnel, format!("Created {}", tunnel.name))
            .with_target(tunnel.name.clone()),
    )
    .unwrap();

    // ── 8. A remote host is registered, and is not yet usable ────────────
    let clean = hosts::validate_draft(&draft()).unwrap();
    assert_eq!(
        clean.service_unit, "cloudflared.service",
        "the suffix is added"
    );
    let host_id = host_store::insert(&connection, &clean).unwrap();
    let host = host_store::get(&connection, host_id).unwrap().unwrap();

    assert!(!host.is_trusted());
    assert!(matches!(
        hosts::preflight(&arc(&MockController::with_defaults()), &host, None).await,
        Err(RemoteError::NotConfigured)
    ));

    // ── 9. The host key is shown, then pinned ────────────────────────────
    let mock = MockController::with_defaults();
    let controller = arc(&mock);

    let key = hosts::probe_host_key(&controller, &host).await.unwrap();
    let decision = TrustDecision::evaluate(&host, key.clone());
    assert!(matches!(decision, TrustDecision::FirstContact { .. }));
    assert!(!decision.may_proceed(), "first contact stops and asks");

    host_store::pin_host_key(&connection, host_id, &key.fingerprint).unwrap();
    let host = host_store::get(&connection, host_id).unwrap().unwrap();
    assert!(host.is_trusted());
    assert!(TrustDecision::evaluate(&host, key.clone()).may_proceed());

    // ── 10. Preflight ────────────────────────────────────────────────────
    let preflight = hosts::preflight(&controller, &host, None).await;
    let status = hosts::status_from(&preflight);
    assert!(!status.is_unreachable());
    assert!(status.can_control());
    let preflight = preflight.unwrap();
    assert!(preflight.can_deploy());
    assert_eq!(preflight.blocking_reason(), None);

    // ── 11. Deploy ───────────────────────────────────────────────────────
    let plan = hosts::plan_deployment(&controller, &host, None, &tunnel.id, &tunnel.name)
        .await
        .unwrap();
    assert!(plan.touches_privileged_files());
    assert!(!plan.replaces_existing_unit);
    for step in &plan.steps {
        assert!(
            !step.preview.contains(RUN_TOKEN),
            "a preview must never contain the token: {}",
            step.preview
        );
    }

    let token = tunnels::fetch_run_token(&client, &account.id, &tunnel.id, &cancel)
        .await
        .unwrap();
    let deployed = hosts::apply_deployment(&controller, &host, None, &plan, &token, false)
        .await
        .unwrap();
    assert!(deployed.is_complete());
    assert_eq!(deployed.service_state, RemoteServiceState::Active);
    assert_eq!(
        mock.received_secrets(),
        vec![RUN_TOKEN.to_string()],
        "the token went to the transport and nowhere else"
    );
    assert_eq!(
        mock.calls(),
        vec![
            MockCall::ProbeHostKey,
            MockCall::Preflight,
            MockCall::PlanDeployment {
                tunnel_id: TUNNEL_ID.into()
            },
            MockCall::ApplyDeployment {
                tunnel_id: TUNNEL_ID.into()
            },
        ]
    );

    host_store::touch(&connection, host_id).unwrap();
    activity::record(
        &connection,
        &ActivityEvent::success(
            ActivityCategory::RemoteService,
            format!("Deployed a connector to {}", host.name),
        ),
    )
    .unwrap();

    // ── 12. Refreshing shows the connector at the edge ───────────────────
    let after = tunnels::list(&client, &account.id, &cancel).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].tunnel.status, TunnelStatus::Healthy);
    assert_eq!(after[0].tunnel.connector_count, 1);
    assert!(!tunnels::summarise(&after).needs_attention());

    let connected = ConnectionState::Connected {
        account: account.clone(),
        token: check.status.clone(),
        capabilities: session.capabilities,
    };
    assert!(connected.is_connected());
    assert_eq!(connected.account().unwrap().id, ACCOUNT_ID);

    // ── 13. Nothing that ran left a credential in the database ───────────
    let dump = dump_text_columns(&connection);
    assert!(
        dump.contains(ACCOUNT_ID) && dump.contains("edge-frankfurt"),
        "the leak check has to be reading the tables it greps, or it proves nothing"
    );
    assert_no_secret_in(&dump);
    assert_eq!(host_store::count(&connection).unwrap(), 1);
    assert_eq!(activity::count(&connection).unwrap(), 2);
}

/// The same journey with both optional steps skipped.
///
/// This is the promise the README makes — that neither a public hostname nor a
/// registered host is required — and the only way to hold the application to
/// it is to assert on what was *not* called.
#[tokio::test]
async fn the_same_journey_with_no_hostname_and_no_ssh_touches_neither_subsystem() {
    let server = MockServer::start().await;
    mount_everything(&server).await;

    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);
    let cancel = Cancel::new();
    let client = client(&server);
    let mock = MockController::with_defaults();

    let check = session::check_token(&client, &cancel).await.unwrap();
    let account = check.accounts[0].clone();
    let mut session = Session::new(client.clone(), account.clone());
    session.load_zones(&cancel).await;

    let request = CreateRequest {
        account_id: account.id.clone(),
        name: "web".into(),
        hostname: None,
    };
    let review = tunnels::review(&request);
    assert_eq!(review.len(), 2);
    assert!(review[1].contains("No public hostname"));

    assert!(tunnels::list(&client, &account.id, &cancel)
        .await
        .unwrap()
        .is_empty());

    let outcome = tunnels::create(&client, &request, &cancel).await;
    assert!(outcome.succeeded());
    assert!(outcome.hostname.is_none());
    assert!(outcome
        .steps
        .iter()
        .any(|step| step.step == Step::CreateDnsRecord && step.status == Status::Skipped));
    assert!(
        outcome
            .manual_instructions()
            .unwrap()
            .contains("--token-file"),
        "a skipped deployment still tells the user how to finish by hand"
    );

    settings::save(
        &connection,
        &Settings {
            account_id: Some(account.id.clone()),
            account_name: Some(account.name.clone()),
            ..Settings::default()
        },
    )
    .unwrap();

    // The end state: connected, usable, one tunnel, no hosts.
    let rows = tunnels::list(&client, &account.id, &cancel).await.unwrap();
    let connected = ConnectionState::Connected {
        account,
        token: check.status,
        capabilities: session.capabilities,
    };
    assert!(connected.is_connected());
    assert!(settings::load(&connection).unwrap().is_configured());
    assert_eq!(rows.len(), 1);
    assert_eq!(host_store::count(&connection).unwrap(), 0);

    // Nothing reached DNS…
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests
            .iter()
            .any(|request| request.url.path().contains("dns_records")),
        "a skipped hostname must not touch the zone: {:?}",
        requests.iter().map(|r| r.url.path()).collect::<Vec<_>>()
    );
    // …and nothing reached a host.
    assert_eq!(mock.call_count(), 0);
    assert!(mock.calls().is_empty());
}

// ── Connecting: the awkward tokens ───────────────────────────────────────

/// An account-scoped token authenticates and is refused by `/accounts`. That
/// is a supported way to run the application, not a failure.
#[tokio::test]
async fn a_token_that_cannot_list_accounts_asks_for_one_by_hand_and_then_works() {
    let server = MockServer::start().await;
    mount_verify(&server, "active").await;
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .respond_with(ResponseTemplate::new(403).set_body_json(api_failure(
            9109,
            "Unauthorized to access requested resource",
        )))
        .mount(&server)
        .await;
    mount_tunnel_list(&server).await;

    let client = client(&server);
    let cancel = Cancel::new();

    let check = session::check_token(&client, &cancel).await.unwrap();
    assert!(check.status.is_active(), "the token itself is fine");
    assert!(check.needs_manual_account());
    assert!(check.accounts.is_empty());
    let unavailable = check
        .accounts_unavailable
        .as_deref()
        .expect("the reason is kept so the onboarding page can explain itself");
    assert!(unavailable.contains("list accounts"), "got {unavailable}");

    // The user types the ID from the dashboard sidebar and carries on.
    let typed = validation::account_id("  8F2C4B1D9E6A70335C1D2E4F6A8B0C1D  ").unwrap();
    assert_eq!(typed, ACCOUNT_ID, "trimmed and lowercased");

    let account = Account {
        id: typed,
        name: "Acme Ltd".into(),
    };
    let rows = tunnels::list(&client, &account.id, &cancel).await.unwrap();
    assert!(rows.is_empty(), "the first listing on a fresh account");
}

#[tokio::test]
async fn a_disabled_token_is_rejected_before_anything_else_is_tried() {
    let server = MockServer::start().await;
    mount_verify(&server, "disabled").await;
    mount_accounts(&server).await;

    let error = session::check_token(&client(&server), &Cancel::new())
        .await
        .expect_err("a disabled token cannot be used");

    assert!(matches!(error, CloudflareError::Unauthorized { .. }));
    assert!(error.is_permission_problem());
    assert!(error.user_message().contains("Connection page"));

    assert!(
        !server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path().ends_with("/accounts")),
        "discovery must not run against a token that is already known to be dead"
    );
}

/// A token scoped to tunnels alone is legitimate: hostname publishing is
/// withdrawn, and everything else keeps working.
#[tokio::test]
async fn a_token_without_zone_read_loses_hostnames_and_keeps_tunnels() {
    let server = MockServer::start().await;
    mount_verify(&server, "active").await;
    mount_accounts(&server).await;
    Mock::given(method("GET"))
        .and(path("/client/v4/zones"))
        .respond_with(ResponseTemplate::new(403).set_body_json(api_failure(9109, "Unauthorized")))
        .mount(&server)
        .await;
    mount_tunnel_list(&server).await;

    let client = client(&server);
    let cancel = Cancel::new();
    let check = session::check_token(&client, &cancel).await.unwrap();
    let mut session = Session::new(client.clone(), check.accounts[0].clone());

    let message = session
        .load_zones(&cancel)
        .await
        .expect("a reason is returned");
    assert!(message.contains("Zone → Zone → Read"), "got {message}");
    assert_eq!(session.capabilities.zones.0, PermissionState::Denied);
    assert_eq!(session.capabilities.dns.0, PermissionState::Denied);
    assert!(!session.can_publish_hostnames());
    assert!(session.capabilities.has_known_gap());
    assert!(session.zone_for(HOSTNAME).is_none());

    // Tunnels still work, which is the whole point.
    assert!(tunnels::list(&client, ACCOUNT_ID, &cancel).await.is_ok());
    session.note_tunnel_access(true);
    assert_eq!(session.capabilities.tunnels.0, PermissionState::Granted);
}

/// The token endpoint needs a write permission, so a read-only token that can
/// list a tunnel is refused here. Saying only "forbidden" would send the user
/// looking for a permission they already have.
#[tokio::test]
async fn a_refused_run_token_says_the_endpoint_needs_edit_rather_than_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{}/{TUNNEL_ID}/token", tunnel_path())))
        .respond_with(ResponseTemplate::new(403).set_body_json(api_failure(
            9109,
            "Unauthorized to access requested resource",
        )))
        .mount(&server)
        .await;

    let error = tunnels::fetch_run_token(&client(&server), ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .expect_err("a read-only token is refused here");

    let message = error.user_message();
    assert!(message.contains("Edit"), "got {message}");
    assert!(message.contains("not Read"), "got {message}");
    assert!(error.is_permission_problem());
}

// ── The review step ──────────────────────────────────────────────────────

#[test]
fn the_review_of_a_tunnel_with_a_hostname_lists_every_change_it_will_make() {
    let changes = tunnels::review(&CreateRequest {
        account_id: ACCOUNT_ID.into(),
        name: "web".into(),
        hostname: Some(HostnameRequest {
            hostname: HOSTNAME.into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            zone_id: Some(ZONE_ID.into()),
        }),
    });

    assert_eq!(changes.len(), 4);
    assert!(changes[0].contains("remotely managed"));
    assert!(changes[1].contains("http://localhost:8080"));
    assert!(changes[2].contains("catch-all"));
    assert!(changes[3].contains("proxied CNAME"));
}

#[test]
fn the_review_of_a_tunnel_without_a_hostname_promises_no_dns_change() {
    let changes = tunnels::review(&CreateRequest {
        account_id: ACCOUNT_ID.into(),
        name: "web".into(),
        hostname: None,
    });

    assert_eq!(changes.len(), 2);
    assert!(changes[1].contains("No public hostname"));
    assert!(
        !changes.iter().any(|change| change.contains("CNAME")),
        "a review that mentions DNS when none will be touched is a lie"
    );
}

// ── SSH-disabled mode ────────────────────────────────────────────────────

/// With no host registered, every Cloudflare feature still works and every
/// remote entry point refuses rather than half-working.
#[tokio::test]
async fn with_no_host_registered_every_cloudflare_feature_still_works() {
    let server = MockServer::start().await;
    mount_everything(&server).await;

    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);
    assert_eq!(host_store::count(&connection).unwrap(), 0);
    assert!(host_store::list(&connection).unwrap().is_empty());
    assert!(host_store::for_tunnel(&connection, TUNNEL_ID)
        .unwrap()
        .is_empty());

    let client = client(&server);
    let cancel = Cancel::new();

    let check = session::check_token(&client, &cancel).await.unwrap();
    let mut session = Session::new(client.clone(), check.accounts[0].clone());
    session.load_zones(&cancel).await;

    assert!(tunnels::list(&client, ACCOUNT_ID, &cancel).await.is_ok());
    let outcome = tunnels::create(
        &client,
        &CreateRequest {
            account_id: ACCOUNT_ID.into(),
            name: "web".into(),
            hostname: None,
        },
        &cancel,
    )
    .await;
    assert!(outcome.succeeded());

    let published = hostnames::publish(
        &client,
        &PublishRequest {
            account_id: ACCOUNT_ID.into(),
            tunnel_id: TUNNEL_ID.into(),
            tunnel_name: "web".into(),
            hostname: HOSTNAME.into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            zone_id: Some(ZONE_ID.into()),
            origin_request: OriginRequestUpdate::Preserve,
        },
        &cancel,
    )
    .await;
    assert!(published.succeeded(), "steps: {:?}", published.steps);

    let loaded = hostnames::load_hostnames(
        &client,
        ACCOUNT_ID,
        TUNNEL_ID,
        "web",
        &session.zones,
        &cancel,
    )
    .await
    .unwrap();
    assert!(loaded.is_empty(), "the mocked configuration is still empty");

    assert!(
        tunnels::fetch_run_token(&client, ACCOUNT_ID, TUNNEL_ID, &cancel)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn every_remote_entry_point_refuses_an_untrusted_host() {
    let controller = arc(&MockController::with_defaults());
    let host = untrusted_host();

    assert!(matches!(
        hosts::preflight(&controller, &host, None).await,
        Err(RemoteError::NotConfigured)
    ));
    assert!(matches!(
        hosts::service_action(&controller, &host, None, ServiceAction::Restart).await,
        Err(RemoteError::NotConfigured)
    ));
    assert!(matches!(
        hosts::logs(&controller, &host, None, 20).await,
        Err(RemoteError::NotConfigured)
    ));
    assert!(matches!(
        hosts::plan_deployment(&controller, &host, None, TUNNEL_ID, "web").await,
        Err(RemoteError::NotConfigured)
    ));

    let status = hosts::status_from(&hosts::preflight(&controller, &host, None).await);
    assert!(matches!(status, RemoteStatus::Unreachable { .. }));
    assert!(!status.can_control());
}

// ── Host-key verification ────────────────────────────────────────────────

#[tokio::test]
async fn a_first_contact_host_key_is_a_decision_the_user_has_to_make() {
    let mock = MockController::with_defaults();
    let controller = arc(&mock);
    let host = untrusted_host();

    let key = hosts::probe_host_key(&controller, &host).await.unwrap();
    assert!(key.fingerprint.starts_with("SHA256:"));
    assert_eq!(key.algorithm, "ssh-ed25519");
    assert_eq!(mock.calls(), vec![MockCall::ProbeHostKey]);

    let decision = TrustDecision::evaluate(&host, key);
    assert!(matches!(decision, TrustDecision::FirstContact { .. }));
    assert!(
        !decision.may_proceed(),
        "nothing may run before a human has seen the fingerprint"
    );
}

#[tokio::test]
async fn a_matching_host_key_is_the_only_decision_that_permits_proceeding() {
    let controller = arc(&MockController::with_defaults());
    let host = trusted_host(mock_fingerprint());

    let key = hosts::probe_host_key(&controller, &host).await.unwrap();
    let decision = TrustDecision::evaluate(&host, key);

    assert_eq!(decision, TrustDecision::Unchanged);
    assert!(decision.may_proceed());
}

/// A key that changed is a refusal, not a prompt: there is deliberately no
/// "trust anyway" path for the UI to offer.
#[tokio::test]
async fn a_changed_host_key_refuses_and_never_permits_proceeding() {
    let controller = arc(&MockController::with_defaults());
    let host = trusted_host("SHA256:aDifferentKeyThanTheOneThisHostNowPresents".into());

    let key = hosts::probe_host_key(&controller, &host).await.unwrap();
    let decision = TrustDecision::evaluate(&host, key);

    match &decision {
        TrustDecision::Changed { expected, .. } => assert!(expected.starts_with("SHA256:")),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(!decision.may_proceed());

    let event = decision.activity(&host);
    assert_eq!(
        event.severity,
        tunnel_manager::domain::ActivitySeverity::Error
    );
    assert!(event.summary.contains("Refused to connect"));
}

// ── Remote service control ───────────────────────────────────────────────

/// Asserted as a sequence rather than as three separate calls, because the
/// thing worth checking is that each action observes the state the last one
/// left rather than a state it assumed.
#[tokio::test]
async fn stopping_starting_and_restarting_walk_the_service_state() {
    let mock = MockController::with_defaults();
    let controller = arc(&mock);
    let host = trusted_host(mock_fingerprint());

    assert_eq!(mock.service_state(), RemoteServiceState::Active);

    let stopped = hosts::service_action(&controller, &host, None, ServiceAction::Stop)
        .await
        .unwrap();
    assert!(stopped.succeeded());
    assert_eq!(stopped.state_before, RemoteServiceState::Active);
    assert_eq!(stopped.state_after, RemoteServiceState::Inactive);
    assert_eq!(mock.service_state(), RemoteServiceState::Inactive);

    let started = hosts::service_action(&controller, &host, None, ServiceAction::Start)
        .await
        .unwrap();
    assert!(started.succeeded());
    assert_eq!(started.state_before, RemoteServiceState::Inactive);
    assert_eq!(started.state_after, RemoteServiceState::Active);

    let restarted = hosts::service_action(&controller, &host, None, ServiceAction::Restart)
        .await
        .unwrap();
    assert!(restarted.succeeded());
    assert_eq!(restarted.state_after, RemoteServiceState::Active);

    assert_eq!(
        mock.calls(),
        vec![
            MockCall::Service(ServiceAction::Stop),
            MockCall::Service(ServiceAction::Start),
            MockCall::Service(ServiceAction::Restart),
        ]
    );
    assert_eq!(mock.service_state(), RemoteServiceState::Active);
}

#[tokio::test]
async fn a_service_action_that_fails_on_the_host_leaves_the_state_alone() {
    let mock = MockController::new(MockScript {
        fail_with: Some(RemoteError::CommandFailed {
            description: "control the service",
            exit_status: Some(1),
            stderr: "Job for cloudflared.service failed".into(),
        }),
        ..MockScript::default()
    });
    let controller = arc(&mock);
    let host = trusted_host(mock_fingerprint());

    let error = hosts::service_action(&controller, &host, None, ServiceAction::Stop)
        .await
        .expect_err("the host refused");

    let message = error.user_message();
    assert!(message.contains("control the service"), "got {message}");
    assert!(message.contains("failed"), "got {message}");
    assert_eq!(
        mock.service_state(),
        RemoteServiceState::Active,
        "a failed stop must not be reported as a stop"
    );
    assert_eq!(mock.calls(), vec![MockCall::Service(ServiceAction::Stop)]);
}

// ── The remote allowlist ─────────────────────────────────────────────────

/// Duplicated from the unit tests on purpose. This asserts the property at the
/// boundary an external consumer sees: there is no other constructor, so a
/// hostile unit name cannot reach a remote shell by any route.
#[test]
fn a_unit_name_with_a_shell_metacharacter_cannot_be_rendered() {
    for hostile in [
        "cloudflared.service; rm -rf /",
        "cloudflared.service && curl evil.test",
        "$(id).service",
        "`id`.service",
        "cloudflared service",
        "cloudflared|tee",
        "cloudflared\nreboot",
        "../../etc/passwd",
        "-x.service",
        "unit'quote.service",
    ] {
        let unit = hostile.to_string();
        assert!(
            RemoteCommand::Service {
                unit: unit.clone(),
                action: ServiceAction::Restart
            }
            .render()
            .is_err(),
            "rendered a hostile unit: {hostile}"
        );
        assert!(RemoteCommand::Logs {
            unit: unit.clone(),
            lines: 10
        }
        .render()
        .is_err());
        assert!(RemoteCommand::EnableNow { unit: unit.clone() }
            .render()
            .is_err());
        assert!(RemoteCommand::ServiceState { unit }.render().is_err());
    }
}

#[test]
fn every_privileged_command_is_non_interactive_sudo_and_no_other_command_uses_sudo() {
    let unit = "cloudflared.service".to_string();
    let commands = vec![
        RemoteCommand::Kernel,
        RemoteCommand::SystemdVersion,
        RemoteCommand::CloudflaredPath,
        RemoteCommand::CloudflaredVersion,
        RemoteCommand::ServiceState { unit: unit.clone() },
        RemoteCommand::SudoCheck { unit: unit.clone() },
        RemoteCommand::Service {
            unit: unit.clone(),
            action: ServiceAction::Restart,
        },
        RemoteCommand::Logs {
            unit: unit.clone(),
            lines: 50,
        },
        RemoteCommand::UnitExecStart { unit: unit.clone() },
        RemoteCommand::PathExists {
            path: "/etc/tunnel-manager".into(),
        },
        RemoteCommand::DaemonReload,
        RemoteCommand::EnableNow { unit: unit.clone() },
        RemoteCommand::MakeTokenDir {
            dir: "/etc/tunnel-manager".into(),
        },
        RemoteCommand::InstallFromStdin {
            path: format!("/etc/tunnel-manager/{TUNNEL_ID}.token"),
            mode: "0600",
        },
    ];

    let mut privileged = 0;
    for command in &commands {
        let rendered = command.render().expect("a safe value renders");
        if command.is_privileged() {
            privileged += 1;
            assert!(
                rendered.contains("sudo -n"),
                "a privileged command that could prompt: {rendered}"
            );
        } else {
            assert!(
                !rendered.contains("sudo"),
                "an unprivileged command reached for sudo: {rendered}"
            );
        }
    }
    assert!(
        privileged >= 7,
        "only {privileged} privileged commands found"
    );
}

#[test]
fn a_log_request_is_clamped_to_the_maximum_this_application_will_ever_pull() {
    let rendered = RemoteCommand::Logs {
        unit: "cloudflared.service".into(),
        lines: u16::MAX,
    }
    .render()
    .unwrap();

    assert!(
        rendered.contains(&format!("-n {MAX_LOG_LINES}")),
        "got {rendered}"
    );
}

// ── Storage ──────────────────────────────────────────────────────────────

#[test]
fn settings_survive_a_round_trip_through_a_file_on_disk() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tunnel-manager.sqlite3");

    let written = {
        let connection = db::open(&path).unwrap();
        assert_eq!(settings::load(&connection).unwrap(), Settings::default());

        let written = Settings {
            account_id: Some(ACCOUNT_ID.into()),
            account_name: Some("Acme Ltd".into()),
            default_zone_id: Some(ZONE_ID.into()),
            default_zone_name: Some("example.com".into()),
            theme: "dark".into(),
            refresh_interval_secs: 300,
            log_level: "debug".into(),
            default_ssh_username: "deploy".into(),
            default_ssh_port: 2222,
            default_service_unit: "cloudflared.service".into(),
            local_service_unit: "cloudflared.service".into(),
            local_tunnel_id: None,
            remote_prompt_dismissed: true,
            token_saved: true,
        };
        settings::save(&connection, &written).unwrap();
        written
    };

    let connection = db::open(&path).unwrap();
    let reloaded = settings::load(&connection).unwrap();
    assert_eq!(reloaded, written);
    assert!(reloaded.is_configured());
    assert_eq!(
        reloaded.refresh_interval(),
        Some(std::time::Duration::from_secs(300))
    );

    settings::clear_connection(&connection).unwrap();
    let disconnected = settings::load(&connection).unwrap();
    assert!(!disconnected.is_configured());
    assert!(!disconnected.token_saved);
    assert_eq!(
        disconnected.theme, "dark",
        "disconnecting clears credentials, not preferences"
    );
}

#[test]
fn a_host_is_saved_reloaded_and_pinned_without_storing_a_credential() {
    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);

    let clean = hosts::validate_draft(&RemoteHostDraft {
        auth: RemoteAuth::KeyFile {
            path: "/home/deploy/.ssh/id_ed25519".into(),
            has_passphrase: true,
        },
        ..draft()
    })
    .unwrap();

    let id = host_store::insert(&connection, &clean).unwrap();
    let host = host_store::get(&connection, id).unwrap().unwrap();

    assert_eq!(host.name, "edge-frankfurt");
    assert_eq!(host.address, "edge.example.test");
    assert_eq!(host.username, "deploy");
    assert_eq!(host.service_unit, "cloudflared.service");
    assert_eq!(host.tunnel_id.as_deref(), Some(TUNNEL_ID));
    assert_eq!(host.display_target(), "deploy@edge.example.test");
    assert!(!host.is_trusted(), "insert never pins a key");
    assert_eq!(
        host.auth,
        RemoteAuth::KeyFile {
            path: "/home/deploy/.ssh/id_ed25519".into(),
            has_passphrase: true
        },
        "the key is referenced by path; the passphrase is only a flag"
    );

    host_store::pin_host_key(
        &connection,
        id,
        "SHA256:2WkKr2CzGCLPXKMWQTaEHwYCC6vLBpTOTOZ4dQ0Fx0Y",
    )
    .unwrap();
    assert!(host_store::get(&connection, id)
        .unwrap()
        .unwrap()
        .is_trusted());

    assert_eq!(
        host_store::for_tunnel(&connection, TUNNEL_ID)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        host_store::detach_tunnel(&connection, TUNNEL_ID).unwrap(),
        1
    );
    assert!(host_store::for_tunnel(&connection, TUNNEL_ID)
        .unwrap()
        .is_empty());
    assert_eq!(
        host_store::count(&connection).unwrap(),
        1,
        "a deleted tunnel detaches its hosts; it does not delete the machines"
    );

    host_store::delete(&connection, id).unwrap();
    assert_eq!(host_store::count(&connection).unwrap(), 0);
}

#[test]
fn activity_events_are_written_and_read_back_newest_first() {
    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);

    activity::record(
        &connection,
        &ActivityEvent::info(ActivityCategory::App, "Started"),
    )
    .unwrap();
    activity::record(
        &connection,
        &ActivityEvent::success(ActivityCategory::Tunnel, "Created web")
            .with_target("web")
            .with_detail("two rules written"),
    )
    .unwrap();
    activity::record(
        &connection,
        &ActivityEvent::error(ActivityCategory::Dns, "Could not create the CNAME"),
    )
    .unwrap();

    assert_eq!(activity::count(&connection).unwrap(), 3);

    let recent = activity::recent(&connection, 10).unwrap();
    assert_eq!(recent.len(), 3);
    assert_eq!(recent[0].summary, "Could not create the CNAME");
    assert_eq!(
        recent[0].severity,
        tunnel_manager::domain::ActivitySeverity::Error
    );
    assert_eq!(recent[1].target.as_deref(), Some("web"));
    assert_eq!(recent[1].detail.as_deref(), Some("two rules written"));
    assert_eq!(recent[2].summary, "Started");

    assert_eq!(activity::recent(&connection, 1).unwrap().len(), 1);
    activity::clear(&connection).unwrap();
    assert_eq!(activity::count(&connection).unwrap(), 0);
}

/// Opening the same file twice must be a no-op the second time. The first
/// version of a migration is the one nobody ever re-runs by hand, so it is the
/// one worth proving is idempotent.
#[test]
fn opening_the_same_database_twice_migrates_once_and_keeps_the_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tunnel-manager.sqlite3");

    let schema_before = {
        let connection = db::open(&path).unwrap();
        settings::save(
            &connection,
            &Settings {
                account_id: Some(ACCOUNT_ID.into()),
                ..Settings::default()
            },
        )
        .unwrap();
        host_store::insert(&connection, &hosts::validate_draft(&draft()).unwrap()).unwrap();
        schema_fingerprint(&connection)
    };

    let connection = db::open(&path).unwrap();
    assert_eq!(schema_fingerprint(&connection), schema_before);
    assert_eq!(
        version_of(&connection),
        db::SCHEMA_VERSION,
        "user_version is the only record of what has been applied"
    );
    assert_eq!(
        settings::load(&connection).unwrap().account_id.as_deref(),
        Some(ACCOUNT_ID)
    );
    assert_eq!(host_store::count(&connection).unwrap(), 1);
}

// ── Redaction, end to end ────────────────────────────────────────────────

/// A failure whose text carries a credential must lose it before it reaches
/// the user, and again before it reaches the database.
#[tokio::test]
async fn a_token_shaped_string_in_an_error_never_reaches_the_activity_log() {
    let server = MockServer::start().await;
    let config_path = format!("{}/{TUNNEL_ID}/configurations", tunnel_path());

    Mock::given(method("GET"))
        .and(path(config_path.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({
            "tunnel_id": TUNNEL_ID, "version": 1, "config": {"ingress": []}
        }))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(dns_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(json!([]))))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(config_path))
        .respond_with(ResponseTemplate::new(400).set_body_json(api_failure(
            1004,
            &format!("the upstream rejected {RUN_TOKEN} while writing"),
        )))
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &PublishRequest {
            account_id: ACCOUNT_ID.into(),
            tunnel_id: TUNNEL_ID.into(),
            tunnel_name: "web".into(),
            hostname: HOSTNAME.into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            zone_id: Some(ZONE_ID.into()),
            origin_request: OriginRequestUpdate::Preserve,
        },
        &Cancel::new(),
    )
    .await;

    let failed = outcome.failed_step().expect("the write failed");
    assert_eq!(failed.step, Step::WriteConfiguration);
    assert!(
        !failed.detail.contains(RUN_TOKEN),
        "the token survived into a user-facing message: {}",
        failed.detail
    );
    assert!(
        failed.detail.contains("[redacted]"),
        "got {}",
        failed.detail
    );
    assert!(!outcome.succeeded());

    // The redactor is the thing being relied on, so assert on it directly too.
    let raw = format!("the upstream rejected {RUN_TOKEN} while writing");
    assert!(!redact(&raw).contains(RUN_TOKEN));
    assert!(redact(&raw).contains("[redacted]"));

    let directory = tempfile::tempdir().unwrap();
    let connection = open_temp_db(&directory);
    activity::record(
        &connection,
        &ActivityEvent::error(ActivityCategory::TunnelConfig, outcome.summary())
            .with_detail(raw.clone())
            .with_target(HOSTNAME),
    )
    .unwrap();

    let stored = &activity::recent(&connection, 1).unwrap()[0];
    assert!(!stored.summary.contains(RUN_TOKEN));
    assert!(!stored.detail.as_deref().unwrap().contains(RUN_TOKEN));

    let dump = dump_text_columns(&connection);
    assert!(
        dump.contains("[redacted]"),
        "the stored row must be in the dump the leak check greps"
    );
    assert_no_secret_in(&dump);
}

// ── Small shared helpers ─────────────────────────────────────────────────

fn arc(controller: &MockController) -> Arc<dyn RemoteConnectorController> {
    // `MockController` shares its state through `Arc` internally, so the clone
    // handed to the service layer is the same recorder the test asserts on.
    Arc::new(controller.clone())
}

/// The fingerprint [`MockController`] presents by default.
fn mock_fingerprint() -> String {
    MockScript::default().host_key.fingerprint
}

fn untrusted_host() -> RemoteHost {
    RemoteHost {
        id: 1,
        name: "edge-frankfurt".into(),
        address: "edge.example.test".into(),
        port: 22,
        username: "deploy".into(),
        auth: RemoteAuth::Agent,
        host_key_fingerprint: None,
        service_unit: "cloudflared.service".into(),
        tunnel_id: Some(TUNNEL_ID.into()),
        created_at: chrono::Utc::now(),
        last_seen_at: None,
        notes: None,
    }
}

fn trusted_host(fingerprint: String) -> RemoteHost {
    RemoteHost {
        host_key_fingerprint: Some(fingerprint),
        ..untrusted_host()
    }
}

/// The configuration document the client actually PUT, parsed back out of the
/// request wiremock recorded.
async fn written_configuration(server: &MockServer) -> TunnelConfig {
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method == wiremock::http::Method::PUT)
        .expect("a configuration was written");

    let body: Value = serde_json::from_slice(&request.body).unwrap();
    serde_json::from_value(body["config"].clone()).unwrap()
}

fn version_of(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

/// Every object in the schema, as one string, so two opens can be compared.
fn schema_fingerprint(connection: &rusqlite::Connection) -> String {
    connection
        .prepare("SELECT type, name, COALESCE(sql, '') FROM sqlite_master ORDER BY type, name")
        .unwrap()
        .query_map([], |row| {
            Ok(format!(
                "{}|{}|{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .join("\n")
}
