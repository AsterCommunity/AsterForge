use std::collections::HashSet;
use std::num::NonZeroUsize;

use aster_forge_cloud_files_core::{
    CloudFilesCoreError, CloudFilesLimits, CloudItemId, CloudItemKey, CloudNamespaceId,
    CloudRootId, CloudScope,
};

fn key(namespace: &str, root: &str, item: &str) -> CloudItemKey {
    CloudItemKey::new(
        CloudScope::new(
            CloudNamespaceId::new(namespace).expect("fixture namespace should be valid"),
            CloudRootId::new(root).expect("fixture root should be valid"),
        ),
        CloudItemId::new(item).expect("fixture item should be valid"),
    )
}

#[test]
fn identity_values_reject_only_empty_opaque_values() {
    assert_eq!(
        CloudNamespaceId::new(""),
        Err(CloudFilesCoreError::EmptyValue {
            field: "cloud namespace id",
        })
    );
    assert!(CloudRootId::new(" ").is_ok());
    assert!(CloudItemId::new("opaque/item/id").is_ok());
}

#[test]
fn identical_backend_item_ids_do_not_collide_across_scopes() {
    let personal = key("provider-a", "root-1", "same-remote-id");
    let other_namespace = key("provider-b", "root-1", "same-remote-id");
    let other_root = key("provider-a", "root-2", "same-remote-id");

    let identities = HashSet::from([personal, other_namespace, other_root]);
    assert_eq!(identities.len(), 3);
}

#[test]
fn item_key_contains_no_path_component_and_survives_rename_state() {
    let before = key("provider", "root", "stable-id");
    let after = before.clone();

    let old_parent_and_name = ("parent-a", "old-name.txt");
    let new_parent_and_name = ("parent-b", "new-name.txt");

    assert_ne!(old_parent_and_name, new_parent_and_name);
    assert_eq!(before, after);
    assert_eq!(after.item_id().as_str(), "stable-id");
}

#[test]
fn native_identity_limit_is_adapter_specific() {
    let windows_limits = CloudFilesLimits {
        native_identity_max_bytes: NonZeroUsize::new(4096),
        ..CloudFilesLimits::default()
    };

    assert!(
        windows_limits
            .validate_native_identity(&vec![0; 4096])
            .is_ok()
    );
    assert_eq!(
        windows_limits.validate_native_identity(&vec![0; 4097]),
        Err(CloudFilesCoreError::NativeIdentityTooLarge {
            actual_bytes: 4097,
            max_bytes: 4096,
        })
    );
    assert!(
        CloudFilesLimits::default()
            .validate_native_identity(&vec![0; 8192])
            .is_ok()
    );
}
