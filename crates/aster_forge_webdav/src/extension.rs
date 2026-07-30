//! Typed WebDAV RFC extension packages and their static discovery descriptors.

use crate::capability::DavResourceState;
use crate::request::{DavMethod, DavMethodSet};

/// Closed set of RFC extension packages understood by the protocol engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DavExtensionPackage {
    AccessControl,
    VersionControl,
    CheckoutInPlace,
    VersionHistory,
    Workspace,
    Update,
    Label,
    WorkingResource,
    Merge,
    Baseline,
    Activity,
    VersionControlledCollection,
    Search,
    Quota,
    CollectionSync,
    ExtendedMkcol,
    CurrentPrincipal,
    OrderedCollections,
    RedirectReferences,
    Bindings,
    AddMember,
    Prefer,
}

impl DavExtensionPackage {
    /// Packages in canonical discovery and validation order.
    pub const ALL: [Self; 22] = [
        Self::AccessControl,
        Self::VersionControl,
        Self::CheckoutInPlace,
        Self::VersionHistory,
        Self::Workspace,
        Self::Update,
        Self::Label,
        Self::WorkingResource,
        Self::Merge,
        Self::Baseline,
        Self::Activity,
        Self::VersionControlledCollection,
        Self::Search,
        Self::Quota,
        Self::CollectionSync,
        Self::ExtendedMkcol,
        Self::CurrentPrincipal,
        Self::OrderedCollections,
        Self::RedirectReferences,
        Self::Bindings,
        Self::AddMember,
        Self::Prefer,
    ];

    const fn index(self) -> u32 {
        match self {
            Self::AccessControl => 0,
            Self::VersionControl => 1,
            Self::CheckoutInPlace => 2,
            Self::VersionHistory => 3,
            Self::Workspace => 4,
            Self::Update => 5,
            Self::Label => 6,
            Self::WorkingResource => 7,
            Self::Merge => 8,
            Self::Baseline => 9,
            Self::Activity => 10,
            Self::VersionControlledCollection => 11,
            Self::Search => 12,
            Self::Quota => 13,
            Self::CollectionSync => 14,
            Self::ExtendedMkcol => 15,
            Self::CurrentPrincipal => 16,
            Self::OrderedCollections => 17,
            Self::RedirectReferences => 18,
            Self::Bindings => 19,
            Self::AddMember => 20,
            Self::Prefer => 21,
        }
    }

    /// Returns the static RFC descriptor for this package.
    #[must_use]
    pub const fn descriptor(self) -> &'static DavExtensionDescriptor {
        &DESCRIPTORS[self.index() as usize]
    }
}

/// Allocation-free set of extension packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavExtensionSet(u32);

impl DavExtensionSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_packages(packages: &[DavExtensionPackage]) -> Self {
        let mut set = Self::empty();
        let mut index = 0;
        while index < packages.len() {
            set = set.with(packages[index]);
            index += 1;
        }
        set
    }

    #[must_use]
    pub const fn with(self, package: DavExtensionPackage) -> Self {
        Self(self.0 | (1u32 << package.index()))
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, package: DavExtensionPackage) -> bool {
        self.0 & (1u32 << package.index()) != 0
    }

    #[must_use]
    pub const fn contains_all(self, required: Self) -> bool {
        required.0 & !self.0 == 0
    }

    #[must_use]
    pub const fn is_subset_of(self, maximum: Self) -> bool {
        maximum.contains_all(self)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn iter(self) -> DavExtensionSetIter {
        DavExtensionSetIter {
            set: self,
            index: 0,
        }
    }
}

/// Iterator over packages in canonical order.
#[derive(Debug, Clone, Copy)]
pub struct DavExtensionSetIter {
    set: DavExtensionSet,
    index: usize,
}

impl Iterator for DavExtensionSetIter {
    type Item = DavExtensionPackage;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < DavExtensionPackage::ALL.len() {
            let package = DavExtensionPackage::ALL[self.index];
            self.index += 1;
            if self.set.contains(package) {
                return Some(package);
            }
        }
        None
    }
}

/// Resource states to which a package or method contribution can apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavResourceStateSet(u16);

impl DavResourceStateSet {
    #[must_use]
    pub const fn from_states(states: &[DavResourceState]) -> Self {
        let mut bits = 0u16;
        let mut index = 0;
        while index < states.len() {
            bits |= 1u16 << resource_index(states[index]);
            index += 1;
        }
        Self(bits)
    }

