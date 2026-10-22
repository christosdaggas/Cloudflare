//! The Cloudflare v4 client, against a mock server.
//!
//! Everything here runs against `wiremock` on `127.0.0.1` over plain HTTP: no
//! token, no network, no display. The point of the file is the awkward parts of
//! the real API rather than the happy path — the envelope's undeclared fields,
//! the two different shapes of `result_info`, the difference between "the token
//! is missing a permission" and "Cloudflare is busy", and the rule that a read
//! may be retried and a write may never be.

use std::time::{Duration, Instant};

use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value};
use tunnel_manager::cloudflare::models::PatchDnsRecordRequest;
use tunnel_manager::cloudflare::{Cancel, Capability, CloudflareClient, CloudflareError, Timeouts};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ACCOUNT_ID: &str = "023e105f4ecef8ad9ca31a8372d0c353";
const ZONE_ID: &str = "9e5c02ac921467fac1744f8bfaa148a4";
const TUNNEL_ID: &str = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";
const RECORD_ID: &str = "372e67954025e0ba6aaa6d586b9e0b59";

/// A client pointed at the mock server. The token is fake and never checked by
/// anything here; `wiremock` speaks plain HTTP, so the base URL has to be
/// injected rather than defaulted.
fn client(server: &MockServer) -> CloudflareClient {
    CloudflareClient::with_base_url(
        SecretString::from("t".repeat(40)),
        format!("{}/client/v4", server.uri()),
        Timeouts::default(),
    )
    .expect("the client builds")
}

fn ok_envelope(result: Value) -> Value {
    json!({"success": true, "errors": [], "messages": [], "result": result})
}

fn ok_page(result: Value, result_info: Value) -> Value {
    json!({
        "success": true,
        "errors": [],
        "messages": [],
        "result": result,
        "result_info": result_info,
    })
}

fn failed_envelope(errors: Value) -> Value {
    json!({"success": false, "errors": errors, "messages": [], "result": null})
}

async fn request_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("request recording is on by default")
        .len()
}

/// The `page` query parameter of every request the server saw, in order.
async fn pages_requested(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("request recording is on by default")
        .iter()
        .filter_map(|request| {
            request
                .url
                .query_pairs()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.into_owned())
        })
        .collect()
}

fn cfd_tunnel(index: usize) -> Value {
    json!({
        "id": format!("00000000-0000-4000-8000-{index:012}"),
        "name": format!("tunnel-{index}"),
        "status": "inactive",
        "tun_type": "cfd_tunnel",
        "remote_config": true,
        "connections": [],
    })
}

// ── Token verification ───────────────────────────────────────────────────

#[tokio::test]
async fn an_active_token_verifies() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": "ed17574386854bf78a67040be0a770b0",
            "status": "active",
        }))))
        .mount(&server)
        .await;

    let status = client(&server).verify_token(&Cancel::new()).await.unwrap();

    assert!(status.is_active());
    assert_eq!(
        status.id.as_deref(),
        Some("ed17574386854bf78a67040be0a770b0")
    );
    assert!(
        status.caveat().is_none(),
        "a live token with no expiry has nothing to warn about"
    );
}

#[tokio::test]
async fn a_disabled_token_is_surfaced_rather_than_read_as_a_failure() {
    // Cloudflare answers 200 for a disabled token; the state is in the body,
    // so treating a 200 as "connected" would show a working Connection page
    // for a token that cannot do anything.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": "ed17574386854bf78a67040be0a770b0",
            "status": "disabled",
        }))))
        .mount(&server)
        .await;

    let status = client(&server).verify_token(&Cancel::new()).await.unwrap();

    assert!(!status.is_active());
    assert_eq!(status.status, "disabled");
    assert!(status.caveat().unwrap().contains("disabled"));
}

#[tokio::test]
async fn a_verify_response_with_no_result_still_means_the_token_is_live() {
    // `/user/tokens/verify` is documented as possibly omitting `result`. A 200
    // is the answer; an absent body must not be read as "unknown" and lock the
    // user out of their own working token.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "errors": [],
            "messages": [{"code": 10000, "message": "This API Token is valid and active"}],
        })))
        .mount(&server)
        .await;

    let status = client(&server).verify_token(&Cancel::new()).await.unwrap();

    assert!(status.is_active());
    assert_eq!(status.status, "active");
    assert!(status.id.is_none());
}

