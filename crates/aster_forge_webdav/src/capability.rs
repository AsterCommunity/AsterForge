//! Resource-aware `WebDAV` capability declarations and validated discovery snapshots.

use std::marker::PhantomData;

use aster_forge_utils::url::parse_absolute_url;
use aster_forge_xml::is_valid_xml_local_name;
use headers::Mime;
use http::HeaderValue;

use crate::extension::{
    DavExtensionBodyKind, DavExtensionPackage, DavExtensionSet, DavLiveProperty, DavPreferenceSet,
    DavReportType, extension_body_kind, extension_methods,
};
use crate::request::{DavBodyPolicy, DavMethod, DavMethodSet};
use crate::{DavBackendError, DavPath};

/// Resource state visible to the capability planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DavResourceState {
    Unmapped,
    File,
    Collection,
    MountRoot,
    Principal,
    RedirectReference,
    AddMemberEndpoint,
}

/// Product-neutral RFC 3253 state of a request target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DavVersioningState {
    /// The target does not expose the core version-control package.
    #[default]
    Unsupported,
    /// An ordinary resource that can be placed under version control.
    Versionable,
    /// A version-controlled resource whose current version is checked in.
    CheckedIn,
    /// A version-controlled resource whose current version is checked out.
    CheckedOut,
    /// An immutable version resource.
    Version,
}

/// Automatic side effects selected by a product for checked-in resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DavAutoVersion {
    /// Mutation requires an explicitly checked-out resource.
    #[default]
    None,
    /// Automatically check out before mutation and check in after it succeeds.
    CheckoutCheckin,
    /// Check out before mutation, then check in unless the resource is write-locked.
    CheckoutUnlockedCheckin,
    /// Automatically check out before mutation and leave the resource checked out.
    Checkout,
    /// Automatically check out only when the checked-in resource is write-locked.
    LockedCheckout,
}

/// RFC 3253 facts projected by the product capability provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DavVersioningCapabilities {
    pub state: DavVersioningState,
    pub auto_version: DavAutoVersion,
    /// Whether the current target is protected by an active DAV write lock.
    pub write_locked: bool,
    /// Whether the current checkout is associated with a write lock after automatic checkout.
    pub auto_checkout_lock: bool,
    /// Whether this server permits DELETE of immutable version resources.
    pub allow_version_delete: bool,
}

/// Request target supplied to a capability provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCapabilityTarget {
    pub path: DavPath,
    pub is_mount_root: bool,
}

impl DavCapabilityTarget {
    #[must_use]
    pub fn new(path: DavPath, is_mount_root: bool) -> Self {
        Self {
            path,
            is_mount_root,
        }
    }
}

/// Request context that can affect a product's capability projection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DavCapabilityContext {
    pub principal: Option<String>,
}

/// Locking compliance advertised for a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavLockingCapability {
    Disabled,
    Class2,
}

/// Optional non-standard compatibility signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavCompatibilityCapabilities {
    pub ms_author_via: bool,
}

/// Conditional request policy required before a partial write can execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavWritePrecondition {
    Optional,
    RequireStrongIfMatch,
}

/// RFC 9110 partial PUT support negotiated by private agreement with the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPartialPutCapability {
    #[default]
    Disabled,
    ContentRangeBytes {
        precondition: DavWritePrecondition,
    },
}

/// Body handling selected by one RFC 5789 patch document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavPatchBodyPolicy {
    Bounded { maximum: usize },
    Stream,
}

/// One statically declared RFC 5789 patch document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavPatchFormat {
    /// Complete media type. Parameters participate in exact matching and duplicate detection.
    pub media_type: &'static str,
    pub body_policy: DavPatchBodyPolicy,
    pub precondition: DavWritePrecondition,
}

/// RFC 5789 PATCH capability for one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPatchCapability {
    #[default]
    Disabled,
    Formats(&'static [DavPatchFormat]),
}

/// Explicitly named private range-update compatibility surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPrivateUpdateRangeCapability {
    #[default]
    Disabled,
    XUpdateRange {
        precondition: DavWritePrecondition,
    },
}

/// Mutation capabilities deliberately separate from ordinary replacement PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavWriteCapabilities {
    pub partial_put: DavPartialPutCapability,
    pub patch: DavPatchCapability,
    pub private_update_range: DavPrivateUpdateRangeCapability,
}

/// RFC 4918 compliance classes represented by a runtime declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavComplianceClasses {
    pub class1: bool,
    /// RFC 4918 class 3 requires class 1 and does not imply class 2 locking.
    pub class3: bool,
}

/// Resource-aware RFC 5323 query grammar discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavSearchCapabilities {
    /// Static grammar descriptors; SEARCH requires [`DavSearchGrammar::BASICSEARCH`].
    pub grammars: &'static [DavSearchGrammar],
}

/// One RFC 5323 query grammar's HTTP identifier and XML element type.
///
/// RFC 5323 explicitly states that the DASL coded-URL does not necessarily correspond to the XML
/// element namespace and local name. Keeping all three fields prevents discovery responses from
/// guessing an XML `QName` from a URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavSearchGrammar {
    /// Absolute-URI rendered inside angle brackets in the `DASL` response header.
    pub coded_url: &'static str,
    /// Namespace URI reference of the grammar element; an empty string means no namespace.
    pub xml_namespace: &'static str,
    /// XML local name of the grammar element.
    pub xml_local_name: &'static str,
}

impl DavSearchGrammar {
    /// Mandatory RFC 5323 basic search grammar.
    pub const BASICSEARCH: Self = Self {
        coded_url: "DAV:basicsearch",
        xml_namespace: "DAV:",
        xml_local_name: "basicsearch",
    };

    /// Declares a static SEARCH grammar without a runtime registry or per-request allocation.
    #[must_use]
    pub const fn new(
        coded_url: &'static str,
        xml_namespace: &'static str,
        xml_local_name: &'static str,
    ) -> Self {
        Self {
            coded_url,
            xml_namespace,
            xml_local_name,
        }
    }
}

/// Product implementation provides RFC 4918 class 1.
pub trait DavClass1Support: Send + Sync {}
/// Product implementation provides RFC 4918 class 2 locking.
pub trait DavClass2Support: DavClass1Support {}
/// Product implementation provides RFC 4918 class 3 revised semantics.
///
/// ```compile_fail
/// use aster_forge_webdav::DavClass3Support;
/// struct MissingClassOne;
/// impl DavClass3Support for MissingClassOne {}
/// ```
pub trait DavClass3Support: DavClass1Support {}

