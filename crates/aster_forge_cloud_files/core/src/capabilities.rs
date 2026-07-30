//! Parameterized backend, platform, and product-host capability negotiation.

use std::num::NonZeroUsize;

use crate::{CloudFilesCoreError, Result};

/// Non-zero physical range alignment in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Alignment(u64);

impl Alignment {
    /// Byte alignment with no additional physical constraint.
    pub const ONE: Self = Self(1);

    /// Creates a non-zero byte alignment.
    #[must_use]
    pub const fn new(bytes: u64) -> Option<Self> {
        if bytes == 0 { None } else { Some(Self(bytes)) }
    }

    /// Returns the alignment in bytes.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Combines two physical alignment requirements using their least common multiple.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn intersection(self, other: Self) -> Result<Self> {
        let divisor = greatest_common_divisor(self.0, other.0);
        let left = self.0 / divisor;
        let Some(bytes) = left.checked_mul(other.0) else {
            return Err(CloudFilesCoreError::AlignmentIntersectionOverflow {
                left: self.0,
                right: other.0,
            });
        };
        Ok(Self(bytes))
    }
}

const fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

/// Stable identity properties exposed by a backend/adapter boundary.
#[expect(
    clippy::struct_excessive_bools,
    reason = "identity guarantees are independent capabilities rather than mutually exclusive state"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdentityCapabilities {
    /// Item identifiers remain stable for the supported lifetime.
    pub stable_item_ids: bool,
    /// Item identifiers do not encode the current path or filename.
    pub path_independent_item_ids: bool,
    /// Rename and move within one root preserve item identity.
    pub same_root_move_preserves_identity: bool,
    /// Move between roots can preserve item identity.
    pub cross_root_move_preserves_identity: bool,
}

impl IdentityCapabilities {
    /// Computes capabilities guaranteed by both sides of an adapter boundary.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            stable_item_ids: self.stable_item_ids && other.stable_item_ids,
            path_independent_item_ids: self.path_independent_item_ids
                && other.path_independent_item_ids,
            same_root_move_preserves_identity: self.same_root_move_preserves_identity
                && other.same_root_move_preserves_identity,
            cross_root_move_preserves_identity: self.cross_root_move_preserves_identity
                && other.cross_root_move_preserves_identity,
        }
    }

    /// Validates the identity invariants required by the core model.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn validate_core_requirements(self) -> Result<()> {
        if !self.stable_item_ids {
            return Err(CloudFilesCoreError::MissingRequiredCapability {
                capability: "stable_item_ids",
            });
        }
        if !self.path_independent_item_ids {
            return Err(CloudFilesCoreError::MissingRequiredCapability {
                capability: "path_independent_item_ids",
            });
        }
        if !self.same_root_move_preserves_identity {
            return Err(CloudFilesCoreError::MissingRequiredCapability {
                capability: "same_root_move_preserves_identity",
            });
        }
        Ok(())
    }
}

/// Metadata and content revision guarantees.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RevisionCapabilities {
    /// Metadata and content can carry independently changing tokens.
    pub separate_metadata_and_content: bool,
    /// Metadata mutations accept a metadata revision precondition.
    pub conditional_metadata_mutation: bool,
    /// Content reads and mutations accept a content revision precondition.
    pub conditional_content_access: bool,
}

impl RevisionCapabilities {
    /// Computes revision guarantees shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            separate_metadata_and_content: self.separate_metadata_and_content
                && other.separate_metadata_and_content,
            conditional_metadata_mutation: self.conditional_metadata_mutation
                && other.conditional_metadata_mutation,
            conditional_content_access: self.conditional_content_access
                && other.conditional_content_access,
        }
    }
}

/// Directory enumeration guarantees.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnumerationCapabilities {
    /// Directory listing can continue using an opaque page cursor.
    pub paged: bool,
    /// A multi-page listing observes a stable backend snapshot.
    pub stable_snapshot: bool,
}

impl EnumerationCapabilities {
    /// Computes enumeration guarantees shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            paged: self.paged && other.paged,
            stable_snapshot: self.stable_snapshot && other.stable_snapshot,
        }
    }
}

/// Extra guarantees attached to an anchored backend change stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnchoredChangeCapabilities {
    /// The backend reports an expired/reset cursor explicitly.
    pub cursor_reset: bool,
    /// Change batches contain durable deletion tombstones.
    pub tombstones: bool,
}