// ── The envelope superset ────────────────────────────────────────────────

async fn tunnel_error(errors: Value) -> CloudflareError {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}"
        )))
        .respond_with(ResponseTemplate::new(400).set_body_json(failed_envelope(errors)))
        .mount(&server)
        .await;

    client(&server)
        .get_tunnel(ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .unwrap_err()
}

#[tokio::test]
async fn an_error_item_of_only_a_code_and_a_message_is_parsed() {
    let error = tunnel_error(json!([{"code": 1004, "message": "Cannot read the tunnel"}])).await;

    let CloudflareError::Api { status, errors, .. } = &error else {
        panic!("expected an API error, got {error:?}");
    };
    assert_eq!(*status, 400);
    assert_eq!(errors[0].code, 1004);
    assert_eq!(errors[0].full_text(), "Cannot read the tunnel");
    assert!(error.user_message().contains("Cannot read the tunnel"));
}

#[tokio::test]
async fn an_error_item_carrying_a_documentation_url_and_a_source_pointer_is_parsed() {
    let error = tunnel_error(json!([{
        "code": 10000,
        "message": "domain_name is required",
        "documentation_url": "https://developers.cloudflare.com/api",
        "source": {"pointer": "/domain_name"},
    }]))
    .await;

    let CloudflareError::Api { errors, .. } = &error else {
        panic!("expected an API error, got {error:?}");
    };
    assert_eq!(
        errors[0].documentation_url.as_deref(),
        Some("https://developers.cloudflare.com/api")
    );
    assert_eq!(
        errors[0].source.as_ref().unwrap().pointer.as_deref(),
        Some("/domain_name")
    );
    assert!(error.user_message().contains("domain_name is required"));
}

#[tokio::test]
async fn a_nested_error_chain_reaches_the_message_the_user_reads() {
    // The upstream link is the part that says what actually went wrong;
    // reporting only the outer wrapper would leave the user with "an error
    // occurred" and nothing to act on.
    let error = tunnel_error(json!([{
        "code": 1000,
        "message": "An unknown error occurred",
        "error_chain": [{"code": 1001, "message": "origin certificate has expired"}],
    }]))
    .await;

    let CloudflareError::Api { errors, .. } = &error else {
        panic!("expected an API error, got {error:?}");
    };
    assert_eq!(
        errors[0].full_text(),
        "An unknown error occurred — origin certificate has expired"
    );
    assert!(error
        .user_message()
        .contains("origin certificate has expired"));
    assert!(error
        .detail()
        .unwrap()
        .contains("origin certificate has expired"));
}

#[tokio::test]
async fn a_tunnel_object_with_undeclared_fields_still_parses() {
    // Cloudflare ships fields it never declared. A strict deserializer would
    // turn a perfectly good response into a broken application.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": TUNNEL_ID,
            "name": "example-tunnel",
            "status": "healthy",
            "tun_type": "cfd_tunnel",
            "remote_config": true,
            "shipped_last_tuesday": {"nested": [1, 2, 3]},
            "connections": [
                {"id": "c1", "client_id": "cid", "colo_name": "ATH", "also_new": true}
            ],
        }))))
        .mount(&server)
        .await;

    let tunnel = client(&server)
        .get_tunnel(ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnel.name, "example-tunnel");
    assert_eq!(tunnel.connector_count, 1);
}

// ── Pagination ───────────────────────────────────────────────────────────