/// Product implementation provides the complete RFC 3744 package.
pub trait DavAccessControlSupport: DavClass1Support {}
/// Product implementation provides RFC 3253 version-control.
pub trait DavVersionControlSupport: DavClass1Support {}
pub trait DavCheckoutInPlaceSupport: DavVersionControlSupport {}
pub trait DavVersionHistorySupport: DavVersionControlSupport {}
/// Complete workspace support includes checkout-in-place and version-history at compile time.
///
/// ```compile_fail
/// use aster_forge_webdav::{DavClass1Support, DavVersionControlSupport, DavWorkspaceSupport};
/// struct IncompleteWorkspace;
/// impl DavClass1Support for IncompleteWorkspace {}
/// impl DavVersionControlSupport for IncompleteWorkspace {}
/// impl DavWorkspaceSupport for IncompleteWorkspace {}
/// ```
pub trait DavWorkspaceSupport: DavCheckoutInPlaceSupport + DavVersionHistorySupport {}
pub trait DavUpdateSupport: DavVersionControlSupport {}
pub trait DavLabelSupport: DavVersionControlSupport {}
pub trait DavWorkingResourceSupport: DavVersionControlSupport {}
pub trait DavMergeSupport: DavVersionControlSupport {}
pub trait DavBaselineSupport: DavVersionControlSupport {}
pub trait DavActivitySupport: DavVersionControlSupport {}
pub trait DavVersionControlledCollectionSupport: DavVersionControlSupport {}
pub trait DavSearchSupport: DavClass1Support {}
pub trait DavQuotaSupport: DavClass1Support {}
pub trait DavCollectionSyncSupport: DavClass1Support {}
pub trait DavExtendedMkcolSupport: DavClass1Support {}
pub trait DavCurrentPrincipalSupport: DavClass1Support {}
pub trait DavOrderedCollectionsSupport: DavClass1Support {}
pub trait DavRedirectReferencesSupport: DavClass1Support {}
pub trait DavBindingsSupport: DavClass1Support {}
pub trait DavAddMemberSupport: DavClass1Support {}
pub trait DavPreferSupport: DavClass1Support {}

pub trait DavPartialPutSupport: Send + Sync {}
pub trait DavPatchSupport: Send + Sync {}
pub trait DavPrivateUpdateRangeSupport: Send + Sync {}

mod sealed {
    pub trait Sealed {}
    pub trait ExtensionSealed {}
    pub trait Class1Profile<Provider: ?Sized> {}
}

/// Static maximum capability profile selected by a product provider.
pub trait DavCapabilityProfile<Provider: ?Sized>: sealed::Sealed {
    const CLASS1: bool;
    const CLASS2: bool;
    const CLASS3: bool;
    const EXTENSIONS: DavExtensionSet;
    const PARTIAL_PUT: bool;
    const PATCH: bool;
    const PRIVATE_UPDATE_RANGE: bool;
}

pub struct DavNonDavProfile;
pub struct DavClass1Profile;
pub struct DavClass2Profile;
/// RFC 4918 classes `1, 3`, without class 2 locking.
pub struct DavClass3Profile;
/// RFC 4918 classes `1, 2, 3`.
pub struct DavClass2And3Profile;

impl sealed::Sealed for DavNonDavProfile {}
impl sealed::Sealed for DavClass1Profile {}
impl sealed::Sealed for DavClass2Profile {}
impl sealed::Sealed for DavClass3Profile {}
impl sealed::Sealed for DavClass2And3Profile {}

macro_rules! base_profile {
    ($profile:ty, $provider:path, $class1:expr, $class2:expr, $class3:expr) => {
        impl<Provider: $provider + ?Sized> DavCapabilityProfile<Provider> for $profile {
            const CLASS1: bool = $class1;
            const CLASS2: bool = $class2;
            const CLASS3: bool = $class3;
            const EXTENSIONS: DavExtensionSet = DavExtensionSet::empty();
            const PARTIAL_PUT: bool = false;
            const PATCH: bool = false;
            const PRIVATE_UPDATE_RANGE: bool = false;
        }
    };
}

impl<Provider: ?Sized> DavCapabilityProfile<Provider> for DavNonDavProfile {
    const CLASS1: bool = false;
    const CLASS2: bool = false;
    const CLASS3: bool = false;
    const EXTENSIONS: DavExtensionSet = DavExtensionSet::empty();
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}
base_profile!(DavClass1Profile, DavClass1Support, true, false, false);
base_profile!(DavClass2Profile, DavClass2Support, true, true, false);
base_profile!(DavClass3Profile, DavClass3Support, true, false, true);

impl<Provider: DavClass1Support + ?Sized> sealed::Class1Profile<Provider> for DavClass1Profile {}
impl<Provider: DavClass2Support + ?Sized> sealed::Class1Profile<Provider> for DavClass2Profile {}
impl<Provider: DavClass3Support + ?Sized> sealed::Class1Profile<Provider> for DavClass3Profile {}

impl<Provider: DavClass2Support + DavClass3Support + ?Sized> DavCapabilityProfile<Provider>
    for DavClass2And3Profile
{
    const CLASS1: bool = true;
    const CLASS2: bool = true;
    const CLASS3: bool = true;
    const EXTENSIONS: DavExtensionSet = DavExtensionSet::empty();
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}

impl<Provider: DavClass2Support + DavClass3Support + ?Sized> sealed::Class1Profile<Provider>
    for DavClass2And3Profile
{
}

/// Adds one statically implemented RFC package to another profile.
pub struct DavWithExtension<Base, Extension>(PhantomData<fn() -> (Base, Extension)>);

impl<Base: sealed::Sealed, Extension> sealed::Sealed for DavWithExtension<Base, Extension> {}

/// Sealed link between a package marker and its product implementation trait.
pub trait DavExtensionMarker<Provider: ?Sized>: sealed::ExtensionSealed {
    const PACKAGES: DavExtensionSet;
}

macro_rules! extension_marker {
    ($marker:ident, $support:path, $package:ident $(, $required:ident)*) => {
        pub struct $marker;
        impl sealed::ExtensionSealed for $marker {}
        impl<Provider: $support + ?Sized> DavExtensionMarker<Provider> for $marker {
            const PACKAGES: DavExtensionSet = DavExtensionSet::from_packages(&[
                $(DavExtensionPackage::$required,)*
                DavExtensionPackage::$package,
            ]);
        }
    };
}

