use std::num::NonZeroUsize;

use aster_forge_cloud_files_core::{
    Alignment, AnchoredChangeCapabilities, CancellationLevel, ChangeCapabilities,
    CloudFilesCapabilities, CloudFilesCoreError, CloudFilesLimits, ContentStorageModes,
    EnumerationCapabilities, EvictionCapabilities, HydrationCapabilities, IdentityCapabilities,
    MaterializationCapabilities, MutationCapabilities, NamingCapabilities,
    RangeHydrationCapabilities, RevisionCapabilities,
};

#[test]
fn effective_capabilities_keep_only_shared_operations() {
    let backend = CloudFilesCapabilities {
        identity: IdentityCapabilities {
            stable_item_ids: true,
            path_independent_item_ids: true,
            same_root_move_preserves_identity: true,
            cross_root_move_preserves_identity: true,
        },
        changes: ChangeCapabilities {
            anchored: Some(AnchoredChangeCapabilities {
                cursor_reset: true,
                tombstones: true,
            }),
            snapshot_diff: true,
            external_invalidation: true,
        },
        hydration: HydrationCapabilities {
            whole_file: true,
            range: Some(RangeHydrationCapabilities {
                alignment: Alignment::new(1024).expect("alignment fixture should be valid"),
                partial_cancel: true,
            }),
            ..HydrationCapabilities::default()
        },
        materialization: MaterializationCapabilities {
            storage_modes: ContentStorageModes {
                platform_managed: true,
                provider_managed: true,
                hybrid: true,
            },
        },
        cancellation: CancellationLevel::RangeSubset,
        limits: CloudFilesLimits {
            native_identity_max_bytes: NonZeroUsize::new(8192),
            max_in_flight_requests: NonZeroUsize::new(128),
            ..CloudFilesLimits::default()
        },
        ..CloudFilesCapabilities::default()
    };
    let platform = CloudFilesCapabilities {
        identity: IdentityCapabilities {
            stable_item_ids: true,
            path_independent_item_ids: true,
            same_root_move_preserves_identity: true,
            cross_root_move_preserves_identity: false,
        },
        changes: ChangeCapabilities {
            anchored: Some(AnchoredChangeCapabilities {
                cursor_reset: true,
                tombstones: false,
            }),
            snapshot_diff: false,
            external_invalidation: true,
        },
        hydration: HydrationCapabilities {
            whole_file: true,
            range: Some(RangeHydrationCapabilities {
                alignment: Alignment::new(4096).expect("alignment fixture should be valid"),
                partial_cancel: false,
            }),
            ..HydrationCapabilities::default()
        },
        materialization: MaterializationCapabilities {
            storage_modes: ContentStorageModes {
                platform_managed: true,
                provider_managed: false,
                hybrid: true,
            },
        },
        cancellation: CancellationLevel::ExactRequest,
        limits: CloudFilesLimits {
            native_identity_max_bytes: NonZeroUsize::new(4096),
            max_in_flight_requests: NonZeroUsize::new(32),
            ..CloudFilesLimits::default()
        },
        ..CloudFilesCapabilities::default()
    };

    let effective = backend
        .intersection(platform)
        .expect("capability intersection should succeed");

    effective
        .validate_core_requirements()
        .expect("required identity capabilities should remain available");
    assert!(!effective.identity.cross_root_move_preserves_identity);
    assert_eq!(
        effective.changes.anchored,
        Some(AnchoredChangeCapabilities {
            cursor_reset: true,
            tombstones: false,
        })
    );
    assert!(!effective.changes.snapshot_diff);
    assert!(effective.changes.external_invalidation);
    assert_eq!(
        effective.hydration.range,
        Some(RangeHydrationCapabilities {
            alignment: Alignment::new(4096).expect("alignment fixture should be valid"),
            partial_cancel: false,
        })
    );
    assert_eq!(effective.cancellation, CancellationLevel::ExactRequest);
    assert_eq!(
        effective.limits.native_identity_max_bytes,
        NonZeroUsize::new(4096)
    );
    assert_eq!(
        effective.limits.max_in_flight_requests,
        NonZeroUsize::new(32)
    );
    assert_eq!(
        effective.materialization.storage_modes,
        ContentStorageModes {
            platform_managed: true,
            provider_managed: false,
            hybrid: true,
        }
    );
}

