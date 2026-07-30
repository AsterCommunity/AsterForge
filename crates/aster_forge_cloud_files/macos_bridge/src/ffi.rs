//! Minimal C ABI for Swift-owned byte transfer and identifier conversion.

use std::{panic::AssertUnwindSafe, ptr};

use aster_forge_cloud_files_core::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};

use crate::identifier::{MAX_FILE_PROVIDER_IDENTIFIER_BYTES, MAX_IDENTITY_FIELD_BYTES};
use crate::{
    MacosBridgeError, MacosErrorCode, MacosExtensionRequestLease, MacosExtensionSession,
    MacosFileProviderIdentifier, Result,
};

/// Owned byte allocation returned to Swift. Release exactly once with
/// [`aster_forge_cloud_files_macos_buffer_release`].
#[derive(Debug)]
#[repr(C)]
pub struct MacosFfiBuffer {
    /// Allocation base, or null for an empty buffer.
    pub ptr: *mut u8,
    /// Logical initialized length.
    pub len: usize,
    /// Allocation capacity required for exact reconstruction during release.
    pub capacity: usize,
}

impl MacosFfiBuffer {
    const fn empty() -> Self {
        Self {
            ptr: ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }

    fn from_vec(mut bytes: Vec<u8>) -> Self {
        if bytes.is_empty() {
            return Self::empty();
        }
        let buffer = Self {
            ptr: bytes.as_mut_ptr(),
            len: bytes.len(),
            capacity: bytes.capacity(),
        };
        std::mem::forget(bytes);
        buffer
    }
}

/// C ABI result containing either an owned byte buffer or a classified error.
#[derive(Debug)]
#[repr(C)]
pub struct MacosFfiResult {
    /// Native-facing result classification.
    pub code: MacosErrorCode,
    /// Owned payload on success; empty for errors.
    pub buffer: MacosFfiBuffer,
}

/// Opaque extension-session owner held by Swift.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MacosFfiSessionHandle {
    /// Opaque Rust allocation pointer. Swift must not dereference or modify it.
    pub raw: *const (),
}

/// Opaque accepted-request owner held until one completion/cancellation terminal path.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MacosFfiRequestHandle {
    /// Opaque Rust allocation pointer. Swift must not dereference or modify it.
    pub raw: *mut (),
}

/// C ABI result for session creation.
#[derive(Debug)]
#[repr(C)]
pub struct MacosFfiSessionResult {
    /// Native-facing result classification.
    pub code: MacosErrorCode,
    /// Owned session handle on success.
    pub handle: MacosFfiSessionHandle,
}

/// C ABI result for accepting one request lease.
#[derive(Debug)]
#[repr(C)]
pub struct MacosFfiRequestResult {
    /// Native-facing result classification.
    pub code: MacosErrorCode,
    /// Owned request handle on success.
    pub handle: MacosFfiRequestHandle,
}

impl MacosFfiResult {
    fn success(bytes: Vec<u8>) -> Self {
        Self {
            code: MacosErrorCode::Success,
            buffer: MacosFfiBuffer::from_vec(bytes),
        }
    }

    const fn failure(code: MacosErrorCode) -> Self {
        Self {
            code,
            buffer: MacosFfiBuffer::empty(),
        }
    }
}

/// Creates one accepting extension session generation.
#[unsafe(no_mangle)]
pub extern "C" fn aster_forge_cloud_files_macos_session_create(
    generation: u64,
) -> MacosFfiSessionResult {
    let result =
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            let generation = aster_forge_cloud_files_core::SessionGeneration::new(generation)
                .map_err(|_| MacosBridgeError::InvalidFfiInput {
                    reason: "extension session generation must be non-zero",
                })?;
            Ok::<_, MacosBridgeError>(MacosExtensionSession::new(generation).into_ffi_handle())
        }));
    match result {
        Ok(Ok(raw)) => MacosFfiSessionResult {
            code: MacosErrorCode::Success,
            handle: MacosFfiSessionHandle { raw },
        },
        Ok(Err(error)) => MacosFfiSessionResult {
            code: error.error_code(),
            handle: MacosFfiSessionHandle { raw: ptr::null() },
        },
        Err(_) => MacosFfiSessionResult {
            code: MacosErrorCode::Internal,
            handle: MacosFfiSessionHandle { raw: ptr::null() },
        },
    }
}