    #[must_use]
    pub const fn contains(self, state: DavResourceState) -> bool {
        self.0 & (1u16 << resource_index(state)) != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

const fn resource_index(state: DavResourceState) -> u32 {
    match state {
        DavResourceState::Unmapped => 0,
        DavResourceState::File => 1,
        DavResourceState::Collection => 2,
        DavResourceState::MountRoot => 3,
        DavResourceState::Principal => 4,
        DavResourceState::RedirectReference => 5,
        DavResourceState::AddMemberEndpoint => 6,
    }
}

/// Body contract contributed by one extension method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavExtensionBodyKind {
    Xml,
    Stream,
}

/// One target-aware method contribution from an RFC package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavExtensionMethod {
    pub method: DavMethod,
    pub resources: DavResourceStateSet,
    pub body: DavExtensionBodyKind,
}

macro_rules! protocol_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $local_name:literal),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Number of variants in this protocol enum.
            pub const COUNT: usize = [$(Self::$variant),+].len();

            /// Values in canonical discovery and parsing order.
            pub const ALL: [Self; Self::COUNT] = [$(Self::$variant),+];

            pub(crate) const fn index(self) -> usize {
                self as usize
            }

            /// DAV namespace local name.
            #[must_use]
            pub const fn local_name(self) -> &'static str {
                match self {
                    $(Self::$variant => $local_name),+
                }
            }
        }
    };
}

protocol_enum! {
    /// Standard live properties that can be discovered through Forge's catalog.
    pub enum DavLiveProperty {
        CreationDate => "creationdate",
        DisplayName => "displayname",
        GetContentLanguage => "getcontentlanguage",
        GetContentLength => "getcontentlength",
        GetContentType => "getcontenttype",
        GetEtag => "getetag",
        GetLastModified => "getlastmodified",
        LockDiscovery => "lockdiscovery",
        ResourceType => "resourcetype",
        SupportedLock => "supportedlock",
        AlternateUriSet => "alternate-URI-set",
        PrincipalUrl => "principal-URL",
        GroupMemberSet => "group-member-set",
        GroupMembership => "group-membership",
        Owner => "owner",
        Group => "group",
        SupportedPrivilegeSet => "supported-privilege-set",
        CurrentUserPrivilegeSet => "current-user-privilege-set",
        Acl => "acl",
        AclRestrictions => "acl-restrictions",
        InheritedAclSet => "inherited-acl-set",
        PrincipalCollectionSet => "principal-collection-set",
        Comment => "comment",
        CreatorDisplayName => "creator-displayname",
        SupportedMethodSet => "supported-method-set",
        SupportedLivePropertySet => "supported-live-property-set",
        SupportedReportSet => "supported-report-set",
        CheckedIn => "checked-in",
        AutoVersion => "auto-version",
        CheckedOut => "checked-out",
        PredecessorSet => "predecessor-set",
        SuccessorSet => "successor-set",
        CheckoutSet => "checkout-set",
        VersionName => "version-name",
        CheckoutFork => "checkout-fork",
        CheckinFork => "checkin-fork",
        VersionSet => "version-set",
        RootVersion => "root-version",
        VersionHistory => "version-history",
        WorkspaceCheckoutSet => "workspace-checkout-set",
        Workspace => "workspace",
        LabelNameSet => "label-name-set",
        AutoUpdate => "auto-update",
        MergeSet => "merge-set",
        AutoMergeSet => "auto-merge-set",
        BaselineControlledCollection => "baseline-controlled-collection",
        SubbaselineSet => "subbaseline-set",
        BaselineCollection => "baseline-collection",
        VersionControlledConfiguration => "version-controlled-configuration",
        BaselineControlledCollectionSet => "baseline-controlled-collection-set",
        ActivityVersionSet => "activity-version-set",
        ActivityCheckoutSet => "activity-checkout-set",
        SubactivitySet => "subactivity-set",
        CurrentWorkspaceSet => "current-workspace-set",
        ActivitySet => "activity-set",
        Unreserved => "unreserved",
        CurrentActivitySet => "current-activity-set",
        EclipsedSet => "eclipsed-set",
        VersionControlledBindingSet => "version-controlled-binding-set",
        SupportedQueryGrammarSet => "supported-query-grammar-set",
        QuotaAvailableBytes => "quota-available-bytes",
        QuotaUsedBytes => "quota-used-bytes",
        SyncToken => "sync-token",
        CurrentUserPrincipal => "current-user-principal",
        OrderingType => "ordering-type",
        RedirectLifetime => "redirect-lifetime",
        RefTarget => "reftarget",
        ResourceId => "resource-id",
        ParentSet => "parent-set",
        AddMember => "add-member",
    }
}