#[test]
fn alignment_intersection_uses_least_common_multiple() {
    let left = Alignment::new(4096).expect("alignment fixture should be valid");
    let right = Alignment::new(6144).expect("alignment fixture should be valid");

    assert_eq!(
        left.intersection(right)
            .expect("alignment intersection should succeed")
            .get(),
        12_288
    );
}

#[test]
fn alignment_intersection_reports_overflow() {
    let left = Alignment::new(u64::MAX).expect("alignment fixture should be valid");
    let right = Alignment::new(u64::MAX - 1).expect("alignment fixture should be valid");

    assert_eq!(
        left.intersection(right),
        Err(CloudFilesCoreError::AlignmentIntersectionOverflow {
            left: u64::MAX,
            right: u64::MAX - 1,
        })
    );
}

#[test]
fn unsupported_optional_operations_remain_normal_state() {
    let effective = CloudFilesCapabilities::default()
        .intersection(CloudFilesCapabilities::default())
        .expect("empty capability intersection should succeed");

    assert!(!effective.hydration.whole_file);
    assert!(effective.hydration.range.is_none());
    assert!(!effective.materialization.storage_modes.is_supported());
    assert_eq!(effective.cancellation, CancellationLevel::None);
}

#[test]
fn unconstrained_capabilities_are_the_intersection_identity() {
    let capabilities = CloudFilesCapabilities {
        identity: IdentityCapabilities {
            stable_item_ids: true,
            path_independent_item_ids: true,
            same_root_move_preserves_identity: true,
            cross_root_move_preserves_identity: false,
        },
        hydration: HydrationCapabilities {
            whole_file: true,
            range: Some(RangeHydrationCapabilities {
                alignment: Alignment::new(4096).expect("alignment fixture should be valid"),
                partial_cancel: false,
            }),
            ..HydrationCapabilities::default()
        },
        cancellation: CancellationLevel::BestEffortRequest,
        limits: CloudFilesLimits {
            native_identity_max_bytes: NonZeroUsize::new(4096),
            ..CloudFilesLimits::default()
        },
        ..CloudFilesCapabilities::default()
    };

    assert_eq!(
        capabilities
            .intersection(CloudFilesCapabilities::unconstrained())
            .expect("identity intersection should succeed"),
        capabilities
    );
}

#[test]
fn core_rejects_unstable_or_path_derived_identity_contracts() {
    let unstable = IdentityCapabilities {
        stable_item_ids: false,
        path_independent_item_ids: true,
        same_root_move_preserves_identity: true,
        cross_root_move_preserves_identity: false,
    };
    assert_eq!(
        unstable.validate_core_requirements(),
        Err(CloudFilesCoreError::MissingRequiredCapability {
            capability: "stable_item_ids",
        })
    );

    let path_derived = IdentityCapabilities {
        stable_item_ids: true,
        path_independent_item_ids: false,
        same_root_move_preserves_identity: true,
        cross_root_move_preserves_identity: false,
    };
    assert_eq!(
        path_derived.validate_core_requirements(),
        Err(CloudFilesCoreError::MissingRequiredCapability {
            capability: "path_independent_item_ids",
        })
    );

    let move_unstable = IdentityCapabilities {
        stable_item_ids: true,
        path_independent_item_ids: true,
        same_root_move_preserves_identity: false,
        cross_root_move_preserves_identity: true,
    };
    assert_eq!(
        move_unstable.validate_core_requirements(),
        Err(CloudFilesCoreError::MissingRequiredCapability {
            capability: "same_root_move_preserves_identity",
        })
    );
}