#[tokio::test]
async fn zones_are_collected_across_every_page_when_total_pages_is_present() {
    let server = MockServer::start().await;
    for page in 1..=3_u32 {
        let first = (page - 1) * 2;
        Mock::given(method("GET"))
            .and(path("/client/v4/zones"))
            .and(query_param("page", page.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(
                json!([
                    {"id": format!("zone{first}"), "name": format!("z{first}.example.com")},
                    {"id": format!("zone{}", first + 1),
                     "name": format!("z{}.example.com", first + 1)},
                ]),
                json!({"page": page, "per_page": 2, "count": 2, "total_count": 6,
                       "total_pages": 3}),
            )))
            .mount(&server)
            .await;
    }

    let zones = client(&server)
        .list_zones(None, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(zones.len(), 6);
    assert_eq!(zones[0].name, "z0.example.com");
    assert_eq!(zones[5].name, "z5.example.com");
    assert_eq!(pages_requested(&server).await, ["1", "2", "3"]);
}

#[tokio::test]
async fn tunnels_are_collected_when_result_info_omits_total_pages() {
    // `/cfd_tunnel` sends `total_count` and no `total_pages`, so the client has
    // to derive the end of the list from the page size it was given.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(
            Value::Array((0..50).map(cfd_tunnel).collect()),
            json!({"page": 1, "per_page": 50, "count": 50, "total_count": 51}),
        )))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .and(query_param("page", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(
            json!([cfd_tunnel(50)]),
            json!({"page": 2, "per_page": 50, "count": 1, "total_count": 51}),
        )))
        .mount(&server)
        .await;

    let tunnels = client(&server)
        .list_tunnels(ACCOUNT_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 51);
    assert_eq!(tunnels[50].name, "tunnel-50");
    assert_eq!(pages_requested(&server).await, ["1", "2"]);
}

#[tokio::test]
async fn a_single_short_page_is_fetched_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/accounts"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_page(
            json!([{"id": ACCOUNT_ID, "name": "Acme"}]),
            json!({"page": 1, "per_page": 50, "count": 1, "total_count": 1, "total_pages": 1}),
        )))
        .mount(&server)
        .await;

    let accounts = client(&server).list_accounts(&Cancel::new()).await.unwrap();

    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].name, "Acme");
    assert_eq!(
        request_count(&server).await,
        1,
        "one short page is the whole list"
    );
}

// ── Permission failures ──────────────────────────────────────────────────

#[tokio::test]
async fn a_401_points_the_user_at_the_connection_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(401).set_body_json(failed_envelope(
            json!([{"code": 10000, "message": "Authentication error"}]),
        )))
        .mount(&server)
        .await;

    let error = client(&server)
        .verify_token(&Cancel::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CloudflareError::Unauthorized { .. }),
        "got {error:?}"
    );
    assert!(error.is_permission_problem());
    assert!(
        !error.is_transient(),
        "a rejected token will not fix itself"
    );
    assert!(
        error.user_message().contains("Connection page"),
        "got {}",
        error.user_message()
    );
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn a_403_creating_a_dns_record_names_the_dns_permission_row() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/client/v4/zones/{ZONE_ID}/dns_records")))
        .respond_with(ResponseTemplate::new(403).set_body_json(failed_envelope(
            json!([{"code": 9109, "message": "Unauthorized to access requested resource"}]),
        )))
        .mount(&server)
        .await;

    let error = client(&server)
        .create_tunnel_cname(ZONE_ID, "app.example.com", TUNNEL_ID, None, &Cancel::new())
        .await
        .unwrap_err();

    assert_eq!(error.missing_capability(), Some(Capability::Dns));
    assert!(error.is_permission_problem());
    let message = error.user_message();
    assert!(message.contains("Zone → DNS → Edit"), "got {message}");
    assert!(message.contains("create the DNS record"), "got {message}");
}

#[tokio::test]
async fn a_403_listing_zones_names_the_zone_permission_row() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/zones"))
        .respond_with(ResponseTemplate::new(403).set_body_json(failed_envelope(
            json!([{"code": 9109, "message": "Unauthorized to access requested resource"}]),
        )))
        .mount(&server)
        .await;

    let error = client(&server)
        .list_zones(None, &Cancel::new())
        .await
        .unwrap_err();

    assert_eq!(error.missing_capability(), Some(Capability::Zones));
    assert!(
        error.user_message().contains("Zone → Zone → Read"),
        "got {}",
        error.user_message()
    );
    assert_eq!(
        request_count(&server).await,
        1,
        "a permission failure is not retried"
    );
}