protocol_enum! {
    /// REPORT types contributed by RFC packages.
    pub enum DavReportType {
        VersionTree => "version-tree",
        ExpandProperty => "expand-property",
        LocateByHistory => "locate-by-history",
        MergePreview => "merge-preview",
        CompareBaseline => "compare-baseline",
        LatestActivityVersion => "latest-activity-version",
        AclPrincipalPropSet => "acl-principal-prop-set",
        PrincipalMatch => "principal-match",
        PrincipalPropertySearch => "principal-property-search",
        PrincipalSearchPropertySet => "principal-search-property-set",
        SyncCollection => "sync-collection",
    }
}

/// RFC 8144 preference behavior exposed by a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavPreferenceSet(u8);

impl DavPreferenceSet {
    pub const RETURN_MINIMAL: Self = Self(1);
    pub const RETURN_REPRESENTATION: Self = Self(1 << 1);
    pub const DEPTH_NO_ROOT: Self = Self(1 << 2);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn all() -> Self {
        Self(Self::RETURN_MINIMAL.0 | Self::RETURN_REPRESENTATION.0 | Self::DEPTH_NO_ROOT.0)
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn contains(self, preference: Self) -> bool {
        self.0 & preference.0 == preference.0
    }
}

/// Static RFC metadata used by capability, dispatch, property and REPORT discovery.
#[derive(Debug, Clone, Copy)]
pub struct DavExtensionDescriptor {
    pub package: DavExtensionPackage,
    pub rfc: &'static str,
    pub dav_token: Option<&'static str>,
    pub prerequisites: DavExtensionSet,
    pub resources: DavResourceStateSet,
    pub methods: &'static [DavExtensionMethod],
    pub live_properties: &'static [DavLiveProperty],
    pub reports: &'static [DavReportType],
    pub preferences: DavPreferenceSet,
}

const ALL_RESOURCES: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::Unmapped,
    DavResourceState::File,
    DavResourceState::Collection,
    DavResourceState::MountRoot,
    DavResourceState::Principal,
    DavResourceState::RedirectReference,
    DavResourceState::AddMemberEndpoint,
]);
const MAPPED_RESOURCES: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::File,
    DavResourceState::Collection,
    DavResourceState::MountRoot,
    DavResourceState::Principal,
    DavResourceState::RedirectReference,
]);
const CONTENT_RESOURCES: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::File,
    DavResourceState::Collection,
    DavResourceState::MountRoot,
]);
const COLLECTIONS: DavResourceStateSet =
    DavResourceStateSet::from_states(&[DavResourceState::Collection, DavResourceState::MountRoot]);
const UNMAPPED: DavResourceStateSet =
    DavResourceStateSet::from_states(&[DavResourceState::Unmapped]);
const ORDERABLE: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::Unmapped,
    DavResourceState::Collection,
    DavResourceState::MountRoot,
]);
const REDIRECT_TARGETS: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::Unmapped,
    DavResourceState::File,
    DavResourceState::Collection,
    DavResourceState::MountRoot,
    DavResourceState::Principal,
    DavResourceState::RedirectReference,
]);
const ADD_MEMBER_TARGETS: DavResourceStateSet = DavResourceStateSet::from_states(&[
    DavResourceState::Collection,
    DavResourceState::MountRoot,
    DavResourceState::AddMemberEndpoint,
]);

const XML: DavExtensionBodyKind = DavExtensionBodyKind::Xml;