#[test]
fn alignment_handles_zero_identity_equal_multiple_and_coprime_boundaries() {
    assert_eq!(Alignment::new(0), None);
    assert_eq!(Alignment::ONE.get(), 1);

    let six = Alignment::new(6).expect("alignment fixture should be valid");
    let same = six
        .intersection(six)
        .expect("equal alignment intersection should succeed");
    assert_eq!(same.get(), 6);

    let multiple = Alignment::new(24).expect("alignment fixture should be valid");
    assert_eq!(
        six.intersection(multiple)
            .expect("multiple alignment intersection should succeed")
            .get(),
        24
    );
    assert_eq!(
        Alignment::ONE
            .intersection(multiple)
            .expect("identity alignment intersection should succeed")
            .get(),
        24
    );
    assert_eq!(
        Alignment::new(5)
            .expect("alignment fixture should be valid")
            .intersection(Alignment::new(7).expect("alignment fixture should be valid"))
            .expect("coprime alignment intersection should succeed")
            .get(),
        35
    );
}

#[test]
fn every_boolean_capability_dimension_uses_logical_intersection() {
    let identity_left = IdentityCapabilities {
        stable_item_ids: true,
        path_independent_item_ids: false,
        same_root_move_preserves_identity: true,
        cross_root_move_preserves_identity: false,
    };
    let identity_right = IdentityCapabilities {
        stable_item_ids: true,
        path_independent_item_ids: true,
        same_root_move_preserves_identity: false,
        cross_root_move_preserves_identity: false,
    };
    assert_eq!(
        identity_left.intersection(identity_right),
        IdentityCapabilities {
            stable_item_ids: true,
            path_independent_item_ids: false,
            same_root_move_preserves_identity: false,
            cross_root_move_preserves_identity: false,
        }
    );

    assert_eq!(
        RevisionCapabilities {
            separate_metadata_and_content: true,
            conditional_metadata_mutation: false,
            conditional_content_access: true,
        }
        .intersection(RevisionCapabilities {
            separate_metadata_and_content: true,
            conditional_metadata_mutation: true,
            conditional_content_access: false,
        }),
        RevisionCapabilities {
            separate_metadata_and_content: true,
            conditional_metadata_mutation: false,
            conditional_content_access: false,
        }
    );
    assert!(
        !RevisionCapabilities {
            separate_metadata_and_content: false,
            conditional_metadata_mutation: true,
            conditional_content_access: false,
        }
        .intersection(RevisionCapabilities {
            separate_metadata_and_content: false,
            conditional_metadata_mutation: false,
            conditional_content_access: false,
        })
        .conditional_metadata_mutation
    );

    assert_eq!(
        EnumerationCapabilities {
            paged: true,
            stable_snapshot: false,
        }
        .intersection(EnumerationCapabilities {
            paged: false,
            stable_snapshot: true,
        }),
        EnumerationCapabilities::default()
    );

    assert_eq!(
        MutationCapabilities {
            create: true,
            modify_metadata: false,
            modify_content: true,
            delete: false,
            atomic_move: true,
            cross_root_move: false,
            resumable_upload: true,
        }
        .intersection(MutationCapabilities {
            create: true,
            modify_metadata: true,
            modify_content: false,
            delete: false,
            atomic_move: false,
            cross_root_move: false,
            resumable_upload: true,
        }),
        MutationCapabilities {
            create: true,
            modify_metadata: false,
            modify_content: false,
            delete: false,
            atomic_move: false,
            cross_root_move: false,
            resumable_upload: true,
        }
    );

    assert_eq!(
        NamingCapabilities {
            case_sensitive: true,
            case_preserving: false,
            normalization_preserving: true,
        }
        .intersection(NamingCapabilities {
            case_sensitive: false,
            case_preserving: true,
            normalization_preserving: true,
        }),
        NamingCapabilities {
            case_sensitive: false,
            case_preserving: false,
            normalization_preserving: true,
        }
    );
}

