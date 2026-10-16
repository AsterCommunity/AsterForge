//! Deterministic refresh concurrency, transaction, rejection and cancellation contracts.
use aster_forge_auth::*;
use async_trait::async_trait;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Barrier, Mutex, Notify};
use tokio_util::sync::CancellationToken;

mod support;

const NOW: i64 = 1_700_000_000;
const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";
struct TestClock(AtomicI64);
impl Clock for TestClock {
    fn now_unix_seconds(&self) -> Result<i64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct Sequence(AtomicU64);
impl IdSource for Sequence {
    fn next_id(&self) -> Result<RefreshJti> {
        RefreshJti::new(format!("id-{}", self.0.fetch_add(1, Ordering::SeqCst)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct State {
    session: Option<RefreshSession>,
    version: u64,
    audits: usize,
    disabled: bool,
    restricted: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    Begin,
    Cas,
    Revoke,
    Audit,
    Commit,
    ConflictRead,
}
struct Store {
    state: Mutex<State>,
    failure: Option<Failure>,
    gate: Option<Arc<Gate>>,
    barrier: Option<Arc<Barrier>>,
}
struct Gate {
    entered: Notify,
    release: Notify,
    first: AtomicBool,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            release: Notify::new(),
            first: AtomicBool::new(true),
        })
    }
}
impl Store {
    async fn apply(
        &self,
        request: &RefreshRotationRequest,
        rotate: bool,
    ) -> std::result::Result<RefreshStoreOutcome, Failure> {
        if self.failure == Some(Failure::Begin) {
            return Err(Failure::Begin);
        }
        let mut original = self.state.lock().await;
        let mut next = original.clone();
        let decision = if request.session_version.value() != next.version {
            RefreshDecision::Reject(RefreshRejection::Unavailable)
        } else if next.disabled {
            RefreshDecision::Reject(RefreshRejection::AccountDisabled)
        } else if next.restricted || request.restriction != Restriction::None {
            RefreshDecision::Reject(RefreshRejection::Restricted)
        } else {
            classify_refresh(request, next.session.as_ref())
        };
        let result = match decision {
            RefreshDecision::Rotate if rotate => {
                if self.failure == Some(Failure::Cas) {
                    return Ok(RefreshStoreOutcome::Conflict);
                }
                let session = next.session.as_mut().unwrap();
                session.previous_refresh_jti = Some(session.current_refresh_jti.clone());
                session.current_refresh_jti = request.next_refresh_jti.clone();
                session.refresh_expires_at = request.next_refresh_expires_at;
                session.last_seen_at = request.now;
                session.client = request.client.clone();
                RefreshStoreOutcome::Rotated
            }
            RefreshDecision::Rotate => return Err(Failure::ConflictRead),
            RefreshDecision::Reject(reason) => {
                if reason == RefreshRejection::ReuseDetected {
                    if self.failure == Some(Failure::Revoke) {
                        return Err(Failure::Revoke);
                    }
                    next.version += 1;
                    next.session = None;
                    if self.failure == Some(Failure::Audit) {
                        return Err(Failure::Audit);
                    }
                    next.audits += 1;
                }
                RefreshStoreOutcome::Rejected(reason)
            }
        };
        if self.failure == Some(Failure::Commit) {
            return Err(Failure::Commit);
        }
        *original = next;
        Ok(result)
    }
}
#[async_trait]
impl RefreshSessionStore for Store {
    type Error = Failure;
    async fn rotate_refresh(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshStoreOutcome, Failure> {
        if let Some(gate) = &self.gate
            && gate.first.swap(false, Ordering::SeqCst)
        {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        if let Some(barrier) = &self.barrier {
            barrier.wait().await;
        }
        self.apply(request, true).await
    }
    async fn resolve_conflict(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshRejection, Failure> {
        match self.apply(request, false).await? {
            RefreshStoreOutcome::Rejected(reason) => Ok(reason),
            _ => Err(Failure::ConflictRead),
        }
    }
}

struct Fixture {
    store: Arc<Store>,
    signer: JwtSigner,
    verifier: JwtVerifier,
    clock: Arc<TestClock>,
    pair: AuthTokenPair,
}
impl Fixture {
    fn new(
        failure: Option<Failure>,
        gate: Option<Arc<Gate>>,
        barrier: Option<Arc<Barrier>>,
    ) -> Self {
        support::initialize_provider();
        let clock = Arc::new(TestClock(AtomicI64::new(NOW)));
        let policy = JwtPolicy::new("issuer", "web", 60, 600).unwrap();
        let signer = JwtSigner::new(
            JwtKey::hmac(KEY).unwrap(),
            policy.clone(),
            clock.clone(),
            Arc::new(Sequence(AtomicU64::new(0))),
        );
        let verifier = JwtVerifier::new(JwtKey::hmac(KEY).unwrap(), policy, clock.clone());
        let pair = signer
            .issue(
                Subject::new("42").unwrap(),
                SessionId::new("session-1").unwrap(),
                SessionVersion::new(7),
                Restriction::None,
            )
            .unwrap();
        let snapshot = RefreshSession {
            subject: Subject::new("42").unwrap(),
            session_id: SessionId::new("session-1").unwrap(),
            current_refresh_jti: pair.refresh_jti.clone(),
            previous_refresh_jti: None,
            last_seen_at: NOW,
            refresh_expires_at: NOW + 600,
            revoked: false,
            client: client(),
        };
        let store = Arc::new(Store {
            state: Mutex::new(State {
                session: Some(snapshot),
                version: 7,
                audits: 0,
                disabled: false,
                restricted: false,
            }),
            failure,
            gate,
            barrier,
        });
        Self {
            store,
            signer,
            verifier,
            clock,
            pair,
        }
    }
    fn service(
        &self,
        coordinator: RefreshCoordinator,
        policy: RefreshPolicy,
    ) -> Arc<RefreshTokenService<Store>> {
        Arc::new(RefreshTokenService::new(
            self.signer.clone(),
            self.verifier.clone(),
            self.store.clone(),
            self.clock.clone(),
            policy,
            coordinator,
        ))
    }
}
fn client() -> ClientContext {
    ClientContext {
        ip_address: Some("127.0.0.1".into()),
        user_agent: Some("browser".into()),
    }
}
fn retry() -> RefreshPolicy {
    RefreshPolicy::SameClientWithin { seconds: 15 }
}
fn spawn(
    service: Arc<RefreshTokenService<Store>>,
    token: String,
) -> tokio::task::JoinHandle<std::result::Result<RefreshOutcome, RefreshError<Failure>>> {
    tokio::spawn(async move { service.refresh(&token, client()).await })
}
async fn wait_requests(coordinator: &RefreshCoordinator, count: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while coordinator.active_requests() != count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("requests should reach observed coordinator state");
}

#[tokio::test]
async fn successful_refresh_preserves_ttls_and_commits_one_new_identity() {
    let f = Fixture::new(None, None, None);
    let coordinator = RefreshCoordinator::default();
    let service = f.service(coordinator.clone(), retry());
    let RefreshOutcome::Rotated(pair) = service
        .refresh(&f.pair.refresh_token, client())
        .await
        .unwrap()
    else {
        panic!("expected rotation")
    };
    assert_eq!(pair.access_expires_at, NOW + 60);
    assert_eq!(pair.refresh_expires_at, NOW + 600);
    let claims = f
        .verifier
        .verify(&pair.refresh_token, AuthTokenType::Refresh)
        .unwrap();
    assert_eq!(claims.claims().sid.as_str(), "session-1");
    assert_eq!(
        f.store
            .state
            .lock()
            .await
            .session
            .as_ref()
            .unwrap()
            .current_refresh_jti,
        pair.refresh_jti
    );
    assert_eq!(
        service
            .refresh(&f.pair.refresh_token, client())
            .await
            .unwrap(),
        RefreshOutcome::Rejected(RefreshRejection::Stale)
    );
    assert_eq!(f.store.state.lock().await.version, 7);
    assert_eq!(coordinator.active_requests(), 0);
    assert_eq!(coordinator.tracked_ids(), 0);
}

#[tokio::test]
async fn shared_coordinator_serializes_real_same_jti_contention_without_sleep() {
    let gate = Gate::new();
    let f = Fixture::new(None, Some(gate.clone()), None);
    let coordinator = RefreshCoordinator::new(2).unwrap();
    let service = f.service(coordinator.clone(), retry());
    let first = spawn(service.clone(), f.pair.refresh_token.clone());
    gate.entered.notified().await;
    // A second workflow uses the same instance resource, as Drive does after TTL reload.
    let second = spawn(
        f.service(coordinator.clone(), retry()),
        f.pair.refresh_token.clone(),
    );
    wait_requests(&coordinator, 2).await;
    assert_eq!(coordinator.tracked_ids(), 1);
    gate.release.notify_one();
    assert!(matches!(
        first.await.unwrap().unwrap(),
        RefreshOutcome::Rotated(_)
    ));
    assert_eq!(
        second.await.unwrap().unwrap(),
        RefreshOutcome::Rejected(RefreshRejection::Stale)
    );
    assert_eq!(coordinator.active_requests(), 0);
    assert_eq!(coordinator.tracked_ids(), 0);
}

#[tokio::test]
async fn separate_instances_and_restart_still_have_one_database_winner() {
    let f = Fixture::new(None, None, Some(Arc::new(Barrier::new(2))));
    let one = spawn(
        f.service(RefreshCoordinator::default(), retry()),
        f.pair.refresh_token.clone(),
    );
    let two = spawn(
        f.service(RefreshCoordinator::default(), retry()),
        f.pair.refresh_token.clone(),
    );
    let one = one.await.unwrap().unwrap();
    let two = two.await.unwrap().unwrap();
    assert_eq!(
        usize::from(matches!(one, RefreshOutcome::Rotated(_)))
            + usize::from(matches!(two, RefreshOutcome::Rotated(_))),
        1
    );
    assert_eq!(
        usize::from(one == RefreshOutcome::Rejected(RefreshRejection::Stale))
            + usize::from(two == RefreshOutcome::Rejected(RefreshRejection::Stale)),
        1
    );
    assert_eq!(f.store.state.lock().await.audits, 0);
}

#[tokio::test]
async fn old_replay_after_multiple_rotations_and_restart_commits_revoke_and_audit_once() {
    let f = Fixture::new(None, None, None);
    let service = f.service(RefreshCoordinator::default(), retry());
    let RefreshOutcome::Rotated(next) = service
        .refresh(&f.pair.refresh_token, client())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        service
            .refresh(&next.refresh_token, client())
            .await
            .unwrap(),
        RefreshOutcome::Rotated(_)
    ));
    let restarted = f.service(RefreshCoordinator::default(), retry());
    assert_eq!(
        restarted
            .refresh(&f.pair.refresh_token, client())
            .await
            .unwrap(),
        RefreshOutcome::Rejected(RefreshRejection::ReuseDetected)
    );
    let state = f.store.state.lock().await;
    assert_eq!(state.version, 8);
    assert_eq!(state.audits, 1);
    assert!(state.session.is_none());
    drop(state);
    assert_eq!(
        restarted
            .refresh(&f.pair.refresh_token, client())
            .await
            .unwrap(),
        RefreshOutcome::Rejected(RefreshRejection::Unavailable)
    );
    assert_eq!(f.store.state.lock().await.audits, 1);
}

#[tokio::test]
async fn stale_retry_policy_is_explicit_and_never_authorizes_second_issuance() {
    for evidence in [
        ClientContext::default(),
        ClientContext {
            user_agent: Some("different".into()),
            ..client()
        },
        ClientContext {
            ip_address: Some("other".into()),
            ..client()
        },
    ] {
        let f = Fixture::new(None, None, None);
        let service = f.service(RefreshCoordinator::default(), retry());
        service
            .refresh(&f.pair.refresh_token, client())
            .await
            .unwrap();
        assert_eq!(
            service
                .refresh(&f.pair.refresh_token, evidence)
                .await
                .unwrap(),
            RefreshOutcome::Rejected(RefreshRejection::ReuseDetected)
        );
        assert_eq!(f.store.state.lock().await.version, 8);
    }
    let f = Fixture::new(None, None, None);
    let strict = f.service(RefreshCoordinator::default(), RefreshPolicy::Strict);
    strict
        .refresh(&f.pair.refresh_token, client())
        .await
        .unwrap();
    assert_eq!(
        strict
            .refresh(&f.pair.refresh_token, client())
            .await
            .unwrap(),
        RefreshOutcome::Rejected(RefreshRejection::ReuseDetected)
    );
}

#[tokio::test]
async fn revoked_disabled_version_invalid_and_restricted_requests_never_get_credentials() {
    for case in 0..6 {
        let f = Fixture::new(None, None, None);
        let mut state = f.store.state.lock().await;
        let expected = match case {
            0 => {
                state.session = None;
                RefreshRejection::Unavailable
            }
            1 => {
                state.session.as_mut().unwrap().revoked = true;
                RefreshRejection::Unavailable
            }
            2 => {
                state.disabled = true;
                RefreshRejection::AccountDisabled
            }
            3 => {
                state.version += 1;
                RefreshRejection::Unavailable
            }
            4 => {
                state.restricted = true;
                RefreshRejection::Restricted
            }
            _ => {
                state.session.as_mut().unwrap().refresh_expires_at = NOW;
                RefreshRejection::Expired
            }
        };
        drop(state);
        assert_eq!(
            f.service(RefreshCoordinator::default(), retry())
                .refresh(&f.pair.refresh_token, client())
                .await
                .unwrap(),
            RefreshOutcome::Rejected(expected)
        );
        assert_eq!(f.store.state.lock().await.audits, 0);
    }
}

#[tokio::test]
async fn begin_commit_and_conflict_read_errors_preserve_the_original_product_error() {
    for failure in [Failure::Begin, Failure::Commit, Failure::Cas] {
        let f = Fixture::new(Some(failure), None, None);
        let before = f.store.state.lock().await.clone();
        let result = f
            .service(RefreshCoordinator::default(), retry())
            .refresh(&f.pair.refresh_token, client())
            .await;
        let expected = if failure == Failure::Cas {
            Failure::ConflictRead
        } else {
            failure
        };
        assert!(matches!(result, Err(RefreshError::Store(error)) if error == expected));
        assert_eq!(*f.store.state.lock().await, before);
    }
}

/// Models another process committing between the losing transaction's read and its CAS.
struct CommittedWinner {
    store: Arc<Store>,
    rotations: AtomicU64,
    resolutions: AtomicU64,
}
#[async_trait]
impl RefreshSessionStore for CommittedWinner {
    type Error = Failure;