const ACL_METHODS: &[DavExtensionMethod] = &[
    DavExtensionMethod {
        method: DavMethod::Acl,
        resources: MAPPED_RESOURCES,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Report,
        resources: MAPPED_RESOURCES,
        body: XML,
    },
];
const VERSION_CONTROL_METHODS: &[DavExtensionMethod] = &[
    DavExtensionMethod {
        method: DavMethod::VersionControl,
        resources: CONTENT_RESOURCES.union(UNMAPPED),
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Report,
        resources: CONTENT_RESOURCES,
        body: XML,
    },
];
const CHECKOUT_METHODS: &[DavExtensionMethod] = &[
    DavExtensionMethod {
        method: DavMethod::Checkout,
        resources: CONTENT_RESOURCES,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Checkin,
        resources: CONTENT_RESOURCES,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Uncheckout,
        resources: CONTENT_RESOURCES,
        body: XML,
    },
];
const WORKSPACE_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Mkworkspace,
    resources: UNMAPPED,
    body: XML,
}];
const UPDATE_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Update,
    resources: CONTENT_RESOURCES,
    body: XML,
}];
const LABEL_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Label,
    resources: CONTENT_RESOURCES,
    body: XML,
}];
const WORKING_RESOURCE_METHODS: &[DavExtensionMethod] = CHECKOUT_METHODS;
const MERGE_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Merge,
    resources: CONTENT_RESOURCES,
    body: XML,
}];
const BASELINE_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::BaselineControl,
    resources: COLLECTIONS,
    body: XML,
}];
const ACTIVITY_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Mkactivity,
    resources: UNMAPPED,
    body: XML,
}];
const SEARCH_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Search,
    resources: MAPPED_RESOURCES,
    body: XML,
}];
const SYNC_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Report,
    resources: COLLECTIONS,
    body: XML,
}];
const EXTENDED_MKCOL_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Mkcol,
    resources: UNMAPPED,
    body: XML,
}];
const ORDERED_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Orderpatch,
    resources: COLLECTIONS,
    body: XML,
}];
const REDIRECT_METHODS: &[DavExtensionMethod] = &[
    DavExtensionMethod {
        method: DavMethod::Mkredirectref,
        resources: UNMAPPED,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Updateredirectref,
        resources: DavResourceStateSet::from_states(&[DavResourceState::RedirectReference]),
        body: XML,
    },
];
const BIND_METHODS: &[DavExtensionMethod] = &[
    DavExtensionMethod {
        method: DavMethod::Bind,
        resources: COLLECTIONS,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Unbind,
        resources: COLLECTIONS,
        body: XML,
    },
    DavExtensionMethod {
        method: DavMethod::Rebind,
        resources: COLLECTIONS,
        body: XML,
    },
];
const ADD_MEMBER_METHODS: &[DavExtensionMethod] = &[DavExtensionMethod {
    method: DavMethod::Post,
    resources: DavResourceStateSet::from_states(&[DavResourceState::AddMemberEndpoint]),
    body: DavExtensionBodyKind::Stream,
}];

const ACL_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::AlternateUriSet,
    DavLiveProperty::PrincipalUrl,
    DavLiveProperty::GroupMemberSet,
    DavLiveProperty::GroupMembership,
    DavLiveProperty::Owner,
    DavLiveProperty::Group,
    DavLiveProperty::SupportedPrivilegeSet,
    DavLiveProperty::CurrentUserPrivilegeSet,
    DavLiveProperty::Acl,
    DavLiveProperty::AclRestrictions,
    DavLiveProperty::InheritedAclSet,
    DavLiveProperty::PrincipalCollectionSet,
];
const VERSION_CONTROL_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::Comment,
    DavLiveProperty::CreatorDisplayName,
    DavLiveProperty::SupportedMethodSet,
    DavLiveProperty::SupportedLivePropertySet,
    DavLiveProperty::SupportedReportSet,
    DavLiveProperty::CheckedIn,
    DavLiveProperty::AutoVersion,
    DavLiveProperty::CheckedOut,
    DavLiveProperty::PredecessorSet,
    DavLiveProperty::SuccessorSet,
    DavLiveProperty::CheckoutSet,
    DavLiveProperty::VersionName,
];
const CHECKOUT_PROPERTIES: &[DavLiveProperty] =
    &[DavLiveProperty::CheckoutFork, DavLiveProperty::CheckinFork];
const VERSION_HISTORY_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::VersionSet,
    DavLiveProperty::RootVersion,
    DavLiveProperty::VersionHistory,
];
const WORKSPACE_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::WorkspaceCheckoutSet,
    DavLiveProperty::Workspace,
];
const LABEL_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::LabelNameSet];
const WORKING_RESOURCE_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::AutoUpdate];
const MERGE_PROPERTIES: &[DavLiveProperty] =
    &[DavLiveProperty::MergeSet, DavLiveProperty::AutoMergeSet];
