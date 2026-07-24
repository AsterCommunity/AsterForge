use aster_forge_webdav::{
    DavBackendError, DavBackendErrorKind, DavPropertyTarget, DavResourceKind, FsError, OpenOptions,
};

#[test]
fn filesystem_errors_map_exhaustively_to_protocol_backend_categories() {
    let cases = [
        (FsError::NotFound, DavBackendErrorKind::NotFound),
        (FsError::Forbidden, DavBackendErrorKind::Forbidden),
        (FsError::GeneralFailure, DavBackendErrorKind::Internal),
        (FsError::Exists, DavBackendErrorKind::AlreadyExists),
        (
            FsError::InsufficientStorage,
            DavBackendErrorKind::InsufficientStorage,
        ),
        (FsError::TooLarge, DavBackendErrorKind::PayloadTooLarge),
        (FsError::BadRequest, DavBackendErrorKind::InvalidInput),
    ];
    for (error, expected) in cases {
        assert_eq!(DavBackendError::from(error).kind, expected);
    }
}

#[test]
fn open_modes_and_property_targets_are_transport_neutral_values() {
    assert_eq!(
        OpenOptions::read(),
        OpenOptions {
            read: true,
            ..OpenOptions::default()
        }
    );
    assert_eq!(
        OpenOptions::write(),
        OpenOptions {
            write: true,
            ..OpenOptions::default()
        }
    );

    let target = DavPropertyTarget {
        kind: DavResourceKind::Collection,
        id: i64::MAX,
    };
    assert_eq!(target.kind, DavResourceKind::Collection);
    assert_eq!(target.id, i64::MAX);
}
