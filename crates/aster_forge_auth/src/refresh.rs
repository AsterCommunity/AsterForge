//! Refresh state-machine decisions and a transaction-aware persistence port.

use crate::{
    AuthError, AuthTokenPair, AuthTokenType, Clock, JwtSigner, JwtVerifier, RefreshCoordinator,
    RefreshJti, Restriction, SessionId, SessionVersion, Subject,
};
use async_trait::async_trait;
use std::{fmt, sync::Arc};
use tokio_util::sync::CancellationToken;

/// Untrusted client metadata used only for stale-retry classification, never device authentication.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientContext {
    /// IP already normalized by the product's trusted-proxy boundary.
    pub ip_address: Option<String>,
    /// User agent supplied by the client; matching it is not proof of identity.
    pub user_agent: Option<String>,
}

/// Explicit replay policy. Both variants reject old credentials; neither grants a second refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RefreshPolicy {
    /// Every identifiable old credential is treated as possible theft.
    #[default]
    Strict,
    /// Classify only the immediately previous JTI with matching nonempty UA and compatible IP as
    /// stale, within this inclusive window. The product explicitly accepts spoofable fingerprint
    /// risk; missing/mismatched evidence still triggers reuse handling.
    SameClientWithin {
        /// Maximum age of the previous rotation, in seconds.
        seconds: u64,
    },
}

/// Authoritative product session snapshot, read under the transaction used for the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshSession {
    /// Account owning this session.
    pub subject: Subject,
    /// Stable session identity used to identify replays older than the previous JTI.
    pub session_id: SessionId,
    /// Currently usable refresh identity.
    pub current_refresh_jti: RefreshJti,
    /// Immediately preceding identity, if this session has rotated.
    pub previous_refresh_jti: Option<RefreshJti>,
    /// Last committed successful rotation time.
    pub last_seen_at: i64,
    /// Exclusive server-side session expiry.
    pub refresh_expires_at: i64,
    /// Whether this session has been revoked.
    pub revoked: bool,
    /// Stored client evidence from the last successful request.
    pub client: ClientContext,
}

/// Candidate mutation submitted to the product's writer transaction. It contains IDs and expiry,
/// never token strings. Stores must recheck current account state, version, restriction and session
/// ownership, then use [`classify_refresh`] before conditional rotation or atomic reuse revocation.
#[derive(Debug, Clone)]
pub struct RefreshRotationRequest {
    /// Verified account subject.
    pub subject: Subject,
    /// Verified session identity.
    pub session_id: SessionId,
    /// Verified account generation.
    pub session_version: SessionVersion,
    /// Verified login restriction.
    pub restriction: Restriction,
    /// Expected usable refresh identity.
    pub current_refresh_jti: RefreshJti,
    /// Exclusive credential deadline (`exp` plus verifier leeway), widened to avoid overflow.
    /// Stores must refresh `now` after waiting for database locks before classification.
    pub current_refresh_valid_until: i128,
    /// Candidate next identity; uniqueness must be enforced by persistence.
    pub next_refresh_jti: RefreshJti,
    /// Candidate next refresh expiry.
    pub next_refresh_expires_at: i64,
    /// Current client evidence. Do not substitute stored evidence before replay classification.
    pub client: ClientContext,
    /// Decision timestamp supplied by the injected clock.
    pub now: i64,
    /// Explicit product-selected stale/reuse policy.
    pub policy: RefreshPolicy,
}

/// Pure state-machine decision; products apply its effects atomically in their own transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDecision {
    /// Current JTI matches; attempt the conditional update.
    Rotate,
    /// Reject with the indicated typed fact. Reuse requires atomic product-owned revocation.
    Reject(RefreshRejection),
}

/// Authentication rejection, distinct from persistence/commit failure and never containing tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRejection {
    /// Missing, revoked, wrong-owner, or generation-invalid session.
    Unavailable,
    /// The credential deadline or authoritative server-side session expiry has passed.
    Expired,
    /// Product account is disabled or deleted.
    AccountDisabled,
    /// Product disallows refreshing this restricted login.
    Restricted,
    /// Identifiable immediate previous token matches the chosen retry policy; always rejected.
    Stale,
    /// Identifiable replay; account/session revocation has committed before returning this fact.
    ReuseDetected,
}