const BASELINE_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::BaselineControlledCollection,
    DavLiveProperty::SubbaselineSet,
    DavLiveProperty::BaselineCollection,
    DavLiveProperty::VersionControlledConfiguration,
    DavLiveProperty::BaselineControlledCollectionSet,
];
const ACTIVITY_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::ActivityVersionSet,
    DavLiveProperty::ActivityCheckoutSet,
    DavLiveProperty::SubactivitySet,
    DavLiveProperty::CurrentWorkspaceSet,
    DavLiveProperty::ActivitySet,
    DavLiveProperty::Unreserved,
    DavLiveProperty::CurrentActivitySet,
];
const VCC_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::EclipsedSet,
    DavLiveProperty::VersionControlledBindingSet,
];
const SEARCH_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::SupportedQueryGrammarSet];
const QUOTA_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::QuotaAvailableBytes,
    DavLiveProperty::QuotaUsedBytes,
];
const SYNC_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::SyncToken];
const CURRENT_PRINCIPAL_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::CurrentUserPrincipal];
const ORDERED_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::OrderingType];
const REDIRECT_PROPERTIES: &[DavLiveProperty] = &[
    DavLiveProperty::RedirectLifetime,
    DavLiveProperty::RefTarget,
];
const BIND_PROPERTIES: &[DavLiveProperty] =
    &[DavLiveProperty::ResourceId, DavLiveProperty::ParentSet];
const ADD_MEMBER_PROPERTIES: &[DavLiveProperty] = &[DavLiveProperty::AddMember];

const ACL_REPORTS: &[DavReportType] = &[
    DavReportType::AclPrincipalPropSet,
    DavReportType::PrincipalMatch,
    DavReportType::PrincipalPropertySearch,
    DavReportType::PrincipalSearchPropertySet,
];
const VERSION_CONTROL_REPORTS: &[DavReportType] =
    &[DavReportType::VersionTree, DavReportType::ExpandProperty];
const VERSION_HISTORY_REPORTS: &[DavReportType] = &[DavReportType::LocateByHistory];
const MERGE_REPORTS: &[DavReportType] = &[DavReportType::MergePreview];
const BASELINE_REPORTS: &[DavReportType] = &[DavReportType::CompareBaseline];
const ACTIVITY_REPORTS: &[DavReportType] = &[DavReportType::LatestActivityVersion];
const SYNC_REPORTS: &[DavReportType] = &[DavReportType::SyncCollection];

const EMPTY_METHODS: &[DavExtensionMethod] = &[];
const EMPTY_PROPERTIES: &[DavLiveProperty] = &[];
const EMPTY_REPORTS: &[DavReportType] = &[];
const VERSION_CONTROL_REQUIRED: DavExtensionSet =
    DavExtensionSet::from_packages(&[DavExtensionPackage::VersionControl]);
const WORKSPACE_REQUIRED: DavExtensionSet = DavExtensionSet::from_packages(&[
    DavExtensionPackage::VersionControl,
    DavExtensionPackage::CheckoutInPlace,
    DavExtensionPackage::VersionHistory,
]);

macro_rules! descriptor {
    (
        $package:expr,
        $rfc:expr,
        $dav_token:expr,
        $prerequisites:expr,
        $resources:expr,
        $methods:expr,
        $live_properties:expr,
        $reports:expr $(,)?
    ) => {
        DavExtensionDescriptor {
            package: $package,
            rfc: $rfc,
            dav_token: $dav_token,
            prerequisites: $prerequisites,
            resources: $resources,
            methods: $methods,
            live_properties: $live_properties,
            reports: $reports,
            preferences: DavPreferenceSet::empty(),
        }
    };
}

