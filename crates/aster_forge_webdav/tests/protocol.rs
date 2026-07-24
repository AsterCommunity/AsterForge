use std::time::{Duration, UNIX_EPOCH};

use aster_forge_webdav::{
    DAV_ALLOW_HEADER, DavBodyPolicy, DavMethod, DavPath, DavPathError, DavPrecondition,
    DavRequestHead, DavRequestOrigin, Depth, IfStateCondition, child_relative_path,
    destination_relative_path, evaluate_http_download_preconditions,
    evaluate_http_etag_preconditions, href_for_relative, parent_relative_path, parse_copy_depth,
    parse_delete_depth, parse_if_header, parse_lock_depth, parse_lock_timeout,
    parse_lock_token_header, parse_move_depth, parse_propfind_depth, submitted_lock_tokens,
    submitted_lock_tokens_for_path,
};
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Uri};

fn headers(name: &'static str, value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_bytes(name.as_bytes()).expect("test header name should be valid"),
        HeaderValue::from_static(value),
    );
    headers
}

#[test]
fn dav_path_canonicalizes_dot_segments_and_rejects_escape() {
    let path = DavPath::new("/projects/./docs/reports/../q1.txt")
        .expect("internal dot segments should normalize");
    assert_eq!(path.as_str(), "/projects/docs/q1.txt");

    let collection = DavPath::new("/projects/docs/reports/%2e%2e")
        .expect("encoded internal parent should normalize");
    assert_eq!(collection.as_str(), "/projects/docs/");
    assert!(collection.is_collection());

    assert!(matches!(
        DavPath::new("/%2e%2e/secret.txt"),
        Err(DavPathError::PathEscape)
    ));
}

#[test]
fn dav_path_preserves_collection_aliases_and_internal_parent_segments() {
    for value in [
        "/projects/docs/.",
        "/projects/docs/reports/..",
        "/projects/docs/reports/%2e%2e",
    ] {
        let path = DavPath::new(value).expect("collection alias should normalize");
        assert_eq!(path.as_str(), "/projects/docs/");
        assert!(path.is_collection());
    }

    let path = DavPath::new("/projects/docs/%2e%2e/manuals/file.txt")
        .expect("internal encoded parent should normalize");
    assert_eq!(path.as_str(), "/projects/manuals/file.txt");
}

#[test]
fn dav_path_rejects_every_root_escape_shape() {
    for value in [
        "/../secret.txt",
        "/projects/../../secret.txt",
        "/%2e%2e/secret.txt",
    ] {
        assert!(matches!(DavPath::new(value), Err(DavPathError::PathEscape)));
    }
}

#[test]
fn href_and_relative_path_helpers_preserve_collection_semantics() {
    assert_eq!(
        href_for_relative("/webdav", "/folder/file name%2B.txt"),
        "/webdav/folder/file%20name%252B.txt"
    );
    assert_eq!(
        child_relative_path("/folder/", b"child", true),
        "/folder/child/"
    );
    assert_eq!(
        parent_relative_path("/folder/child/"),
        Some("/folder/".to_string())
    );
    assert_eq!(parent_relative_path("/file.txt"), Some("/".to_string()));
    assert_eq!(parent_relative_path("/"), None);
}

#[test]
fn method_parser_recognizes_webdav_extensions() {
    assert_eq!(DavMethod::from_method(&Method::GET), Some(DavMethod::Get));
    assert_eq!(
        DavMethod::from_method(&Method::from_bytes(b"PROPFIND").expect("valid method")),
        Some(DavMethod::Propfind)
    );
    assert_eq!(DavMethod::from_method(&Method::PATCH), None);
}

#[test]
fn method_body_policies_keep_protocol_and_product_streaming_responsibilities_separate() {
    for method in [
        DavMethod::Options,
        DavMethod::Mkcol,
        DavMethod::Delete,
        DavMethod::Copy,
        DavMethod::Move,
        DavMethod::Unlock,
    ] {
        assert_eq!(method.body_policy(), DavBodyPolicy::Empty);
    }
    for method in [
        DavMethod::Propfind,
        DavMethod::Proppatch,
        DavMethod::Lock,
        DavMethod::Report,
    ] {
        assert_eq!(method.body_policy(), DavBodyPolicy::BoundedXml);
    }
    assert_eq!(DavMethod::Put.body_policy(), DavBodyPolicy::Stream);
    for method in [DavMethod::Get, DavMethod::Head, DavMethod::VersionControl] {
        assert_eq!(method.body_policy(), DavBodyPolicy::Unused);
    }
}

