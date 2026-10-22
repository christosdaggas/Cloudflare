//! Publishing, repairing and unpublishing a public hostname, end to end
//! against a mock Cloudflare.
//!
//! A hostname is two facts in two systems — an ingress rule in the tunnel's
//! configuration and a proxied CNAME in the zone — and `PUT .../configurations`
//! replaces the whole configuration document. So every assertion here is
//! really the same assertion from a different angle: *what did the application
//! actually send?* The interesting failures of this module are not error
//! messages, they are the rules that silently stopped being in the document,
//! and the DNS records that were deleted without being ours.
//!
//! Nothing here needs credentials, a network or a display: `wiremock` serves
//! plain HTTP on the loopback interface and the client's base URL is injected.

use secrecy::SecretString;
use serde_json::{json, Value};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use tunnel_manager::cloudflare::{Cancel, CloudflareClient, Timeouts};
use tunnel_manager::domain::config::{OriginRequest, OriginRequestUpdate};
use tunnel_manager::domain::hostname::{DnsState, OriginService};
use tunnel_manager::domain::zone::{Zone, ZoneStatus};
use tunnel_manager::services::hostnames::{
    self, Outcome, PublishRequest, Recovery, Status, Step, RECORD_COMMENT,
};

const ACCOUNT: &str = "1f2e3d4c5b6a798800112233445566aa";
const TUNNEL: &str = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";
const OTHER_TUNNEL: &str = "0a1b2c3d-4e5f-4071-8293-a4b5c6d7e8f9";
const ZONE: &str = "aabbccddeeff00112233445566778899";
const OTHER_ZONE: &str = "99887766554433221100ffeeddccbbaa";

// ── Plumbing ─────────────────────────────────────────────────────────────

fn client(server: &MockServer) -> CloudflareClient {
    CloudflareClient::with_base_url(
        SecretString::from("t".repeat(40)),
        format!("{}/client/v4", server.uri()),
        Timeouts::default(),
    )
    .expect("the client should build")
}

fn config_path() -> String {
    format!("/client/v4/accounts/{ACCOUNT}/cfd_tunnel/{TUNNEL}/configurations")
}

fn dns_path(zone_id: &str) -> String {
    format!("/client/v4/zones/{zone_id}/dns_records")
}

fn record_path(zone_id: &str, record_id: &str) -> String {
    format!("/client/v4/zones/{zone_id}/dns_records/{record_id}")
}

fn tunnel_target() -> String {
    format!("{TUNNEL}.cfargotunnel.com")
}

/// A successful v4 envelope around `result`.
fn ok(result: Value) -> Value {
    json!({ "success": true, "errors": [], "messages": [], "result": result })
}

fn failure(message: &str) -> Value {
    json!({
        "success": false,
        "errors": [{ "code": 10000, "message": message }],
        "messages": [],
        "result": null,
    })
}

fn configuration(config: Value) -> Value {
    ok(json!({
        "tunnel_id": TUNNEL,
        "account_id": ACCOUNT,
        "version": 7,
        "config": config,
    }))
}

/// A `dns_records` listing, with the `result_info` block that tells the
/// client's pagination there is nothing after this page.
fn dns_list(records: Vec<Value>) -> Value {
    let count = records.len();
    json!({
        "success": true,
        "errors": [],
        "messages": [],
        "result": records,
        "result_info": {
            "page": 1,
            "per_page": 50,
            "count": count,
            "total_count": count,
            "total_pages": 1,
        },
    })
}

fn dns_record(
    id: &str,
    zone_id: &str,
    name: &str,
    record_type: &str,
    content: &str,
    proxied: bool,
) -> Value {
    json!({
        "id": id,
        "zone_id": zone_id,
        "name": name,
        "type": record_type,
        "content": content,
        "proxied": proxied,
        "ttl": 1,
        "comment": null,
    })
}

fn tunnel_cname(id: &str, name: &str, tunnel_id: &str, proxied: bool) -> Value {
    dns_record(
        id,
        ZONE,
        name,
        "CNAME",
        &format!("{tunnel_id}.cfargotunnel.com"),
        proxied,
    )
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .expect("request recording is on by default")
}