/// Classifies authoritative session state without accessing product entities or performing effects.
///
/// Same-client stale is allowed only for the previous JTI of this exact account/session, with a
/// nonnegative rotation age inside the explicit window. Older replays remain identifiable by `sid`;
/// foreign or absent sessions never cause another account's revocation.
#[must_use]
pub fn classify_refresh(
    request: &RefreshRotationRequest,
    session: Option<&RefreshSession>,
) -> RefreshDecision {
    let reject = RefreshDecision::Reject;
    // Database-lock waits can outlast JWT validity even after the coordinator recheck.
    if i128::from(request.now) >= request.current_refresh_valid_until {
        return reject(RefreshRejection::Expired);
    }
    let Some(session) = session else {
        return reject(RefreshRejection::Unavailable);
    };
    if session.subject != request.subject
        || session.session_id != request.session_id
        || session.revoked
    {
        return reject(RefreshRejection::Unavailable);
    }
    if request.now >= session.refresh_expires_at {
        return reject(RefreshRejection::Expired);
    }
    if session.current_refresh_jti == request.current_refresh_jti {
        return RefreshDecision::Rotate;
    }
    if let RefreshPolicy::SameClientWithin { seconds } = request.policy {
        let recent = request
            .now
            .checked_sub(session.last_seen_at)
            .and_then(|age| u64::try_from(age).ok())
            .is_some_and(|age| age <= seconds);
        let same_client = session
            .client
            .user_agent
            .as_deref()
            .is_some_and(|ua| !ua.trim().is_empty())
            && session.client.user_agent == request.client.user_agent
            && (session.client.ip_address.is_none()
                || request.client.ip_address.is_none()
                || session.client.ip_address == request.client.ip_address);
        if session.previous_refresh_jti.as_ref() == Some(&request.current_refresh_jti)
            && recent
            && same_client
        {
            return reject(RefreshRejection::Stale);
        }
    }
    reject(RefreshRejection::ReuseDetected)
}

/// Transaction result. A store must not return success or reuse until all required writes commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshStoreOutcome {
    /// Conditional rotation and its transaction committed.
    Rotated,
    /// CAS lost; leave this transaction and re-read the committed winner using `resolve_conflict`.
    Conflict,
    /// Authentication rejection. Security effects required for reuse have already committed.
    Rejected(RefreshRejection),
}

/// Product persistence boundary. Forge never opens a database transaction or maps product errors.
#[async_trait]
pub trait RefreshSessionStore: Send + Sync {
    /// Product error preserved through the coordinator, including database/audit/commit failures.
    type Error: Send;

    /// Checks current account/version/restriction state and applies a conditional JTI rotation or
    /// rejection. Reuse classification, version bump, session purge and required security audit
    /// must use the same transaction. Refresh `request.now` from the product's trusted clock after
    /// acquiring database locks, so waiting cannot extend the verified credential deadline.
    /// Authentication rejection is an `Ok(Rejected(..))`, so it
    /// cannot accidentally roll back already-required revocation.
    ///
    /// # Errors
    /// Returns the original product error if any required operation or transaction commit fails.
    async fn rotate_refresh(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshStoreOutcome, Self::Error>;

    /// Resolves a lost CAS in a fresh writer transaction after the winner becomes visible.
    /// Recheck account/version/session state, classify, and atomically revoke on reuse. This method
    /// may only reject; it must never rotate or release the losing request's candidate credentials.
    ///
    /// # Errors
    /// Returns the product's database, audit, or commit error without claiming security success.
    async fn resolve_conflict(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshRejection, Self::Error>;
}

/// Refresh failure separates mechanism failures from the product's unmodified persistence error.
pub enum RefreshError<E> {
    /// Signature, policy, resource-budget, or cancellation failure.
    Auth(AuthError),
    /// Product database/transaction/audit failure. Debug/Display do not expose its contents.
    Store(E),
}
impl<E> fmt::Debug for RefreshError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(error) => f.debug_tuple("Auth").field(error).finish(),
            Self::Store(_) => f.write_str("Store(<redacted>)"),
        }
    }
}
impl<E> fmt::Display for RefreshError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(error) => error.fmt(f),
            Self::Store(_) => f.write_str("refresh persistence operation failed"),
        }
    }
}
impl<E> std::error::Error for RefreshError<E> {}
impl<E> From<AuthError> for RefreshError<E> {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

/// Post-commit result. Only the successful branch contains usable replacement credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Candidate identity and all required writes have committed.
    Rotated(AuthTokenPair),
    /// Authentication was rejected; the typed security fact is safe to map at the product boundary.
    Rejected(RefreshRejection),
}

