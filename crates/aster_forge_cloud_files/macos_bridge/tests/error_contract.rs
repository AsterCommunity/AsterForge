use aster_forge_cloud_files_core::{CloudBackendError, CloudBackendErrorKind, SessionState};
use aster_forge_cloud_files_macos_bridge::{MacosBridgeError, MacosErrorCode, backend_error_code};

#[test]
fn backend_error_kinds_map_to_stable_file_provider_classifications() {
    let cases = [
        (CloudBackendErrorKind::NotFound, MacosErrorCode::NotFound),
        (
            CloudBackendErrorKind::AuthenticationRequired,
            MacosErrorCode::NotAuthenticated,
        ),
        (
            CloudBackendErrorKind::PermissionDenied,
            MacosErrorCode::PermissionDenied,
        ),
        (
            CloudBackendErrorKind::Conflict,
            MacosErrorCode::VersionOutOfDate,
        ),
        (
            CloudBackendErrorKind::PreconditionFailed,
            MacosErrorCode::VersionOutOfDate,
        ),
        (
            CloudBackendErrorKind::InvalidRequest,
            MacosErrorCode::InvalidArgument,
        ),
        (CloudBackendErrorKind::RateLimited, MacosErrorCode::TryAgain),
        (
            CloudBackendErrorKind::TemporarilyUnavailable,
            MacosErrorCode::TryAgain,
        ),
        (
            CloudBackendErrorKind::Unsupported,
            MacosErrorCode::NotSupported,
        ),
        (
            CloudBackendErrorKind::InvalidResponse,
            MacosErrorCode::Internal,
        ),
        (CloudBackendErrorKind::Internal, MacosErrorCode::Internal),
    ];

    for (kind, expected) in cases {
        assert_eq!(backend_error_code(kind), expected);
        assert_eq!(
            MacosBridgeError::Backend(CloudBackendError::new(kind)).error_code(),
            expected
        );
    }
}

#[test]
fn bridge_errors_map_without_leaking_product_error_policy() {
    let cases = [
        (
            MacosBridgeError::InvalidIdentifier { reason: "fixture" },
            MacosErrorCode::InvalidArgument,
        ),
        (
            MacosBridgeError::SystemContainerIsNotItem,
            MacosErrorCode::InvalidArgument,
        ),
        (
            MacosBridgeError::InvalidItemVersion { reason: "fixture" },
            MacosErrorCode::InvalidArgument,
        ),
        (
            MacosBridgeError::InvalidFfiInput { reason: "fixture" },
            MacosErrorCode::InvalidArgument,
        ),
        (
            MacosBridgeError::UnsupportedSystemContainer,
            MacosErrorCode::NotSupported,
        ),
        (
            MacosBridgeError::StaleSessionGeneration {
                expected: 2,
                actual: 1,
            },
            MacosErrorCode::ProviderNotFound,
        ),
        (
            MacosBridgeError::SessionNotAccepting {
                state: SessionState::Closing,
            },
            MacosErrorCode::ProviderNotFound,
        ),
        (
            MacosBridgeError::InvalidSessionTransition {
                from: SessionState::Accepting,
                to: SessionState::Draining,
            },
            MacosErrorCode::ProviderNotFound,
        ),
        (
            MacosBridgeError::ActiveRequestCountOverflow,
            MacosErrorCode::Internal,
        ),
        (
            MacosBridgeError::InvalidBackendResponse { reason: "fixture" },
            MacosErrorCode::Internal,
        ),
        (MacosBridgeError::FfiPanic, MacosErrorCode::Internal),
    ];

    for (error, expected) in cases {
        assert_eq!(error.error_code(), expected, "{error}");
    }
}

#[test]
fn ffi_error_code_discriminants_are_stable() {
    let cases = [
        (MacosErrorCode::Success, 0),
        (MacosErrorCode::NotFound, 1),
        (MacosErrorCode::NotAuthenticated, 2),
        (MacosErrorCode::PermissionDenied, 3),
        (MacosErrorCode::VersionOutOfDate, 4),
        (MacosErrorCode::TryAgain, 5),
        (MacosErrorCode::NotSupported, 6),
        (MacosErrorCode::InvalidArgument, 7),
        (MacosErrorCode::SyncAnchorExpired, 8),
        (MacosErrorCode::Cancelled, 9),
        (MacosErrorCode::ProviderNotFound, 10),
        (MacosErrorCode::Internal, 11),
    ];

    for (code, discriminant) in cases {
        assert_eq!(code as i32, discriminant);
    }
}