extension_marker!(
    DavAccessControlExtension,
    DavAccessControlSupport,
    AccessControl
);
extension_marker!(
    DavVersionControlExtension,
    DavVersionControlSupport,
    VersionControl
);
extension_marker!(
    DavCheckoutInPlaceExtension,
    DavCheckoutInPlaceSupport,
    CheckoutInPlace,
    VersionControl
);
extension_marker!(
    DavVersionHistoryExtension,
    DavVersionHistorySupport,
    VersionHistory,
    VersionControl
);
extension_marker!(
    DavWorkspaceExtension,
    DavWorkspaceSupport,
    Workspace,
    VersionControl,
    CheckoutInPlace,
    VersionHistory
);
extension_marker!(DavUpdateExtension, DavUpdateSupport, Update, VersionControl);
extension_marker!(DavLabelExtension, DavLabelSupport, Label, VersionControl);
extension_marker!(
    DavWorkingResourceExtension,
    DavWorkingResourceSupport,
    WorkingResource,
    VersionControl
);
extension_marker!(DavMergeExtension, DavMergeSupport, Merge, VersionControl);
extension_marker!(
    DavBaselineExtension,
    DavBaselineSupport,
    Baseline,
    VersionControl
);
extension_marker!(
    DavActivityExtension,
    DavActivitySupport,
    Activity,
    VersionControl
);
extension_marker!(
    DavVersionControlledCollectionExtension,
    DavVersionControlledCollectionSupport,
    VersionControlledCollection,
    VersionControl
);
extension_marker!(DavSearchExtension, DavSearchSupport, Search);
extension_marker!(DavQuotaExtension, DavQuotaSupport, Quota);
extension_marker!(
    DavCollectionSyncExtension,
    DavCollectionSyncSupport,
    CollectionSync
);
extension_marker!(
    DavExtendedMkcolExtension,
    DavExtendedMkcolSupport,
    ExtendedMkcol
);
extension_marker!(
    DavCurrentPrincipalExtension,
    DavCurrentPrincipalSupport,
    CurrentPrincipal
);
extension_marker!(
    DavOrderedCollectionsExtension,
    DavOrderedCollectionsSupport,
    OrderedCollections
);
extension_marker!(
    DavRedirectReferencesExtension,
    DavRedirectReferencesSupport,
    RedirectReferences
);
extension_marker!(DavBindingsExtension, DavBindingsSupport, Bindings);
extension_marker!(DavAddMemberExtension, DavAddMemberSupport, AddMember);
extension_marker!(DavPreferExtension, DavPreferSupport, Prefer);

impl<Provider, Base, Extension> DavCapabilityProfile<Provider> for DavWithExtension<Base, Extension>
where
    Provider: ?Sized,
    Base: DavCapabilityProfile<Provider> + sealed::Class1Profile<Provider>,
    Extension: DavExtensionMarker<Provider>,
{
    const CLASS1: bool = Base::CLASS1;
    const CLASS2: bool = Base::CLASS2;
    const CLASS3: bool = Base::CLASS3;
    const EXTENSIONS: DavExtensionSet = Base::EXTENSIONS.union(Extension::PACKAGES);
    const PARTIAL_PUT: bool = Base::PARTIAL_PUT;
    const PATCH: bool = Base::PATCH;
    const PRIVATE_UPDATE_RANGE: bool = Base::PRIVATE_UPDATE_RANGE;
}

impl<Provider, Base, Extension> sealed::Class1Profile<Provider>
    for DavWithExtension<Base, Extension>
where
    Provider: ?Sized,
    Base: sealed::Class1Profile<Provider>,
    Extension: DavExtensionMarker<Provider>,
{
}

/// Builds a readable aggregate profile without a runtime registry.
///
/// Selecting a package without its implementation marker is a compile-time error:
///
/// ```compile_fail
/// use aster_forge_webdav::{
///     DavBackendError, DavCapabilityContext, DavCapabilityDeclaration, DavCapabilityProvider,
///     DavCapabilityTarget, DavClass1Profile, DavClass1Support, DavQuotaExtension,
///     dav_capability_profile,
/// };
/// struct MissingQuotaImplementation;
/// impl DavClass1Support for MissingQuotaImplementation {}
/// impl DavCapabilityProvider for MissingQuotaImplementation {
///     type Profile = dav_capability_profile!(DavClass1Profile; DavQuotaExtension);
///     async fn capabilities(
///         &self,
///         _target: &DavCapabilityTarget,
///         _context: &DavCapabilityContext,
///     ) -> Result<DavCapabilityDeclaration, DavBackendError> {
///         loop {}
///     }
/// }
/// ```
///
/// An RFC package also cannot be attached to a non-Class-1 base profile:
///
/// ```compile_fail
/// use aster_forge_webdav::{
///     DavBackendError, DavCapabilityContext, DavCapabilityDeclaration, DavCapabilityProvider,
///     DavCapabilityTarget, DavClass1Support, DavNonDavProfile, DavQuotaExtension,
///     DavQuotaSupport, dav_capability_profile,
/// };
/// struct InvalidBase;
/// impl DavClass1Support for InvalidBase {}
/// impl DavQuotaSupport for InvalidBase {}
/// impl DavCapabilityProvider for InvalidBase {
///     type Profile = dav_capability_profile!(DavNonDavProfile; DavQuotaExtension);
///     async fn capabilities(
///         &self,
///         _target: &DavCapabilityTarget,
///         _context: &DavCapabilityContext,
///     ) -> Result<DavCapabilityDeclaration, DavBackendError> {
///         loop {}
///     }
/// }
/// ```
#[macro_export]
macro_rules! dav_capability_profile {
    ($base:ty $(,)?) => { $base };
    ($base:ty; $extension:ty $(,)?) => {
        $crate::DavWithExtension<$base, $extension>
    };
    ($base:ty; $extension:ty, $($remaining:ty),+ $(,)?) => {
        $crate::dav_capability_profile!(
            $crate::DavWithExtension<$base, $extension>;
            $($remaining),+
        )
    };
}

pub struct DavWithPartialPut<Base>(PhantomData<fn() -> Base>);
pub struct DavWithPatch<Base>(PhantomData<fn() -> Base>);
pub struct DavWithPrivateUpdateRange<Base>(PhantomData<fn() -> Base>);

impl<Provider: ?Sized, Base: sealed::Class1Profile<Provider>> sealed::Class1Profile<Provider>
    for DavWithPartialPut<Base>
{
}
impl<Provider: ?Sized, Base: sealed::Class1Profile<Provider>> sealed::Class1Profile<Provider>
    for DavWithPatch<Base>
{
}
impl<Provider: ?Sized, Base: sealed::Class1Profile<Provider>> sealed::Class1Profile<Provider>
    for DavWithPrivateUpdateRange<Base>
{
}

impl<Base: sealed::Sealed> sealed::Sealed for DavWithPartialPut<Base> {}
impl<Base: sealed::Sealed> sealed::Sealed for DavWithPatch<Base> {}
impl<Base: sealed::Sealed> sealed::Sealed for DavWithPrivateUpdateRange<Base> {}

