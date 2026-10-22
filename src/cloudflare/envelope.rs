//! Cloudflare's v4 response envelope.
//!
//! Every v4 endpoint wraps its payload in `{success, errors, messages, result}`,
//! but the *shape of the parts* is not uniform. `documentation_url` and
//! `source.pointer` exist on the newer endpoints and not the older ones;
//! `error_chain`, `type` and `meta` are returned by the live API without being
//! declared anywhere; `result_info.total_pages` is present on `/zones` and
//! `/dns_records` and absent on `/accounts`, `/cfd_tunnel` and `/connections`.
//!
//! So this models the **superset**, everything optional but `code` and
//! `message`, and nothing anywhere uses `deny_unknown_fields`. A field
//! Cloudflare adds next month must not turn into a parse failure in a user's
//! hands.

use serde::{Deserialize, Serialize};

/// One entry of `errors` or `messages`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiMessage {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ApiErrorSource>,
    /// Upstream detail for errors that wrap a downstream service failure.
    /// Recursive, and sparsely declared in Cloudflare's own schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub error_chain: Vec<ApiMessage>,
}

impl ApiMessage {
    /// The message, plus any upstream detail, on one line.
    pub fn full_text(&self) -> String {
        let mut text = self.message.clone();
        for link in &self.error_chain {
            text.push_str(" — ");
            text.push_str(&link.message);
        }
        text
    }
}

impl std::fmt::Display for ApiMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.full_text(), self.code)
    }
}

/// A JSON Pointer at the request field Cloudflare objected to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ApiErrorSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
}

/// The pagination block on a list response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResultInfo {
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
    /// How many items are in *this* page.
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub total_count: Option<u32>,
    /// Absent on `/accounts`, `/cfd_tunnel` and `/connections`.
    #[serde(default)]
    pub total_pages: Option<u32>,
}

impl ResultInfo {
    /// Whether another page exists.
    ///
    /// `total_pages` is used when Cloudflare sends it and derived from
    /// `total_count` when it does not — which is the case on three of the
    /// endpoints this application calls, so the fallback is the normal path,
    /// not an edge case.
    pub fn has_more(&self) -> bool {
        let page = self.page.unwrap_or(1);

        if let Some(total_pages) = self.total_pages {
            return page < total_pages;
        }

        match (self.total_count, self.per_page) {
            (Some(total), Some(per_page)) if per_page > 0 => {
                u64::from(page) * u64::from(per_page) < u64::from(total)
            }
            // Without either signal, fall back to "a full page means there may
            // be more". `count < per_page` is a reliable last page.
            _ => match (self.count, self.per_page) {
                (Some(count), Some(per_page)) => count >= per_page && count > 0,
                _ => false,
            },
        }
    }

    pub fn next_page(&self) -> u32 {
        self.page.unwrap_or(1).saturating_add(1)
    }
}

/// The envelope itself.
///
/// `success` is `Option` because `DELETE /zones/{id}/dns_records/{id}` is
/// documented as returning `result` alone. HTTP status is the authority; this
/// field only refines the message.
#[derive(Debug, Clone, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub errors: Vec<ApiMessage>,
    #[serde(default)]
    pub messages: Vec<ApiMessage>,
    #[serde(default = "default_none")]
    pub result: Option<T>,
    #[serde(default)]
    pub result_info: Option<ResultInfo>,
}

fn default_none<T>() -> Option<T> {
    None
}

impl<T> Envelope<T> {
    pub fn is_success(&self) -> bool {
        self.success.unwrap_or(true) && self.errors.is_empty()
    }

    /// The first error's code, which is what the error mapper branches on for
    /// wording — never for control flow, because code `10000` means both
    /// "authentication failed" and "that field is required" depending on the
    /// endpoint.
    pub fn first_code(&self) -> Option<i64> {
        self.errors.first().map(|error| error.code)
    }

