//! Resource-aware WebDAV capability declarations and response snapshots.

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

mod sealed {
    pub trait Sealed {}
}

/// Static maximum capability profile selected by a product provider.
pub trait DavCapabilityProfile<Provider: ?Sized>: sealed::Sealed {
    const CLASS1: bool;
    const CLASS2: bool;
    const VERSION_CONTROL: bool;
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

impl sealed::Sealed for DavNonDavProfile {}
impl sealed::Sealed for DavClass1Profile {}
impl sealed::Sealed for DavClass2Profile {}
impl sealed::Sealed for DavClass1VersioningProfile {}
impl sealed::Sealed for DavClass2VersioningProfile {}

impl<Provider: ?Sized> DavCapabilityProfile<Provider> for DavNonDavProfile {
    const CLASS1: bool = false;
    const CLASS2: bool = false;
    const VERSION_CONTROL: bool = false;
}

impl<Provider: DavClass1Support + ?Sized> DavCapabilityProfile<Provider> for DavClass1Profile {
    const CLASS1: bool = true;
    const CLASS2: bool = false;
    const VERSION_CONTROL: bool = false;
}

impl<Provider: DavClass2Support + ?Sized> DavCapabilityProfile<Provider> for DavClass2Profile {
    const CLASS1: bool = true;
    const CLASS2: bool = true;
    const VERSION_CONTROL: bool = false;
}

impl<Provider: DavCoreVersioningSupport + ?Sized> DavCapabilityProfile<Provider>
    for DavClass1VersioningProfile
{
    const CLASS1: bool = true;
    const CLASS2: bool = false;
    const VERSION_CONTROL: bool = true;
}

impl<Provider: DavClass2Support + DavCoreVersioningSupport + ?Sized> DavCapabilityProfile<Provider>
    for DavClass2VersioningProfile
{
    const CLASS1: bool = true;
    const CLASS2: bool = true;
    const VERSION_CONTROL: bool = true;
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
    #[error("runtime class 1 capability exceeds the provider's static profile")]
    Class1ExceedsProfile,
    #[error("runtime class 2 capability exceeds the provider's static profile")]
    Class2ExceedsProfile,
    #[error("runtime version-control capability exceeds the provider's static profile")]
    VersionControlExceedsProfile,
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

    let ms_author_via = declaration.compatibility.ms_author_via;
    Ok(DavCapabilitySnapshot {
        declaration,
        allow,
        dav: dav_text,
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
    plan_capabilities(declaration).map_err(Into::into)
}