// ── Rate limiting ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_429_carries_retry_after_into_the_error_and_the_message() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "42")
                .set_body_json(failed_envelope(
                    json!([{"code": 971, "message": "Rate limited"}]),
                )),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .verify_token(&Cancel::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CloudflareError::RateLimited { .. }),
        "got {error:?}"
    );
    assert_eq!(error.retry_after(), Some(Duration::from_secs(42)));
    assert!(
        error.user_message().contains("42"),
        "got {}",
        error.user_message()
    );
}

#[tokio::test]
async fn the_structured_ratelimit_header_is_the_fallback_when_retry_after_is_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("ratelimit", "\"default\";r=0;t=30")
                .set_body_json(failed_envelope(
                    json!([{"code": 971, "message": "Rate limited"}]),
                )),
        )
        .mount(&server)
        .await;

    let error = client(&server)
        .verify_token(&Cancel::new())
        .await
        .unwrap_err();

    assert_eq!(error.retry_after(), Some(Duration::from_secs(30)));
    assert!(
        error.user_message().contains("30"),
        "got {}",
        error.user_message()
    );
}

// ── Retries ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_read_survives_two_transient_failures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(503).set_body_json(failed_envelope(
            json!([{"code": 1000, "message": "Service temporarily unavailable"}]),
        )))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": "ed17574386854bf78a67040be0a770b0",
            "status": "active",
        }))))
        .mount(&server)
        .await;

    let status = client(&server).verify_token(&Cancel::new()).await.unwrap();

    assert!(status.is_active());
    assert_eq!(
        request_count(&server).await,
        3,
        "two failures, then the answer"
    );
}

#[tokio::test]
async fn a_long_retry_after_is_reported_rather_than_slept_through() {
    // A rate-limit window is five minutes. Holding a spinner open for that long
    // is worse than telling the user what happened.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "300")
                .set_body_json(failed_envelope(
                    json!([{"code": 971, "message": "Rate limited"}]),
                )),
        )
        .mount(&server)
        .await;

    let started = Instant::now();
    let error = client(&server)
        .verify_token(&Cancel::new())
        .await
        .unwrap_err();

    assert_eq!(error.retry_after(), Some(Duration::from_secs(300)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the wait was slept through"
    );
    assert_eq!(request_count(&server).await, 1);
}

#[tokio::test]
async fn a_create_that_fails_transiently_is_never_retried() {
    // A create that timed out may have succeeded. Trying again would leave two
    // tunnels behind, which is worse than reporting the failure.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .respond_with(ResponseTemplate::new(503).set_body_json(failed_envelope(
            json!([{"code": 1000, "message": "Service temporarily unavailable"}]),
        )))
        .mount(&server)
        .await;

    let error = client(&server)
        .create_tunnel(ACCOUNT_ID, "api-tunnel", &Cancel::new())
        .await
        .unwrap_err();

    assert!(error.is_transient(), "the failure itself was transient");
    assert_eq!(
        request_count(&server).await,
        1,
        "a write is sent exactly once"
    );
}

// ── Statuses and bad bodies ──────────────────────────────────────────────

#[tokio::test]
async fn a_404_is_a_not_found_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(failed_envelope(
            json!([{"code": 1003, "message": "Tunnel not found"}]),
        )))
        .mount(&server)
        .await;

    let error = client(&server)
        .get_tunnel(ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .unwrap_err();

    assert!(error.is_not_found(), "got {error:?}");
    assert!(!error.is_transient());
    assert!(
        error.user_message().contains("does not exist"),
        "got {}",
        error.user_message()
    );
}

#[tokio::test]
async fn a_200_that_is_not_json_is_a_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/client/v4/user/tokens/verify"))
        .respond_with(ResponseTemplate::new(200).set_body_string("welcome to the wifi portal"))
        .mount(&server)
        .await;

    let error = client(&server)
        .verify_token(&Cancel::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CloudflareError::Decode { .. }),
        "got {error:?}"
    );
    assert_eq!(
        request_count(&server).await,
        1,
        "a malformed body will not improve on retry"
    );
}