macro_rules! write_profile {
    ($wrapper:ident, $support:path, $partial:expr, $patch:expr, $private:expr) => {
        impl<Provider, Base> DavCapabilityProfile<Provider> for $wrapper<Base>
        where
            Provider: $support + ?Sized,
            Base: DavCapabilityProfile<Provider>,
        {
            const CLASS1: bool = Base::CLASS1;
            const CLASS2: bool = Base::CLASS2;
            const CLASS3: bool = Base::CLASS3;
            const EXTENSIONS: DavExtensionSet = Base::EXTENSIONS;
            const PARTIAL_PUT: bool = $partial || Base::PARTIAL_PUT;
            const PATCH: bool = $patch || Base::PATCH;
            const PRIVATE_UPDATE_RANGE: bool = $private || Base::PRIVATE_UPDATE_RANGE;
        }
    };
}

write_profile!(DavWithPartialPut, DavPartialPutSupport, true, false, false);
write_profile!(DavWithPatch, DavPatchSupport, false, true, false);
write_profile!(
    DavWithPrivateUpdateRange,
    DavPrivateUpdateRangeSupport,
    false,
    false,
    true
);

/// Product-owned facts projected for one target and request context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCapabilityDeclaration {
    pub resource: DavResourceState,
    pub versioning: DavVersioningCapabilities,
    /// Methods allowed for this principal and target. Package implementation methods are derived.
    pub methods: DavMethodSet,
    pub locking: DavLockingCapability,
    pub extensions: DavExtensionSet,
    pub search: DavSearchCapabilities,
    pub compatibility: DavCompatibilityCapabilities,
    pub writes: DavWriteCapabilities,
    pub compliance: DavComplianceClasses,
}

impl DavCapabilityDeclaration {
    #[must_use]
    pub const fn new(resource: DavResourceState, methods: DavMethodSet) -> Self {
        Self {
            resource,
            versioning: DavVersioningCapabilities {
                state: DavVersioningState::Unsupported,
                auto_version: DavAutoVersion::None,
                write_locked: false,
                auto_checkout_lock: false,
                allow_version_delete: false,
            },
            methods,
            locking: DavLockingCapability::Disabled,
            extensions: DavExtensionSet::empty(),
            search: DavSearchCapabilities { grammars: &[] },
            compatibility: DavCompatibilityCapabilities {
                ms_author_via: false,
            },
            writes: DavWriteCapabilities {
                partial_put: DavPartialPutCapability::Disabled,
                patch: DavPatchCapability::Disabled,
                private_update_range: DavPrivateUpdateRangeCapability::Disabled,
            },
            compliance: DavComplianceClasses {
                class1: false,
                class3: false,
            },
        }
    }
}

/// Failure raised when a product declaration violates the protocol model.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavCapabilityPlanError {
    #[error("OPTIONS must be advertised for every WebDAV target")]
    OptionsMissing,
    #[error("HEAD requires GET in a WebDAV capability declaration")]
    HeadWithoutGet,
    #[error("DAV locking compliance class 2 requires class 1")]
    Class2WithoutClass1,
    #[error("DAV compliance class 2 requires LOCK and UNLOCK methods")]
    Class2WithoutLockMethods,
    #[error("LOCK and UNLOCK require DAV compliance class 2")]
    LockMethodsWithoutClass2,
    #[error("DAV compliance class 3 requires class 1")]
    Class3WithoutClass1,
    #[error("RFC extension {package:?} requires class 1")]
    ExtensionWithoutClass1 { package: DavExtensionPackage },
    #[error("RFC extension {package:?} requires {required:?}")]
    ExtensionMissingPrerequisite {
        package: DavExtensionPackage,
        required: DavExtensionPackage,
    },
    #[error("RFC extension {package:?} does not apply to {resource:?}")]
    ExtensionNotApplicable {
        package: DavExtensionPackage,
        resource: DavResourceState,
    },
    #[error("the RFC 3253 version-control package requires explicit target versioning facts")]
    VersionControlWithoutTarget,
    #[error("versioning target facts require the RFC 3253 version-control package")]
    VersioningTargetWithoutPackage,
    #[error("automatic versioning is only valid for a checked-in or checked-out resource")]
    AutoVersionNotApplicable,
    #[error("a write-locked versioning target requires DAV locking compliance class 2")]
    WriteLockWithoutClass2,
    #[error("an automatic checkout lock is only valid for a write-locked checked-out resource")]
    AutoCheckoutLockNotApplicable,
    #[error(
        "an automatic checkout lock requires an auto-version mode that can leave a locked resource checked out"
    )]
    AutoCheckoutLockWithoutApplicableMode,
    #[error("version DELETE policy is only applicable to immutable version resources")]
    VersionDeletePolicyNotApplicable,
    #[error("VERSION-CONTROL is only applicable to versionable or version-controlled resources")]
    VersionControlMethodNotApplicable,
    #[error("method {method:?} requires an applicable RFC extension package")]
    ExtensionMethodWithoutPackage { method: DavMethod },
    #[error("SEARCH requires at least DAV:basicsearch")]
    SearchWithoutBasicSearch,
    #[error("SEARCH grammars require the SEARCH extension package")]
    SearchGrammarsWithoutPackage,
    #[error("SEARCH grammar {index} has invalid coded URL {coded_url:?}")]
    InvalidSearchGrammarCodedUrl {
        index: usize,
        coded_url: &'static str,
    },
    #[error("SEARCH grammar {index} has invalid XML local name {xml_local_name:?}")]
    InvalidSearchGrammarXmlLocalName {
        index: usize,
        xml_local_name: &'static str,
    },
    #[error("SEARCH grammar {index} has invalid XML namespace {xml_namespace:?}")]
    InvalidSearchGrammarXmlNamespace {
        index: usize,
        xml_namespace: &'static str,
    },
    #[error("SEARCH grammar {index} duplicates grammar {previous_index}")]
    DuplicateSearchGrammar {
        index: usize,
        previous_index: usize,
        coded_url: &'static str,
        xml_namespace: &'static str,
        xml_local_name: &'static str,
    },
    #[error("partial PUT requires the PUT method")]
    PartialPutWithoutPut,
    #[error("PATCH requires at least one declared patch document format")]
    PatchWithoutFormats,
    #[error("declared patch document formats require the PATCH method")]
    PatchFormatsWithoutMethod,
    #[error("a patch document media type is invalid")]
    InvalidPatchMediaType,
    #[error("patch document media types must be unique")]
    DuplicatePatchMediaType,
    #[error("private X-Update-Range support requires the PUT method")]
    PrivateUpdateRangeWithoutPut,
    #[error("runtime class 1 capability exceeds the provider's static profile")]
    Class1ExceedsProfile,
    #[error("runtime class 2 capability exceeds the provider's static profile")]
    Class2ExceedsProfile,
    #[error("runtime class 3 capability exceeds the provider's static profile")]
    Class3ExceedsProfile,
    #[error("runtime extension {package:?} exceeds the provider's static profile")]
    ExtensionExceedsProfile { package: DavExtensionPackage },
    #[error("runtime partial PUT capability exceeds the provider's static profile")]
    PartialPutExceedsProfile,
    #[error("runtime PATCH capability exceeds the provider's static profile")]
    PatchExceedsProfile,
    #[error("runtime private update-range capability exceeds the provider's static profile")]
    PrivateUpdateRangeExceedsProfile,
    #[error("capability header representation is invalid")]
    InvalidHeaderRepresentation,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavCapabilityEvaluationError {
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error(transparent)]
    Plan(#[from] DavCapabilityPlanError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavMethodGateError {
    #[error("WebDAV method is not allowed for this resource")]
    MethodNotAllowed,
}

