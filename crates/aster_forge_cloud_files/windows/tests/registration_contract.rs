use aster_forge_cloud_files_core::{
    CloudFilesCoreError, CloudItemId, CloudItemKey, CloudNamespaceId, CloudRootId, CloudScope,
};
use aster_forge_cloud_files_windows::{
    CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES, WindowsCloudFilesError, WindowsFileIdentity,
    WindowsHardLinkPolicy, WindowsHydrationPolicy, WindowsHydrationPolicyPrimary,
    WindowsInSyncPolicy, WindowsPlaceholderManagementPolicy, WindowsPlatformVersion,
    WindowsPopulationPolicy, WindowsProviderId, WindowsSyncRootIdentity, WindowsSyncRootPolicies,
    WindowsSyncRootRegistration, WindowsSyncRootRegistrationOptions,
};

const HEADER_LEN: usize = 13;

fn scope(namespace: &str, root: &str) -> CloudScope {
    CloudScope::new(
        CloudNamespaceId::new(namespace).unwrap(),
        CloudRootId::new(root).unwrap(),
    )
}

fn key(namespace: &str, root: &str, item: &str) -> CloudItemKey {
    CloudItemKey::new(scope(namespace, root), CloudItemId::new(item).unwrap())
}

fn provider_id() -> WindowsProviderId {
    WindowsProviderId::new(0x12345678_1234_5678_9abc_def012345678).unwrap()
}

fn platform(integration: u32) -> WindowsPlatformVersion {
    WindowsPlatformVersion {
        build: 10,
        revision: 20,
        integration,
    }
}

fn hydration(primary: WindowsHydrationPolicyPrimary) -> WindowsHydrationPolicy {
    WindowsHydrationPolicy {
        primary,
        validation_required: false,
        streaming_allowed: false,
        auto_dehydration_allowed: false,
        allow_full_restart_hydration: false,
    }
}

fn policies() -> WindowsSyncRootPolicies {
    WindowsSyncRootPolicies {
        hydration: hydration(WindowsHydrationPolicyPrimary::Progressive),
        population: WindowsPopulationPolicy::Full,
        in_sync: WindowsInSyncPolicy::FILE_LAST_WRITE_TIME
            | WindowsInSyncPolicy::DIRECTORY_LAST_WRITE_TIME,
        hard_links: WindowsHardLinkPolicy::Forbidden,
        placeholder_management: WindowsPlaceholderManagementPolicy::default(),
    }
}

#[test]
fn provider_id_requires_stable_nonzero_guid_and_preserves_canonical_value() {
    assert!(matches!(
        WindowsProviderId::new(0),
        Err(WindowsCloudFilesError::EmptyProviderId)
    ));
    let id = provider_id();
    assert_eq!(id.as_u128(), 0x12345678_1234_5678_9abc_def012345678);
    assert_eq!(
        format!("{id:?}"),
        "WindowsProviderId(12345678123456789abcdef012345678)"
    );
}

#[test]
fn sync_root_identity_round_trip_preserves_scope_exact_bytes_and_redacted_debug() {
    let scope = scope(" namespace/保留 ", "root:opaque/路径");
    let identity = WindowsSyncRootIdentity::encode(&scope).unwrap();

    assert_eq!(identity.decode().unwrap(), scope);
    assert!(!identity.is_empty());
    assert_eq!(identity.len(), identity.as_bytes().len());
    assert_eq!(&identity.as_bytes()[..4], b"AFSR");
    assert_eq!(identity.as_bytes()[4], 1);
    assert_eq!(
        format!("{identity:?}"),
        format!(
            "WindowsSyncRootIdentity {{ format_version: 1, byte_len: {} }}",
            identity.len()
        )
    );

    let bytes = identity.clone().into_bytes();
    assert_eq!(
        WindowsSyncRootIdentity::from_bytes(bytes.clone()).unwrap(),
        identity
    );
    assert_eq!(identity.into_bytes(), bytes);
}