#[tokio::test]
async fn an_html_error_page_is_reported_as_the_status_and_not_as_a_decode_bug() {
    // An edge error page is Cloudflare's problem, not a bug in this build, and
    // telling the user otherwise sends them to the wrong place.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}"
        )))
        .respond_with(ResponseTemplate::new(502).set_body_raw(
            "<html><head><title>502 Bad Gateway</title></head><body></body></html>",
            "text/html",
        ))
        .mount(&server)
        .await;

    let error = client(&server)
        .get_tunnel(ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .unwrap_err();

    assert!(
        matches!(error, CloudflareError::Api { status: 502, .. }),
        "got {error:?}"
    );
    assert!(
        error.user_message().contains("502"),
        "got {}",
        error.user_message()
    );
}

// ── Request bodies ───────────────────────────────────────────────────────

#[tokio::test]
async fn create_always_asks_for_a_remotely_managed_tunnel_and_hands_back_the_run_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .and(body_json(
            json!({"name": "api-tunnel", "config_src": "cloudflare"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": TUNNEL_ID,
            "name": "api-tunnel",
            "status": "inactive",
            "remote_config": true,
            "token": "eyJhIjoiNWFiNGU5Wg==",
        }))))
        .mount(&server)
        .await;

    let (tunnel, token) = client(&server)
        .create_tunnel(ACCOUNT_ID, "api-tunnel", &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnel.name, "api-tunnel");
    assert_eq!(token.unwrap().expose_secret(), "eyJhIjoiNWFiNGU5Wg==");
}

#[tokio::test]
async fn a_rename_never_sends_the_tunnel_secret() {
    // The same endpoint rotates the run token when `tunnel_secret` is present,
    // which would take every running connector down as a side effect of typing
    // a new name.
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": TUNNEL_ID,
            "name": "renamed",
            "status": "inactive",
            "remote_config": true,
        }))))
        .mount(&server)
        .await;

    let tunnel = client(&server)
        .rename_tunnel(ACCOUNT_ID, TUNNEL_ID, "renamed", &Cancel::new())
        .await
        .unwrap();
    assert_eq!(tunnel.name, "renamed");

    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(body, json!({"name": "renamed"}));
    assert!(body.get("tunnel_secret").is_none());
}

#[tokio::test]
async fn the_run_token_endpoint_returns_a_bare_json_string() {
    // `result` here is a string, not an object with a `token` field.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel/{TUNNEL_ID}/token"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ok_envelope(json!("eyJhIjoiNWFiNGU5Wg=="))),
        )
        .mount(&server)
        .await;

    let token = client(&server)
        .get_tunnel_token(ACCOUNT_ID, TUNNEL_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(token.expose_secret(), "eyJhIjoiNWFiNGU5Wg==");
}

#[tokio::test]
async fn listing_tunnels_asks_cloudflare_to_leave_out_deleted_ones() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .and(query_param("is_deleted", "false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!([cfd_tunnel(1)]))))
        .mount(&server)
        .await;

    let tunnels = client(&server)
        .list_tunnels(ACCOUNT_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    let requests = server.received_requests().await.unwrap();
    assert!(
        requests[0]
            .url
            .query_pairs()
            .any(|(key, value)| key == "is_deleted" && value == "false"),
        "omitting the parameter returns deleted tunnels alongside live ones"
    );
}

#[tokio::test]
async fn a_warp_connector_is_not_listed_as_a_tunnel() {
    // WARP connectors and Magic WAN tunnels share this endpoint and nothing
    // else: none of the tunnel screens can do anything useful with one.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!([
            cfd_tunnel(1),
            {"id": TUNNEL_ID, "name": "warp", "tun_type": "warp_connector",
             "status": "healthy", "remote_config": false},
        ]))))
        .mount(&server)
        .await;

    let tunnels = client(&server)
        .list_tunnels(ACCOUNT_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].name, "tunnel-1");
}