#[test]
fn advertised_methods_are_exactly_the_methods_recognized_by_the_protocol_layer() {
    let methods = DAV_ALLOW_HEADER.split(", ").collect::<Vec<_>>();
    assert_eq!(methods.len(), 14);
    for method in methods {
        assert!(DavMethod::from_name(method).is_some(), "{method}");
    }
}

#[test]
fn depth_rules_remain_method_specific() {
    assert_eq!(
        parse_propfind_depth(&HeaderMap::new()).expect("default depth"),
        Depth::Infinity
    );
    assert_eq!(
        parse_copy_depth(&headers("Depth", "0")).expect("copy depth zero"),
        Depth::Zero
    );
    assert!(parse_copy_depth(&headers("Depth", "1")).is_err());
    assert_eq!(
        parse_lock_depth(&HeaderMap::new()).expect("default lock depth"),
        Depth::Infinity
    );
    assert!(parse_lock_depth(&headers("Depth", "1")).is_err());
    assert_eq!(
        parse_move_depth(&headers("Depth", "0")).expect("move depth"),
        Depth::Zero
    );
    assert_eq!(
        parse_delete_depth(&headers("Depth", "1")).expect("delete depth"),
        Depth::One
    );
}

#[test]
fn depth_values_are_case_insensitive_but_not_whitespace_tolerant() {
    assert_eq!(
        parse_propfind_depth(&headers("Depth", "Infinity")).expect("case-insensitive depth"),
        Depth::Infinity
    );
    assert!(parse_propfind_depth(&headers("Depth", "")).is_err());
    assert!(parse_propfind_depth(&headers("Depth", " infinity ")).is_err());

    let mut non_utf8 = HeaderMap::new();
    non_utf8.insert(
        HeaderName::from_static("depth"),
        HeaderValue::from_bytes(&[0xff]).expect("test header value"),
    );
    assert!(parse_propfind_depth(&non_utf8).is_err());
}

#[test]
fn destination_is_same_origin_and_mount_scoped() {
    let relative = destination_relative_path(
        &headers("Destination", "/webdav/folder/file%20name.txt"),
        "/webdav",
        "https",
        "dav.example",
    )
    .expect("relative destination under mount should parse");
    assert_eq!(relative.relative, "/folder/file name.txt");

    let absolute = destination_relative_path(
        &headers(
            "Destination",
            "HTTPS://DAV.EXAMPLE/webdav/folder/file%20name.txt",
        ),
        "/webdav",
        "https",
        "dav.example",
    )
    .expect("matching absolute destination should parse");
    assert_eq!(absolute.path.as_str(), "/folder/file name.txt");

    let cross_origin = destination_relative_path(
        &headers("Destination", "https://other.example/webdav/file.txt"),
        "/webdav",
        "https",
        "dav.example",
    )
    .expect_err("cross-origin destination should fail");
    assert_eq!(
        cross_origin.message(),
        "Destination must stay on this WebDAV server"
    );

    let outside = destination_relative_path(
        &headers("Destination", "/webdavish/file.txt"),
        "/webdav",
        "https",
        "dav.example",
    )
    .expect_err("outside mount destination should fail");
    assert_eq!(
        outside.message(),
        "Destination must stay under WebDAV prefix"
    );
}

#[test]
fn destination_reports_stable_missing_and_malformed_errors() {
    let missing = destination_relative_path(&HeaderMap::new(), "/webdav", "https", "dav.example")
        .expect_err("missing destination should fail");
    assert_eq!(missing.message(), "Missing Destination header");

    let malformed = destination_relative_path(
        &headers("Destination", "not-an-absolute-path"),
        "/webdav",
        "https",
        "dav.example",
    )
    .expect_err("relative reference should fail");
    assert_eq!(malformed.message(), "Invalid Destination header");
}

#[test]
fn if_header_preserves_tagged_groups_not_and_etags() {
    let parsed = parse_if_header(&headers(
        "If",
        r#"</webdav/a.txt> (<urn:uuid:a> ["etag-a"]) </webdav/b.txt> (Not <urn:uuid:b>)"#,
    ))
    .expect("If header should parse")
    .expect("If header should exist");

    assert_eq!(parsed.groups.len(), 2);
    assert_eq!(
        parsed.groups[0].tagged_path.as_deref(),
        Some("/webdav/a.txt")
    );
    assert_eq!(
        parsed.groups[1].lists[0].conditions,
        [IfStateCondition::Token {
            value: "urn:uuid:b".to_string(),
            negated: true,
        }]
    );
}