#[test]
fn change_and_hydration_optional_capabilities_require_both_boundaries() {
    let anchored = Some(AnchoredChangeCapabilities {
        cursor_reset: true,
        tombstones: true,
    });
    let left = ChangeCapabilities {
        anchored,
        snapshot_diff: true,
        external_invalidation: false,
    };
    assert_eq!(
        left.intersection(ChangeCapabilities {
            anchored: None,
            snapshot_diff: true,
            external_invalidation: true,
        }),
        ChangeCapabilities {
            anchored: None,
            snapshot_diff: true,
            external_invalidation: false,
        }
    );

    let aligned = RangeHydrationCapabilities {
        alignment: Alignment::new(4).expect("alignment fixture should be valid"),
        partial_cancel: true,
    };
    let hydration = HydrationCapabilities {
        whole_file: true,
        range: Some(aligned),
        progressive_range: Some(aligned),
        incremental_from_existing: true,
    };
    let no_optional = HydrationCapabilities {
        whole_file: true,
        range: None,
        progressive_range: None,
        incremental_from_existing: false,
    };
    assert_eq!(
        hydration
            .intersection(no_optional)
            .expect("optional capability intersection should succeed"),
        HydrationCapabilities {
            whole_file: true,
            range: None,
            progressive_range: None,
            incremental_from_existing: false,
        }
    );

    let progressive = HydrationCapabilities {
        progressive_range: Some(RangeHydrationCapabilities {
            alignment: Alignment::new(6).expect("alignment fixture should be valid"),
            partial_cancel: true,
        }),
        ..HydrationCapabilities::default()
    }
    .intersection(HydrationCapabilities {
        progressive_range: Some(RangeHydrationCapabilities {
            alignment: Alignment::new(10).expect("alignment fixture should be valid"),
            partial_cancel: false,
        }),
        ..HydrationCapabilities::default()
    })
    .expect("progressive range intersection should succeed");
    assert_eq!(
        progressive.progressive_range,
        Some(RangeHydrationCapabilities {
            alignment: Alignment::new(30).expect("alignment fixture should be valid"),
            partial_cancel: false,
        })
    );
}

#[test]
fn storage_materialization_and_eviction_intersections_cover_all_flags() {
    let left_modes = ContentStorageModes {
        platform_managed: true,
        provider_managed: false,
        hybrid: true,
    };
    let right_modes = ContentStorageModes {
        platform_managed: false,
        provider_managed: true,
        hybrid: true,
    };
    let intersection = left_modes.intersection(right_modes);
    assert_eq!(
        intersection,
        ContentStorageModes {
            platform_managed: false,
            provider_managed: false,
            hybrid: true,
        }
    );
    assert!(intersection.is_supported());
    assert!(!ContentStorageModes::default().is_supported());
    assert_eq!(
        MaterializationCapabilities {
            storage_modes: left_modes,
        }
        .intersection(MaterializationCapabilities {
            storage_modes: right_modes,
        })
        .storage_modes,
        intersection
    );

    let full = EvictionCapabilities {
        supported: true,
        pinning: true,
        dirty_guard: true,
        open_lease_guard: true,
    };
    let unsupported = EvictionCapabilities {
        supported: false,
        pinning: true,
        dirty_guard: true,
        open_lease_guard: true,
    };
    assert_eq!(
        full.intersection(unsupported),
        EvictionCapabilities::default()
    );
    assert_eq!(
        full.intersection(EvictionCapabilities {
            supported: true,
            pinning: false,
            dirty_guard: true,
            open_lease_guard: false,
        }),
        EvictionCapabilities {
            supported: true,
            pinning: false,
            dirty_guard: true,
            open_lease_guard: false,
        }
    );
}

#[test]
fn every_cancellation_level_pair_returns_the_weaker_guarantee() {
    let levels = [
        CancellationLevel::None,
        CancellationLevel::BestEffortRequest,
        CancellationLevel::ExactRequest,
        CancellationLevel::RangeSubset,
    ];
    for left in levels {
        for right in levels {
            assert_eq!(left.intersection(right), std::cmp::min(left, right));
        }
    }
}