#[test]
fn sync_root_identity_accepts_exact_64k_boundary_and_rejects_one_byte_more() {
    let exact_root_len = CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES - HEADER_LEN - 1;
    let exact = WindowsSyncRootIdentity::encode(&scope("n", &"r".repeat(exact_root_len)))
        .expect("exact 64 KiB identity should be valid");
    assert_eq!(exact.len(), CFAPI_SYNC_ROOT_IDENTITY_MAX_BYTES);

    let error = WindowsSyncRootIdentity::encode(&scope("n", &"r".repeat(exact_root_len + 1)))
        .expect_err("one byte above 64 KiB should fail");
    assert!(matches!(
        error,
        WindowsCloudFilesError::SyncRootIdentityTooLarge {
            actual: 65537,
            maximum: 65536,
        }
    ));

    let error = WindowsSyncRootIdentity::from_bytes(vec![0; 65_537])
        .expect_err("oversized imported identity should fail before parsing");
    assert!(matches!(
        error,
        WindowsCloudFilesError::SyncRootIdentityTooLarge {
            actual: 65537,
            maximum: 65536,
        }
    ));
}

#[test]
fn malformed_sync_root_identity_envelopes_are_rejected() {
    let valid = WindowsSyncRootIdentity::encode(&scope("n", "r"))
        .unwrap()
        .into_bytes();

    for bytes in [Vec::new(), valid[..HEADER_LEN - 1].to_vec()] {
        assert_invalid_sync_root(bytes, "identity envelope is truncated");
    }

    let mut magic = valid.clone();
    magic[0] = b'X';
    assert_invalid_sync_root(magic, "identity envelope magic does not match");

    let mut version = valid.clone();
    version[4] = 2;
    assert_invalid_sync_root(version, "identity envelope version is unsupported");

    let mut length = valid.clone();
    length[5..9].copy_from_slice(&2u32.to_le_bytes());
    assert_invalid_sync_root(length, "identity field lengths do not match the envelope");

    let mut trailing = valid.clone();
    trailing.push(0);
    assert_invalid_sync_root(trailing, "identity field lengths do not match the envelope");

    let mut utf8 = valid;
    utf8[HEADER_LEN] = 0xff;
    assert_invalid_sync_root(utf8, "identity field is not UTF-8");
}

#[test]
fn decoded_empty_sync_root_field_keeps_core_classification() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"AFSR");
    bytes.push(1);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(b"r");

    let error = WindowsSyncRootIdentity::from_bytes(bytes).unwrap_err();
    assert!(matches!(
        error,
        WindowsCloudFilesError::Core(CloudFilesCoreError::EmptyValue {
            field: "cloud namespace id",
        })
    ));
}

#[test]
fn hydration_policy_rejects_conflict_and_gates_full_restart_integration() {
    for primary in [
        WindowsHydrationPolicyPrimary::Progressive,
        WindowsHydrationPolicyPrimary::Full,
        WindowsHydrationPolicyPrimary::AlwaysFull,
    ] {
        let conflict = WindowsHydrationPolicy {
            primary,
            validation_required: true,
            streaming_allowed: true,
            auto_dehydration_allowed: false,
            allow_full_restart_hydration: false,
        };
        assert!(matches!(
            conflict.validate(platform(0x500)),
            Err(WindowsCloudFilesError::InvalidSyncRootPolicy { .. })
        ));
    }

    let restart = WindowsHydrationPolicy {
        allow_full_restart_hydration: true,
        ..hydration(WindowsHydrationPolicyPrimary::Progressive)
    };
    assert!(matches!(
        restart.validate(platform(0x4ff)),
        Err(WindowsCloudFilesError::UnsupportedPlatformIntegration {
            feature: "allow-full-restart-hydration",
            required: 0x500,
            actual: 0x4ff,
        })
    ));
    restart
        .validate(platform(0x500))
        .expect("exact required integration should pass");
}