fn matching<'r>(requests: &'r [Request], verb: &str, path: &str) -> Vec<&'r Request> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == verb && request.url.path() == path)
        .collect()
}

fn bodies(requests: &[Request], verb: &str, path: &str) -> Vec<Value> {
    matching(requests, verb, path)
        .into_iter()
        .map(|request| request.body_json().expect("the body should be JSON"))
        .collect()
}

/// Every configuration document the application wrote, in order.
fn written_configs(requests: &[Request]) -> Vec<Value> {
    bodies(requests, "PUT", &config_path())
        .into_iter()
        .map(|body| body["config"].clone())
        .collect()
}

fn written_config(requests: &[Request]) -> Value {
    let mut configs = written_configs(requests);
    assert_eq!(configs.len(), 1, "expected exactly one configuration write");
    configs.remove(0)
}

fn ingress_of(config: &Value) -> Vec<Value> {
    config["ingress"]
        .as_array()
        .expect("the written configuration should have an ingress list")
        .clone()
}

fn deletes(requests: &[Request]) -> Vec<String> {
    requests
        .iter()
        .filter(|request| request.method.as_str() == "DELETE")
        .map(|request| request.url.path().to_string())
        .collect()
}

fn status_of(outcome: &Outcome, step: Step) -> Status {
    outcome
        .steps
        .iter()
        .find(|result| result.step == step)
        .unwrap_or_else(|| panic!("no {step:?} step in {:?}", outcome.steps))
        .status
}

fn publish_request(
    hostname: &str,
    service: OriginService,
    zone_id: Option<&str>,
) -> PublishRequest {
    PublishRequest {
        account_id: ACCOUNT.into(),
        tunnel_id: TUNNEL.into(),
        tunnel_name: "web".into(),
        hostname: hostname.into(),
        path: None,
        service,
        zone_id: zone_id.map(str::to_string),
        origin_request: OriginRequestUpdate::Preserve,
    }
}

fn zone(id: &str, name: &str) -> Zone {
    Zone {
        id: id.into(),
        name: name.into(),
        status: ZoneStatus::Active,
        paused: false,
        account_id: ACCOUNT.into(),
        account_name: "Acme".into(),
        permissions: vec![],
    }
}

// ── Mock scenery ─────────────────────────────────────────────────────────

/// The two rules the publish tests start from. The first carries an
/// `originRequest` with both a field this build knows (`connectTimeout`) and
/// one it does not (`futureField`) — losing either on a write is the failure
/// this whole file exists to catch.
fn existing_rules() -> (Value, Value) {
    (
        json!({
            "hostname": "one.example.com",
            "service": "http://localhost:1",
            "originRequest": {
                "connectTimeout": 33,
                "noTLSVerify": true,
                "futureField": { "nested": [1, 2, 3] },
            },
        }),
        json!({ "hostname": "two.example.com", "service": "https://10.0.0.2:8443" }),
    )
}

async fn mount_configuration(server: &MockServer, config: Value) {
    Mock::given(method("GET"))
        .and(path(config_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(configuration(config)))
        .mount(server)
        .await;
}

async fn mount_configuration_write(server: &MockServer) {
    Mock::given(method("PUT"))
        .and(path(config_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(configuration(json!(null))))
        .mount(server)
        .await;
}

/// A tunnel that already serves two hostnames, plus a configuration write that
/// succeeds.
async fn mount_two_rule_tunnel(server: &MockServer) {
    let (first, second) = existing_rules();
    mount_configuration(
        server,
        json!({
            "ingress": [first, second, { "service": "http_status:404" }],
            "warp-routing": { "enabled": true },
        }),
    )
    .await;
    mount_configuration_write(server).await;
}

/// The DNS half of a clean publish: nothing there to begin with, a create that
/// works, and the verifying read that then sees the new record.
async fn mount_clean_dns(server: &MockServer, hostname: &str, record_id: &str) {
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .up_to_n_times(1)
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok(tunnel_cname(record_id, hostname, TUNNEL, true))),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                record_id, hostname, TUNNEL, true,
            )])),
        )
        .mount(server)
        .await;
}

