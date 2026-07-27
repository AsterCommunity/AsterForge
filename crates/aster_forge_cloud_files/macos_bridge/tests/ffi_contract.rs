use std::ptr;

use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER, MacosErrorCode,
    aster_forge_cloud_files_macos_buffer_release, aster_forge_cloud_files_macos_identifier_decode,
    aster_forge_cloud_files_macos_identifier_encode, aster_forge_cloud_files_macos_request_release,
    aster_forge_cloud_files_macos_session_begin_closing,
    aster_forge_cloud_files_macos_session_begin_request,
    aster_forge_cloud_files_macos_session_create,
    aster_forge_cloud_files_macos_session_mark_disconnected,
    aster_forge_cloud_files_macos_session_release,
};

fn result_bytes(result: &aster_forge_cloud_files_macos_bridge::MacosFfiResult) -> &[u8] {
    if result.buffer.len == 0 {
        return &[];
    }
    // SAFETY: a successful FFI result exposes one readable allocation until its release call.
    unsafe { std::slice::from_raw_parts(result.buffer.ptr, result.buffer.len) }
}

#[test]
fn ffi_identifier_round_trip_preserves_exact_fields_and_releases_owned_buffers() {
    let namespace = b"namespace";
    let root = b"team-root";
    let item = "文件-item".as_bytes();
    // SAFETY: all three slices remain readable for the duration of this call.
    let encoded = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            namespace.as_ptr(),
            namespace.len(),
            root.as_ptr(),
            root.len(),
            item.as_ptr(),
            item.len(),
        )
    };
    assert_eq!(encoded.code, MacosErrorCode::Success);
    let encoded_text = std::str::from_utf8(result_bytes(&encoded))
        .expect("identifier should be UTF-8")
        .to_owned();
    // SAFETY: the encoded allocation remains live until released below.
    let decoded = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(encoded.buffer.ptr, encoded.buffer.len)
    };
    assert_eq!(decoded.code, MacosErrorCode::Success);
    assert_eq!(
        result_bytes(&decoded),
        b"namespace\0team-root\0\xE6\x96\x87\xE4\xBB\xB6-item"
    );
    assert!(encoded_text.starts_with("afcf1."));
    // SAFETY: each buffer is released once with its original fields unchanged.
    unsafe {
        aster_forge_cloud_files_macos_buffer_release(decoded.buffer);
        aster_forge_cloud_files_macos_buffer_release(encoded.buffer);
    }
}

#[test]
fn ffi_rejects_null_empty_invalid_utf8_nul_and_system_container_inputs() {
    let valid = b"valid";
    let invalid_utf8 = [0xff];
    let embedded_nul = b"bad\0field";
    // SAFETY: pointer/length pairs are either valid readable slices or the explicitly supported
    // null+nonzero validation case, which must be rejected before dereference.
    let null = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            ptr::null(),
            1,
            valid.as_ptr(),
            valid.len(),
            valid.as_ptr(),
            valid.len(),
        )
    };
    assert_eq!(null.code, MacosErrorCode::InvalidArgument);
    // SAFETY: the non-null slices remain readable; zero length is validated before reading.
    let empty = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            valid.as_ptr(),
            0,
            valid.as_ptr(),
            valid.len(),
            valid.as_ptr(),
            valid.len(),
        )
    };
    assert_eq!(empty.code, MacosErrorCode::InvalidArgument);
    // SAFETY: all declared slices remain readable for the call.
    let utf8 = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            invalid_utf8.as_ptr(),
            invalid_utf8.len(),
            valid.as_ptr(),
            valid.len(),
            valid.as_ptr(),
            valid.len(),
        )
    };
    assert_eq!(utf8.code, MacosErrorCode::InvalidArgument);
    // SAFETY: all declared slices remain readable for the call.
    let nul = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            embedded_nul.as_ptr(),
            embedded_nul.len(),
            valid.as_ptr(),
            valid.len(),
            valid.as_ptr(),
            valid.len(),
        )
    };
    assert_eq!(nul.code, MacosErrorCode::InvalidArgument);
    // SAFETY: the static system identifier remains readable for the call.
    let system = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(
            FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER.as_ptr(),
            FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER.len(),
        )
    };
    assert_eq!(system.code, MacosErrorCode::InvalidArgument);
}

#[test]
fn releasing_an_empty_buffer_is_a_noop() {
    // SAFETY: this is the canonical empty buffer representation accepted by the release contract.
    unsafe {
        aster_forge_cloud_files_macos_buffer_release(
            aster_forge_cloud_files_macos_bridge::MacosFfiBuffer {
                ptr: ptr::null_mut(),
                len: 0,
                capacity: 0,
            },
        );
    }
}

