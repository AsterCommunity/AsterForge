use aster_forge_cloud_files_core::{
    CloudFilesCoreError, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};
use aster_forge_cloud_files_windows::{
    CFAPI_FILE_IDENTITY_MAX_BYTES, WindowsCloudFilesError, WindowsFileIdentity,
};

const HEADER_LEN: usize = 17;

fn key(namespace: &str, root: &str, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        CloudScope::new(
            CloudNamespaceId::new(namespace).expect("namespace fixture should be valid"),
            CloudRootId::new(root).expect("root fixture should be valid"),
        ),
        CloudItemId::new(item).expect("item fixture should be valid"),
    )
}

#[test]
fn identity_round_trip_preserves_scope_unicode_delimiters_and_exact_bytes() {
    let key = key(" namespace/保留 ", "root\0/root", "item:路径/opaque");
    let identity = WindowsFileIdentity::encode(&key).expect("identity should encode");

    assert_eq!(identity.decode().expect("identity should decode"), key);
    assert!(!identity.is_empty());
    assert_eq!(identity.len(), identity.as_bytes().len());
    assert_eq!(&identity.as_bytes()[..4], b"AFCF");
    assert_eq!(identity.as_bytes()[4], 1);
    assert_eq!(
        format!("{identity:?}"),
        format!(
            "WindowsFileIdentity {{ format_version: 1, byte_len: {} }}",
            identity.len()
        )
    );

    let bytes = identity.clone().into_bytes();
    assert_eq!(
        WindowsFileIdentity::from_bytes(bytes.clone()).unwrap(),
        identity
    );
    assert_eq!(identity.into_bytes(), bytes);
}

#[test]
fn identity_is_path_independent_and_scope_sensitive() {
    let original = key("ns", "root-a", "stable-item");
    let renamed = original.clone();
    let other_root = key("ns", "root-b", "stable-item");
    let other_namespace = key("other-ns", "root-a", "stable-item");

    assert_eq!(
        WindowsFileIdentity::encode(&original).unwrap(),
        WindowsFileIdentity::encode(&renamed).unwrap()
    );
    assert_ne!(
        WindowsFileIdentity::encode(&original).unwrap(),
        WindowsFileIdentity::encode(&other_root).unwrap()
    );
    assert_ne!(
        WindowsFileIdentity::encode(&original).unwrap(),
        WindowsFileIdentity::encode(&other_namespace).unwrap()
    );
}

#[test]
fn identity_accepts_exact_cfapi_limit_and_rejects_one_byte_more() {
    let exact_item_len = CFAPI_FILE_IDENTITY_MAX_BYTES - HEADER_LEN - 2;
    let exact = WindowsFileIdentity::encode(&key("n", "r", &"i".repeat(exact_item_len)))
        .expect("exact CFAPI limit should be valid");
    assert_eq!(exact.len(), CFAPI_FILE_IDENTITY_MAX_BYTES);

    let error = WindowsFileIdentity::encode(&key("n", "r", &"i".repeat(exact_item_len + 1)))
        .expect_err("one byte above the CFAPI limit should fail");
    assert!(matches!(
        error,
        WindowsCloudFilesError::FileIdentityTooLarge {
            actual: 4097,
            maximum: 4096,
        }
    ));

    let error = WindowsFileIdentity::from_bytes(vec![0; CFAPI_FILE_IDENTITY_MAX_BYTES + 1])
        .expect_err("oversized imported bytes should fail before parsing");
    assert!(matches!(
        error,
        WindowsCloudFilesError::FileIdentityTooLarge {
            actual: 4097,
            maximum: 4096,
        }
    ));
}

#[test]
fn malformed_identity_envelopes_are_rejected_precisely() {
    let valid = WindowsFileIdentity::encode(&key("n", "r", "i"))
        .expect("identity fixture should encode")
        .into_bytes();

    for bytes in [Vec::new(), valid[..HEADER_LEN - 1].to_vec()] {
        assert_invalid(bytes, "identity envelope is truncated");
    }

    let mut wrong_magic = valid.clone();
    wrong_magic[0] = b'X';
    assert_invalid(wrong_magic, "identity envelope magic does not match");

    let mut wrong_version = valid.clone();
    wrong_version[4] = 2;
    assert_invalid(wrong_version, "identity envelope version is unsupported");

    let mut wrong_lengths = valid.clone();
    wrong_lengths[5..9].copy_from_slice(&2u32.to_le_bytes());
    assert_invalid(
        wrong_lengths,
        "identity field lengths do not match the envelope",
    );

    let mut trailing = valid.clone();
    trailing.push(0);
    assert_invalid(trailing, "identity field lengths do not match the envelope");

    let mut invalid_utf8 = valid;
    invalid_utf8[HEADER_LEN] = 0xff;
    assert_invalid(invalid_utf8, "identity field is not UTF-8");
}

#[test]
fn decoded_empty_core_identity_keeps_core_field_classification() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"AFCF");
    bytes.push(1);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(b"ri");

    let error = WindowsFileIdentity::from_bytes(bytes)
        .expect_err("empty namespace must be rejected by the core identity contract");
    assert!(matches!(
        error,
        WindowsCloudFilesError::Core(CloudFilesCoreError::EmptyValue {
            field: "cloud namespace id",
        })
    ));
}

fn assert_invalid(bytes: Vec<u8>, expected_reason: &'static str) {
    let error = WindowsFileIdentity::from_bytes(bytes).expect_err("identity should be invalid");
    assert!(matches!(
        error,
        WindowsCloudFilesError::InvalidFileIdentity { reason } if reason == expected_reason
    ));
}