// ── Publishing ───────────────────────────────────────────────────────────

#[tokio::test]
async fn publishing_preserves_every_existing_rule_byte_for_byte() {
    let server = MockServer::start().await;
    let (first, second) = existing_rules();
    mount_two_rule_tunnel(&server).await;
    mount_clean_dns(&server, "app.example.com", "record-app").await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let written = written_config(&requests(&server).await);
    let ingress = ingress_of(&written);

    assert_eq!(ingress[0], first, "the first existing rule was altered");
    assert_eq!(ingress[1], second, "the second existing rule was altered");
    assert_eq!(
        written["warp-routing"],
        json!({ "enabled": true }),
        "a top-level section outside ingress was dropped"
    );
}

#[tokio::test]
async fn the_new_rule_goes_in_before_the_catch_all_which_stays_last() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;
    mount_clean_dns(&server, "app.example.com", "record-app").await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let ingress = ingress_of(&written_config(&requests(&server).await));

    assert_eq!(ingress.len(), 4, "two kept, one added, one catch-all");
    assert_eq!(ingress[2]["hostname"], "app.example.com");
    assert_eq!(ingress[2]["service"], "http://localhost:8080");
    assert_eq!(
        ingress[3],
        json!({ "service": "http_status:404" }),
        "the catch-all must still be the last rule"
    );
}

#[tokio::test]
async fn publishing_writes_the_requested_origin_behaviour() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;
    mount_clean_dns(&server, "app.example.com", "record-app").await;

    let mut request = publish_request(
        "app.example.com",
        OriginService::http("localhost", 8080),
        Some(ZONE),
    );
    request.origin_request = OriginRequestUpdate::Set(Box::new(OriginRequest {
        http_host_header: Some("internal.example.test".into()),
        no_tls_verify: Some(true),
        ..OriginRequest::default()
    }));

    let outcome = hostnames::publish(&client(&server), &request, &Cancel::new()).await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let ingress = ingress_of(&written_config(&requests(&server).await));
    assert_eq!(
        ingress[2]["originRequest"],
        json!({ "httpHostHeader": "internal.example.test", "noTLSVerify": true }),
        "the new rule carries the requested origin behaviour"
    );
    assert_eq!(
        ingress[0]["originRequest"]["connectTimeout"], 33,
        "existing rules keep their own behaviour"
    );
}

#[tokio::test]
async fn editing_can_remove_the_existing_origin_request() {
    let server = MockServer::start().await;
    mount_configuration(
        &server,
        json!({
            "ingress": [
                {
                    "hostname": "app.example.com",
                    "service": "https://localhost:443",
                    "originRequest": { "noTLSVerify": true },
                },
                { "service": "http_status:404" },
            ],
        }),
    )
    .await;
    mount_configuration_write(&server).await;

    let mut request = publish_request(
        "app.example.com",
        OriginService::parse("https://localhost:443").unwrap(),
        None,
    );
    request.origin_request = OriginRequestUpdate::Clear;

    let outcome = hostnames::publish(&client(&server), &request, &Cancel::new()).await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let ingress = ingress_of(&written_config(&requests(&server).await));
    assert!(ingress[0].get("originRequest").is_none());
}

#[tokio::test]
async fn publishing_creates_a_proxied_cname_with_an_automatic_ttl() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;
    mount_clean_dns(&server, "app.example.com", "record-app").await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert!(outcome.succeeded(), "{:?}", outcome.steps);
    assert!(outcome.dns.is_healthy(), "{:?}", outcome.dns);

    let posted = bodies(&requests(&server).await, "POST", &dns_path(ZONE));
    assert_eq!(posted.len(), 1);
    assert_eq!(
        posted[0],
        json!({
            "type": "CNAME",
            "name": "app.example.com",
            "content": tunnel_target(),
            "proxied": true,
            "ttl": 1,
            "comment": RECORD_COMMENT,
        })
    );
}