#[test]
fn if_header_rejects_ambiguous_or_empty_grammar() {
    for value in [
        "()",
        r#"(Notified <urn:uuid:one>)"#,
        r#"(<urn:uuid:one>) </webdav/file.txt> (<urn:uuid:two>)"#,
        r#"</webdav/current.txt> (<urn:uuid:current>"#,
    ] {
        assert!(parse_if_header(&headers("If", value)).is_err());
    }
}

#[test]
fn if_header_accepts_case_insensitive_not_and_groups_tagged_lists() {
    let parsed = parse_if_header(&headers(
        "If",
        r#"</webdav/a.txt> (<urn:uuid:a1>) (<urn:uuid:a2>) </webdav/b.txt> (nOt <urn:uuid:b>)"#,
    ))
    .expect("If header should parse")
    .expect("If header should exist");

    assert_eq!(parsed.groups.len(), 2);
    assert_eq!(parsed.groups[0].lists.len(), 2);
    assert_eq!(
        parsed.groups[1].lists[0].conditions,
        [IfStateCondition::Token {
            value: "urn:uuid:b".to_string(),
            negated: true,
        }]
    );
}

#[test]
fn submitted_tokens_apply_only_to_matching_tagged_resource() {
    let headers = headers(
        "If",
        r#"<http://localhost:8080/webdav/current.txt> (<urn:uuid:current>) <http://remote.example/webdav/current.txt> (<urn:uuid:remote>)"#,
    );
    assert_eq!(
        submitted_lock_tokens_for_path(&headers, "/webdav/current.txt", "http", "localhost:8080"),
        ["urn:uuid:current".to_string()]
    );
}

#[test]
fn submitted_tokens_are_decoded_deduplicated_and_include_negated_conditions() {
    let headers = headers(
        "If",
        r#"</webdav/current%20file.txt> (<urn:uuid:current>) (<urn:uuid:current>) (Not <urn:uuid:other>)"#,
    );
    assert_eq!(
        submitted_lock_tokens_for_path(&headers, "/webdav/current file.txt", "http", "localhost"),
        ["urn:uuid:current".to_string(), "urn:uuid:other".to_string()]
    );
}

#[test]
fn submitted_tokens_ignore_other_resources_and_lock_token_header() {
    let mut headers = headers(
        "If",
        r#"</webdav/other.txt> (<urn:uuid:other>) </webdav/current.txt> (<urn:uuid:current>)"#,
    );
    headers.insert(
        HeaderName::from_static("lock-token"),
        HeaderValue::from_static("<urn:uuid:header>"),
    );
    assert_eq!(
        submitted_lock_tokens_for_path(&headers, "/webdav/current.txt", "http", "localhost"),
        ["urn:uuid:current".to_string()]
    );
}