/// Immutable capability state consumed by discovery, dispatch and property planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCapabilitySnapshot {
    declaration: DavCapabilityDeclaration,
    supported_methods: DavMethodSet,
    dispatch_methods: DavMethodSet,
    live_properties: DavLivePropertySet,
    reports: DavReportSet,
    preferences: DavPreferenceSet,
    allow: HeaderValue,
    dav: Option<HeaderValue>,
    dasl: Option<HeaderValue>,
    accept_patch: Option<HeaderValue>,
    ms_author_via: bool,
}

impl DavCapabilitySnapshot {
    #[must_use]
    pub fn declaration(&self) -> &DavCapabilityDeclaration {
        &self.declaration
    }

    #[must_use]
    pub const fn methods(&self) -> DavMethodSet {
        self.declaration.methods
    }

    #[must_use]
    pub const fn supported_methods(&self) -> DavMethodSet {
        self.supported_methods
    }

    #[must_use]
    pub const fn allows(&self, method: DavMethod) -> bool {
        self.methods().contains(method)
    }

    #[must_use]
    pub const fn supports(&self, method: DavMethod) -> bool {
        self.supported_methods.contains(method)
    }

    /// Returns whether the protocol engine should dispatch this method for target-state handling.
    #[must_use]
    pub const fn dispatches(&self, method: DavMethod) -> bool {
        self.dispatch_methods.contains(method)
    }

    #[must_use]
    pub const fn extensions(&self) -> DavExtensionSet {
        self.declaration.extensions
    }

    #[must_use]
    pub const fn supports_extension(&self, package: DavExtensionPackage) -> bool {
        self.extensions().contains(package)
    }

    #[must_use]
    pub const fn supports_live_property(&self, property: DavLiveProperty) -> bool {
        self.live_properties.contains(property)
    }

    #[must_use]
    pub const fn supports_report(&self, report: DavReportType) -> bool {
        self.reports.contains(report)
    }

    #[must_use]
    pub const fn preferences(&self) -> DavPreferenceSet {
        self.preferences
    }

    #[must_use]
    pub fn allow_header(&self) -> &HeaderValue {
        &self.allow
    }

    #[must_use]
    pub fn dav_header(&self) -> Option<&HeaderValue> {
        self.dav.as_ref()
    }

    #[must_use]
    pub fn dasl_header(&self) -> Option<&HeaderValue> {
        self.dasl.as_ref()
    }

    #[must_use]
    pub fn accept_patch_header(&self) -> Option<&HeaderValue> {
        self.accept_patch.as_ref()
    }

    #[must_use]
    pub const fn writes(&self) -> DavWriteCapabilities {
        self.declaration.writes
    }

    pub(crate) const fn patch_formats(&self) -> Option<&'static [DavPatchFormat]> {
        match self.declaration.writes.patch {
            DavPatchCapability::Disabled => None,
            DavPatchCapability::Formats(formats) => Some(formats),
        }
    }

    #[must_use]
    pub const fn has_ms_author_via(&self) -> bool {
        self.ms_author_via
    }

    /// Selects body handling from the same snapshot used by dispatch and discovery.
    ///
    /// # Errors
    ///
    /// Returns [`DavMethodGateError`] when the capability snapshot does not dispatch the method.
    pub fn body_policy(
        &self,
        method: DavMethod,
        xml_limit: usize,
    ) -> Result<Option<DavBodyPolicy>, DavMethodGateError> {
        if !self.dispatches(method) {
            return Err(DavMethodGateError::MethodNotAllowed);
        }
        let extension_kind = || {
            extension_body_kind(
                self.declaration.extensions,
                self.declaration.resource,
                method,
            )
        };
        Ok(match method {
            DavMethod::Patch => None,
            DavMethod::Mkcol => Some(extension_kind().map_or(DavBodyPolicy::Empty, |kind| {
                extension_body_policy(kind, xml_limit)
            })),
            DavMethod::Options
            | DavMethod::Delete
            | DavMethod::Copy
            | DavMethod::Move
            | DavMethod::Unlock => Some(DavBodyPolicy::Empty),
            DavMethod::Propfind | DavMethod::Proppatch | DavMethod::Lock => {
                Some(DavBodyPolicy::BoundedXml { maximum: xml_limit })
            }
            DavMethod::Post => Some(extension_kind().map_or(DavBodyPolicy::Stream, |kind| {
                extension_body_policy(kind, xml_limit)
            })),
            DavMethod::Put => Some(DavBodyPolicy::Stream),
            DavMethod::Get | DavMethod::Head => Some(DavBodyPolicy::Unused),
            DavMethod::Acl
            | DavMethod::Report
            | DavMethod::VersionControl
            | DavMethod::Checkout
            | DavMethod::Checkin
            | DavMethod::Uncheckout
            | DavMethod::Mkworkspace
            | DavMethod::Update
            | DavMethod::Label
            | DavMethod::Merge
            | DavMethod::BaselineControl
            | DavMethod::Mkactivity
            | DavMethod::Search
            | DavMethod::Orderpatch
            | DavMethod::Mkredirectref
            | DavMethod::Updateredirectref
            | DavMethod::Bind
            | DavMethod::Unbind
            | DavMethod::Rebind => {
                extension_kind().map(|kind| extension_body_policy(kind, xml_limit))
            }
        })
    }
}

