use aster_forge_webdav::{DavBackendErrorKind, DavEventOutcome};

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