impl AnchoredChangeCapabilities {
    const fn intersection(self, other: Self) -> Self {
        Self {
            cursor_reset: self.cursor_reset && other.cursor_reset,
            tombstones: self.tombstones && other.tombstones,
        }
    }
}

/// Supported strategies for discovering remote changes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeCapabilities {
    /// Anchored change-feed support and its guarantees.
    pub anchored: Option<AnchoredChangeCapabilities>,
    /// Core may compare a persisted baseline with a current full snapshot.
    pub snapshot_diff: bool,
    /// An external caller may explicitly invalidate a scope or item.
    pub external_invalidation: bool,
}

impl ChangeCapabilities {
    /// Computes change-discovery strategies shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        let anchored = match (self.anchored, other.anchored) {
            (Some(left), Some(right)) => Some(left.intersection(right)),
            _ => None,
        };
        Self {
            anchored,
            snapshot_diff: self.snapshot_diff && other.snapshot_diff,
            external_invalidation: self.external_invalidation && other.external_invalidation,
        }
    }
}

/// Range-read and cancellation constraints for hydration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeHydrationCapabilities {
    /// Required physical transfer alignment.
    pub alignment: Alignment,
    /// A cancellation request may release only a subset of the original range.
    pub partial_cancel: bool,
}

impl RangeHydrationCapabilities {
    /// Computes range guarantees and physical constraints shared by both sides.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn intersection(self, other: Self) -> Result<Self> {
        Ok(Self {
            alignment: self.alignment.intersection(other.alignment)?,
            partial_cancel: self.partial_cancel && other.partial_cancel,
        })
    }
}

/// Supported content hydration strategies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HydrationCapabilities {
    /// Complete-file hydration is available.
    pub whole_file: bool,
    /// Arbitrary or aligned range hydration is available.
    pub range: Option<RangeHydrationCapabilities>,
    /// Progressive background range hydration is available.
    pub progressive_range: Option<RangeHydrationCapabilities>,
    /// Content can be updated relative to an existing local revision.
    pub incremental_from_existing: bool,
}

impl HydrationCapabilities {
    /// Computes hydration strategies shared by both sides.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn intersection(self, other: Self) -> Result<Self> {
        let range = match (self.range, other.range) {
            (Some(left), Some(right)) => Some(left.intersection(right)?),
            _ => None,
        };
        let progressive_range = match (self.progressive_range, other.progressive_range) {
            (Some(left), Some(right)) => Some(left.intersection(right)?),
            _ => None,
        };
        Ok(Self {
            whole_file: self.whole_file && other.whole_file,
            range,
            progressive_range,
            incremental_from_existing: self.incremental_from_existing
                && other.incremental_from_existing,
        })
    }
}

/// Mutation operations supported across a backend/adapter boundary.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mutation support is an independent capability set rather than mutually exclusive state"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MutationCapabilities {
    /// Create file or directory items.
    pub create: bool,
    /// Modify item metadata.
    pub modify_metadata: bool,
    /// Replace or upload item content.
    pub modify_content: bool,
    /// Delete items.
    pub delete: bool,
    /// Move or rename an item atomically within one root.
    pub atomic_move: bool,
    /// Move an item between roots.
    pub cross_root_move: bool,
    /// Resume an interrupted upload session.
    pub resumable_upload: bool,
}

impl MutationCapabilities {
    /// Computes mutation operations shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            create: self.create && other.create,
            modify_metadata: self.modify_metadata && other.modify_metadata,
            modify_content: self.modify_content && other.modify_content,
            delete: self.delete && other.delete,
            atomic_move: self.atomic_move && other.atomic_move,
            cross_root_move: self.cross_root_move && other.cross_root_move,
            resumable_upload: self.resumable_upload && other.resumable_upload,
        }
    }
}

/// Physical content-storage modes supported by a platform/host combination.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentStorageModes {
    /// The operating system owns the materialized local copy.
    pub platform_managed: bool,
    /// The provider owns the backing content cache.
    pub provider_managed: bool,
    /// Platform and provider each own distinct physical cache layers.
    pub hybrid: bool,
}

impl ContentStorageModes {
    /// Computes storage modes shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            platform_managed: self.platform_managed && other.platform_managed,
            provider_managed: self.provider_managed && other.provider_managed,
            hybrid: self.hybrid && other.hybrid,
        }
    }

    /// Returns whether at least one materialization mode remains available.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        self.platform_managed || self.provider_managed || self.hybrid
    }
}

/// Materialization ownership capabilities.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterializationCapabilities {
    /// Supported physical ownership modes.
    pub storage_modes: ContentStorageModes,
}

