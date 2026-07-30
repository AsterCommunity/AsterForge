use std::time::{Duration, UNIX_EPOCH};

use aster_forge_webdav::{
    DavBackendError, DavConditionalEvaluationError, DavConditionalOutcome, DavConditionalPlanError,
    DavConditionalResource, DavIfResourceState, DavIfStateResolver, DavMethod, DavPath,
    DavProtocolErrorKind, DavRangeEvaluation, plan_conditionals, plan_http_conditionals,
    protocol_error_response,
};
use async_trait::async_trait;
use http::header::{
    ETAG, HeaderName, IF_MATCH, IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_UNMODIFIED_SINCE,
    LAST_MODIFIED,
};
use http::{HeaderMap, HeaderValue, StatusCode};

fn modified() -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_secs(2_000_000)
}

fn mapped(etag: Option<&str>) -> DavConditionalResource<'_> {
    DavConditionalResource {
        exists: true,
        etag,
        last_modified: Some(modified()),
    }
}

fn headers(name: HeaderName, value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(name, HeaderValue::from_static(value));
    headers
}

#[test]
fn get_head_and_unsafe_methods_select_distinct_matching_outcomes() {
    let headers = headers(IF_NONE_MATCH, "\"v1\"");
    for method in [DavMethod::Get, DavMethod::Head] {
        let plan = plan_http_conditionals(method, &headers, mapped(Some("\"v1\"")))
            .expect("retrieval conditions");
        assert_eq!(plan.outcome, DavConditionalOutcome::NotModified);
        assert_eq!(plan.range, DavRangeEvaluation::Skip);
    }
    for method in [
        DavMethod::Put,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Lock,
    ] {
        let plan = plan_http_conditionals(method, &headers, mapped(Some("\"v1\"")))
            .expect("unsafe conditions");
        assert_eq!(plan.outcome, DavConditionalOutcome::PreconditionFailed);
        assert_eq!(plan.range, DavRangeEvaluation::Skip);
    }
}

#[test]
fn all_supported_methods_have_explicit_range_and_matching_semantics() {
    for method in [
        DavMethod::Options,
        DavMethod::Propfind,
        DavMethod::Proppatch,
        DavMethod::Get,
        DavMethod::Head,
        DavMethod::Put,
        DavMethod::Mkcol,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Lock,
        DavMethod::Unlock,
        DavMethod::Report,
        DavMethod::VersionControl,
    ] {
        let proceed = plan_http_conditionals(method, &HeaderMap::new(), mapped(Some("v1")))
            .expect("unconditional method plan");
        assert_eq!(proceed.outcome, DavConditionalOutcome::Proceed);
        assert_eq!(
            proceed.range,
            if method == DavMethod::Get {
                DavRangeEvaluation::Evaluate
            } else {
                DavRangeEvaluation::Skip
            },
            "{method:?}"
        );

        let matched = plan_http_conditionals(
            method,
            &headers(IF_NONE_MATCH, "\"v1\""),
            mapped(Some("v1")),
        )
        .expect("matching method plan");
        let expected = if matches!(method, DavMethod::Get | DavMethod::Head) {
            DavConditionalOutcome::NotModified
        } else {
            DavConditionalOutcome::PreconditionFailed
        };
        assert_eq!(matched.outcome, expected, "{method:?}");
        assert_eq!(matched.range, DavRangeEvaluation::Skip, "{method:?}");
    }
}

#[test]
fn wildcard_conditions_distinguish_mapped_and_unmapped_targets() {
    let if_match = headers(IF_MATCH, "*");
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_match, mapped(None))
            .expect("mapped wildcard")
            .outcome,
        DavConditionalOutcome::Proceed
    );
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_match, DavConditionalResource::missing())
            .expect("unmapped wildcard")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let if_none_match = headers(IF_NONE_MATCH, "*");
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_none_match, mapped(None))
            .expect("mapped none wildcard")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );
    assert_eq!(
        plan_http_conditionals(
            DavMethod::Put,
            &if_none_match,
            DavConditionalResource::missing(),
        )
        .expect("unmapped none wildcard")
        .outcome,
        DavConditionalOutcome::Proceed
    );
}

#[test]
fn empty_etag_lists_follow_rfc_zero_member_condition_semantics() {
    let if_match = headers(IF_MATCH, "");
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_match, mapped(Some("v1")))
            .expect("empty If-Match list")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let if_none_match = headers(IF_NONE_MATCH, "");
    assert_eq!(
        plan_http_conditionals(DavMethod::Get, &if_none_match, mapped(Some("v1")))
            .expect("empty If-None-Match list")
            .outcome,
        DavConditionalOutcome::Proceed
    );
}