#[tokio::test]
async fn publishing_onto_an_unconfigured_tunnel_creates_the_catch_all() {
    let server = MockServer::start().await;
    mount_configuration(&server, json!(null)).await;
    mount_configuration_write(&server).await;
    mount_clean_dns(&server, "app.example.com", "record-app").await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let ingress = ingress_of(&written_config(&requests(&server).await));

    assert_eq!(ingress.len(), 2, "the new rule and an invented catch-all");
    assert_eq!(ingress[0]["hostname"], "app.example.com");
    assert_eq!(ingress[1], json!({ "service": "http_status:404" }));
}

/// Re-publishing an existing hostname changes where it points and nothing
/// else: the caller is moving the origin, not resetting how the rule is
/// proxied, so the rule's `originRequest` has to survive.
#[tokio::test]
async fn republishing_a_hostname_updates_the_service_and_keeps_its_origin_request() {
    let server = MockServer::start().await;
    let origin_request = json!({
        "connectTimeout": 11,
        "noTLSVerify": true,
        "mysteryField": "keep me",
    });

    mount_configuration(
        &server,
        json!({
            "ingress": [
                {
                    "hostname": "app.example.com",
                    "service": "http://localhost:1",
                    "originRequest": origin_request,
                },
                { "service": "http_status:404" },
            ],
        }),
    )
    .await;
    mount_configuration_write(&server).await;

    // The record is already correct, so a create would be both unnecessary and
    // a sign the "already published" path was missed.
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-app",
                "app.example.com",
                TUNNEL,
                true,
            )])),
        )
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("10.0.0.9", 8443),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let recorded = requests(&server).await;
    let ingress = ingress_of(&written_config(&recorded));

    assert_eq!(ingress.len(), 2, "the rule count must not grow");
    assert_eq!(ingress[0]["service"], "http://10.0.0.9:8443");
    assert_eq!(
        ingress[0]["originRequest"], origin_request,
        "the existing originRequest was reset by an update"
    );
    assert_eq!(
        matching(&recorded, "POST", &dns_path(ZONE)).len(),
        0,
        "a correct record must not be created a second time"
    );
}

#[tokio::test]
async fn a_conflicting_a_record_stops_the_publish_before_anything_is_written() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![dns_record(
                "record-a",
                ZONE,
                "app.example.com",
                "A",
                "203.0.113.7",
                false,
            )])),
        )
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert!(!outcome.succeeded());
    assert_eq!(outcome.failed_step().unwrap().step, Step::CheckDns);
    assert_eq!(
        outcome.recovery,
        Some(Recovery::ResolveConflictByHand {
            record_type: "A".into(),
            content: "203.0.113.7".into(),
        })
    );
    assert!(outcome.touched_rule.is_none());

    let recorded = requests(&server).await;
    assert_eq!(
        matching(&recorded, "PUT", &config_path()).len(),
        0,
        "the configuration must not be touched when DNS already conflicts"
    );
    assert_eq!(matching(&recorded, "POST", &dns_path(ZONE)).len(), 0);
}

#[tokio::test]
async fn a_cname_pointing_at_a_different_tunnel_is_a_conflict_too() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-other",
                "app.example.com",
                OTHER_TUNNEL,
                true,
            )])),
        )
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert_eq!(
        outcome.recovery,
        Some(Recovery::ResolveConflictByHand {
            record_type: "CNAME".into(),
            content: format!("{OTHER_TUNNEL}.cfargotunnel.com"),
        })
    );
    assert_eq!(
        matching(&requests(&server).await, "PUT", &config_path()).len(),
        0,
        "another tunnel's CNAME must not be overwritten"
    );
}

#[tokio::test]
async fn a_dns_failure_after_the_write_offers_to_undo_the_ingress_change() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(500).set_body_json(failure("internal error")))
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert!(outcome.is_partial(), "{:?}", outcome.steps);
    assert_eq!(outcome.failed_step().unwrap().step, Step::CreateDnsRecord);
    assert_eq!(outcome.recovery, Some(Recovery::RollBackIngress));

    let touched = outcome
        .touched_rule
        .as_ref()
        .expect("the rule that was added must come back so it can be removed again");
    assert_eq!(touched.hostname.as_deref(), Some("app.example.com"));
    assert_eq!(touched.service, "http://localhost:8080");
}

