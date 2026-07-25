use std::time::Duration;

use aster_forge_webdav::{
    DavBackendErrorKind, DavEvent, DavEventOutcome, DavMethod, DavOperation, DavPath,
    DavRequestHead, DavRequestOrigin, Destination, IfHeader, IfResourceGroup, IfStateCondition,
    IfStateList,
};

#[test]
fn event_outcome_classifies_the_complete_http_status_boundary() {
    for status in [100, 200, 207, 304, 399] {
        assert_eq!(
            DavEventOutcome::from_status(status, Some(DavBackendErrorKind::Internal)),
            DavEventOutcome::Succeeded { status }
        );
    }

    for status in [400, 404, 423, 500, 599] {
        assert_eq!(
            DavEventOutcome::from_status(status, Some(DavBackendErrorKind::Internal)),
            DavEventOutcome::Failed {
                status,
                backend_error: Some(DavBackendErrorKind::Internal),
            }
        );
    }
}

#[test]
fn event_outcome_exposes_its_transport_neutral_status() {
    assert_eq!(DavEventOutcome::from_status(207, None).status(), 207);
    assert_eq!(DavEventOutcome::from_status(423, None).status(), 423);
}

#[test]
fn completed_event_copies_only_protocol_routing_data() {
    let request_head = DavRequestHead {
        method: DavMethod::Copy,
        target: DavPath::new("/source.txt").expect("source path"),
        origin: DavRequestOrigin {
            scheme: "https".to_owned(),
            host: "dav.example".to_owned(),
        },
        depth: None,
        overwrite: Some(true),
        destination: Some(Destination {
            path: DavPath::new("/destination.txt").expect("destination path"),
            relative: "/destination.txt".to_owned(),
        }),
        if_header: Some(IfHeader {
            groups: vec![IfResourceGroup {
                tagged_path: None,
                lists: vec![IfStateList {
                    conditions: vec![IfStateCondition::Token {
                        value: "urn:uuid:sensitive-lock-token".to_owned(),
                        negated: false,
                    }],
                }],
            }],
        }),
    };

    let event = DavEvent::completed(
        &request_head,
        423,
        Duration::from_millis(12),
        Some(DavBackendErrorKind::Conflict),
    );

    assert_eq!(event.request_id, None);
    assert_eq!(event.operation, DavOperation::Copy);
    assert_eq!(event.source.as_str(), "/source.txt");
    assert_eq!(
        event.destination.as_ref().map(|path| path.as_str()),
        Some("/destination.txt")
    );
    assert_eq!(
        event.outcome,
        DavEventOutcome::Failed {
            status: 423,
            backend_error: Some(DavBackendErrorKind::Conflict),
        }
    );
    assert_eq!(event.elapsed, Duration::from_millis(12));
    let debug = format!("{event:?}");
    assert!(debug.contains("source.txt"));
    assert!(!debug.contains("sensitive-lock-token"));
}