impl MaterializationCapabilities {
    /// Computes materialization modes shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            storage_modes: self.storage_modes.intersection(other.storage_modes),
        }
    }
}

/// Eviction and pinning mechanics available to the effective adapter.
#[expect(
    clippy::struct_excessive_bools,
    reason = "eviction guarantees are independent capabilities rather than mutually exclusive state"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvictionCapabilities {
    /// Physical content can be evicted or dehydrated.
    pub supported: bool,
    /// A native or provider-owned pin state is available.
    pub pinning: bool,
    /// Eviction can preserve dirty-content guards.
    pub dirty_guard: bool,
    /// Eviction can preserve open/read/write lease guards.
    pub open_lease_guard: bool,
}

impl EvictionCapabilities {
    /// Computes eviction guarantees shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        let supported = self.supported && other.supported;
        Self {
            supported,
            pinning: supported && self.pinning && other.pinning,
            dirty_guard: supported && self.dirty_guard && other.dirty_guard,
            open_lease_guard: supported && self.open_lease_guard && other.open_lease_guard,
        }
    }
}

/// Cancellation precision supported by an operation boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum CancellationLevel {
    /// Cancellation is not propagated.
    #[default]
    None,
    /// Cancellation is advisory and may race with completion.
    BestEffortRequest,
    /// One complete request can be cancelled precisely.
    ExactRequest,
    /// A subset of one range request can be cancelled while other ranges continue.
    RangeSubset,
}

impl CancellationLevel {
    /// Returns the strongest cancellation level guaranteed by both sides.
    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        std::cmp::min(self, other)
    }
}

/// Filename comparison and preservation properties.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NamingCapabilities {
    /// Distinct names may differ only by case.
    pub case_sensitive: bool,
    /// Original filename casing is preserved.
    pub case_preserving: bool,
    /// The boundary preserves the caller's Unicode normalization form.
    pub normalization_preserving: bool,
}

impl NamingCapabilities {
    /// Computes naming guarantees shared by both sides.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            case_sensitive: self.case_sensitive && other.case_sensitive,
            case_preserving: self.case_preserving && other.case_preserving,
            normalization_preserving: self.normalization_preserving
                && other.normalization_preserving,
        }
    }
}

/// Quantitative limits applied to an effective cloud-files session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudFilesLimits {
    /// Maximum encoded native item identity length; `None` means no extra boundary limit.
    pub native_identity_max_bytes: Option<NonZeroUsize>,
    /// Maximum bytes transferred in one operation; `None` means no extra boundary limit.
    pub max_transfer_chunk: Option<NonZeroUsize>,
    /// Maximum concurrent requests; `None` means no extra boundary limit.
    pub max_in_flight_requests: Option<NonZeroUsize>,
    /// Preferred upper page size; `None` means no shared hint.
    pub directory_page_size_hint: Option<NonZeroUsize>,
    /// Maximum encoded filename length; `None` means no extra boundary limit.
    pub name_max_encoded_bytes: Option<NonZeroUsize>,
}

impl CloudFilesLimits {
    /// Computes the tightest quantitative limits imposed by either side.
    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        Self {
            native_identity_max_bytes: minimum_optional_limit(
                self.native_identity_max_bytes,
                other.native_identity_max_bytes,
            ),
            max_transfer_chunk: minimum_optional_limit(
                self.max_transfer_chunk,
                other.max_transfer_chunk,
            ),
            max_in_flight_requests: minimum_optional_limit(
                self.max_in_flight_requests,
                other.max_in_flight_requests,
            ),
            directory_page_size_hint: minimum_optional_limit(
                self.directory_page_size_hint,
                other.directory_page_size_hint,
            ),
            name_max_encoded_bytes: minimum_optional_limit(
                self.name_max_encoded_bytes,
                other.name_max_encoded_bytes,
            ),
        }
    }

    /// Validates an adapter's encoded native identity against the effective platform limit.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn validate_native_identity(self, encoded: &[u8]) -> Result<()> {
        let Some(max_bytes) = self.native_identity_max_bytes else {
            return Ok(());
        };
        if encoded.len() > max_bytes.get() {
            return Err(CloudFilesCoreError::NativeIdentityTooLarge {
                actual_bytes: encoded.len(),
                max_bytes: max_bytes.get(),
            });
        }
        Ok(())
    }
}