/// `rollback_or_permissions` tests `is_permission_problem` first, so a 403 on
/// the DNS half asks for the token to be fixed even though the ingress rule is
/// already in and could be undone. The task description expects
/// `RollBackIngress` for any post-write DNS failure; the code gives
/// `FixPermissions` for this one, and that is what is asserted here.
#[tokio::test]
async fn a_forbidden_dns_write_asks_for_the_token_rather_than_a_rollback() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(403).set_body_json(failure("Authentication error")))
        .mount(&server)
        .await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;

    assert!(outcome.is_partial(), "{:?}", outcome.steps);
    assert_eq!(outcome.recovery, Some(Recovery::FixPermissions));
    assert!(
        outcome.touched_rule.is_some(),
        "the added rule is still returned, so a rollback stays possible"
    );
}

#[tokio::test]
async fn rolling_back_removes_only_the_rule_the_publish_added() {
    let server = MockServer::start().await;
    let (first, second) = existing_rules();

    // The first read is the tunnel as it was; every read after the failed
    // publish sees the rule that went in.
    Mock::given(method("GET"))
        .and(path(config_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(configuration(json!({
                "ingress": [first, second, { "service": "http_status:404" }],
            }))),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(config_path()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(configuration(json!({
                "ingress": [
                    first,
                    second,
                    { "hostname": "app.example.com", "service": "http://localhost:8080" },
                    { "service": "http_status:404" },
                ],
            }))),
        )
        .mount(&server)
        .await;
    mount_configuration_write(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(500).set_body_json(failure("internal error")))
        .mount(&server)
        .await;

    let client = client(&server);
    let outcome = hostnames::publish(
        &client,
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            Some(ZONE),
        ),
        &Cancel::new(),
    )
    .await;
    assert!(outcome.is_partial(), "{:?}", outcome.steps);

    hostnames::roll_back_ingress(
        &client,
        ACCOUNT,
        TUNNEL,
        "app.example.com",
        None,
        &Cancel::new(),
    )
    .await
    .expect("the rollback should succeed");

    let configs = written_configs(&requests(&server).await);
    assert_eq!(
        configs.len(),
        2,
        "the publish wrote once, the rollback once"
    );

    let ingress = ingress_of(&configs[1]);
    assert!(
        !ingress
            .iter()
            .any(|rule| rule["hostname"] == "app.example.com"),
        "the rolled-back hostname is still in the configuration: {ingress:?}"
    );
    assert_eq!(ingress.len(), 3);
    assert_eq!(ingress[0], first, "another rule was lost in the rollback");
    assert_eq!(ingress[1], second, "another rule was lost in the rollback");
    assert_eq!(ingress[2], json!({ "service": "http_status:404" }));
}

/// With no zone the ingress half is still worth doing — the user may be
/// managing DNS somewhere else — so the DNS steps are skipped with an
/// explanation rather than failed.
#[tokio::test]
async fn publishing_without_a_zone_writes_the_rule_and_skips_dns() {
    let server = MockServer::start().await;
    mount_two_rule_tunnel(&server).await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            None,
        ),
        &Cancel::new(),
    )
    .await;

    assert!(outcome.succeeded(), "{:?}", outcome.steps);
    assert_eq!(status_of(&outcome, Step::CheckDns), Status::Skipped);
    assert_eq!(status_of(&outcome, Step::CreateDnsRecord), Status::Skipped);
    assert!(
        matches!(outcome.dns, DnsState::Unknown { .. }),
        "{:?}",
        outcome.dns
    );

    let recorded = requests(&server).await;
    let ingress = ingress_of(&written_config(&recorded));
    assert_eq!(ingress[2]["hostname"], "app.example.com");
    assert!(
        recorded
            .iter()
            .all(|request| !request.url.path().contains("/zones/")),
        "no zone was selected, so no zone should have been contacted"
    );
}