const DESCRIPTORS: [DavExtensionDescriptor; 22] = [
    descriptor!(
        DavExtensionPackage::AccessControl,
        "RFC 3744",
        Some("access-control"),
        DavExtensionSet::empty(),
        MAPPED_RESOURCES,
        ACL_METHODS,
        ACL_PROPERTIES,
        ACL_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::VersionControl,
        "RFC 3253 section 3",
        Some("version-control"),
        DavExtensionSet::empty(),
        ALL_RESOURCES,
        VERSION_CONTROL_METHODS,
        VERSION_CONTROL_PROPERTIES,
        VERSION_CONTROL_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::CheckoutInPlace,
        "RFC 3253 section 4",
        Some("checkout-in-place"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        CHECKOUT_METHODS,
        CHECKOUT_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::VersionHistory,
        "RFC 3253 section 5",
        Some("version-history"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        EMPTY_METHODS,
        VERSION_HISTORY_PROPERTIES,
        VERSION_HISTORY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Workspace,
        "RFC 3253 section 6",
        Some("workspace"),
        WORKSPACE_REQUIRED,
        ALL_RESOURCES,
        WORKSPACE_METHODS,
        WORKSPACE_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Update,
        "RFC 3253 section 7",
        Some("update"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        UPDATE_METHODS,
        EMPTY_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Label,
        "RFC 3253 section 8",
        Some("label"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        LABEL_METHODS,
        LABEL_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::WorkingResource,
        "RFC 3253 section 9",
        Some("working-resource"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        WORKING_RESOURCE_METHODS,
        WORKING_RESOURCE_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Merge,
        "RFC 3253 section 11",
        Some("merge"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        MERGE_METHODS,
        MERGE_PROPERTIES,
        MERGE_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Baseline,
        "RFC 3253 section 12",
        Some("baseline"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        BASELINE_METHODS,
        BASELINE_PROPERTIES,
        BASELINE_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Activity,
        "RFC 3253 section 13",
        Some("activity"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        ACTIVITY_METHODS,
        ACTIVITY_PROPERTIES,
        ACTIVITY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::VersionControlledCollection,
        "RFC 3253 section 14",
        Some("version-controlled-collection"),
        VERSION_CONTROL_REQUIRED,
        ALL_RESOURCES,
        EMPTY_METHODS,
        VCC_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Search,
        "RFC 5323",
        None,
        DavExtensionSet::empty(),
        MAPPED_RESOURCES,
        SEARCH_METHODS,
        SEARCH_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Quota,
        "RFC 4331",
        None,
        DavExtensionSet::empty(),
        CONTENT_RESOURCES,
        EMPTY_METHODS,
        QUOTA_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::CollectionSync,
        "RFC 6578",
        None,
        DavExtensionSet::empty(),
        COLLECTIONS,
        SYNC_METHODS,
        SYNC_PROPERTIES,
        SYNC_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::ExtendedMkcol,
        "RFC 5689",
        Some("extended-mkcol"),
        DavExtensionSet::empty(),
        UNMAPPED,
        EXTENDED_MKCOL_METHODS,
        EMPTY_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::CurrentPrincipal,
        "RFC 5397",
        None,
        DavExtensionSet::empty(),
        MAPPED_RESOURCES,
        EMPTY_METHODS,
        CURRENT_PRINCIPAL_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::OrderedCollections,
        "RFC 3648",
        Some("ordered-collections"),
        DavExtensionSet::empty(),
        ORDERABLE,
        ORDERED_METHODS,
        ORDERED_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::RedirectReferences,
        "RFC 4437",
        Some("redirectrefs"),
        DavExtensionSet::empty(),
        REDIRECT_TARGETS,
        REDIRECT_METHODS,
        REDIRECT_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::Bindings,
        "RFC 5842",
        Some("bind"),
        DavExtensionSet::empty(),
        MAPPED_RESOURCES,
        BIND_METHODS,
        BIND_PROPERTIES,
        EMPTY_REPORTS,
    ),
    descriptor!(
        DavExtensionPackage::AddMember,
        "RFC 5995",
        None,
        DavExtensionSet::empty(),
        ADD_MEMBER_TARGETS,
        ADD_MEMBER_METHODS,
        ADD_MEMBER_PROPERTIES,
        EMPTY_REPORTS,
    ),
    DavExtensionDescriptor {
        package: DavExtensionPackage::Prefer,
        rfc: "RFC 8144",
        dav_token: None,
        prerequisites: DavExtensionSet::empty(),
        resources: ALL_RESOURCES,
        methods: EMPTY_METHODS,
        live_properties: EMPTY_PROPERTIES,
        reports: EMPTY_REPORTS,
        preferences: DavPreferenceSet::all(),
    },
];

/// Returns methods implemented by enabled packages for the selected resource state.
#[must_use]
pub fn extension_methods(packages: DavExtensionSet, resource: DavResourceState) -> DavMethodSet {
    let mut methods = DavMethodSet::empty();
    for package in packages.iter() {
        for contribution in package.descriptor().methods {
            if contribution.resources.contains(resource) {
                methods = methods.with(contribution.method);
            }
        }
    }
    methods
}

/// Returns the extension body contract for a method on the selected resource.
#[must_use]
pub fn extension_body_kind(
    packages: DavExtensionSet,
    resource: DavResourceState,
    method: DavMethod,
) -> Option<DavExtensionBodyKind> {
    for package in packages.iter() {
        for contribution in package.descriptor().methods {
            if contribution.method == method && contribution.resources.contains(resource) {
                return Some(contribution.body);
            }
        }
    }
    None
}