/// Builds and validates a capability snapshot from product-owned runtime facts.
///
/// # Errors
///
/// Returns [`DavCapabilityPlanError`] when the declaration violates capability rules.
pub fn plan_capabilities(
    mut declaration: DavCapabilityDeclaration,
) -> Result<DavCapabilitySnapshot, DavCapabilityPlanError> {
    validate_base_methods(&mut declaration)?;
    validate_compliance(&declaration)?;
    validate_extensions(&declaration)?;
    validate_extension_methods(&declaration)?;
    validate_writes(&declaration)?;

    let package_methods = extension_methods_for_declaration(&declaration);
    let supported_methods = declaration.methods.union(package_methods);
    let dispatch_methods = declaration
        .methods
        .union(unmapped_dispatch_methods(declaration.resource));
    let allow = header_value(&declaration.methods.render())?;
    let dav = render_dav_header(&declaration)?;
    let dasl = render_dasl_header(&declaration)?;
    let accept_patch = match declaration.writes.patch {
        DavPatchCapability::Disabled => None,
        DavPatchCapability::Formats(formats) => Some(header_value(&render_patch_formats(formats))?),
    };
    let mut live_properties = DavLivePropertySet::empty();
    let mut reports = DavReportSet::empty();
    let mut preferences = DavPreferenceSet::empty();
    for package in declaration.extensions.iter() {
        let descriptor = package.descriptor();
        for property in descriptor.live_properties {
            if package != DavExtensionPackage::VersionControl
                || versioning_live_property(declaration.versioning.state, *property)
            {
                live_properties.insert(*property);
            }
        }
        for report in descriptor.reports {
            if package != DavExtensionPackage::VersionControl
                || versioning_report(declaration.versioning.state, *report)
            {
                reports.insert(*report);
            }
        }
        preferences = preferences.union(descriptor.preferences);
    }
    let ms_author_via = declaration.compatibility.ms_author_via;
    Ok(DavCapabilitySnapshot {
        declaration,
        supported_methods,
        dispatch_methods,
        live_properties,
        reports,
        preferences,
        allow,
        dav,
        dasl,
        accept_patch,
        ms_author_via,
    })
}

const fn unmapped_dispatch_methods(resource: DavResourceState) -> DavMethodSet {
    if matches!(resource, DavResourceState::Unmapped) {
        DavMethodSet::from_methods(&[
            DavMethod::Get,
            DavMethod::Head,
            DavMethod::Delete,
            DavMethod::Copy,
            DavMethod::Move,
            DavMethod::Propfind,
            DavMethod::Proppatch,
        ])
    } else {
        DavMethodSet::empty()
    }
}

#[expect(
    async_fn_in_trait,
    reason = "The provider is generic and intentionally preserves native async futures."
)]
pub trait DavCapabilityProvider: Send + Sync {
    type Profile: DavCapabilityProfile<Self>;