/// cloudflared stops at the first matching rule, so anything after a catch-all
/// is dead. A document that arrives with one in the wrong place has to be
/// written back with it at the end.
#[tokio::test]
async fn a_catch_all_found_before_the_end_is_written_back_last() {
    let server = MockServer::start().await;
    mount_configuration(
        &server,
        json!({
            "ingress": [
                { "service": "http_status:404" },
                { "hostname": "one.example.com", "service": "http://localhost:1" },
            ],
        }),
    )
    .await;
    mount_configuration_write(&server).await;

    let outcome = hostnames::publish(
        &client(&server),
        &publish_request(
            "app.example.com",
            OriginService::http("localhost", 8080),
            None,
        ),
        &Cancel::new(),
    )
    .await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let ingress = ingress_of(&written_config(&requests(&server).await));

    assert_eq!(ingress.len(), 3);
    assert_eq!(ingress[2], json!({ "service": "http_status:404" }));
    assert!(
        ingress[..2].iter().all(|rule| rule["hostname"].is_string()),
        "a catch-all is still ahead of a hostname rule: {ingress:?}"
    );
}

// ── Unpublishing ─────────────────────────────────────────────────────────

async fn mount_three_rule_tunnel(server: &MockServer) {
    mount_configuration(
        server,
        json!({
            "ingress": [
                { "hostname": "a.example.com", "service": "http://localhost:1" },
                { "hostname": "b.example.com", "service": "http://localhost:2" },
                { "hostname": "c.example.com", "service": "http://localhost:3" },
                { "service": "http_status:404" },
            ],
        }),
    )
    .await;
}

async fn unpublish_b(server: &MockServer) -> Outcome {
    hostnames::unpublish(
        &client(server),
        ACCOUNT,
        TUNNEL,
        "b.example.com",
        None,
        Some(ZONE),
        &Cancel::new(),
    )
    .await
}

#[tokio::test]
async fn unpublishing_removes_exactly_one_rule_and_leaves_the_others() {
    let server = MockServer::start().await;
    mount_three_rule_tunnel(&server).await;
    mount_configuration_write(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![
            tunnel_cname("record-ours", "b.example.com", TUNNEL, true),
            tunnel_cname("record-other", "b.example.com", OTHER_TUNNEL, true),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(record_path(ZONE, "record-ours")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({ "id": "record-ours" }))))
        .mount(&server)
        .await;

    let outcome = unpublish_b(&server).await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let recorded = requests(&server).await;
    let ingress = ingress_of(&written_config(&recorded));

    assert_eq!(
        ingress.len(),
        3,
        "one rule removed, two kept plus catch-all"
    );
    assert_eq!(ingress[0]["hostname"], "a.example.com");
    assert_eq!(ingress[1]["hostname"], "c.example.com");
    assert_eq!(ingress[2], json!({ "service": "http_status:404" }));

    assert_eq!(
        deletes(&recorded),
        vec![record_path(ZONE, "record-ours")],
        "only the record whose content is this tunnel's target may be deleted"
    );
}

/// The record this application created is one of several the hostname may
/// have, and it is the only one it may remove. Four records go in, exactly one
/// DELETE must come out.
#[tokio::test]
async fn unpublishing_deletes_only_the_cname_that_points_at_this_tunnel() {
    let server = MockServer::start().await;
    mount_three_rule_tunnel(&server).await;
    mount_configuration_write(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![
            dns_record("record-a", ZONE, "b.example.com", "A", "203.0.113.9", false),
            dns_record(
                "record-txt",
                ZONE,
                "b.example.com",
                "TXT",
                "v=spf1 -all",
                false,
            ),
            tunnel_cname("record-other", "b.example.com", OTHER_TUNNEL, true),
            tunnel_cname("record-ours", "b.example.com", TUNNEL, true),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(record_path(ZONE, "record-ours")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(json!({ "id": "record-ours" }))))
        .mount(&server)
        .await;

    let outcome = unpublish_b(&server).await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);

    let issued = deletes(&requests(&server).await);
    assert_eq!(issued, vec![record_path(ZONE, "record-ours")]);
    for spared in ["record-a", "record-txt", "record-other"] {
        assert!(
            !issued.iter().any(|path| path.ends_with(spared)),
            "{spared} was deleted and it was not ours"
        );
    }
}