#[test]
fn etag_lists_use_strong_if_match_and_weak_if_none_match_comparison() {
    let strong = headers(IF_MATCH, "W/\"v1\", \"v2\"");
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &strong, mapped(Some("\"v1\"")))
            .expect("valid strong list")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );
    let matching_strong = headers(IF_MATCH, "W/\"other\", \"v1\"");
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &matching_strong, mapped(Some("\"v1\"")),)
            .expect("matching strong list")
            .outcome,
        DavConditionalOutcome::Proceed
    );
    assert_eq!(
        plan_http_conditionals(
            DavMethod::Put,
            &headers(IF_MATCH, "\"v1\""),
            mapped(Some("W/\"v1\"")),
        )
        .expect("weak current validator")
        .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let weak = headers(IF_NONE_MATCH, "W/\"v1\", \"v2\"");
    assert_eq!(
        plan_http_conditionals(DavMethod::Get, &weak, mapped(Some("\"v1\"")))
            .expect("valid weak list")
            .outcome,
        DavConditionalOutcome::NotModified
    );
}

#[test]
fn etag_headers_suppress_their_date_fallbacks() {
    let mut if_match = headers(IF_MATCH, "\"v1\"");
    if_match.insert(IF_UNMODIFIED_SINCE, HeaderValue::from_static("not a date"));
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_match, mapped(Some("\"v1\"")))
            .expect("If-Match suppresses date parsing")
            .outcome,
        DavConditionalOutcome::Proceed
    );

    let mut if_none_match = headers(IF_NONE_MATCH, "\"different\"");
    if_none_match.insert(IF_MODIFIED_SINCE, HeaderValue::from_static("not a date"));
    assert_eq!(
        plan_http_conditionals(DavMethod::Get, &if_none_match, mapped(Some("\"v1\"")),)
            .expect("If-None-Match suppresses date parsing")
            .outcome,
        DavConditionalOutcome::Proceed
    );
}

