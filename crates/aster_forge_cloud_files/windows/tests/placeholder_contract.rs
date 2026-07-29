use aster_forge_cloud_files_core::{
    CloudContentMetadata, CloudItem, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId,
    CloudScope, ContentRevision, MetadataRevision,
};
use aster_forge_cloud_files_windows::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, WindowsCloudFilesError, WindowsFileIdentity,
    WindowsFileTimes, WindowsPlaceholder, WindowsPlaceholderMetadata, WindowsPlaceholderOptions,
};

fn key(item: &str) -> CloudItemKey {
    CloudItemKey::new(
        CloudScope::new(
            CloudNamespaceId::new("ns").unwrap(),
            CloudRootId::new("root").unwrap(),
        ),
        CloudItemId::new(item).unwrap(),
    )
}

fn file(item: &str, name: &str, size: u64) -> CloudItem {
    CloudItem::file(
        key(item),
        CloudItemId::new("parent").unwrap(),
        name,
        MetadataRevision::new("metadata-1").unwrap(),
        CloudContentMetadata::new(ContentRevision::new("content-1").unwrap(), None, size),
    )
    .unwrap()
}

fn directory(item: &str, name: &str) -> CloudItem {
    CloudItem::directory(
        key(item),
        Some(CloudItemId::new("parent").unwrap()),
        name,
        MetadataRevision::new("metadata-1").unwrap(),
    )
    .unwrap()
}

#[test]
fn file_and_directory_metadata_map_exact_cfapi_shape() {
    let times = WindowsFileTimes {
        creation: 1,
        last_access: 2,
        last_write: 3,
        change: 4,
    };
    let file_metadata = WindowsPlaceholderMetadata::from_item(&file("f", "file.txt", 42), times)
        .expect("file metadata should map");
    assert_eq!(file_metadata.file_size(), 42);
    assert_eq!(file_metadata.file_attributes(), FILE_ATTRIBUTE_NORMAL);
    assert_eq!(file_metadata.times(), times);

    let directory_metadata =
        WindowsPlaceholderMetadata::from_item(&directory("d", "folder"), times)
            .expect("directory metadata should map");
    assert_eq!(directory_metadata.file_size(), 0);
    assert_eq!(
        directory_metadata.file_attributes(),
        FILE_ATTRIBUTE_DIRECTORY
    );
    assert_eq!(directory_metadata.times(), times);
}

#[test]
fn file_size_must_fit_signed_cfapi_metadata() {
    let item = file("large", "large.bin", i64::MAX as u64 + 1);
    let error = WindowsPlaceholderMetadata::from_item(&item, WindowsFileTimes::default())
        .expect_err("CFAPI file size is signed");
    assert!(matches!(
        error,
        WindowsCloudFilesError::FileSizeTooLarge { size } if size == i64::MAX as u64 + 1
    ));
}

#[test]
fn placeholder_binds_identity_name_metadata_options_and_owned_parts() {
    let item = file("item", "报告 2026.txt", 123);
    let identity = WindowsFileIdentity::encode(item.key()).unwrap();
    let times = WindowsFileTimes {
        creation: 11,
        last_access: 12,
        last_write: 13,
        change: 14,
    };
    let options = WindowsPlaceholderOptions {
        mark_in_sync: true,
        disable_on_demand_population: true,
        always_full: false,
        supersede: true,
    };
    let placeholder = WindowsPlaceholder::from_item(&item, identity.clone(), times, options)
        .expect("placeholder should validate");

    assert_eq!(placeholder.relative_name(), "报告 2026.txt");
    assert_eq!(placeholder.identity(), &identity);
    assert_eq!(placeholder.metadata().file_size(), 123);
    assert_eq!(placeholder.options(), options);

    let (name, metadata, consumed_identity, consumed_options) = placeholder.into_parts();
    assert_eq!(name, "报告 2026.txt");
    assert_eq!(metadata.file_size(), 123);
    assert_eq!(consumed_identity, identity);
    assert_eq!(consumed_options, options);
}

#[test]
fn placeholder_rejects_identity_for_another_item() {
    let item = file("item-a", "file.txt", 1);
    let other_identity = WindowsFileIdentity::encode(&key("item-b")).unwrap();
    let error = WindowsPlaceholder::from_item(
        &item,
        other_identity,
        WindowsFileTimes::default(),
        WindowsPlaceholderOptions::default(),
    )
    .expect_err("placeholder identity must describe the exact item");
    assert!(matches!(
        error,
        WindowsCloudFilesError::IdentityItemMismatch
    ));
}

#[test]
fn placeholder_rejects_every_windows_reserved_name_boundary() {
    let invalid_names = [
        ".",
        "..",
        "trailing-space ",
        "trailing-dot.",
        "a/b",
        "a\\b",
        "a:b",
        "a*b",
        "a?b",
        "a\"b",
        "a<b",
        "a>b",
        "a|b",
        "control\u{1f}",
        "CON",
        "con.txt",
        "PRN.log",
        "AUX",
        "NUL.data",
        "COM1",
        "COM¹",
        "com².txt",
        "COM³",
        "com9.txt",
        "LPT1",
        "LPT¹",
        "lpt².log",
        "LPT³",
        "lpt9.bin",
    ];

    for (index, name) in invalid_names.into_iter().enumerate() {
        let item = file(&format!("item-{index}"), name, 1);
        let identity = WindowsFileIdentity::encode(item.key()).unwrap();
        let error = WindowsPlaceholder::from_item(
            &item,
            identity,
            WindowsFileTimes::default(),
            WindowsPlaceholderOptions::default(),
        )
        .expect_err("reserved Windows name should fail");
        assert!(
            matches!(error, WindowsCloudFilesError::InvalidPlaceholderName { .. }),
            "unexpected error for {name:?}: {error:?}"
        );
    }
}

#[test]
fn similar_non_reserved_device_names_remain_valid() {
    for (index, name) in ["COM0", "COM10", "LPT0", "LPT10", "CONSOLE", "NULLED"]
        .into_iter()
        .enumerate()
    {
        let item = file(&format!("valid-{index}"), name, 1);
        let identity = WindowsFileIdentity::encode(item.key()).unwrap();
        WindowsPlaceholder::from_item(
            &item,
            identity,
            WindowsFileTimes::default(),
            WindowsPlaceholderOptions::default(),
        )
        .expect("similar non-device name should remain valid");
    }
}