#[test]
fn limits_keep_the_tightest_value_for_none_one_sided_and_two_sided_inputs() {
    let left = CloudFilesLimits {
        native_identity_max_bytes: NonZeroUsize::new(4096),
        max_transfer_chunk: None,
        max_in_flight_requests: NonZeroUsize::new(64),
        directory_page_size_hint: NonZeroUsize::new(1000),
        name_max_encoded_bytes: None,
    };
    let right = CloudFilesLimits {
        native_identity_max_bytes: None,
        max_transfer_chunk: NonZeroUsize::new(8 * 1024 * 1024),
        max_in_flight_requests: NonZeroUsize::new(32),
        directory_page_size_hint: NonZeroUsize::new(2000),
        name_max_encoded_bytes: NonZeroUsize::new(255),
    };
    assert_eq!(
        left.intersection(right),
        CloudFilesLimits {
            native_identity_max_bytes: NonZeroUsize::new(4096),
            max_transfer_chunk: NonZeroUsize::new(8 * 1024 * 1024),
            max_in_flight_requests: NonZeroUsize::new(32),
            directory_page_size_hint: NonZeroUsize::new(1000),
            name_max_encoded_bytes: NonZeroUsize::new(255),
        }
    );
    assert_eq!(
        CloudFilesLimits::default().intersection(CloudFilesLimits::default()),
        CloudFilesLimits::default()
    );
}

#[test]
fn default_and_unconstrained_cover_every_capability_dimension() {
    let default = CloudFilesCapabilities::default();
    assert_eq!(default.identity, IdentityCapabilities::default());
    assert_eq!(default.revisions, RevisionCapabilities::default());
    assert_eq!(default.enumeration, EnumerationCapabilities::default());
    assert_eq!(default.changes, ChangeCapabilities::default());
    assert_eq!(default.hydration, HydrationCapabilities::default());
    assert_eq!(default.mutations, MutationCapabilities::default());
    assert_eq!(
        default.materialization,
        MaterializationCapabilities::default()
    );
    assert_eq!(default.eviction, EvictionCapabilities::default());
    assert_eq!(default.cancellation, CancellationLevel::None);
    assert_eq!(default.naming, NamingCapabilities::default());
    assert_eq!(default.limits, CloudFilesLimits::default());

    let all = CloudFilesCapabilities::unconstrained();
    assert!(all.identity.stable_item_ids);
    assert!(all.identity.path_independent_item_ids);
    assert!(all.identity.same_root_move_preserves_identity);
    assert!(all.identity.cross_root_move_preserves_identity);
    assert!(all.revisions.separate_metadata_and_content);
    assert!(all.revisions.conditional_metadata_mutation);
    assert!(all.revisions.conditional_content_access);
    assert!(all.enumeration.paged);
    assert!(all.enumeration.stable_snapshot);
    assert!(all.changes.anchored.is_some());
    assert!(all.changes.snapshot_diff);
    assert!(all.changes.external_invalidation);
    assert!(all.hydration.whole_file);
    assert!(all.hydration.range.is_some());
    assert!(all.hydration.progressive_range.is_some());
    assert!(all.hydration.incremental_from_existing);
    assert!(all.mutations.create);
    assert!(all.mutations.modify_metadata);
    assert!(all.mutations.modify_content);
    assert!(all.mutations.delete);
    assert!(all.mutations.atomic_move);
    assert!(all.mutations.cross_root_move);
    assert!(all.mutations.resumable_upload);
    assert!(all.materialization.storage_modes.platform_managed);
    assert!(all.materialization.storage_modes.provider_managed);
    assert!(all.materialization.storage_modes.hybrid);
    assert!(all.eviction.supported);
    assert!(all.eviction.pinning);
    assert!(all.eviction.dirty_guard);
    assert!(all.eviction.open_lease_guard);
    assert_eq!(all.cancellation, CancellationLevel::RangeSubset);
    assert!(all.naming.case_sensitive);
    assert!(all.naming.case_preserving);
    assert!(all.naming.normalization_preserving);
    assert_eq!(all.limits, CloudFilesLimits::default());
}
