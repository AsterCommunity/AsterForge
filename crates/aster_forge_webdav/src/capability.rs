//! Resource-aware WebDAV capability declarations and response snapshots.

use std::marker::PhantomData;

use headers::Mime;
use http::HeaderValue;

use crate::{DavBackendError, DavMethod, DavPath};

/// The resource state visible to the WebDAV capability planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavResourceState {
    Unmapped,
    File,
    Collection,
    MountRoot,
}

/// A request target supplied to a capability provider.
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

/// RFC 3253 core versioning compliance advertised for a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavVersioningCapability {
    Disabled,
    Core,
}

/// Optional non-standard compatibility signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavCompatibilityCapabilities {
    pub ms_author_via: bool,
}

/// Conditional request policy required before a partial write can execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavWritePrecondition {
    /// Apply the ordinary RFC 9110 conditional request rules when headers are present.
    Optional,
    /// Require an `If-Match` field containing at least one strong entity-tag.
    RequireStrongIfMatch,
}

/// RFC 9110 partial PUT support negotiated by private agreement with the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPartialPutCapability {
    /// Reject every PUT request carrying `Content-Range`.
    #[default]
    Disabled,
    /// Accept a single `bytes` content range under the declared precondition policy.
    ContentRangeBytes { precondition: DavWritePrecondition },
}

/// Body handling selected by one RFC 5789 patch document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavPatchBodyPolicy {
    /// Collect at most the declared number of bytes before invoking product code.
    Bounded { maximum: usize },
    /// Leave the body as a stream for the product patch adapter.
    Stream,
}

/// One statically declared RFC 5789 patch document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DavPatchFormat {
    /// Media type advertised through `Accept-Patch` and matched against `Content-Type`.
    ///
    /// Matching and duplicate detection use equality of the complete parsed [`Mime`], including
    /// every declared parameter. Missing, additional, or different parameter values do not match.
    pub media_type: &'static str,
    /// Transport body contract for this patch document format.
    pub body_policy: DavPatchBodyPolicy,
    /// Conditional request policy required by this format's base-point semantics.
    pub precondition: DavWritePrecondition,
}

/// RFC 5789 PATCH capability for one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPatchCapability {
    /// PATCH is unavailable and must not appear in `Allow` or `Accept-Patch`.
    #[default]
    Disabled,
    /// PATCH is available for the statically declared patch document formats.
    Formats(&'static [DavPatchFormat]),
}

/// Explicitly named private range-update compatibility surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DavPrivateUpdateRangeCapability {
    /// Reject `X-Update-Range` instead of treating it as an ordinary PUT.
    #[default]
    Disabled,
    /// Pass one validated `X-Update-Range` field to the product adapter.
    XUpdateRange { precondition: DavWritePrecondition },
}

/// Mutation capabilities that are deliberately separate from ordinary full-replacement PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavWriteCapabilities {
    pub partial_put: DavPartialPutCapability,
    pub patch: DavPatchCapability,
    pub private_update_range: DavPrivateUpdateRangeCapability,
}

/// RFC compliance classes represented by the declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavComplianceClasses {
    pub class1: bool,
}

/// Product implementation provides the RFC 4918 class 1 backend contract.
pub trait DavClass1Support: Send + Sync {}

/// Product implementation provides the complete RFC 4918 class 2 locking contract.
pub trait DavClass2Support: DavClass1Support {}

/// Product implementation provides the complete RFC 3253 core versioning contract.
pub trait DavCoreVersioningSupport: DavClass1Support {}

/// Product implementation provides the storage contract required by partial PUT.
pub trait DavPartialPutSupport: Send + Sync {}

/// Product implementation provides atomic application for its declared PATCH formats.
pub trait DavPatchSupport: Send + Sync {}

/// Product implementation provides the named private `X-Update-Range` contract.
pub trait DavPrivateUpdateRangeSupport: Send + Sync {}

mod sealed {
    pub trait Sealed {}
}

/// Static maximum capability profile selected by a product provider.
pub trait DavCapabilityProfile<Provider: ?Sized>: sealed::Sealed {
    const CLASS1: bool;
    const CLASS2: bool;
    const VERSION_CONTROL: bool;
    const PARTIAL_PUT: bool;
    const PATCH: bool;
    const PRIVATE_UPDATE_RANGE: bool;
}

/// Profile for targets that expose HTTP methods without DAV compliance.
pub struct DavNonDavProfile;

/// Profile for RFC 4918 class 1 implementations.
pub struct DavClass1Profile;

/// Profile for complete RFC 4918 class 2 implementations.
pub struct DavClass2Profile;