#[test]
fn ffi_decode_rejects_null_empty_invalid_utf8_and_malformed_identifiers() {
    let valid = b"afcf1.YQ.Yg.Yw";
    let invalid_utf8 = [0xff];
    let malformed = b"not-an-identifier";
    let decoded_nul = b"afcf1.YmFkAGZpZWxk.dmFsaWQ.dmFsaWQ";

    // SAFETY: null+nonzero is the explicit validation case and is rejected before dereference.
    let null = unsafe { aster_forge_cloud_files_macos_identifier_decode(ptr::null(), 1) };
    assert_eq!(null.code, MacosErrorCode::InvalidArgument);
    assert!(null.buffer.ptr.is_null());
    // SAFETY: zero length is rejected before the readable pointer is used.
    let empty = unsafe { aster_forge_cloud_files_macos_identifier_decode(valid.as_ptr(), 0) };
    assert_eq!(empty.code, MacosErrorCode::InvalidArgument);
    // SAFETY: the declared one-byte slice remains readable for this call.
    let utf8 = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(invalid_utf8.as_ptr(), invalid_utf8.len())
    };
    assert_eq!(utf8.code, MacosErrorCode::InvalidArgument);
    // SAFETY: the malformed identifier slice remains readable for this call.
    let malformed = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(malformed.as_ptr(), malformed.len())
    };
    assert_eq!(malformed.code, MacosErrorCode::InvalidArgument);
    // SAFETY: this canonical identifier remains readable and decodes to a NUL-containing field.
    let nul = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(decoded_nul.as_ptr(), decoded_nul.len())
    };
    assert_eq!(nul.code, MacosErrorCode::InvalidArgument);
}

#[test]
fn ffi_encode_rejects_empty_and_nul_in_every_identity_position() {
    let valid = b"valid";
    let nul = b"bad\0field";
    let cases = [
        (
            valid.as_slice(),
            0,
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            valid.len(),
        ),
        (
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            0,
            valid.as_slice(),
            valid.len(),
        ),
        (
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            0,
        ),
        (
            nul.as_slice(),
            nul.len(),
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            valid.len(),
        ),
        (
            valid.as_slice(),
            valid.len(),
            nul.as_slice(),
            nul.len(),
            valid.as_slice(),
            valid.len(),
        ),
        (
            valid.as_slice(),
            valid.len(),
            valid.as_slice(),
            valid.len(),
            nul.as_slice(),
            nul.len(),
        ),
    ];

    for (namespace, namespace_len, root, root_len, item, item_len) in cases {
        // SAFETY: every non-empty declared slice remains readable for this call; zero-length cases
        // are rejected before reading the corresponding pointer.
        let result = unsafe {
            aster_forge_cloud_files_macos_identifier_encode(
                namespace.as_ptr(),
                namespace_len,
                root.as_ptr(),
                root_len,
                item.as_ptr(),
                item_len,
            )
        };
        assert_eq!(result.code, MacosErrorCode::InvalidArgument);
        assert!(result.buffer.ptr.is_null());
        assert_eq!(result.buffer.len, 0);
        assert_eq!(result.buffer.capacity, 0);
    }
}

#[test]
fn ffi_large_valid_identity_fields_round_trip_without_truncation() {
    let namespace = vec![b'n'; 4096];
    let root = vec![b'r'; 4096];
    let item = vec![b'i'; 4096];
    // SAFETY: all declared vectors remain readable for this call.
    let encoded = unsafe {
        aster_forge_cloud_files_macos_identifier_encode(
            namespace.as_ptr(),
            namespace.len(),
            root.as_ptr(),
            root.len(),
            item.as_ptr(),
            item.len(),
        )
    };
    assert_eq!(encoded.code, MacosErrorCode::Success);
    assert!(!encoded.buffer.ptr.is_null());
    assert!(encoded.buffer.capacity >= encoded.buffer.len);
    // SAFETY: the encoded allocation remains live and readable until released below.
    let decoded = unsafe {
        aster_forge_cloud_files_macos_identifier_decode(encoded.buffer.ptr, encoded.buffer.len)
    };
    assert_eq!(decoded.code, MacosErrorCode::Success);
    let expected_len = namespace.len() + root.len() + item.len() + 2;
    assert_eq!(decoded.buffer.len, expected_len);
    let bytes = result_bytes(&decoded);
    assert_eq!(&bytes[..namespace.len()], namespace.as_slice());
    assert_eq!(bytes[namespace.len()], 0);
    assert_eq!(
        &bytes[namespace.len() + 1..namespace.len() + 1 + root.len()],
        root.as_slice()
    );
    assert_eq!(bytes[namespace.len() + 1 + root.len()], 0);
    assert_eq!(&bytes[namespace.len() + root.len() + 2..], item.as_slice());
    // SAFETY: each successful allocation is released exactly once with unchanged fields.
    unsafe {
        aster_forge_cloud_files_macos_buffer_release(decoded.buffer);
        aster_forge_cloud_files_macos_buffer_release(encoded.buffer);
    }
}

