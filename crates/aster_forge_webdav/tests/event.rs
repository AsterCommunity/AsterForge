use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aster_forge_webdav::{
    DavBackendErrorKind, DavEvent, DavEventOutcome, DavEventSink, DavMethod, DavObservationError,
    DavOperation, DavOperationObservations, DavPath, DavProtocolFailureClass, DavRequestHead,
    DavRequestOrigin, DavStreamOutcome, Destination, IfHeader, IfResourceGroup, IfStateCondition,
    IfStateList, NoopDavEventSink, publish_non_authoritative,
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
    assert_eq!(event.observations, DavOperationObservations::default());
    let debug = format!("{event:?}");
    assert!(debug.contains("source.txt"));
    assert!(!debug.contains("sensitive-lock-token"));
}

#[test]
fn completed_event_preserves_every_typed_observation_without_string_payloads() {
    let request_head = DavRequestHead {
        method: DavMethod::Get,
        target: DavPath::new("/video.mp4").expect("source path"),
        origin: DavRequestOrigin {
            scheme: "https".to_owned(),
            host: "dav.example".to_owned(),
        },
        depth: None,
        overwrite: None,
        destination: None,
        if_header: None,
    };
    let observations = DavOperationObservations {
        bytes_received: Some(0),
        bytes_sent: Some(u64::MAX),
        requested_ranges: Some(3),
        served_ranges: Some(2),
        resources: Some(1),
        backend_open_count: Some(2),
        backend_call_count: Some(7),
        protocol_failure: Some(DavProtocolFailureClass::Transport),
        stream: Some(DavStreamOutcome::Failed {
            response_started: true,
        }),
    };
    let event = DavEvent::completed_with_observations(
        &request_head,
        500,
        Duration::from_secs(1),
        Some(DavBackendErrorKind::Internal),
        observations,
    );

    assert_eq!(event.observations, observations);
    assert_eq!(event.observations.bytes_received, Some(0));
    assert_eq!(event.observations.bytes_sent, Some(u64::MAX));
}

#[test]
fn stream_and_protocol_failure_enums_cover_all_observation_classes() {
    assert_ne!(
        DavOperationObservations::default().bytes_sent,
        Some(0),
        "uncollected and collected-zero observations must remain distinct"
    );
    assert_eq!(
        [
            DavStreamOutcome::Completed,
            DavStreamOutcome::Cancelled {
                response_started: false,
            },
            DavStreamOutcome::Cancelled {
                response_started: true,
            },
            DavStreamOutcome::Failed {
                response_started: false,
            },
            DavStreamOutcome::Failed {
                response_started: true,
            },
        ]
        .len(),
        5
    );
    assert_eq!(
        [
            DavProtocolFailureClass::Request,
            DavProtocolFailureClass::Precondition,
            DavProtocolFailureClass::Capability,
            DavProtocolFailureClass::Backend,
            DavProtocolFailureClass::Response,
            DavProtocolFailureClass::Transport,
        ]
        .len(),
        6
    );
}

struct FailingSink {
    calls: AtomicUsize,
}

impl DavEventSink for FailingSink {
    fn publish(&self, _event: &DavEvent) -> Result<(), DavObservationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(DavObservationError)
    }
}

struct PanickingSink;

impl DavEventSink for PanickingSink {
    fn publish(&self, _event: &DavEvent) -> Result<(), DavObservationError> {
        panic!("observation panic must not cross the non-authoritative boundary");
    }
}

#[test]
fn absent_and_failing_observers_do_not_change_authoritative_completion() {
    let request_head = DavRequestHead {
        method: DavMethod::Put,
        target: DavPath::new("/file.txt").expect("source path"),
        origin: DavRequestOrigin {
            scheme: "https".to_owned(),
            host: "dav.example".to_owned(),
        },
        depth: None,
        overwrite: None,
        destination: None,
        if_header: None,
    };
    let event = DavEvent::completed(&request_head, 204, Duration::ZERO, None);
    publish_non_authoritative(None, &event);

    let sink = FailingSink {
        calls: AtomicUsize::new(0),
    };
    publish_non_authoritative(Some(&sink), &event);
    assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    publish_non_authoritative(Some(&PanickingSink), &event);
    assert_eq!(event.outcome, DavEventOutcome::Succeeded { status: 204 });
    assert_eq!(NoopDavEventSink.publish(&event), Ok(()));
}