    async fn rotate_refresh(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshStoreOutcome, Failure> {
        self.rotations.fetch_add(1, Ordering::SeqCst);
        let mut winner = request.clone();
        winner.next_refresh_jti = RefreshJti::new("external-winner").unwrap();
        assert_eq!(
            self.store.apply(&winner, true).await?,
            RefreshStoreOutcome::Rotated
        );
        Ok(RefreshStoreOutcome::Conflict)
    }

    async fn resolve_conflict(
        &self,
        request: &RefreshRotationRequest,
    ) -> std::result::Result<RefreshRejection, Failure> {
        self.resolutions.fetch_add(1, Ordering::SeqCst);
        self.store.resolve_conflict(request).await
    }
}

#[tokio::test]
async fn lost_cas_rereads_committed_winner_and_only_rejects_without_reissuing() {
    for (policy, expected) in [
        (retry(), RefreshRejection::Stale),
        (RefreshPolicy::Strict, RefreshRejection::ReuseDetected),
    ] {
        let f = Fixture::new(None, None, None);
        let store = Arc::new(CommittedWinner {
            store: f.store.clone(),
            rotations: AtomicU64::new(0),
            resolutions: AtomicU64::new(0),
        });
        let coordinator = RefreshCoordinator::default();
        let service = RefreshTokenService::new(
            f.signer.clone(),
            f.verifier.clone(),
            store.clone(),
            f.clock.clone(),
            policy,
            coordinator.clone(),
        );
        assert_eq!(
            service
                .refresh(&f.pair.refresh_token, client())
                .await
                .unwrap(),
            RefreshOutcome::Rejected(expected)
        );
        assert_eq!(store.rotations.load(Ordering::SeqCst), 1);
        assert_eq!(store.resolutions.load(Ordering::SeqCst), 1);
        let state = f.store.state.lock().await;
        if expected == RefreshRejection::Stale {
            assert_eq!(
                state.session.as_ref().unwrap().current_refresh_jti.as_str(),
                "external-winner"
            );
            assert_eq!((state.version, state.audits), (7, 0));
        } else {
            assert!(state.session.is_none());
            assert_eq!((state.version, state.audits), (8, 1));
        }
        assert_eq!(coordinator.active_requests(), 0);
        assert_eq!(coordinator.tracked_ids(), 0);
    }
}

#[tokio::test]
async fn revoke_audit_and_commit_failures_roll_back_required_security_effects() {
    for failure in [Failure::Revoke, Failure::Audit, Failure::Commit] {
        let f = Fixture::new(Some(failure), None, None);
        {
            let mut state = f.store.state.lock().await;
            state.session.as_mut().unwrap().current_refresh_jti =
                RefreshJti::new("rotated-before-restart").unwrap();
        }
        let before = f.store.state.lock().await.clone();
        let result = f
            .service(RefreshCoordinator::default(), RefreshPolicy::Strict)
            .refresh(&f.pair.refresh_token, client())
            .await;
        assert!(matches!(result, Err(RefreshError::Store(error)) if error == failure));
        assert_eq!(*f.store.state.lock().await, before);
    }
}

#[tokio::test]
async fn cancellation_while_waiting_and_shutdown_release_locks_and_budget() {
    for close in [false, true] {
        let gate = Gate::new();
        let f = Fixture::new(None, Some(gate.clone()), None);
        let coordinator = RefreshCoordinator::new(2).unwrap();
        let service = f.service(coordinator.clone(), retry());
        let first = spawn(service.clone(), f.pair.refresh_token.clone());
        gate.entered.notified().await;
        let cancel = CancellationToken::new();
        let waiter_cancel = cancel.clone();
        let token = f.pair.refresh_token.clone();
        let second = tokio::spawn(async move {
            service
                .refresh_with_cancellation(&token, client(), &waiter_cancel)
                .await
        });
        wait_requests(&coordinator, 2).await;
        if close {
            coordinator.close();
        } else {
            cancel.cancel();
        }
        assert!(matches!(
            second.await.unwrap(),
            Err(RefreshError::Auth(AuthError::Cancelled))
        ));
        gate.release.notify_one();
        assert!(matches!(
            first.await.unwrap().unwrap(),
            RefreshOutcome::Rotated(_)
        ));
        assert_eq!(coordinator.tracked_ids(), 0);
        assert_eq!(coordinator.active_requests(), 0);
    }
}

#[tokio::test]
async fn abandoned_waiter_and_store_future_are_reclaimed_and_uncommitted_state_is_unchanged() {
    let gate = Gate::new();
    let f = Fixture::new(None, Some(gate.clone()), None);
    let coordinator = RefreshCoordinator::new(2).unwrap();
    let service = f.service(coordinator.clone(), retry());
    let before = f.store.state.lock().await.clone();
    let first = spawn(service.clone(), f.pair.refresh_token.clone());
    gate.entered.notified().await;
    let second = spawn(service, f.pair.refresh_token.clone());
    wait_requests(&coordinator, 2).await;
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(coordinator.tracked_ids(), 0);
    assert_eq!(coordinator.active_requests(), 0);
    assert_eq!(*f.store.state.lock().await, before);
}

#[tokio::test]
async fn cancellation_after_store_entry_preserves_the_committed_outcome() {
    let gate = Gate::new();
    let f = Fixture::new(None, Some(gate.clone()), None);
    let coordinator = RefreshCoordinator::default();
    let service = f.service(coordinator.clone(), retry());
    let cancel = CancellationToken::new();
    let request_cancel = cancel.clone();
    let token = f.pair.refresh_token.clone();
    let task = tokio::spawn(async move {
        service
            .refresh_with_cancellation(&token, client(), &request_cancel)
            .await
    });
    gate.entered.notified().await;
    cancel.cancel();
    gate.release.notify_one();
    let RefreshOutcome::Rotated(pair) = task.await.unwrap().unwrap() else {
        panic!("an entered store transaction must finish with its actual commit outcome");
    };
    assert_eq!(
        f.store
            .state
            .lock()
            .await
            .session
            .as_ref()
            .unwrap()
            .current_refresh_jti,
        pair.refresh_jti
    );
    assert_eq!(coordinator.tracked_ids(), 0);
    assert_eq!(coordinator.active_requests(), 0);
}

#[tokio::test]
async fn resource_exhaustion_is_prompt_and_invalid_tokens_never_enter_the_store() {
    assert!(RefreshCoordinator::new(0).is_err());
    assert!(RefreshCoordinator::new(usize::MAX).is_err());
    let gate = Gate::new();
    let f = Fixture::new(None, Some(gate.clone()), None);
    let coordinator = RefreshCoordinator::new(1).unwrap();
    let service = f.service(coordinator.clone(), retry());
    let first = spawn(service.clone(), f.pair.refresh_token.clone());
    gate.entered.notified().await;
    assert!(matches!(
        service.refresh(&f.pair.refresh_token, client()).await,
        Err(RefreshError::Auth(AuthError::ResourceLimit))
    ));
    assert!(matches!(
        service.refresh(&f.pair.access_token, client()).await,
        Err(RefreshError::Auth(AuthError::WrongTokenType))
    ));
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(coordinator.tracked_ids(), 0);
}

#[tokio::test]
async fn queued_token_is_revalidated_when_expired_during_the_first_transaction() {
    let gate = Gate::new();
    let f = Fixture::new(None, Some(gate.clone()), None);
    let coordinator = RefreshCoordinator::new(2).unwrap();
    let service = f.service(coordinator.clone(), retry());
    let first = spawn(service.clone(), f.pair.refresh_token.clone());
    gate.entered.notified().await;
    let second = spawn(service, f.pair.refresh_token.clone());
    wait_requests(&coordinator, 2).await;
    f.clock.0.store(NOW + 600, Ordering::SeqCst);
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    assert!(matches!(
        second.await.unwrap(),
        Err(RefreshError::Auth(AuthError::Expired))
    ));
    assert_eq!(coordinator.tracked_ids(), 0);
}

#[tokio::test]
async fn pure_classifier_checks_window_owner_session_previous_jti_and_spoofable_evidence() {
    let f = Fixture::new(None, None, None);
    let mut session = f.store.state.lock().await.session.clone().unwrap();
    let request = RefreshRotationRequest {
        subject: session.subject.clone(),
        session_id: session.session_id.clone(),
        session_version: SessionVersion::new(7),
        restriction: Restriction::None,
        current_refresh_jti: f.pair.refresh_jti.clone(),
        current_refresh_valid_until: i128::from(NOW + 600),
        next_refresh_jti: RefreshJti::new("next").unwrap(),
        next_refresh_expires_at: NOW + 600,
        client: client(),
        now: NOW,
        policy: retry(),
    };
    session.previous_refresh_jti = Some(session.current_refresh_jti.clone());
    session.current_refresh_jti = RefreshJti::new("new").unwrap();
    for (age, expected) in [
        (-1, RefreshRejection::ReuseDetected),
        (0, RefreshRejection::Stale),
        (15, RefreshRejection::Stale),
        (16, RefreshRejection::ReuseDetected),
    ] {
        let request = RefreshRotationRequest {
            now: NOW + age,
            ..request.clone()
        };
        assert_eq!(
            classify_refresh(&request, Some(&session)),
            RefreshDecision::Reject(expected)
        );
    }
    let foreign = RefreshRotationRequest {
        subject: Subject::new("other").unwrap(),
        ..request.clone()
    };
    assert_eq!(
        classify_refresh(&foreign, Some(&session)),
        RefreshDecision::Reject(RefreshRejection::Unavailable)
    );
    let foreign = RefreshRotationRequest {
        session_id: SessionId::new("other").unwrap(),
        ..request.clone()
    };
    assert_eq!(
        classify_refresh(&foreign, Some(&session)),
        RefreshDecision::Reject(RefreshRejection::Unavailable)
    );
    // Spoofed UA/IP can match; it only changes rejection severity, never authorizes rotation.
    assert_eq!(
        classify_refresh(&request, Some(&session)),
        RefreshDecision::Reject(RefreshRejection::Stale)
    );
    for (ip, ua, expected) in [
        (None, Some("browser"), RefreshRejection::Stale),
        (
            Some("127.0.0.2"),
            Some("browser"),
            RefreshRejection::ReuseDetected,
        ),
        (
            Some("127.0.0.1"),
            Some("other-browser"),
            RefreshRejection::ReuseDetected,
        ),
        (Some("127.0.0.1"), None, RefreshRejection::ReuseDetected),
        (None, None, RefreshRejection::ReuseDetected),
    ] {
        let request = RefreshRotationRequest {
            client: ClientContext {
                ip_address: ip.map(str::to_string),
                user_agent: ua.map(str::to_string),
            },
            ..request.clone()
        };
        assert_eq!(
            classify_refresh(&request, Some(&session)),
            RefreshDecision::Reject(expected)
        );
    }
    session.client.user_agent = Some(String::new());
    assert_eq!(
        classify_refresh(&request, Some(&session)),
        RefreshDecision::Reject(RefreshRejection::ReuseDetected)
    );
}

#[tokio::test]
async fn database_lock_wait_cannot_extend_presented_credential_validity() {
    let f = Fixture::new(None, None, None);
    let session = f.store.state.lock().await.session.clone().unwrap();
    // Persistence can live longer than this credential (for example after another rotation).
    let request = RefreshRotationRequest {
        subject: session.subject.clone(),
        session_id: session.session_id.clone(),
        session_version: SessionVersion::new(7),
        restriction: Restriction::None,
        current_refresh_jti: session.current_refresh_jti.clone(),
        current_refresh_valid_until: i128::from(NOW + 1),
        next_refresh_jti: RefreshJti::new("candidate").unwrap(),
        next_refresh_expires_at: NOW + 601,
        client: client(),
        now: NOW,
        policy: retry(),
    };
    assert_eq!(
        classify_refresh(&request, Some(&session)),
        RefreshDecision::Rotate
    );
    for now in [NOW + 1, NOW + 2] {
        let delayed = RefreshRotationRequest {
            now,
            ..request.clone()
        };
        assert_eq!(
            classify_refresh(&delayed, Some(&session)),
            RefreshDecision::Reject(RefreshRejection::Expired)
        );
    }
    // A verifier's accepted skew is part of the deadline, without narrowing or wrapping it.
    let skewed = RefreshRotationRequest {
        now: NOW + 1,
        current_refresh_valid_until: i128::from(NOW + 2),
        ..request
    };
    assert_eq!(
        classify_refresh(&skewed, Some(&session)),
        RefreshDecision::Rotate
    );
}