/// Profile for RFC 4918 class 1 plus RFC 3253 core versioning.
pub struct DavClass1VersioningProfile;

/// Profile for RFC 4918 class 2 plus RFC 3253 core versioning.
pub struct DavClass2VersioningProfile;

/// Adds partial PUT to another static capability profile.
pub struct DavWithPartialPut<Base>(PhantomData<fn() -> Base>);

/// Adds RFC 5789 PATCH to another static capability profile.
pub struct DavWithPatch<Base>(PhantomData<fn() -> Base>);

/// Adds the private `X-Update-Range` surface to another static capability profile.
pub struct DavWithPrivateUpdateRange<Base>(PhantomData<fn() -> Base>);

impl sealed::Sealed for DavNonDavProfile {}
impl sealed::Sealed for DavClass1Profile {}
impl sealed::Sealed for DavClass2Profile {}
impl sealed::Sealed for DavClass1VersioningProfile {}
impl sealed::Sealed for DavClass2VersioningProfile {}
impl<Base: sealed::Sealed> sealed::Sealed for DavWithPartialPut<Base> {}
impl<Base: sealed::Sealed> sealed::Sealed for DavWithPatch<Base> {}
impl<Base: sealed::Sealed> sealed::Sealed for DavWithPrivateUpdateRange<Base> {}

macro_rules! static_profile {
    ($profile:ty, $class1:expr, $class2:expr, $version_control:expr) => {
        impl<Provider: ?Sized> DavCapabilityProfile<Provider> for $profile {
            const CLASS1: bool = $class1;
            const CLASS2: bool = $class2;
            const VERSION_CONTROL: bool = $version_control;
            const PARTIAL_PUT: bool = false;
            const PATCH: bool = false;
            const PRIVATE_UPDATE_RANGE: bool = false;
        }
    };
}

static_profile!(DavNonDavProfile, false, false, false);

impl<Provider: DavClass1Support + ?Sized> DavCapabilityProfile<Provider> for DavClass1Profile {
    const CLASS1: bool = true;
    const CLASS2: bool = false;
    const VERSION_CONTROL: bool = false;
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}

impl<Provider: DavClass2Support + ?Sized> DavCapabilityProfile<Provider> for DavClass2Profile {
    const CLASS1: bool = true;
    const CLASS2: bool = true;
    const VERSION_CONTROL: bool = false;
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}

impl<Provider: DavCoreVersioningSupport + ?Sized> DavCapabilityProfile<Provider>
    for DavClass1VersioningProfile
{
    const CLASS1: bool = true;
    const CLASS2: bool = false;
    const VERSION_CONTROL: bool = true;
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}

impl<Provider: DavClass2Support + DavCoreVersioningSupport + ?Sized> DavCapabilityProfile<Provider>
    for DavClass2VersioningProfile
{
    const CLASS1: bool = true;
    const CLASS2: bool = true;
    const VERSION_CONTROL: bool = true;
    const PARTIAL_PUT: bool = false;
    const PATCH: bool = false;
    const PRIVATE_UPDATE_RANGE: bool = false;
}

impl<Provider, Base> DavCapabilityProfile<Provider> for DavWithPartialPut<Base>
where
    Provider: DavPartialPutSupport + ?Sized,
    Base: DavCapabilityProfile<Provider>,
{
    const CLASS1: bool = Base::CLASS1;
    const CLASS2: bool = Base::CLASS2;
    const VERSION_CONTROL: bool = Base::VERSION_CONTROL;
    const PARTIAL_PUT: bool = true;
    const PATCH: bool = Base::PATCH;
    const PRIVATE_UPDATE_RANGE: bool = Base::PRIVATE_UPDATE_RANGE;
}

impl<Provider, Base> DavCapabilityProfile<Provider> for DavWithPatch<Base>
where
    Provider: DavPatchSupport + ?Sized,
    Base: DavCapabilityProfile<Provider>,
{
    const CLASS1: bool = Base::CLASS1;
    const CLASS2: bool = Base::CLASS2;
    const VERSION_CONTROL: bool = Base::VERSION_CONTROL;
    const PARTIAL_PUT: bool = Base::PARTIAL_PUT;
    const PATCH: bool = true;
    const PRIVATE_UPDATE_RANGE: bool = Base::PRIVATE_UPDATE_RANGE;
}