    async fn capabilities(
        &self,
        target: &DavCapabilityTarget,
        context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError>;
}

/// Resolves product facts and enforces the static profile maximum before planning.
///
/// # Errors
///
/// Returns an error when provider lookup fails or its declaration exceeds the static profile.
pub async fn plan_capabilities_with_provider<Provider: DavCapabilityProvider>(
    provider: &Provider,
    target: &DavCapabilityTarget,
    context: &DavCapabilityContext,
) -> Result<DavCapabilitySnapshot, DavCapabilityEvaluationError> {
    let declaration = provider.capabilities(target, context).await?;
    if declaration.compliance.class1 && !Provider::Profile::CLASS1 {
        return Err(DavCapabilityPlanError::Class1ExceedsProfile.into());
    }
    if declaration.locking == DavLockingCapability::Class2 && !Provider::Profile::CLASS2 {
        return Err(DavCapabilityPlanError::Class2ExceedsProfile.into());
    }
    if declaration.compliance.class3 && !Provider::Profile::CLASS3 {
        return Err(DavCapabilityPlanError::Class3ExceedsProfile.into());
    }
    for package in declaration.extensions.iter() {
        if !Provider::Profile::EXTENSIONS.contains(package) {
            return Err(DavCapabilityPlanError::ExtensionExceedsProfile { package }.into());
        }
    }
    if declaration.writes.partial_put != DavPartialPutCapability::Disabled
        && !Provider::Profile::PARTIAL_PUT
    {
        return Err(DavCapabilityPlanError::PartialPutExceedsProfile.into());
    }
    if declaration.writes.patch != DavPatchCapability::Disabled && !Provider::Profile::PATCH {
        return Err(DavCapabilityPlanError::PatchExceedsProfile.into());
    }
    if declaration.writes.private_update_range != DavPrivateUpdateRangeCapability::Disabled
        && !Provider::Profile::PRIVATE_UPDATE_RANGE
    {
        return Err(DavCapabilityPlanError::PrivateUpdateRangeExceedsProfile.into());
    }
    plan_capabilities(declaration).map_err(Into::into)
}

fn validate_base_methods(
    declaration: &mut DavCapabilityDeclaration,
) -> Result<(), DavCapabilityPlanError> {
    if !declaration.methods.contains(DavMethod::Options) {
        return Err(DavCapabilityPlanError::OptionsMissing);
    }
    if declaration.methods.contains(DavMethod::Get) {
        declaration.methods = declaration.methods.with(DavMethod::Head);
    }
    if declaration.methods.contains(DavMethod::Head)
        && !declaration.methods.contains(DavMethod::Get)
    {
        return Err(DavCapabilityPlanError::HeadWithoutGet);
    }
    Ok(())
}

fn validate_compliance(
    declaration: &DavCapabilityDeclaration,
) -> Result<(), DavCapabilityPlanError> {
    let locking_enabled = declaration.locking == DavLockingCapability::Class2;
    if locking_enabled && !declaration.compliance.class1 {
        return Err(DavCapabilityPlanError::Class2WithoutClass1);
    }
    if declaration.compliance.class3 && !declaration.compliance.class1 {
        return Err(DavCapabilityPlanError::Class3WithoutClass1);
    }
    let has_lock_method = declaration.methods.contains(DavMethod::Lock)
        || declaration.methods.contains(DavMethod::Unlock);
    let has_lock_methods = declaration.methods.contains(DavMethod::Lock)
        && declaration.methods.contains(DavMethod::Unlock);
    if (has_lock_method || locking_enabled) && !has_lock_methods {
        return Err(DavCapabilityPlanError::Class2WithoutLockMethods);
    }
    if has_lock_methods && !locking_enabled {
        return Err(DavCapabilityPlanError::LockMethodsWithoutClass2);
    }
    Ok(())
}

fn validate_extensions(
    declaration: &DavCapabilityDeclaration,
) -> Result<(), DavCapabilityPlanError> {
    if !declaration.compliance.class1
        && let Some(package) = declaration.extensions.iter().next()
    {
        return Err(DavCapabilityPlanError::ExtensionWithoutClass1 { package });
    }
    for package in declaration.extensions.iter() {
        let descriptor = package.descriptor();
        for required in descriptor.prerequisites.iter() {
            if !declaration.extensions.contains(required) {
                return Err(DavCapabilityPlanError::ExtensionMissingPrerequisite {
                    package,
                    required,
                });
            }
        }
        if !descriptor.resources.contains(declaration.resource) {
            return Err(DavCapabilityPlanError::ExtensionNotApplicable {
                package,
                resource: declaration.resource,
            });
        }
    }
    let has_version_control = declaration
        .extensions
        .contains(DavExtensionPackage::VersionControl);
    if has_version_control && declaration.versioning.state == DavVersioningState::Unsupported {
        return Err(DavCapabilityPlanError::VersionControlWithoutTarget);
    }
    if !has_version_control && declaration.versioning.state != DavVersioningState::Unsupported {
        return Err(DavCapabilityPlanError::VersioningTargetWithoutPackage);
    }
    if declaration.versioning.auto_version != DavAutoVersion::None
        && !matches!(
            declaration.versioning.state,
            DavVersioningState::CheckedIn | DavVersioningState::CheckedOut
        )
    {
        return Err(DavCapabilityPlanError::AutoVersionNotApplicable);
    }
    if declaration.versioning.write_locked && declaration.locking != DavLockingCapability::Class2 {
        return Err(DavCapabilityPlanError::WriteLockWithoutClass2);
    }
    if declaration.versioning.auto_checkout_lock
        && (!declaration.versioning.write_locked
            || declaration.versioning.state != DavVersioningState::CheckedOut)
    {
        return Err(DavCapabilityPlanError::AutoCheckoutLockNotApplicable);
    }
    if declaration.versioning.auto_checkout_lock
        && !matches!(
            declaration.versioning.auto_version,
            DavAutoVersion::CheckoutUnlockedCheckin
                | DavAutoVersion::Checkout
                | DavAutoVersion::LockedCheckout
        )
    {
        return Err(DavCapabilityPlanError::AutoCheckoutLockWithoutApplicableMode);
    }
    if declaration.versioning.allow_version_delete
        && declaration.versioning.state != DavVersioningState::Version
    {
        return Err(DavCapabilityPlanError::VersionDeletePolicyNotApplicable);
    }
    let search_enabled = declaration.extensions.contains(DavExtensionPackage::Search);
    if search_enabled {
        if !declaration
            .search
            .grammars
            .contains(&DavSearchGrammar::BASICSEARCH)
        {
            return Err(DavCapabilityPlanError::SearchWithoutBasicSearch);
        }
        validate_search_grammars(declaration.search.grammars)?;
    } else if !declaration.search.grammars.is_empty() {
        return Err(DavCapabilityPlanError::SearchGrammarsWithoutPackage);
    }
    Ok(())
}

fn validate_extension_methods(
    declaration: &DavCapabilityDeclaration,
) -> Result<(), DavCapabilityPlanError> {
    if declaration
        .extensions
        .contains(DavExtensionPackage::VersionControl)
        && declaration.methods.contains(DavMethod::VersionControl)
        && !matches!(
            declaration.versioning.state,
            DavVersioningState::Versionable
                | DavVersioningState::CheckedIn
                | DavVersioningState::CheckedOut
        )
    {
        return Err(DavCapabilityPlanError::VersionControlMethodNotApplicable);
    }
    let package_methods = extension_methods_for_declaration(declaration);
    for method in declaration.methods.iter() {
        if is_extension_only_method(method) && !package_methods.contains(method) {
            return Err(DavCapabilityPlanError::ExtensionMethodWithoutPackage { method });
        }
    }
    Ok(())
}

const fn versioning_methods(methods: DavMethodSet, state: DavVersioningState) -> DavMethodSet {
    match state {
        DavVersioningState::Versionable
        | DavVersioningState::CheckedIn
        | DavVersioningState::CheckedOut => methods,
        DavVersioningState::Version => methods.without(DavMethod::VersionControl),
        DavVersioningState::Unsupported => methods
            .without(DavMethod::VersionControl)
            .without(DavMethod::Report),
    }
}

fn extension_methods_for_declaration(declaration: &DavCapabilityDeclaration) -> DavMethodSet {
    let methods = extension_methods(declaration.extensions, declaration.resource);
    if declaration
        .extensions
        .contains(DavExtensionPackage::VersionControl)
    {
        versioning_methods(methods, declaration.versioning.state)
    } else {
        methods
    }
}

const fn versioning_report(state: DavVersioningState, report: DavReportType) -> bool {
    match report {
        DavReportType::ExpandProperty => !matches!(state, DavVersioningState::Unsupported),
        DavReportType::VersionTree => matches!(
            state,
            DavVersioningState::CheckedIn
                | DavVersioningState::CheckedOut
                | DavVersioningState::Version
        ),
        _ => true,
    }
}

const fn versioning_live_property(state: DavVersioningState, property: DavLiveProperty) -> bool {
    match property {
        DavLiveProperty::CheckedIn => matches!(state, DavVersioningState::CheckedIn),
        DavLiveProperty::CheckedOut => matches!(state, DavVersioningState::CheckedOut),
        DavLiveProperty::AutoVersion => matches!(
            state,
            DavVersioningState::CheckedIn | DavVersioningState::CheckedOut
        ),
        DavLiveProperty::PredecessorSet => matches!(
            state,
            DavVersioningState::CheckedOut | DavVersioningState::Version
        ),
        DavLiveProperty::SuccessorSet
        | DavLiveProperty::CheckoutSet
        | DavLiveProperty::VersionName => matches!(state, DavVersioningState::Version),
        _ => true,
    }
}

const fn is_extension_only_method(method: DavMethod) -> bool {
    matches!(
        method,
        DavMethod::Acl
            | DavMethod::Report
            | DavMethod::VersionControl
            | DavMethod::Checkout
            | DavMethod::Checkin
            | DavMethod::Uncheckout
            | DavMethod::Mkworkspace
            | DavMethod::Update
            | DavMethod::Label
            | DavMethod::Merge
            | DavMethod::BaselineControl
            | DavMethod::Mkactivity
            | DavMethod::Search
            | DavMethod::Orderpatch
            | DavMethod::Mkredirectref
            | DavMethod::Updateredirectref
            | DavMethod::Bind
            | DavMethod::Unbind
            | DavMethod::Rebind
    )
}

fn validate_writes(declaration: &DavCapabilityDeclaration) -> Result<(), DavCapabilityPlanError> {
    let put_enabled = declaration.methods.contains(DavMethod::Put);
    if declaration.writes.partial_put != DavPartialPutCapability::Disabled && !put_enabled {
        return Err(DavCapabilityPlanError::PartialPutWithoutPut);
    }
    if declaration.writes.private_update_range != DavPrivateUpdateRangeCapability::Disabled
        && !put_enabled
    {
        return Err(DavCapabilityPlanError::PrivateUpdateRangeWithoutPut);
    }
    let patch_enabled = declaration.methods.contains(DavMethod::Patch);
    match declaration.writes.patch {
        DavPatchCapability::Disabled if patch_enabled => {
            Err(DavCapabilityPlanError::PatchWithoutFormats)
        }
        DavPatchCapability::Disabled => Ok(()),
        DavPatchCapability::Formats(_) if !patch_enabled => {
            Err(DavCapabilityPlanError::PatchFormatsWithoutMethod)
        }
        DavPatchCapability::Formats([]) => Err(DavCapabilityPlanError::PatchWithoutFormats),
        DavPatchCapability::Formats(formats) => validate_patch_formats(formats),
    }
}

fn render_dav_header(
    declaration: &DavCapabilityDeclaration,
) -> Result<Option<HeaderValue>, DavCapabilityPlanError> {
    let mut rendered = String::new();
    if declaration.compliance.class1 {
        push_token(&mut rendered, "1");
    }
    if declaration.locking == DavLockingCapability::Class2 {
        push_token(&mut rendered, "2");
    }
    if declaration.compliance.class3 {
        push_token(&mut rendered, "3");
    }
    for package in declaration.extensions.iter() {
        if let Some(token) = package.descriptor().dav_token {
            push_token(&mut rendered, token);
        }
    }
    if rendered.is_empty() {
        Ok(None)
    } else {
        header_value(&rendered).map(Some)
    }
}

fn render_dasl_header(
    declaration: &DavCapabilityDeclaration,
) -> Result<Option<HeaderValue>, DavCapabilityPlanError> {
    if declaration.search.grammars.is_empty() {
        return Ok(None);
    }
    let mut rendered = String::new();
    for grammar in declaration.search.grammars {
        if !rendered.is_empty() {
            rendered.push_str(", ");
        }
        rendered.push('<');
        rendered.push_str(grammar.coded_url);
        rendered.push('>');
    }
    header_value(&rendered).map(Some)
}

fn validate_search_grammars(grammars: &[DavSearchGrammar]) -> Result<(), DavCapabilityPlanError> {
    for (index, grammar) in grammars.iter().enumerate() {
        if grammar.coded_url.is_empty()
            || grammar.coded_url.bytes().any(|byte| {
                byte <= b' ' || byte == b'<' || byte == b'>' || byte == b',' || byte == 0x7f
            })
            || parse_absolute_url(grammar.coded_url, "SEARCH grammar coded-URL").is_err()
        {
            return Err(DavCapabilityPlanError::InvalidSearchGrammarCodedUrl {
                index,
                coded_url: grammar.coded_url,
            });
        }
        if !is_valid_xml_local_name(grammar.xml_local_name) {
            return Err(DavCapabilityPlanError::InvalidSearchGrammarXmlLocalName {
                index,
                xml_local_name: grammar.xml_local_name,
            });
        }
        if !grammar.xml_namespace.is_empty()
            && (grammar.xml_namespace.trim() != grammar.xml_namespace
                || parse_absolute_url(grammar.xml_namespace, "SEARCH grammar namespace").is_err())
        {
            return Err(DavCapabilityPlanError::InvalidSearchGrammarXmlNamespace {
                index,
                xml_namespace: grammar.xml_namespace,
            });
        }
        if let Some(previous_index) = grammars[..index].iter().position(|previous| {
            previous.coded_url == grammar.coded_url
                || (previous.xml_namespace == grammar.xml_namespace
                    && previous.xml_local_name == grammar.xml_local_name)
        }) {
            return Err(DavCapabilityPlanError::DuplicateSearchGrammar {
                index,
                previous_index,
                coded_url: grammar.coded_url,
                xml_namespace: grammar.xml_namespace,
                xml_local_name: grammar.xml_local_name,
            });
        }
    }
    Ok(())
}

const LIVE_PROPERTY_SET_WORDS: usize = 2;
const LIVE_PROPERTY_SET_CAPACITY: usize = LIVE_PROPERTY_SET_WORDS * u64::BITS as usize;
const REPORT_SET_CAPACITY: usize = u16::BITS as usize;

const _: () = assert!(DavLiveProperty::COUNT <= LIVE_PROPERTY_SET_CAPACITY);
const _: () = assert!(DavReportType::COUNT <= REPORT_SET_CAPACITY);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct DavLivePropertySet([u64; LIVE_PROPERTY_SET_WORDS]);

impl DavLivePropertySet {
    const fn empty() -> Self {
        Self([0; LIVE_PROPERTY_SET_WORDS])
    }