/// Accepts one request against the exact active session generation.
///
/// # Safety
///
/// `session` must be a live unreleased handle returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_session_begin_request(
    session: MacosFfiSessionHandle,
    generation: u64,
) -> MacosFfiRequestResult {
    let result =
        std::panic::catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: required by this exported function's handle contract.
            let session = unsafe { MacosExtensionSession::clone_from_ffi_handle(session.raw) }?;
            let generation = aster_forge_cloud_files_core::SessionGeneration::new(generation)
                .map_err(|_| MacosBridgeError::InvalidFfiInput {
                    reason: "request generation must be non-zero",
                })?;
            Ok::<_, MacosBridgeError>(session.begin_request(generation)?.into_ffi_handle())
        }));
    match result {
        Ok(Ok(raw)) => MacosFfiRequestResult {
            code: MacosErrorCode::Success,
            handle: MacosFfiRequestHandle { raw },
        },
        Ok(Err(error)) => MacosFfiRequestResult {
            code: error.error_code(),
            handle: MacosFfiRequestHandle {
                raw: ptr::null_mut(),
            },
        },
        Err(_) => MacosFfiRequestResult {
            code: MacosErrorCode::Internal,
            handle: MacosFfiRequestHandle {
                raw: ptr::null_mut(),
            },
        },
    }
}

/// Starts idempotent closing for one live extension session.
///
/// # Safety
///
/// `session` must be a live unreleased handle returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_session_begin_closing(
    session: MacosFfiSessionHandle,
) -> MacosErrorCode {
    ffi_code(|| {
        // SAFETY: required by this exported function's handle contract.
        let session = unsafe { MacosExtensionSession::clone_from_ffi_handle(session.raw) }?;
        let _transitioned = session.begin_closing();
        Ok(())
    })
}

/// Records native disconnect and moves the session to draining/closed.
///
/// # Safety
///
/// `session` must be a live unreleased handle returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_session_mark_disconnected(
    session: MacosFfiSessionHandle,
) -> MacosErrorCode {
    ffi_code(|| {
        // SAFETY: required by this exported function's handle contract.
        let session = unsafe { MacosExtensionSession::clone_from_ffi_handle(session.raw) }?;
        session.mark_disconnected()
    })
}

/// Releases one accepted request lease after exactly one terminal Swift completion path.
///
/// # Safety
///
/// `request` must be null or one live unreleased handle returned by `begin_request`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_request_release(
    request: MacosFfiRequestHandle,
) {
    // SAFETY: required by this exported function's request-handle contract.
    unsafe { MacosExtensionRequestLease::release_ffi_handle(request.raw) };
}

/// Releases one extension session owner after native disconnect and request cleanup.
///
/// # Safety
///
/// `session` must be null or one live unreleased handle returned by `session_create`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_session_release(
    session: MacosFfiSessionHandle,
) {
    // SAFETY: required by this exported function's session-handle contract.
    unsafe { MacosExtensionSession::release_ffi_handle(session.raw) };
}

/// Encodes three UTF-8 identity fields into one persistent File Provider identifier.
///
/// # Safety
///
/// Every non-null pointer must reference its declared readable byte length for this call. The
/// referenced memory must not be mutated concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_identifier_encode(
    namespace_ptr: *const u8,
    namespace_len: usize,
    root_ptr: *const u8,
    root_len: usize,
    item_ptr: *const u8,
    item_len: usize,
) -> MacosFfiResult {
    ffi_result(|| {
        // SAFETY: upheld by the exported function contract; each value is copied before return.
        let namespace =
            unsafe { read_utf8(namespace_ptr, namespace_len, MAX_IDENTITY_FIELD_BYTES) }?;
        // SAFETY: upheld by the exported function contract; each value is copied before return.
        let root = unsafe { read_utf8(root_ptr, root_len, MAX_IDENTITY_FIELD_BYTES) }?;
        // SAFETY: upheld by the exported function contract; each value is copied before return.
        let item = unsafe { read_utf8(item_ptr, item_len, MAX_IDENTITY_FIELD_BYTES) }?;
        if namespace.contains('\0') || root.contains('\0') || item.contains('\0') {
            return Err(MacosBridgeError::InvalidFfiInput {
                reason: "FFI identity fields must not contain NUL",
            });
        }
        let namespace =
            CloudNamespaceId::new(namespace).map_err(|_| MacosBridgeError::InvalidFfiInput {
                reason: "FFI namespace must not be empty",
            })?;
        let root = CloudRootId::new(root).map_err(|_| MacosBridgeError::InvalidFfiInput {
            reason: "FFI root must not be empty",
        })?;
        let item = CloudItemId::new(item).map_err(|_| MacosBridgeError::InvalidFfiInput {
            reason: "FFI item must not be empty",
        })?;
        let key = CloudItemKey::new(CloudScope::new(namespace, root), item);
        Ok(MacosFileProviderIdentifier::encode(&key)?
            .into_string()
            .into_bytes())
    })
}