/// Refresh workflow combining strict JWT checks, a shared instance coordinator, and product CAS.
/// The coordinator can be reused while products rebuild signers for dynamic lifetime configuration.
pub struct RefreshTokenService<S: ?Sized> {
    signer: JwtSigner,
    verifier: JwtVerifier,
    store: Arc<S>,
    clock: Arc<dyn Clock>,
    policy: RefreshPolicy,
    coordinator: RefreshCoordinator,
}
impl<S: ?Sized> fmt::Debug for RefreshTokenService<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefreshTokenService")
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}
impl<S: RefreshSessionStore + ?Sized> RefreshTokenService<S> {
    /// Creates a workflow using the product-owned, cloneable coordinator resource.
    #[must_use]
    pub fn new(
        signer: JwtSigner,
        verifier: JwtVerifier,
        store: Arc<S>,
        clock: Arc<dyn Clock>,
        policy: RefreshPolicy,
        coordinator: RefreshCoordinator,
    ) -> Self {
        Self {
            signer,
            verifier,
            store,
            clock,
            policy,
            coordinator,
        }
    }
    /// Rotates or rejects a refresh credential. Dropping the future releases coordination resources.
    ///
    /// # Errors
    /// Returns classified mechanism failures or the original store error; a failed commit never
    /// returns credentials. If the response is lost after commit, retrying cannot mint another pair.
    pub async fn refresh(
        &self,
        token: &str,
        client: ClientContext,
    ) -> std::result::Result<RefreshOutcome, RefreshError<S::Error>> {
        self.refresh_with_cancellation(token, client, &CancellationToken::new())
            .await
    }
    /// Refreshes with cancellation while queued or before entering persistence. Once a store
    /// transaction starts, it is allowed to finish, preserving explicit commit semantics rather
    /// than returning cancellation with an unknown mutation outcome. Shutdown rejects new/queued
    /// work; products drain already-started transactions before closing the writer database.
    ///
    /// # Errors
    /// Returns cancellation/resource/auth failures or the original store/commit error.
    pub async fn refresh_with_cancellation(
        &self,
        token: &str,
        client: ClientContext,
        cancel: &CancellationToken,
    ) -> std::result::Result<RefreshOutcome, RefreshError<S::Error>> {
        let claims = self
            .verifier
            .verify(token, AuthTokenType::Refresh)?
            .into_claims();
        let current = claims.jti.ok_or(AuthError::MissingClaim)?;
        let _guard = self.coordinator.acquire(current.clone(), cancel).await?;
        // Queued credentials may expire while awaiting the previous request's transaction.
        self.verifier.verify(token, AuthTokenType::Refresh)?;
        let now = self.clock.now_unix_seconds()?;
        let candidate = self.signer.issue(
            claims.sub.clone(),
            claims.sid.clone(),
            claims.session_version,
            claims.restriction,
        )?;
        if candidate.refresh_jti == current {
            return Err(AuthError::InvalidPolicy.into());
        }
        if cancel.is_cancelled() || self.coordinator.is_closed() {
            return Err(AuthError::Cancelled.into());
        }
        let request = RefreshRotationRequest {
            subject: claims.sub,
            session_id: claims.sid,
            session_version: claims.session_version,
            restriction: claims.restriction,
            current_refresh_jti: current,
            current_refresh_valid_until: i128::from(claims.exp)
                + i128::from(self.verifier.leeway_secs()),
            next_refresh_jti: candidate.refresh_jti.clone(),
            next_refresh_expires_at: candidate.refresh_expires_at,
            client,
            now,
            policy: self.policy,
        };
        match self
            .store
            .rotate_refresh(&request)
            .await
            .map_err(RefreshError::Store)?
        {
            RefreshStoreOutcome::Rotated => Ok(RefreshOutcome::Rotated(candidate)),
            RefreshStoreOutcome::Rejected(reason) => Ok(RefreshOutcome::Rejected(reason)),
            RefreshStoreOutcome::Conflict => {
                let reason = self
                    .store
                    .resolve_conflict(&request)
                    .await
                    .map_err(RefreshError::Store)?;
                Ok(RefreshOutcome::Rejected(reason))
            }
        }
    }
}
