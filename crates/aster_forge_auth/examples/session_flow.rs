//! Executable product-port fixture for issuance, authentication, refresh, replay and shutdown.
//! This in-memory store demonstrates atomic results, not a production persistence schema.
use aster_forge_auth::{
    AuthError, AuthTokenType, ClientContext, Clock, JwtKey, JwtPolicy, JwtSigner, JwtVerifier,
    RandomIdSource, RefreshCoordinator, RefreshDecision, RefreshOutcome, RefreshPolicy,
    RefreshRejection, RefreshRotationRequest, RefreshSession, RefreshSessionStore,
    RefreshStoreOutcome, RefreshTokenService, Restriction, Result, SessionId, SessionVersion,
    Subject, SystemClock, classify_refresh,
};
use async_trait::async_trait;
use std::{
    error::Error,
    sync::{Arc, Mutex},
};

struct Store {
    state: Mutex<(SessionVersion, Option<RefreshSession>)>,
}
impl Store {
    fn transition(
        &self,
        request: &RefreshRotationRequest,
        rotate: bool,
    ) -> Result<RefreshStoreOutcome> {
        let mut state = self.state.lock().map_err(|_| AuthError::InvalidPolicy)?;
        if state.0 != request.session_version {
            return Ok(RefreshStoreOutcome::Rejected(RefreshRejection::Unavailable));
        }
        match classify_refresh(request, state.1.as_ref()) {
            RefreshDecision::Rotate if rotate => {
                if let Some(session) = &mut state.1 {
                    session.previous_refresh_jti = Some(session.current_refresh_jti.clone());
                    session.current_refresh_jti = request.next_refresh_jti.clone();
                    session.refresh_expires_at = request.next_refresh_expires_at;
                    session.last_seen_at = request.now;
                    session.client = request.client.clone();
                }
                Ok(RefreshStoreOutcome::Rotated)
            }
            RefreshDecision::Rotate => Err(AuthError::InvalidPolicy),
            RefreshDecision::Reject(reason) => {
                if reason == RefreshRejection::ReuseDetected {
                    state.0 = state.0.next()?;
                    state.1 = None;
                }
                Ok(RefreshStoreOutcome::Rejected(reason))
            }
        }
    }
}
#[async_trait]
impl RefreshSessionStore for Store {
    type Error = AuthError;
    async fn rotate_refresh(
        &self,
        request: &RefreshRotationRequest,
    ) -> Result<RefreshStoreOutcome> {
        self.transition(request, true)
    }
    async fn resolve_conflict(&self, request: &RefreshRotationRequest) -> Result<RefreshRejection> {
        match self.transition(request, false)? {
            RefreshStoreOutcome::Rejected(reason) => Ok(reason),
            _ => Err(AuthError::InvalidPolicy),
        }
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn Error>> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let policy = JwtPolicy::new("example-product", "local-web", 900, 604_800)?;
    let key = JwtKey::hmac([42; 32])?; // Fixture only; products load independent high-entropy keys.
    let signer = JwtSigner::new(
        key.clone(),
        policy.clone(),
        clock.clone(),
        Arc::new(RandomIdSource),
    );
    let verifier = JwtVerifier::new(key, policy, clock.clone());
    let subject = Subject::new("42")?;
    let sid = SessionId::new("session-1")?;
    let version = SessionVersion::new(1);
    let pair = signer.issue(subject.clone(), sid.clone(), version, Restriction::None)?;
    let store = Arc::new(Store {
        state: Mutex::new((
            version,
            Some(RefreshSession {
                subject,
                session_id: sid,
                current_refresh_jti: pair.refresh_jti.clone(),
                previous_refresh_jti: None,
                last_seen_at: clock.now_unix_seconds()?,
                refresh_expires_at: pair.refresh_expires_at,
                revoked: false,
                client: ClientContext::default(),
            }),
        )),
    });
    // Credentials are publishable only after the product's session creation has committed.
    let authenticated = verifier.verify(&pair.access_token, AuthTokenType::Access)?;
    {
        let state = store.state.lock().map_err(|_| AuthError::InvalidPolicy)?;
        assert_eq!(authenticated.claims().session_version, state.0);
    }
    let coordinator = RefreshCoordinator::default();
    let service = RefreshTokenService::new(
        signer,
        verifier,
        store.clone(),
        clock,
        RefreshPolicy::Strict,
        coordinator.clone(),
    );
    assert!(matches!(
        service
            .refresh(&pair.refresh_token, ClientContext::default())
            .await?,
        RefreshOutcome::Rotated(_)
    ));
    assert_eq!(
        service
            .refresh(&pair.refresh_token, ClientContext::default())
            .await?,
        RefreshOutcome::Rejected(RefreshRejection::ReuseDetected)
    );
    assert!(
        store
            .state
            .lock()
            .map_err(|_| AuthError::InvalidPolicy)?
            .1
            .is_none()
    );
    coordinator.close();
    assert_eq!(coordinator.active_requests(), 0);
    Ok(())
}