impl<Provider, Base> DavCapabilityProfile<Provider> for DavWithPrivateUpdateRange<Base>
where
    Provider: DavPrivateUpdateRangeSupport + ?Sized,
    Base: DavCapabilityProfile<Provider>,
{
    const CLASS1: bool = Base::CLASS1;
    const CLASS2: bool = Base::CLASS2;
    const VERSION_CONTROL: bool = Base::VERSION_CONTROL;
    const PARTIAL_PUT: bool = Base::PARTIAL_PUT;
    const PATCH: bool = Base::PATCH;
    const PRIVATE_UPDATE_RANGE: bool = true;
}

/// Compact, duplicate-free set of methods advertised for one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DavMethodSet(u16);

impl DavMethodSet {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_methods(methods: &[DavMethod]) -> Self {
        let mut set = Self::empty();
        let mut index = 0;
        while index < methods.len() {
            set = set.with(methods[index]);
            index += 1;
        }
        set
    }

    #[must_use]
    pub const fn with(self, method: DavMethod) -> Self {
        Self(self.0 | (1u16 << method.index()))
    }

    #[must_use]
    pub const fn contains(self, method: DavMethod) -> bool {
        self.0 & (1u16 << method.index()) != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn iter(self) -> DavMethodSetIter {
        DavMethodSetIter {
            set: self,
            index: 0,
        }
    }

    #[must_use]
    pub fn render(self) -> String {
        self.iter()
            .map(DavMethod::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Iterator over methods in the canonical protocol order.
#[derive(Debug, Clone, Copy)]
pub struct DavMethodSetIter {
    set: DavMethodSet,
    index: usize,
}

impl Iterator for DavMethodSetIter {
    type Item = DavMethod;

    fn next(&mut self) -> Option<Self::Item> {
        while self.index < DavMethod::ALL.len() {
            let method = DavMethod::ALL[self.index];
            self.index += 1;
            if self.set.contains(method) {
                return Some(method);
            }
        }
        None
    }
}

/// Product-owned facts projected for one target and request context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCapabilityDeclaration {
    pub resource: DavResourceState,
    pub methods: DavMethodSet,
    pub locking: DavLockingCapability,
    pub versioning: DavVersioningCapability,
    pub compatibility: DavCompatibilityCapabilities,
    pub writes: DavWriteCapabilities,
    pub compliance: DavComplianceClasses,
}

impl DavCapabilityDeclaration {
    #[must_use]
    pub const fn new(resource: DavResourceState, methods: DavMethodSet) -> Self {
        Self {
            resource,
            methods,
            locking: DavLockingCapability::Disabled,
            versioning: DavVersioningCapability::Disabled,
            compatibility: DavCompatibilityCapabilities {
                ms_author_via: false,
            },
            writes: DavWriteCapabilities {
                partial_put: DavPartialPutCapability::Disabled,
                patch: DavPatchCapability::Disabled,
                private_update_range: DavPrivateUpdateRangeCapability::Disabled,
            },
            compliance: DavComplianceClasses { class1: false },
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
    #[error("RFC 3253 version-control requires DAV compliance class 1")]
    VersionControlWithoutClass1,
    #[error("RFC 3253 version-control requires REPORT and VERSION-CONTROL methods")]
    VersionControlWithoutMethods,
    #[error("VERSION-CONTROL requires the RFC 3253 core versioning capability")]
    VersionControlMethodWithoutCore,
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
    #[error("runtime version-control capability exceeds the provider's static profile")]
    VersionControlExceedsProfile,
    #[error("runtime partial PUT capability exceeds the provider's static profile")]
    PartialPutExceedsProfile,
    #[error("runtime PATCH capability exceeds the provider's static profile")]
    PatchExceedsProfile,
    #[error("runtime private update-range capability exceeds the provider's static profile")]
    PrivateUpdateRangeExceedsProfile,
    #[error("capability header representation is invalid")]
    InvalidHeaderRepresentation,
}

/// Failure while resolving and planning capabilities through a product provider.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DavCapabilityEvaluationError {
    #[error(transparent)]
    Backend(#[from] DavBackendError),
    #[error(transparent)]
    Plan(#[from] DavCapabilityPlanError),
}

/// Result of checking a request method against a validated capability snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DavMethodGateError {
    #[error("WebDAV method is not allowed for this resource")]
    MethodNotAllowed,
}

/// Immutable, validated capability state consumed by all response and dispatch paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavCapabilitySnapshot {
    declaration: DavCapabilityDeclaration,
    allow: HeaderValue,
    dav: Option<HeaderValue>,
    accept_patch: Option<HeaderValue>,
    ms_author_via: bool,
}

impl DavCapabilitySnapshot {
    #[must_use]
    pub fn declaration(&self) -> &DavCapabilityDeclaration {
        &self.declaration
    }

    #[must_use]
    pub fn methods(&self) -> DavMethodSet {
        self.declaration.methods
    }

    #[must_use]
    pub fn allows(&self, method: DavMethod) -> bool {
        self.methods().contains(method)
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
}

/// Builds and validates a capability snapshot from a product declaration.
pub fn plan_capabilities(
    mut declaration: DavCapabilityDeclaration,
) -> Result<DavCapabilitySnapshot, DavCapabilityPlanError> {
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
    let locking_enabled = declaration.locking == DavLockingCapability::Class2;
    if locking_enabled && !declaration.compliance.class1 {
        return Err(DavCapabilityPlanError::Class2WithoutClass1);
    }
    let has_lock_method = declaration.methods.contains(DavMethod::Lock)
        || declaration.methods.contains(DavMethod::Unlock);
    let has_lock_methods = declaration.methods.contains(DavMethod::Lock)
        && declaration.methods.contains(DavMethod::Unlock);
    if has_lock_method && !has_lock_methods {
        return Err(DavCapabilityPlanError::Class2WithoutLockMethods);
    }
    if locking_enabled && !has_lock_methods {
        return Err(DavCapabilityPlanError::Class2WithoutLockMethods);
    }
    if has_lock_methods && !locking_enabled {
        return Err(DavCapabilityPlanError::LockMethodsWithoutClass2);
    }
    let versioning_enabled = declaration.versioning == DavVersioningCapability::Core;
    if versioning_enabled && !declaration.compliance.class1 {
        return Err(DavCapabilityPlanError::VersionControlWithoutClass1);
    }
    if versioning_enabled
        && !(declaration.methods.contains(DavMethod::Report)
            && declaration.methods.contains(DavMethod::VersionControl))
    {
        return Err(DavCapabilityPlanError::VersionControlWithoutMethods);
    }
    if declaration.methods.contains(DavMethod::VersionControl) && !versioning_enabled {
        return Err(DavCapabilityPlanError::VersionControlMethodWithoutCore);
    }
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
    let patch_formats = match declaration.writes.patch {
        DavPatchCapability::Disabled => {
            if patch_enabled {
                return Err(DavCapabilityPlanError::PatchWithoutFormats);
            }
            None
        }
        DavPatchCapability::Formats(formats) => {
            if !patch_enabled {
                return Err(DavCapabilityPlanError::PatchFormatsWithoutMethod);
            }
            if formats.is_empty() {
                return Err(DavCapabilityPlanError::PatchWithoutFormats);
            }
            validate_patch_formats(formats)?;
            Some(formats)
        }
    };

    let allow = HeaderValue::from_str(&declaration.methods.render())
        .map_err(|_| DavCapabilityPlanError::InvalidHeaderRepresentation)?;
    let dav_text = {
        let mut tokens = Vec::new();
        if declaration.compliance.class1 {
            tokens.push("1");
        }
        if locking_enabled {
            tokens.push("2");
        }
        if versioning_enabled {
            tokens.push("version-control");
        }
        if tokens.is_empty() {
            None
        } else {
            Some(
                HeaderValue::from_str(&tokens.join(", "))
                    .map_err(|_| DavCapabilityPlanError::InvalidHeaderRepresentation)?,
            )
        }
    };
    let accept_patch = patch_formats
        .map(render_patch_formats)
        .map(|value| {
            HeaderValue::from_str(&value)
                .map_err(|_| DavCapabilityPlanError::InvalidHeaderRepresentation)
        })
        .transpose()?;

    let ms_author_via = declaration.compatibility.ms_author_via;
    Ok(DavCapabilitySnapshot {
        declaration,
        allow,
        dav: dav_text,
        accept_patch,
        ms_author_via,
    })
}

/// Product capability provider used by transport adapters and WebDAV dispatchers.
///
/// The associated profile is checked against support supertraits at compile time. For example,
/// selecting [`DavClass2Profile`] requires the provider to implement [`DavClass2Support`], which
/// itself requires [`DavClass1Support`].
#[expect(
    async_fn_in_trait,
    reason = "The provider is intentionally generic and uses async fn without boxing futures."
)]
pub trait DavCapabilityProvider: Send + Sync {
    type Profile: DavCapabilityProfile<Self>;

    async fn capabilities(
        &self,
        target: &DavCapabilityTarget,
        context: &DavCapabilityContext,
    ) -> Result<DavCapabilityDeclaration, DavBackendError>;
}

/// Resolves product facts and returns the validated snapshot used by the request.
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
    if declaration.versioning == DavVersioningCapability::Core
        && !Provider::Profile::VERSION_CONTROL
    {
        return Err(DavCapabilityPlanError::VersionControlExceedsProfile.into());
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