#[test]
fn earlier_failed_preconditions_short_circuit_later_invalid_fields() {
    let mut if_match_headers = headers(IF_MATCH, "\"different\"");
    if_match_headers.insert(IF_NONE_MATCH, HeaderValue::from_static("bare-etag"));
    assert_eq!(
        plan_http_conditionals(DavMethod::Put, &if_match_headers, mapped(Some("v1")))
            .expect("failed If-Match short-circuits If-None-Match parsing")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let mut date_headers = headers(IF_UNMODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT");
    date_headers.insert(IF_NONE_MATCH, HeaderValue::from_static("bare-etag"));
    assert_eq!(
        plan_http_conditionals(DavMethod::Delete, &date_headers, mapped(Some("v1")))
            .expect("failed date short-circuits If-None-Match parsing")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );
}

#[test]
fn invalid_or_repeated_date_fields_are_ignored_per_rfc_9110() {
    for name in [IF_UNMODIFIED_SINCE, IF_MODIFIED_SINCE] {
        let invalid = headers(name.clone(), "not a date");
        assert_eq!(
            plan_http_conditionals(DavMethod::Get, &invalid, mapped(Some("v1")))
                .expect("invalid HTTP-date is ignored")
                .outcome,
            DavConditionalOutcome::Proceed
        );

        let mut repeated = HeaderMap::new();
        repeated.append(
            name.clone(),
            HeaderValue::from_static("Thu, 01 Jan 1970 00:00:00 GMT"),
        );
        repeated.append(
            name,
            HeaderValue::from_static("Sat, 24 Jan 1970 03:33:20 GMT"),
        );
        assert_eq!(
            plan_http_conditionals(DavMethod::Get, &repeated, mapped(Some("v1")))
                .expect("date lists are ignored")
                .outcome,
            DavConditionalOutcome::Proceed
        );
    }

    let without_modified = DavConditionalResource {
        exists: true,
        etag: Some("v1"),
        last_modified: None,
    };
    assert_eq!(
        plan_http_conditionals(
            DavMethod::Put,
            &headers(IF_UNMODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT",),
            without_modified,
        )
        .expect("missing modification date ignores the condition")
        .outcome,
        DavConditionalOutcome::Proceed
    );

    for name in [IF_UNMODIFIED_SINCE, IF_MODIFIED_SINCE] {
        let mut non_utf8 = HeaderMap::new();
        non_utf8.insert(
            name,
            HeaderValue::from_bytes(&[0xff]).expect("opaque date field value"),
        );
        assert_eq!(
            plan_http_conditionals(DavMethod::Get, &non_utf8, mapped(Some("v1")))
                .expect("non-UTF-8 HTTP-date is ignored")
                .outcome,
            DavConditionalOutcome::Proceed
        );
    }
}

#[test]
fn unsafe_methods_apply_if_unmodified_since_without_if_match() {
    let headers = headers(IF_UNMODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT");
    for method in [
        DavMethod::Put,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Lock,
    ] {
        assert_eq!(
            plan_http_conditionals(method, &headers, mapped(Some("\"v1\"")))
                .expect("valid date")
                .outcome,
            DavConditionalOutcome::PreconditionFailed,
            "{method:?}"
        );
    }
}

#[test]
fn retrieval_methods_return_not_modified_for_a_fresh_if_modified_since() {
    let request_headers = headers(IF_MODIFIED_SINCE, "Sat, 24 Jan 1970 03:33:20 GMT");
    for method in [DavMethod::Get, DavMethod::Head] {
        let plan = plan_http_conditionals(method, &request_headers, mapped(Some("v1")))
            .expect("valid If-Modified-Since");
        assert_eq!(plan.outcome, DavConditionalOutcome::NotModified);
        assert_eq!(plan.range, DavRangeEvaluation::Skip);
    }

    let request_headers = headers(IF_MODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT");
    for method in [DavMethod::Get, DavMethod::Head] {
        let plan = plan_http_conditionals(method, &request_headers, mapped(Some("v1")))
            .expect("stale If-Modified-Since");
        assert_eq!(plan.outcome, DavConditionalOutcome::Proceed);
        assert_eq!(
            plan.range,
            if method == DavMethod::Get {
                DavRangeEvaluation::Evaluate
            } else {
                DavRangeEvaluation::Skip
            }
        );
    }
}

#[test]
fn http_dates_compare_at_one_second_resolution_and_validate_metadata_only_when_needed() {
    let exact = DavConditionalResource {
        exists: true,
        etag: None,
        last_modified: Some(UNIX_EPOCH + Duration::from_millis(1_999)),
    };
    assert_eq!(
        plan_http_conditionals(
            DavMethod::Put,
            &headers(IF_UNMODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:01 GMT",),
            exact,
        )
        .expect("subsecond values use HTTP-date resolution")
        .outcome,
        DavConditionalOutcome::Proceed
    );

    let pre_epoch = DavConditionalResource {
        exists: true,
        etag: Some("v1"),
        last_modified: Some(UNIX_EPOCH - Duration::from_secs(2)),
    };
    let plan = plan_http_conditionals(DavMethod::Get, &HeaderMap::new(), pre_epoch)
        .expect("unconditional requests skip an unrenderable date");
    assert_eq!(plan.outcome, DavConditionalOutcome::Proceed);
    let mut response_headers = HeaderMap::new();
    plan.apply_response_headers(StatusCode::OK, &mut response_headers);
    assert_eq!(response_headers.get(ETAG).expect("valid ETag"), "\"v1\"");
    assert!(response_headers.get(LAST_MODIFIED).is_none());
    assert!(matches!(
        plan_http_conditionals(
            DavMethod::Get,
            &headers(IF_MODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT"),
            pre_epoch,
        ),
        Err(DavConditionalPlanError::InvalidRepresentation)
    ));

    let after_http_date_range = DavConditionalResource {
        exists: true,
        etag: Some("invalid\netag"),
        last_modified: Some(UNIX_EPOCH + Duration::from_hours(70_389_528)),
    };
    let plan = plan_http_conditionals(DavMethod::Get, &HeaderMap::new(), after_http_date_range)
        .expect("unconditional requests skip all unrenderable validators");
    let mut response_headers = HeaderMap::new();
    plan.apply_response_headers(StatusCode::OK, &mut response_headers);
    assert!(response_headers.is_empty());
    assert!(matches!(
        plan_http_conditionals(
            DavMethod::Put,
            &headers(IF_UNMODIFIED_SINCE, "Thu, 01 Jan 1970 00:00:00 GMT"),
            after_http_date_range,
        ),
        Err(DavConditionalPlanError::InvalidRepresentation)
    ));
}

#[test]
fn etag_metadata_is_validated_only_for_conditions_that_need_the_current_tag() {
    let resource = DavConditionalResource {
        exists: true,
        etag: Some("invalid\netag"),
        last_modified: Some(modified()),
    };
    let plan = plan_http_conditionals(DavMethod::Get, &HeaderMap::new(), resource)
        .expect("unconditional requests skip an invalid ETag");
    let mut response_headers = HeaderMap::new();
    plan.apply_response_headers(StatusCode::OK, &mut response_headers);
    assert!(response_headers.get(ETAG).is_none());
    assert_eq!(
        response_headers
            .get(LAST_MODIFIED)
            .expect("valid Last-Modified"),
        "Sat, 24 Jan 1970 03:33:20 GMT"
    );

    for name in [IF_MATCH, IF_NONE_MATCH] {
        assert!(matches!(
            plan_http_conditionals(DavMethod::Get, &headers(name, "\"candidate\""), resource),
            Err(DavConditionalPlanError::InvalidRepresentation)
        ));
    }

    for (name, value) in [(IF_MATCH, "*"), (IF_NONE_MATCH, "*"), (IF_MATCH, "")] {
        assert!(
            plan_http_conditionals(DavMethod::Get, &headers(name.clone(), value), resource).is_ok(),
            "{name} {value:?} does not inspect the current ETag"
        );
    }
}

#[test]
fn opaque_backend_etags_with_weak_marker_text_remain_opaque() {
    let resource = mapped(Some("W/backend-value"));
    let plan = plan_http_conditionals(
        DavMethod::Get,
        &headers(IF_MATCH, "\"W/backend-value\""),
        resource,
    )
    .expect("opaque backend ETag");
    assert_eq!(plan.outcome, DavConditionalOutcome::Proceed);

    let mut response_headers = HeaderMap::new();
    plan.apply_response_headers(StatusCode::OK, &mut response_headers);
    assert_eq!(
        response_headers.get(ETAG).expect("ETag"),
        "\"W/backend-value\""
    );
}

#[test]
fn invalid_etag_lists_are_stable_bad_requests_and_obs_text_is_preserved() {
    for (name, value) in [(IF_MATCH, "bare-etag"), (IF_NONE_MATCH, "*, \"v1\"")] {
        let error =
            plan_http_conditionals(DavMethod::Get, &headers(name, value), mapped(Some("v1")))
                .expect_err("malformed request header");
        assert!(matches!(
            error,
            DavConditionalPlanError::Protocol(error)
                if error.kind() == DavProtocolErrorKind::BadRequest
        ));
    }

    let mut obs_text = HeaderMap::new();
    obs_text.insert(
        IF_MATCH,
        HeaderValue::from_bytes(&[b'"', 0xff, b'"']).expect("obs-text entity-tag"),
    );
    assert_eq!(
        plan_http_conditionals(DavMethod::Get, &obs_text, mapped(Some("v1")))
            .expect("RFC 9110 entity-tag allows obs-text")
            .outcome,
        DavConditionalOutcome::PreconditionFailed
    );

    let mut malformed_bytes = HeaderMap::new();
    malformed_bytes.insert(
        IF_MATCH,
        HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
    );
    assert!(matches!(
        plan_http_conditionals(DavMethod::Get, &malformed_bytes, mapped(Some("v1"))),
        Err(DavConditionalPlanError::Protocol(error))
            if error.kind() == DavProtocolErrorKind::BadRequest
    ));
}

#[test]
fn response_validator_contract_covers_success_and_conditional_statuses() {
    let plan = plan_http_conditionals(DavMethod::Get, &HeaderMap::new(), mapped(Some("W/\"v1\"")))
        .expect("conditional plan");
    for status in [
        StatusCode::OK,
        StatusCode::PARTIAL_CONTENT,
        StatusCode::NOT_MODIFIED,
        StatusCode::PRECONDITION_FAILED,
        StatusCode::RANGE_NOT_SATISFIABLE,
    ] {
        let mut response_headers = HeaderMap::new();
        plan.apply_response_headers(status, &mut response_headers);
        assert_eq!(response_headers.get(ETAG).expect("ETag"), "W/\"v1\"");
        assert_eq!(
            response_headers.get(LAST_MODIFIED).expect("Last-Modified"),
            "Sat, 24 Jan 1970 03:33:20 GMT"
        );
    }

    let mut created_headers = HeaderMap::new();
    plan.apply_response_headers(StatusCode::CREATED, &mut created_headers);
    assert!(created_headers.is_empty());
}

struct FixedResolver(DavIfResourceState);

#[async_trait]
impl DavIfStateResolver for FixedResolver {
    async fn resolve_if_state(
        &self,
        _path: &DavPath,
    ) -> Result<DavIfResourceState, DavBackendError> {
        Ok(self.0.clone())
    }
}

struct FailingResolver;

#[async_trait]
impl DavIfStateResolver for FailingResolver {
    async fn resolve_if_state(
        &self,
        _path: &DavPath,
    ) -> Result<DavIfResourceState, DavBackendError> {
        Err(DavBackendError::new(
            aster_forge_webdav::DavBackendErrorKind::Internal,
        ))
    }
}

#[test]
fn webdav_if_runs_before_http_conditionals_through_the_combined_entrypoint() {
    let if_header = aster_forge_webdav::parse_if_header(&headers(
        HeaderName::from_static("if"),
        "([\"required\"])",
    ))
    .expect("If grammar")
    .expect("If header");
    let request_path = DavPath::new("/file.txt").expect("path");
    let invalid_http = headers(IF_MATCH, "bare-etag");

    let failing_resolver = FixedResolver(DavIfResourceState {
        etag: Some("different".to_owned()),
        lock_tokens: Vec::new(),
    });
    let error = futures::executor::block_on(plan_conditionals(
        Some(&if_header),
        &failing_resolver,
        &request_path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Put,
        &invalid_http,
        mapped(Some("required")),
    ))
    .expect_err("WebDAV If should fail first");
    assert!(matches!(
        &error,
        DavConditionalEvaluationError::Protocol(error)
            if error.kind() == DavProtocolErrorKind::PreconditionFailed
    ));
    let response = match &error {
        DavConditionalEvaluationError::Protocol(error) => protocol_error_response(error),
        _ => panic!("expected protocol precondition failure"),
    };
    assert_eq!(response.status, StatusCode::PRECONDITION_FAILED);
    assert_eq!(response.headers.get("Cache-Control").unwrap(), "no-store");

    let passing_resolver = FixedResolver(DavIfResourceState {
        etag: Some("required".to_owned()),
        lock_tokens: Vec::new(),
    });
    let error = futures::executor::block_on(plan_conditionals(
        Some(&if_header),
        &passing_resolver,
        &request_path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Put,
        &invalid_http,
        mapped(Some("required")),
    ))
    .expect_err("HTTP condition should run after WebDAV If passes");
    assert!(matches!(
        error,
        DavConditionalEvaluationError::Protocol(error)
            if error.kind() == DavProtocolErrorKind::BadRequest
    ));
}

#[test]
fn combined_entrypoint_preserves_backend_errors() {
    let if_header = aster_forge_webdav::parse_if_header(&headers(
        HeaderName::from_static("if"),
        "([\"required\"])",
    ))
    .expect("If grammar")
    .expect("If header");
    let error = futures::executor::block_on(plan_conditionals(
        Some(&if_header),
        &FailingResolver,
        &DavPath::new("/file.txt").expect("path"),
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Put,
        &HeaderMap::new(),
        mapped(Some("required")),
    ))
    .expect_err("backend failure should remain classified");
    assert!(matches!(
        error,
        DavConditionalEvaluationError::Backend(DavBackendError {
            kind: aster_forge_webdav::DavBackendErrorKind::Internal
        })
    ));
}

#[test]
fn tagged_webdav_if_and_request_target_http_conditions_keep_separate_scopes() {
    let if_header = aster_forge_webdav::parse_if_header(&headers(
        HeaderName::from_static("if"),
        "</webdav/destination.txt> ([\"destination\"])",
    ))
    .expect("If grammar")
    .expect("If header");
    let request_path = DavPath::new("/source.txt").expect("path");
    let resolver = FixedResolver(DavIfResourceState {
        etag: Some("destination".to_owned()),
        lock_tokens: Vec::new(),
    });

    let plan = futures::executor::block_on(plan_conditionals(
        Some(&if_header),
        &resolver,
        &request_path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Copy,
        &headers(IF_MATCH, "\"source\""),
        mapped(Some("source")),
    ))
    .expect("tagged destination and request target both match");
    assert_eq!(plan.outcome, DavConditionalOutcome::Proceed);

    let plan = futures::executor::block_on(plan_conditionals(
        Some(&if_header),
        &resolver,
        &request_path,
        "/webdav",
        "https",
        "dav.example",
        DavMethod::Copy,
        &headers(IF_MATCH, "\"other-source\""),
        mapped(Some("source")),
    ))
    .expect("HTTP condition is evaluated against the request target");
    assert_eq!(plan.outcome, DavConditionalOutcome::PreconditionFailed);
}