#[tokio::test]
async fn a_soft_deleted_tunnel_is_filtered_out_even_though_the_request_excluded_them() {
    // `is_deleted=false` is a request, not a guarantee; the client checks
    // `deleted_at` as well so a deleted tunnel can never reach a delete button.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/client/v4/accounts/{ACCOUNT_ID}/cfd_tunnel")))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!([
            cfd_tunnel(1),
            {"id": TUNNEL_ID, "name": "gone", "tun_type": "cfd_tunnel",
             "status": "inactive", "remote_config": true,
             "deleted_at": "2025-02-18T22:41:43.534395Z"},
        ]))))
        .mount(&server)
        .await;

    let tunnels = client(&server)
        .list_tunnels(ACCOUNT_ID, &Cancel::new())
        .await
        .unwrap();

    assert_eq!(tunnels.len(), 1);
    assert_eq!(tunnels[0].name, "tunnel-1");
}

#[tokio::test]
async fn a_dns_delete_response_of_only_a_result_is_accepted() {
    // `DELETE /zones/{id}/dns_records/{id}` is documented as returning the id
    // alone — no `success`, no `errors`. Requiring `success` would report a
    // successful deletion as a failure and prompt a pointless retry.
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/client/v4/zones/{ZONE_ID}/dns_records/{RECORD_ID}"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"result": {"id": RECORD_ID}})),
        )
        .mount(&server)
        .await;

    client(&server)
        .delete_dns_record(ZONE_ID, RECORD_ID, &Cancel::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_dns_patch_sends_only_the_fields_that_were_set() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(format!(
            "/client/v4/zones/{ZONE_ID}/dns_records/{RECORD_ID}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_envelope(json!({
            "id": RECORD_ID,
            "name": "app.example.com",
            "type": "CNAME",
            "content": format!("{TUNNEL_ID}.cfargotunnel.com"),
            "proxied": true,
            "ttl": 1,
        }))))
        .mount(&server)
        .await;

    let record = client(&server)
        .patch_dns_record(
            ZONE_ID,
            RECORD_ID,
            PatchDnsRecordRequest {
                proxied: Some(true),
                ..PatchDnsRecordRequest::default()
            },
            &Cancel::new(),
        )
        .await
        .unwrap();
    assert!(record.proxied);

    let requests = server.received_requests().await.unwrap();
    let body: Value = requests[0].body_json().unwrap();
    assert_eq!(
        body,
        json!({"proxied": true}),
        "PATCH must not resend a field it is not changing"
    );
}

// ── Cancellation and secret hygiene ──────────────────────────────────────

#[tokio::test]
async fn a_cancelled_call_never_reaches_the_network() {
    let server = MockServer::start().await;
    let cancel = Cancel::new();
    cancel.cancel();

    let error = client(&server).verify_token(&cancel).await.unwrap_err();

    assert!(error.is_cancelled(), "got {error:?}");
    assert_eq!(
        request_count(&server).await,
        0,
        "the request was never sent"
    );
}

#[tokio::test]
async fn the_token_never_appears_anywhere_in_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/client/v4/zones/{ZONE_ID}/dns_records")))
        .respond_with(ResponseTemplate::new(403).set_body_json(failed_envelope(
            json!([{"code": 9109, "message": "Unauthorized to access requested resource"}]),
        )))
        .mount(&server)
        .await;

    let token = "DiStInCtIvE-tOkEn-0123456789-abcdefghij";
    let client = CloudflareClient::with_base_url(
        SecretString::from(token),
        format!("{}/client/v4", server.uri()),
        Timeouts::default(),
    )
    .unwrap();

    let error = client
        .create_tunnel_cname(ZONE_ID, "app.example.com", TUNNEL_ID, None, &Cancel::new())
        .await
        .unwrap_err();

    assert!(
        !format!("{error:?}").contains(token),
        "the Debug output leaked the token"
    );
    assert!(!error.user_message().contains(token));
    assert!(!error.detail().unwrap_or_default().contains(token));
    assert!(!error.to_string().contains(token));
}

#[tokio::test]
async fn the_token_never_appears_in_the_client_debug_output() {
    let server = MockServer::start().await;
    let token = "DiStInCtIvE-tOkEn-0123456789-abcdefghij";
    let client = CloudflareClient::with_base_url(
        SecretString::from(token),
        format!("{}/client/v4", server.uri()),
        Timeouts::default(),
    )
    .unwrap();

    let debug = format!("{client:?}");

    assert!(!debug.contains(token), "got {debug}");
    assert!(debug.contains("[redacted]"));
}