    pub fn error_text(&self) -> String {
        if self.errors.is_empty() {
            return String::new();
        }
        self.errors
            .iter()
            .map(ApiMessage::full_text)
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_minimal_envelope() {
        let envelope: Envelope<Vec<String>> =
            serde_json::from_str(r#"{"success":true,"errors":[],"messages":[],"result":[]}"#)
                .unwrap();
        assert!(envelope.is_success());
        assert_eq!(envelope.result.unwrap().len(), 0);
        assert!(envelope.result_info.is_none());
    }

    #[test]
    fn parses_the_newer_error_envelope_with_documentation_and_source() {
        let envelope: Envelope<serde_json::Value> = serde_json::from_str(
            r#"{
              "success": false,
              "errors": [{
                "code": 10000,
                "message": "domain_name is required",
                "documentation_url": "https://developers.cloudflare.com/x",
                "source": {"pointer": "/domain_name"}
              }],
              "messages": [],
              "result": null
            }"#,
        )
        .unwrap();

        assert!(!envelope.is_success());
        let error = &envelope.errors[0];
        assert_eq!(error.code, 10000);
        assert_eq!(
            error.source.as_ref().unwrap().pointer.as_deref(),
            Some("/domain_name")
        );
        assert!(error.documentation_url.is_some());
    }

    #[test]
    fn parses_an_error_chain() {
        let envelope: Envelope<serde_json::Value> = serde_json::from_str(
            r#"{"success":false,"errors":[{"code":1000,"message":"outer",
                 "error_chain":[{"code":1001,"message":"inner"}]}],
                 "messages":[],"result":null}"#,
        )
        .unwrap();
        assert_eq!(envelope.errors[0].full_text(), "outer — inner");
    }

    #[test]
    fn tolerates_undeclared_fields() {
        // `type` and `meta` are returned but declared nowhere; a strict
        // deserializer would fail on a perfectly good response.
        let envelope: Envelope<serde_json::Value> = serde_json::from_str(
            r#"{"success":true,"errors":[],
                "messages":[{"code":10000,"message":"This API Token is valid and active",
                             "type":null,"meta":null}],
                "result":{"id":"x"},
                "brand_new_field":42}"#,
        )
        .unwrap();
        assert!(envelope.is_success());
        assert_eq!(envelope.messages[0].code, 10000);
    }

    #[test]
    fn tolerates_a_response_with_only_a_result() {
        // DELETE /zones/{z}/dns_records/{id} is documented this way.
        let envelope: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"result":{"id":"abc"}}"#).unwrap();
        assert!(envelope.is_success(), "absent `success` is not a failure");
        assert_eq!(envelope.result.unwrap()["id"], "abc");
    }

    #[test]
    fn tolerates_a_null_result() {
        let envelope: Envelope<Vec<String>> =
            serde_json::from_str(r#"{"success":true,"errors":[],"messages":[],"result":null}"#)
                .unwrap();
        assert!(envelope.result.is_none());
    }

    #[test]
    fn pagination_uses_total_pages_when_present() {
        let info = ResultInfo {
            page: Some(1),
            per_page: Some(20),
            count: Some(20),
            total_count: Some(45),
            total_pages: Some(3),
        };
        assert!(info.has_more());
        assert_eq!(info.next_page(), 2);

        let last = ResultInfo {
            page: Some(3),
            total_pages: Some(3),
            ..info
        };
        assert!(!last.has_more());
    }

    #[test]
    fn pagination_derives_from_total_count_when_total_pages_is_absent() {
        // This is the /cfd_tunnel and /accounts case.
        let first = ResultInfo {
            page: Some(1),
            per_page: Some(20),
            count: Some(20),
            total_count: Some(45),
            total_pages: None,
        };
        assert!(first.has_more());

        let second = ResultInfo {
            page: Some(2),
            ..first
        };
        assert!(second.has_more(), "2 * 20 = 40 < 45");

        let third = ResultInfo {
            page: Some(3),
            count: Some(5),
            ..first
        };
        assert!(!third.has_more(), "3 * 20 = 60 >= 45");
    }

    #[test]
    fn pagination_falls_back_to_a_short_page_meaning_the_end() {
        let short = ResultInfo {
            page: Some(1),
            per_page: Some(20),
            count: Some(7),
            total_count: None,
            total_pages: None,
        };
        assert!(!short.has_more());

        let full = ResultInfo {
            count: Some(20),
            ..short
        };
        assert!(full.has_more());
    }

    #[test]
    fn pagination_of_an_empty_result_stops() {
        let empty = ResultInfo {
            page: Some(1),
            per_page: Some(20),
            count: Some(0),
            total_count: Some(0),
            total_pages: None,
        };
        assert!(!empty.has_more());
    }

    #[test]
    fn missing_result_info_is_not_a_parse_error() {
        let envelope: Envelope<Vec<String>> =
            serde_json::from_str(r#"{"success":true,"errors":[],"messages":[],"result":["a"]}"#)
                .unwrap();
        assert!(envelope.result_info.is_none());
    }
}