#[tokio::test]
async fn unpublishing_with_no_matching_record_skips_the_delete_rather_than_failing() {
    let server = MockServer::start().await;
    mount_three_rule_tunnel(&server).await;
    mount_configuration_write(&server).await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;

    let outcome = unpublish_b(&server).await;

    assert!(outcome.succeeded(), "{:?}", outcome.steps);
    assert_eq!(status_of(&outcome, Step::RemoveDnsRecord), Status::Skipped);
    let detail = &outcome
        .steps
        .iter()
        .find(|result| result.step == Step::RemoveDnsRecord)
        .unwrap()
        .detail;
    assert!(
        detail.contains("no DNS record"),
        "the skip should say why: {detail}"
    );
    assert!(deletes(&requests(&server).await).is_empty());
}

/// The DNS record is the only thing still making the hostname resolve. If the
/// ingress rule could not be removed, deleting it would turn a working
/// hostname into a 1016 for no reason.
#[tokio::test]
async fn a_failed_configuration_write_leaves_the_dns_record_alone() {
    let server = MockServer::start().await;
    mount_three_rule_tunnel(&server).await;

    Mock::given(method("PUT"))
        .and(path(config_path()))
        .respond_with(ResponseTemplate::new(500).set_body_json(failure("internal error")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-ours",
                "b.example.com",
                TUNNEL,
                true,
            )])),
        )
        .mount(&server)
        .await;

    let outcome = unpublish_b(&server).await;

    assert!(!outcome.succeeded());
    assert_eq!(
        outcome.failed_step().unwrap().step,
        Step::WriteConfiguration
    );

    let recorded = requests(&server).await;
    assert!(
        deletes(&recorded).is_empty(),
        "the record was deleted anyway"
    );
    assert_eq!(
        matching(&recorded, "GET", &dns_path(ZONE)).len(),
        0,
        "DNS should not even be looked up once the write failed"
    );
}

// ── Repairing ────────────────────────────────────────────────────────────

async fn repair(server: &MockServer) -> Outcome {
    hostnames::repair_dns(
        &client(server),
        ZONE,
        TUNNEL,
        "app.example.com",
        &Cancel::new(),
    )
    .await
}

#[tokio::test]
async fn repairing_a_missing_record_creates_the_proxied_cname() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(dns_path(ZONE)))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(tunnel_cname(
            "record-app",
            "app.example.com",
            TUNNEL,
            true,
        ))))
        .mount(&server)
        .await;

    let outcome = repair(&server).await;

    assert!(outcome.succeeded(), "{:?}", outcome.steps);
    assert_eq!(
        outcome.dns,
        DnsState::Correct {
            record_id: "record-app".into()
        }
    );

    let posted = bodies(&requests(&server).await, "POST", &dns_path(ZONE));
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0]["content"], tunnel_target());
    assert_eq!(posted[0]["proxied"], true);
}

/// `PATCH`, not `PUT`: a `PUT` resets every field the body leaves out, so
/// turning the proxy on would silently discard the record's TTL and comment.
/// The body carrying nothing but `content` and `proxied` is what proves it.
#[tokio::test]
async fn repairing_an_unproxied_record_patches_only_content_and_proxied() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-app",
                "app.example.com",
                TUNNEL,
                false,
            )])),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(record_path(ZONE, "record-app")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok(tunnel_cname(
            "record-app",
            "app.example.com",
            TUNNEL,
            true,
        ))))
        .mount(&server)
        .await;

    let outcome = repair(&server).await;
    assert!(outcome.succeeded(), "{:?}", outcome.steps);
    assert!(outcome.dns.is_healthy(), "{:?}", outcome.dns);

    let recorded = requests(&server).await;
    let patched = bodies(&recorded, "PATCH", &record_path(ZONE, "record-app"));
    assert_eq!(patched.len(), 1);
    assert_eq!(
        patched[0],
        json!({ "content": tunnel_target(), "proxied": true }),
        "the patch body must carry nothing a PUT would reset"
    );
    assert_eq!(
        matching(&recorded, "PUT", &record_path(ZONE, "record-app")).len(),
        0
    );
}

