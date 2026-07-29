use aster_forge_cloud_files_core::{
    CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};
use aster_forge_cloud_files_macos_bridge::{
    FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER, FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER,
    FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER, MAX_FILE_PROVIDER_IDENTIFIER_BYTES,
    MAX_IDENTITY_FIELD_BYTES, MacosBridgeError, MacosFileProviderIdentifier,
    MacosFileProviderSystemContainer,
};

fn key(namespace: &str, root: &str, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        CloudScope::new(
            CloudNamespaceId::new(namespace).expect("namespace should be valid"),
            CloudRootId::new(root).expect("root should be valid"),
        ),
        CloudItemId::new(item).expect("item should be valid"),
    )
}

#[test]
fn item_identifier_round_trips_unicode_delimiters_and_path_like_values() {
    let original = key("命名.空间/one", "root:team", "文件/../stable.item");
    let encoded =
        MacosFileProviderIdentifier::encode(&original).expect("identifier fixture should fit");
    assert!(encoded.as_str().starts_with("afcf1."));
    assert!(!encoded.as_str().contains("文件"));
    let decoded =
        MacosFileProviderIdentifier::parse(encoded.as_str()).expect("identifier should decode");
    assert_eq!(
        decoded.item_key().expect("item key should exist"),
        &original
    );
    assert_eq!(decoded.as_str(), encoded.as_str());
}

#[test]
fn system_containers_are_exact_and_never_decode_as_product_items() {
    let cases = [
        (
            FILE_PROVIDER_ROOT_CONTAINER_IDENTIFIER,
            MacosFileProviderSystemContainer::Root,
        ),
        (
            FILE_PROVIDER_CURRENT_WORKING_SET_IDENTIFIER,
            MacosFileProviderSystemContainer::WorkingSet,
        ),
        (
            FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER,
            MacosFileProviderSystemContainer::Trash,
        ),
    ];
    for (raw, expected) in cases {
        let identifier =
            MacosFileProviderIdentifier::parse(raw).expect("system identifier should parse");
        assert_eq!(identifier.system_container(), Some(expected));
        assert_eq!(identifier.as_str(), raw);
        assert!(matches!(
            identifier.item_key(),
            Err(MacosBridgeError::SystemContainerIsNotItem)
        ));
        assert_eq!(identifier.clone().into_string(), raw);
    }
}

#[test]
fn malformed_version_field_count_base64_utf8_and_empty_fields_are_rejected() {
    for value in [
        "",
        "afcf2.YQ.Yg.Yw",
        "afcf1.YQ.Yg",
        "afcf1.YQ.Yg.Yw.ZA",
        "afcf1.YQ.!g.Yw",
        "afcf1.YQ..Yw",
        "afcf1._w.Yg.Yw",
        "afcf1.YQ==.Yg.Yw",
        "afcf1.YR.Yg.Yw",
    ] {
        assert!(matches!(
            MacosFileProviderIdentifier::parse(value),
            Err(MacosBridgeError::InvalidIdentifier { .. })
        ));
    }
}

#[test]
fn identical_item_ids_remain_distinct_across_scopes_and_rename_has_no_identity_effect() {
    let first = MacosFileProviderIdentifier::encode(&key("namespace-a", "root", "same"))
        .expect("identifier fixture should fit");
    let second = MacosFileProviderIdentifier::encode(&key("namespace-b", "root", "same"))
        .expect("identifier fixture should fit");
    assert_ne!(first, second);
    let stable = key("namespace-a", "root", "same");
    assert_eq!(
        MacosFileProviderIdentifier::encode(&stable).expect("identifier fixture should fit"),
        MacosFileProviderIdentifier::encode(&stable).expect("identifier fixture should fit")
    );
    assert!(first.system_container().is_none());
    assert_eq!(first.clone().into_string(), first.as_str());
}

#[test]
fn item_debug_output_reports_shape_without_disclosing_opaque_identity() {
    let identifier = MacosFileProviderIdentifier::encode(&key(
        "sensitive-namespace",
        "sensitive-root",
        "sensitive-item",
    ));
    let debug = format!("{identifier:?}");
    assert!(debug.contains("MacosFileProviderIdentifier"));
    assert!(debug.contains("byte_len"));
    assert!(!debug.contains("sensitive"));

    let system = MacosFileProviderIdentifier::parse(FILE_PROVIDER_TRASH_CONTAINER_IDENTIFIER)
        .expect("trash should parse");
    assert!(format!("{system:?}").contains("Trash"));
}

#[test]
fn identifier_limits_accept_the_boundary_and_reject_boundary_plus_one() {
    let at_limit = key(&"n".repeat(MAX_IDENTITY_FIELD_BYTES), "root", "item");
    let encoded = MacosFileProviderIdentifier::encode(&at_limit)
        .expect("identity field at the limit should encode");
    assert!(encoded.as_str().len() <= MAX_FILE_PROVIDER_IDENTIFIER_BYTES);

    let oversized = key(&"n".repeat(MAX_IDENTITY_FIELD_BYTES + 1), "root", "item");
    assert!(matches!(
        MacosFileProviderIdentifier::encode(&oversized),
        Err(MacosBridgeError::InvalidIdentifier { .. })
    ));
    assert!(matches!(
        MacosFileProviderIdentifier::parse("x".repeat(MAX_FILE_PROVIDER_IDENTIFIER_BYTES + 1)),
        Err(MacosBridgeError::InvalidIdentifier { .. })
    ));
}