#[test]
fn ffi_session_handles_fence_generation_closing_and_request_ownership() {
    let invalid = aster_forge_cloud_files_macos_session_create(0);
    assert_eq!(invalid.code, MacosErrorCode::InvalidArgument);
    assert!(invalid.handle.raw.is_null());

    let session = aster_forge_cloud_files_macos_session_create(9);
    assert_eq!(session.code, MacosErrorCode::Success);
    assert!(!session.handle.raw.is_null());
    // SAFETY: `session.handle` remains a live unreleased handle throughout these calls.
    let stale = unsafe { aster_forge_cloud_files_macos_session_begin_request(session.handle, 8) };
    assert_eq!(stale.code, MacosErrorCode::ProviderNotFound);
    assert!(stale.handle.raw.is_null());
    // SAFETY: the session handle remains live and the matching generation is supplied.
    let request = unsafe { aster_forge_cloud_files_macos_session_begin_request(session.handle, 9) };
    assert_eq!(request.code, MacosErrorCode::Success);
    assert!(!request.handle.raw.is_null());
    // SAFETY: the session handle remains live and unreleased.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_begin_closing(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: the live session is now closing, so ingress is classified and rejected.
    let closing = unsafe { aster_forge_cloud_files_macos_session_begin_request(session.handle, 9) };
    assert_eq!(closing.code, MacosErrorCode::ProviderNotFound);
    // SAFETY: the live closing session may transition to draining.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_mark_disconnected(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: each opaque handle is released exactly once with its original value.
    unsafe {
        aster_forge_cloud_files_macos_request_release(request.handle);
        aster_forge_cloud_files_macos_session_release(session.handle);
    }
}

#[test]
fn ffi_null_session_and_request_releases_are_noops_or_classified_errors() {
    let null_session =
        aster_forge_cloud_files_macos_bridge::MacosFfiSessionHandle { raw: ptr::null() };
    // SAFETY: null is explicitly accepted as an invalid classified session handle.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_begin_closing(null_session) },
        MacosErrorCode::InvalidArgument
    );
    // SAFETY: null is explicitly accepted as an invalid classified session handle.
    let request = unsafe { aster_forge_cloud_files_macos_session_begin_request(null_session, 1) };
    assert_eq!(request.code, MacosErrorCode::InvalidArgument);
    assert!(request.handle.raw.is_null());
    // SAFETY: null is explicitly accepted as an invalid classified session handle.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_mark_disconnected(null_session) },
        MacosErrorCode::InvalidArgument
    );
    // SAFETY: null releases are explicitly idempotent no-ops.
    unsafe {
        aster_forge_cloud_files_macos_request_release(
            aster_forge_cloud_files_macos_bridge::MacosFfiRequestHandle {
                raw: ptr::null_mut(),
            },
        );
        aster_forge_cloud_files_macos_session_release(null_session);
    }
}

#[test]
fn ffi_session_zero_generation_and_disconnect_order_are_classified_and_idempotent() {
    let session = aster_forge_cloud_files_macos_session_create(11);
    assert_eq!(session.code, MacosErrorCode::Success);

    // SAFETY: the session handle is live; zero generation is rejected before request acceptance.
    let zero = unsafe { aster_forge_cloud_files_macos_session_begin_request(session.handle, 0) };
    assert_eq!(zero.code, MacosErrorCode::InvalidArgument);
    assert!(zero.handle.raw.is_null());
    // SAFETY: the live accepting session rejects disconnect before closing.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_mark_disconnected(session.handle) },
        MacosErrorCode::ProviderNotFound
    );
    // SAFETY: the same live handle supports idempotent closing.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_begin_closing(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: repeated closing is an idempotent success for the same live handle.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_begin_closing(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: with no active requests the live closing session becomes closed.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_mark_disconnected(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: repeated disconnect is an idempotent success for the same live handle.
    assert_eq!(
        unsafe { aster_forge_cloud_files_macos_session_mark_disconnected(session.handle) },
        MacosErrorCode::Success
    );
    // SAFETY: the session owner remains live, but closed ingress is rejected.
    let closed = unsafe { aster_forge_cloud_files_macos_session_begin_request(session.handle, 11) };
    assert_eq!(closed.code, MacosErrorCode::ProviderNotFound);
    // SAFETY: the session owner is released exactly once.
    unsafe { aster_forge_cloud_files_macos_session_release(session.handle) };
}