#[test]
fn placeholder_management_gates_each_unrestricted_permission() {
    let permission_sets = [
        WindowsPlaceholderManagementPolicy {
            create_unrestricted: true,
            ..Default::default()
        },
        WindowsPlaceholderManagementPolicy {
            convert_unrestricted: true,
            ..Default::default()
        },
        WindowsPlaceholderManagementPolicy {
            update_unrestricted: true,
            ..Default::default()
        },
        WindowsPlaceholderManagementPolicy {
            create_unrestricted: true,
            convert_unrestricted: true,
            update_unrestricted: true,
        },
    ];

    assert!(!WindowsPlaceholderManagementPolicy::default().is_unrestricted());
    for placeholder_management in permission_sets {
        assert!(placeholder_management.is_unrestricted());
        let policy = WindowsSyncRootPolicies {
            placeholder_management,
            ..policies()
        };
        assert!(matches!(
            policy.validate(platform(0x30f)),
            Err(WindowsCloudFilesError::UnsupportedPlatformIntegration {
                feature: "unrestricted-placeholder-management",
                required: 0x310,
                actual: 0x30f,
            })
        ));
        policy
            .validate(platform(0x310))
            .expect("exact required integration should pass");
    }
}

#[test]
fn in_sync_policy_composes_every_exposed_bit_without_hidden_normalization() {
    let all = WindowsInSyncPolicy::FILE_CREATION_TIME
        | WindowsInSyncPolicy::FILE_READ_ONLY
        | WindowsInSyncPolicy::FILE_HIDDEN
        | WindowsInSyncPolicy::FILE_SYSTEM
        | WindowsInSyncPolicy::DIRECTORY_CREATION_TIME
        | WindowsInSyncPolicy::DIRECTORY_READ_ONLY
        | WindowsInSyncPolicy::DIRECTORY_HIDDEN
        | WindowsInSyncPolicy::DIRECTORY_SYSTEM
        | WindowsInSyncPolicy::FILE_LAST_WRITE_TIME
        | WindowsInSyncPolicy::DIRECTORY_LAST_WRITE_TIME
        | WindowsInSyncPolicy::PRESERVE_FOR_SYNC_ENGINE;

    assert_eq!(WindowsInSyncPolicy::default(), WindowsInSyncPolicy::NONE);
    assert!(all.contains(WindowsInSyncPolicy::FILE_CREATION_TIME));
    assert!(all.contains(WindowsInSyncPolicy::DIRECTORY_LAST_WRITE_TIME));
    assert!(all.contains(WindowsInSyncPolicy::PRESERVE_FOR_SYNC_ENGINE));
    assert!(all.contains(WindowsInSyncPolicy::NONE));
    assert_eq!(all.bits(), 0x8000_03ff);
}

#[test]
fn registration_text_uses_utf16_code_unit_boundary_and_rejects_empty_or_nul() {
    for (field, name, version) in [
        ("provider name", "", "1"),
        ("provider version", "Forge", ""),
        ("provider name", "bad\0name", "1"),
        ("provider version", "Forge", "bad\0version"),
    ] {
        let error = registration(name, version).unwrap_err();
        assert!(matches!(
            error,
            WindowsCloudFilesError::InvalidRegistrationString { field: actual, .. }
                if actual == field
        ));
    }

    registration(&"a".repeat(255), &"😀".repeat(127))
        .expect("255 ASCII and 254 UTF-16 units should pass");
    registration(&("😀".repeat(127) + "a"), "1").expect("exact 255 UTF-16 units should pass");

    for (field, name, version) in [
        ("provider name", "a".repeat(256), "1".to_owned()),
        ("provider version", "Forge".to_owned(), "😀".repeat(128)),
    ] {
        let error = registration(&name, &version).unwrap_err();
        assert!(matches!(
            error,
            WindowsCloudFilesError::InvalidRegistrationString { field: actual, .. }
                if actual == field
        ));
    }
}

#[test]
fn registration_rejects_root_file_identity_from_another_scope() {
    let other_identity = WindowsFileIdentity::encode(&key("ns", "other-root", "root-item"))
        .expect("identity fixture should encode");
    let error = registration_with_root_identity("Forge", "1.0", other_identity).unwrap_err();
    assert!(matches!(
        error,
        WindowsCloudFilesError::RootFileIdentityScopeMismatch
    ));
}