fn minimum_optional_limit(
    left: Option<NonZeroUsize>,
    right: Option<NonZeroUsize>,
) -> Option<NonZeroUsize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(std::cmp::min(left, right)),
        (Some(limit), None) | (None, Some(limit)) => Some(limit),
        (None, None) => None,
    }
}

/// Complete product-neutral capability description for one cloud-files boundary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudFilesCapabilities {
    /// Stable identity guarantees.
    pub identity: IdentityCapabilities,
    /// Metadata/content revision guarantees.
    pub revisions: RevisionCapabilities,
    /// Directory enumeration guarantees.
    pub enumeration: EnumerationCapabilities,
    /// Remote change discovery strategies.
    pub changes: ChangeCapabilities,
    /// Content hydration strategies.
    pub hydration: HydrationCapabilities,
    /// Local/remote mutation operations.
    pub mutations: MutationCapabilities,
    /// Physical content ownership modes.
    pub materialization: MaterializationCapabilities,
    /// Eviction and pinning guarantees.
    pub eviction: EvictionCapabilities,
    /// Cancellation precision.
    pub cancellation: CancellationLevel,
    /// Filename comparison and preservation guarantees.
    pub naming: NamingCapabilities,
    /// Quantitative limits.
    pub limits: CloudFilesLimits,
}

impl CloudFilesCapabilities {
    /// Returns the identity element for capability intersection.
    ///
    /// Use this as a pass-through baseline when one boundary does not constrain some dimensions,
    /// then replace every dimension that boundary actually owns. It is not a declaration that a
    /// concrete backend or platform has been validated to support every operation.
    #[must_use]
    pub const fn unconstrained() -> Self {
        let unconstrained_range = RangeHydrationCapabilities {
            alignment: Alignment::ONE,
            partial_cancel: true,
        };
        Self {
            identity: IdentityCapabilities {
                stable_item_ids: true,
                path_independent_item_ids: true,
                same_root_move_preserves_identity: true,
                cross_root_move_preserves_identity: true,
            },
            revisions: RevisionCapabilities {
                separate_metadata_and_content: true,
                conditional_metadata_mutation: true,
                conditional_content_access: true,
            },
            enumeration: EnumerationCapabilities {
                paged: true,
                stable_snapshot: true,
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
                range: Some(unconstrained_range),
                progressive_range: Some(unconstrained_range),
                incremental_from_existing: true,
            },
            mutations: MutationCapabilities {
                create: true,
                modify_metadata: true,
                modify_content: true,
                delete: true,
                atomic_move: true,
                cross_root_move: true,
                resumable_upload: true,
            },
            materialization: MaterializationCapabilities {
                storage_modes: ContentStorageModes {
                    platform_managed: true,
                    provider_managed: true,
                    hybrid: true,
                },
            },
            eviction: EvictionCapabilities {
                supported: true,
                pinning: true,
                dirty_guard: true,
                open_lease_guard: true,
            },
            cancellation: CancellationLevel::RangeSubset,
            naming: NamingCapabilities {
                case_sensitive: true,
                case_preserving: true,
                normalization_preserving: true,
            },
            limits: CloudFilesLimits {
                native_identity_max_bytes: None,
                max_transfer_chunk: None,
                max_in_flight_requests: None,
                directory_page_size_hint: None,
                name_max_encoded_bytes: None,
            },
        }
    }

    /// Computes effective capabilities shared by two boundaries.
    ///
    /// Call this successively for backend, platform, and product-host descriptions. Unsupported
    /// optional operations remain ordinary capability state rather than producing an error.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn intersection(self, other: Self) -> Result<Self> {
        Ok(Self {
            identity: self.identity.intersection(other.identity),
            revisions: self.revisions.intersection(other.revisions),
            enumeration: self.enumeration.intersection(other.enumeration),
            changes: self.changes.intersection(other.changes),
            hydration: self.hydration.intersection(other.hydration)?,
            mutations: self.mutations.intersection(other.mutations),
            materialization: self.materialization.intersection(other.materialization),
            eviction: self.eviction.intersection(other.eviction),
            cancellation: self.cancellation.intersection(other.cancellation),
            naming: self.naming.intersection(other.naming),
            limits: self.limits.intersection(other.limits),
        })
    }

    /// Validates the non-negotiable identity invariants required by the core model.
    /// # Errors
    ///
    /// Returns an error when validation fails or an underlying backend, store, or platform
    /// operation fails.
    pub fn validate_core_requirements(self) -> Result<()> {
        self.identity.validate_core_requirements()
    }
}