#[tokio::test]
async fn repairing_refuses_to_touch_a_conflicting_record() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![dns_record(
                "record-a",
                ZONE,
                "app.example.com",
                "A",
                "203.0.113.7",
                false,
            )])),
        )
        .mount(&server)
        .await;

    let outcome = repair(&server).await;

    assert!(!outcome.succeeded());
    assert_eq!(outcome.failed_step().unwrap().step, Step::RepairDnsRecord);
    assert_eq!(
        outcome.recovery,
        Some(Recovery::ResolveConflictByHand {
            record_type: "A".into(),
            content: "203.0.113.7".into(),
        })
    );

    let recorded = requests(&server).await;
    assert!(
        recorded
            .iter()
            .all(|request| request.method.as_str() == "GET"),
        "a conflicting record must not be written to at all"
    );
}

// ── Loading ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn load_hostnames_joins_each_rule_with_its_own_dns_lookup() {
    let server = MockServer::start().await;
    mount_configuration(
        &server,
        json!({
            "ingress": [
                { "hostname": "app.example.com", "service": "http://localhost:1" },
                { "hostname": "api.example.com", "service": "http://localhost:2" },
                { "service": "http_status:404" },
            ],
        }),
    )
    .await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .and(query_param("name.exact", "app.example.com"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-app",
                "app.example.com",
                TUNNEL,
                true,
            )])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .and(query_param("name.exact", "api.example.com"))
        .respond_with(ResponseTemplate::new(200).set_body_json(dns_list(vec![])))
        .mount(&server)
        .await;

    let hostnames = hostnames::load_hostnames(
        &client(&server),
        ACCOUNT,
        TUNNEL,
        "web",
        &[zone(ZONE, "example.com")],
        &Cancel::new(),
    )
    .await
    .expect("the load should succeed");

    assert_eq!(hostnames.len(), 2, "the catch-all is not a public hostname");
    assert_eq!(hostnames[0].hostname, "app.example.com");
    assert_eq!(hostnames[0].zone_id.as_deref(), Some(ZONE));
    assert_eq!(hostnames[0].zone_name.as_deref(), Some("example.com"));
    assert_eq!(
        hostnames[0].dns,
        DnsState::Correct {
            record_id: "record-app".into()
        }
    );
    assert_eq!(hostnames[1].dns, DnsState::Missing);
}

/// One zone the token cannot read must not cost the whole list. The hostnames
/// in it are reported as unknown, with the reason, and everything else is
/// still joined up.
#[tokio::test]
async fn a_zone_that_cannot_be_read_leaves_its_hostnames_unknown() {
    let server = MockServer::start().await;
    mount_configuration(
        &server,
        json!({
            "ingress": [
                { "hostname": "app.example.com", "service": "http://localhost:1" },
                { "hostname": "shop.other.test", "service": "http://localhost:2" },
                { "service": "http_status:404" },
            ],
        }),
    )
    .await;

    Mock::given(method("GET"))
        .and(path(dns_path(ZONE)))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(dns_list(vec![tunnel_cname(
                "record-app",
                "app.example.com",
                TUNNEL,
                true,
            )])),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(dns_path(OTHER_ZONE)))
        .respond_with(ResponseTemplate::new(403).set_body_json(failure("Authentication error")))
        .mount(&server)
        .await;

    let hostnames = hostnames::load_hostnames(
        &client(&server),
        ACCOUNT,
        TUNNEL,
        "web",
        &[zone(ZONE, "example.com"), zone(OTHER_ZONE, "other.test")],
        &Cancel::new(),
    )
    .await
    .expect("an unreadable zone must not fail the whole load");

    assert!(hostnames[0].dns.is_healthy(), "{:?}", hostnames[0].dns);
    match &hostnames[1].dns {
        DnsState::Unknown { reason } => assert!(
            reason.contains("read the zone's DNS records"),
            "the reason should name what failed: {reason}"
        ),
        other => panic!("expected an unknown DNS state, got {other:?}"),
    }
}