#[test]
fn registration_accessors_and_owned_parts_preserve_complete_contract() {
    let file_identity = WindowsFileIdentity::encode(&key("ns", "root", "root-item")).unwrap();
    let registration =
        registration_with_root_identity("Forge Cloud Files", "1.2.3", file_identity.clone())
            .expect("registration fixture should be valid");

    assert_eq!(registration.provider_name(), "Forge Cloud Files");
    assert_eq!(registration.provider_version(), "1.2.3");
    assert_eq!(registration.provider_id(), provider_id());
    assert_eq!(
        registration.sync_root_identity().decode().unwrap(),
        scope("ns", "root")
    );
    assert_eq!(registration.root_file_identity(), &file_identity);
    assert_eq!(registration.policies(), policies());
    assert_eq!(
        registration.options(),
        WindowsSyncRootRegistrationOptions {
            update_existing: true,
            disable_on_demand_population_on_root: true,
            mark_in_sync_on_root: true,
        }
    );
    registration
        .validate_for_platform(platform(0x500))
        .expect("modern platform should accept baseline policies");

    let (name, version, id, sync_identity, root_identity, actual_policies, options) =
        registration.into_parts();
    assert_eq!(name, "Forge Cloud Files");
    assert_eq!(version, "1.2.3");
    assert_eq!(id, provider_id());
    assert_eq!(sync_identity.decode().unwrap(), scope("ns", "root"));
    assert_eq!(root_identity, file_identity);
    assert_eq!(actual_policies, policies());
    assert!(options.update_existing);
    assert!(options.disable_on_demand_population_on_root);
    assert!(options.mark_in_sync_on_root);
}

#[test]
fn every_primary_population_and_hard_link_shape_validates_on_modern_platform() {
    for primary in [
        WindowsHydrationPolicyPrimary::Progressive,
        WindowsHydrationPolicyPrimary::Full,
        WindowsHydrationPolicyPrimary::AlwaysFull,
    ] {
        for population in [
            WindowsPopulationPolicy::Full,
            WindowsPopulationPolicy::AlwaysFull,
        ] {
            for hard_links in [
                WindowsHardLinkPolicy::Forbidden,
                WindowsHardLinkPolicy::Allowed,
            ] {
                WindowsSyncRootPolicies {
                    hydration: WindowsHydrationPolicy {
                        primary,
                        validation_required: false,
                        streaming_allowed: true,
                        auto_dehydration_allowed: true,
                        allow_full_restart_hydration: true,
                    },
                    population,
                    in_sync: WindowsInSyncPolicy::NONE,
                    hard_links,
                    placeholder_management: WindowsPlaceholderManagementPolicy {
                        create_unrestricted: true,
                        convert_unrestricted: true,
                        update_unrestricted: true,
                    },
                }
                .validate(platform(0x500))
                .expect("every exposed shape should map on a modern platform");
            }
        }
    }
}

fn registration(
    name: &str,
    version: &str,
) -> Result<WindowsSyncRootRegistration, WindowsCloudFilesError> {
    registration_with_root_identity(
        name,
        version,
        WindowsFileIdentity::encode(&key("ns", "root", "root-item"))
            .expect("default root identity fixture should encode"),
    )
}

fn registration_with_root_identity(
    name: &str,
    version: &str,
    root_file_identity: WindowsFileIdentity,
) -> Result<WindowsSyncRootRegistration, WindowsCloudFilesError> {
    WindowsSyncRootRegistration::new(
        name,
        version,
        provider_id(),
        WindowsSyncRootIdentity::encode(&scope("ns", "root")).unwrap(),
        root_file_identity,
        policies(),
        WindowsSyncRootRegistrationOptions {
            update_existing: true,
            disable_on_demand_population_on_root: true,
            mark_in_sync_on_root: true,
        },
    )
}

fn assert_invalid_sync_root(bytes: Vec<u8>, expected_reason: &'static str) {
    let error = WindowsSyncRootIdentity::from_bytes(bytes).unwrap_err();
    assert!(matches!(
        error,
        WindowsCloudFilesError::InvalidSyncRootIdentity { reason }
            if reason == expected_reason
    ));
}
