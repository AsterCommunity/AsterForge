//! Product-neutral Rust bridge contracts for Apple File Provider extensions.
//!
//! This crate owns persistent identifier/version mapping, owned enumeration results,
//! extension-session fencing, and the narrow C ABI used by a thin Swift extension shell. Swift
//! retains all `NSFileProvider*` objects and completion handlers. Product crates retain backend
//! adapters, authentication, persistence, domain policy, packaging, signing, and user-visible
//! errors.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![deny(clippy::undocumented_unsafe_blocks, unsafe_op_in_unsafe_fn)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod engine;
mod enumeration;
mod error;
mod ffi;
mod identifier;
mod item;
mod session;
mod version;

pub use engine::{MacosContentFetchPlan, MacosFetchedContentChunk, MacosReadOnlyEngine};
pub use enumeration::{
    MAX_ENUMERATION_ITEMS, MAX_ENUMERATION_PAGE_ITEMS, MAX_ENUMERATION_STATE_BYTES,
};
pub use enumeration::{MacosEnumerationPage, MacosEnumerationRequest, MacosEnumerationState};
pub use error::{MacosBridgeError, MacosErrorCode, Result, backend_error_code};
pub use ffi::{
    MacosFfiBuffer, MacosFfiRequestHandle, MacosFfiRequestResult, MacosFfiResult,
    MacosFfiSessionHandle, MacosFfiSessionResult, aster_forge_cloud_files_macos_buffer_release,
    aster_forge_cloud_files_macos_identifier_decode,
    aster_forge_cloud_files_macos_identifier_encode, aster_forge_cloud_files_macos_request_release,
    aster_forge_cloud_files_macos_session_begin_closing,
    aster_forge_cloud_files_macos_session_begin_request,
    aster_forge_cloud_files_macos_session_create,
    aster_forge_cloud_files_macos_session_mark_disconnected,
    aster_forge_cloud_files_macos_session_release,
};
pub use identifier::{
    FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER, FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER,
    FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER, MAX_FILE_PROVIDER_IDENTIFIER_BYTES,
    MAX_IDENTITY_FIELD_BYTES, MacosFileProviderIdentifier, MacosFileProviderSystemContainer,
};
pub use item::{MacosFileProviderItem, MacosFileProviderItemKind};
pub use session::{MacosExtensionRequestLease, MacosExtensionSession};
pub use version::{FILE_PROVIDER_ITEM_VERSION_COMPONENT_MAX_BYTES, MacosFileProviderItemVersion};