    fn insert(&mut self, property: DavLiveProperty) {
        let index = property.index();
        self.0[index / 64] |= 1u64 << (index % 64);
    }

    const fn contains(self, property: DavLiveProperty) -> bool {
        let index = property.index();
        self.0[index / 64] & (1u64 << (index % 64)) != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct DavReportSet(u16);

impl DavReportSet {
    const fn empty() -> Self {
        Self(0)
    }

    fn insert(&mut self, report: DavReportType) {
        self.0 |= 1u16 << report.index();
    }

    const fn contains(self, report: DavReportType) -> bool {
        self.0 & (1u16 << report.index()) != 0
    }
}

fn validate_patch_formats(formats: &[DavPatchFormat]) -> Result<(), DavCapabilityPlanError> {
    for (index, format) in formats.iter().enumerate() {
        let media_type = format
            .media_type
            .parse::<Mime>()
            .map_err(|_| DavCapabilityPlanError::InvalidPatchMediaType)?;
        for duplicate in &formats[..index] {
            let other = duplicate
                .media_type
                .parse::<Mime>()
                .map_err(|_| DavCapabilityPlanError::InvalidPatchMediaType)?;
            if media_type == other {
                return Err(DavCapabilityPlanError::DuplicatePatchMediaType);
            }
        }
    }
    Ok(())
}

fn render_patch_formats(formats: &[DavPatchFormat]) -> String {
    let capacity = formats
        .iter()
        .map(|format| format.media_type.len())
        .sum::<usize>()
        + formats.len().saturating_sub(1) * 2;
    let mut rendered = String::with_capacity(capacity);
    for (index, format) in formats.iter().enumerate() {
        if index != 0 {
            rendered.push_str(", ");
        }
        rendered.push_str(format.media_type);
    }
    rendered
}

fn header_value(value: &str) -> Result<HeaderValue, DavCapabilityPlanError> {
    HeaderValue::from_str(value).map_err(|_| DavCapabilityPlanError::InvalidHeaderRepresentation)
}

fn push_token(rendered: &mut String, token: &str) {
    if !rendered.is_empty() {
        rendered.push_str(", ");
    }
    rendered.push_str(token);
}

const fn extension_body_policy(kind: DavExtensionBodyKind, xml_limit: usize) -> DavBodyPolicy {
    match kind {
        DavExtensionBodyKind::Xml => DavBodyPolicy::BoundedXml { maximum: xml_limit },
        DavExtensionBodyKind::Stream => DavBodyPolicy::Stream,
    }
}