/// Decodes one item identifier into `namespace\0root\0item` owned UTF-8 bytes.
///
/// # Safety
///
/// A non-null pointer must reference `identifier_len` readable bytes for this call. The referenced
/// memory must not be mutated concurrently.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_identifier_decode(
    identifier_ptr: *const u8,
    identifier_len: usize,
) -> MacosFfiResult {
    ffi_result(|| {
        // SAFETY: upheld by the exported function contract; the identifier is copied.
        let identifier = unsafe {
            read_utf8(
                identifier_ptr,
                identifier_len,
                MAX_FILE_PROVIDER_IDENTIFIER_BYTES,
            )
        }?;
        let parsed = MacosFileProviderIdentifier::parse(identifier)?;
        let key = parsed.item_key()?;
        let fields = [
            key.scope().namespace_id().as_str(),
            key.scope().root_id().as_str(),
            key.item_id().as_str(),
        ];
        if fields.iter().any(|field| field.contains('\0')) {
            return Err(MacosBridgeError::InvalidFfiInput {
                reason: "decoded FFI identity fields must not contain NUL",
            });
        }
        let total_len = fields.iter().try_fold(2usize, |total, field| {
            total
                .checked_add(field.len())
                .ok_or(MacosBridgeError::InvalidFfiInput {
                    reason: "decoded identifier fields exceed addressable memory",
                })
        })?;
        let mut output = Vec::with_capacity(total_len);
        for (index, field) in fields.into_iter().enumerate() {
            if index != 0 {
                output.push(0);
            }
            output.extend_from_slice(field.as_bytes());
        }
        Ok(output)
    })
}

/// Releases one owned buffer returned by this library. Empty buffers are ignored.
///
/// # Safety
///
/// `buffer` must be an unreleased value returned by this library with every field unchanged.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aster_forge_cloud_files_macos_buffer_release(buffer: MacosFfiBuffer) {
    if buffer.ptr.is_null() {
        return;
    }
    if buffer.len > buffer.capacity || buffer.capacity == 0 {
        return;
    }
    // SAFETY: non-empty buffers exposed by this module originate from `Vec<u8>` with these exact
    // pointer, length, and capacity values. Ownership is transferred to this release call once.
    let bytes = unsafe { Vec::from_raw_parts(buffer.ptr, buffer.len, buffer.capacity) };
    drop(bytes);
}

fn ffi_result(operation: impl FnOnce() -> Result<Vec<u8>>) -> MacosFfiResult {
    match std::panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(bytes)) => MacosFfiResult::success(bytes),
        Ok(Err(error)) => MacosFfiResult::failure(error.error_code()),
        Err(_) => MacosFfiResult::failure(MacosErrorCode::Internal),
    }
}

fn ffi_code(operation: impl FnOnce() -> Result<()>) -> MacosErrorCode {
    match std::panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => MacosErrorCode::Success,
        Ok(Err(error)) => error.error_code(),
        Err(_) => MacosErrorCode::Internal,
    }
}

unsafe fn read_utf8(pointer: *const u8, length: usize, maximum_length: usize) -> Result<String> {
    if length == 0 {
        return Err(MacosBridgeError::InvalidFfiInput {
            reason: "FFI string must not be empty",
        });
    }
    if pointer.is_null() {
        return Err(MacosBridgeError::InvalidFfiInput {
            reason: "non-empty FFI string used a null pointer",
        });
    }
    if length > maximum_length {
        return Err(MacosBridgeError::InvalidFfiInput {
            reason: "FFI string exceeds the accepted byte length",
        });
    }
    // SAFETY: the C caller promises `pointer` references `length` readable bytes for this call.
    // The slice is copied into an owned `String` before the function returns.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
    let text = std::str::from_utf8(bytes).map_err(|_| MacosBridgeError::InvalidFfiInput {
        reason: "FFI string is not valid UTF-8",
    })?;
    Ok(text.to_owned())
}