#[test]
fn submitted_tokens_from_parsed_if_header_preserve_scope_and_deduplicate() {
    let untagged = parse_if_header(&headers("If", r#"(<urn:uuid:untagged>)"#))
        .expect("untagged If header should parse")
        .expect("If header should exist");
    assert_eq!(
        submitted_lock_tokens(
            &untagged,
            "/webdav/current file.txt",
            "https",
            "dav.example"
        ),
        ["urn:uuid:untagged".to_string()]
    );

    let tagged = parse_if_header(&headers(
        "If",
        r#"</webdav/current%20file.txt> (<urn:uuid:current>) (<urn:uuid:current>) (Not <urn:uuid:negated>) </webdav/other.txt> (<urn:uuid:other>) <https://remote.example/webdav/current%20file.txt> (<urn:uuid:remote>)"#,
    ))
    .expect("If header should parse")
    .expect("If header should exist");

    assert_eq!(
        submitted_lock_tokens(&tagged, "/webdav/current file.txt", "https", "dav.example"),
        [
            "urn:uuid:current".to_string(),
            "urn:uuid:negated".to_string(),
        ]
    );
}

#[test]
fn lock_timeout_uses_bounded_server_policy() {
    let maximum = Duration::from_secs(604_800);
    assert_eq!(
        parse_lock_timeout(&HeaderMap::new(), maximum).expect("missing timeout should use maximum"),
        maximum
    );
    assert_eq!(
        parse_lock_timeout(&headers("Timeout", "Infinite"), maximum)
            .expect("Infinite should use maximum"),
        maximum
    );
    assert_eq!(
        parse_lock_timeout(&headers("Timeout", "Second-3600"), maximum)
            .expect("bounded timeout should parse"),
        Duration::from_secs(3600)
    );
    assert_eq!(
        parse_lock_timeout(&headers("Timeout", "Second-604800"), maximum)
            .expect("exact maximum should parse"),
        maximum
    );
    assert_eq!(
        parse_lock_timeout(&headers("Timeout", "Extension, Second-60"), maximum)
            .expect("unknown candidate should not hide a valid timeout"),
        Duration::from_secs(60)
    );

    for value in ["Second-604801", "Second-18446744073709551615", "Extension"] {
        assert!(
            parse_lock_timeout(&headers("Timeout", value), maximum).is_err(),
            "{value} should be rejected"
        );
    }

    let mut non_utf8 = HeaderMap::new();
    non_utf8.insert(
        HeaderName::from_static("timeout"),
        HeaderValue::from_bytes(&[0xff]).expect("test header value"),
    );
    assert!(parse_lock_timeout(&non_utf8, maximum).is_err());
}

#[test]
fn lock_token_header_requires_one_nonempty_angle_bracketed_token() {
    assert_eq!(
        parse_lock_token_header(&headers("Lock-Token", " <urn:uuid:lock> "))
            .expect("valid lock token should parse"),
        "urn:uuid:lock"
    );

    for value in ["", "<>", "urn:uuid:lock", "<<urn:uuid:lock>>", "<one><two>"] {
        assert!(
            parse_lock_token_header(&headers("Lock-Token", value)).is_err(),
            "{value:?} should be rejected"
        );
    }
    assert!(parse_lock_token_header(&HeaderMap::new()).is_err());

    let mut non_utf8 = HeaderMap::new();
    non_utf8.insert(
        HeaderName::from_static("lock-token"),
        HeaderValue::from_bytes(&[0xff]).expect("test header value"),
    );
    assert!(parse_lock_token_header(&non_utf8).is_err());
}

#[test]
fn etag_and_date_preconditions_keep_http_precedence() {
    let mut matching = HeaderMap::new();
    matching.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"v1\""));
    assert_eq!(
        evaluate_http_etag_preconditions(&matching, true, Some("\"v1\""), true)
            .expect("safe matching If-None-Match"),
        DavPrecondition::NotModified
    );
    assert!(evaluate_http_etag_preconditions(&matching, true, Some("\"v1\""), false).is_err());

    let modified = UNIX_EPOCH + Duration::from_secs(2_000_000);
    let mut download = HeaderMap::new();
    download.insert(
        header::IF_MODIFIED_SINCE,
        HeaderValue::from_static("Sat, 24 Jan 1970 03:33:20 GMT"),
    );
    assert_eq!(
        evaluate_http_download_preconditions(&download, None, Some(modified))
            .expect("valid date precondition"),
        DavPrecondition::NotModified
    );
}

#[test]
fn request_head_parses_method_specific_contract() {
    let method = DavMethod::from_method(
        &Method::from_bytes(b"COPY").expect("COPY should be a valid HTTP extension method"),
    )
    .expect("COPY should be supported");
    let uri: Uri = "/webdav/source.txt".parse().expect("valid request URI");
    let mut request_headers = headers("Destination", "/webdav/destination.txt");
    request_headers.insert("Depth", HeaderValue::from_static("0"));
    request_headers.insert("Overwrite", HeaderValue::from_static("F"));

    let request = DavRequestHead::parse(
        method,
        &uri,
        &request_headers,
        "/webdav",
        &DavRequestOrigin {
            scheme: "https".to_string(),
            host: "dav.example".to_string(),
        },
    )
    .expect("COPY request head should parse");

    assert_eq!(request.target.as_str(), "/source.txt");
    assert_eq!(request.depth, Some(Depth::Zero));
    assert_eq!(request.overwrite, Some(false));
    assert_eq!(
        request.destination.expect("destination").path.as_str(),
        "/destination.txt"
    );
}

#[test]
fn request_head_rejects_targets_outside_the_mount() {
    let uri: Uri = "/webdavish/source.txt".parse().expect("valid request URI");
    let error = DavRequestHead::parse(
        DavMethod::Get,
        &uri,
        &HeaderMap::new(),
        "/webdav",
        &DavRequestOrigin {
            scheme: "https".to_string(),
            host: "dav.example".to_string(),
        },
    )
    .expect_err("lookalike mount prefix must be rejected");
    assert_eq!(
        error.message(),
        "Request target must stay under WebDAV prefix"
    );
}

#[test]
fn request_head_accepts_a_root_mount() {
    let uri: Uri = "/folder/source.txt".parse().expect("valid request URI");
    let request = DavRequestHead::parse(
        DavMethod::Get,
        &uri,
        &HeaderMap::new(),
        "/",
        &DavRequestOrigin {
            scheme: "https".to_string(),
            host: "dav.example".to_string(),
        },
    )
    .expect("root-mounted WebDAV should accept descendant paths");
    assert_eq!(request.target.as_str(), "/folder/source.txt");
}
