use aster_forge_webdav::{
    DavPropfindRequest, DavRequestedProperty, DavResponseBody, DavXmlError, build_propfind_item,
    build_proppatch_item, dav_property_name_element, dav_property_text_element,
    format_creation_date, plan_atomic_proppatch, property_multistatus_response,
    propfind_finite_depth_response, propfind_xml_error_response, proppatch_xml_error_response,
};
use http::StatusCode;
use std::time::{Duration, UNIX_EPOCH};

#[test]
fn creation_date_uses_rfc3339_utc_format() {
    assert_eq!(
        format_creation_date(UNIX_EPOCH + Duration::from_secs(784_111_777)),
        "1994-11-06T08:49:37+00:00"
    );
}

#[test]
fn atomic_proppatch_plan_selects_success_forbidden_and_failed_dependency() {
    let success = plan_atomic_proppatch([false, false]);
    assert!(success.apply);
    assert_eq!(success.statuses, [StatusCode::OK, StatusCode::OK]);

    let rejected = plan_atomic_proppatch([false, true, false]);
    assert!(!rejected.apply);
    assert_eq!(
        rejected.statuses,
        [
            StatusCode::FAILED_DEPENDENCY,
            StatusCode::FORBIDDEN,
            StatusCode::FAILED_DEPENDENCY,
        ]
    );

    let empty = plan_atomic_proppatch([]);
    assert!(empty.apply);
    assert!(empty.statuses.is_empty());
}

fn property(name: &str, namespace: Option<&str>, prefix: Option<&str>) -> DavRequestedProperty {
    DavRequestedProperty {
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        prefix: prefix.map(str::to_string),
    }
}

fn resolve(
    requested: &DavRequestedProperty,
) -> Result<Option<aster_forge_webdav::DavXmlElement>, ()> {
    Ok(match requested.name.as_str() {
        "displayname" => Some(dav_property_text_element(requested, "report.txt")),
        "color" => Some(dav_property_text_element(requested, "blue")),
        _ => None,
    })
}

#[test]
fn named_properties_are_grouped_into_ordered_200_and_404_propstats() {
    let request = DavPropfindRequest::Prop(vec![
        property("missing", Some("DAV:"), Some("D")),
        property("displayname", Some("DAV:"), Some("x")),
        property("color", Some("urn:test"), Some("t")),
    ]);
    let item =
        build_propfind_item("/dav/a".to_string(), &request, &[], resolve).expect("property item");

    assert_eq!(item.propstats.len(), 2);
    assert_eq!(item.propstats[0].status, 200);
    assert_eq!(item.propstats[0].properties.len(), 2);
    assert_eq!(item.propstats[1].status, 404);
    assert_eq!(item.propstats[1].properties.len(), 1);
}

#[test]
fn allprop_deduplicates_include_by_expanded_name_and_reports_missing_include() {
    let available = vec![
        property("displayname", Some("DAV:"), Some("D")),
        property("color", Some("urn:test"), Some("stored")),
    ];
    let request = DavPropfindRequest::AllProp {
        include: vec![
            property("displayname", Some("DAV:"), Some("other")),
            property("extra", Some("urn:test"), Some("t")),
        ],
    };
    let item = build_propfind_item("/dav/a".to_string(), &request, &available, resolve)
        .expect("allprop item");

    assert_eq!(item.propstats.len(), 2);
    assert_eq!(item.propstats[0].status, 200);
    assert_eq!(item.propstats[0].properties.len(), 2);
    assert_eq!(item.propstats[1].status, 404);
    assert_eq!(item.propstats[1].properties.len(), 1);
}

#[test]
fn propname_uses_catalog_names_without_resolving_values() {
    let available = vec![
        property("displayname", Some("DAV:"), Some("D")),
        property("color", Some("urn:test"), Some("stored")),
    ];
    let mut calls = 0;
    let item = build_propfind_item(
        "/dav/a".to_string(),
        &DavPropfindRequest::PropName,
        &available,
        |_| {
            calls += 1;
            Ok::<_, ()>(None)
        },
    )
    .expect("propname item");

    assert_eq!(calls, 0);
    assert_eq!(item.propstats.len(), 1);
    assert_eq!(item.propstats[0].status, 200);
    assert_eq!(item.propstats[0].properties.len(), 2);
}

#[test]
fn proppatch_groups_outcomes_by_numeric_status_and_preserves_property_order() {
    let first = property("first", Some("urn:test"), Some("t"));
    let second = property("second", Some("urn:test"), Some("t"));
    let failed = property("failed", Some("urn:test"), Some("t"));
    let item = build_proppatch_item(
        "/dav/a".to_owned(),
        [
            (424, dav_property_name_element(&failed)),
            (200, dav_property_name_element(&first)),
            (200, dav_property_name_element(&second)),
        ],
    );

    assert_eq!(item.propstats.len(), 2);
    assert_eq!(item.propstats[0].status, 200);
    assert_eq!(item.propstats[0].properties[0].name, "first");
    assert_eq!(item.propstats[0].properties[1].name, "second");
    assert_eq!(item.propstats[1].status, 424);
    assert_eq!(item.propstats[1].properties[0].name, "failed");
}

#[test]
fn property_response_helpers_own_multistatus_depth_and_xml_error_contracts() {
    let item = build_proppatch_item(
        "/dav/a".to_owned(),
        [(
            200,
            dav_property_name_element(&property("color", Some("urn:test"), Some("t"))),
        )],
    );
    let multistatus = property_multistatus_response(vec![item]).expect("multistatus");
    assert_eq!(multistatus.status, StatusCode::MULTI_STATUS);
    assert!(multistatus.headers.get("Cache-Control").is_none());

    let finite = propfind_finite_depth_response().expect("finite depth response");
    assert_eq!(finite.status, StatusCode::FORBIDDEN);
    assert_eq!(finite.headers.get("Cache-Control").unwrap(), "no-store");
    let DavResponseBody::Bytes(body) = finite.body else {
        panic!("finite depth should have XML body");
    };
    assert!(
        String::from_utf8(body.to_vec())
            .unwrap()
            .contains("propfind-finite-depth")
    );

    let propfind = propfind_xml_error_response(DavXmlError::InvalidGrammar).unwrap();
    assert_eq!(propfind.status, StatusCode::BAD_REQUEST);
    let DavResponseBody::Bytes(body) = propfind.body else {
        panic!("PROPFIND error should have text body");
    };
    assert_eq!(body.as_ref(), b"Invalid PROPFIND body");

    let proppatch = proppatch_xml_error_response(DavXmlError::InvalidGrammar).unwrap();
    let DavResponseBody::Bytes(body) = proppatch.body else {
        panic!("PROPPATCH error should have text body");
    };
    assert_eq!(body.as_ref(), b"Invalid PROPPATCH body");
}
