use aster_forge_cloud_files_core::{SessionGeneration, SessionState};
use aster_forge_cloud_files_macos_bridge::{MacosBridgeError, MacosExtensionSession};

fn generation(value: u64) -> SessionGeneration {
    SessionGeneration::new(value).expect("generation should be valid")
}

#[test]
fn session_rejects_stale_generation_and_closing_ingress_then_drains_leases() {
    let session = MacosExtensionSession::new(generation(7));
    assert!(matches!(
        session.begin_request(generation(6)),
        Err(MacosBridgeError::StaleSessionGeneration {
            expected: 7,
            actual: 6
        })
    ));
    let first = session
        .begin_request(generation(7))
        .expect("request should be accepted");
    let second = session
        .begin_request(generation(7))
        .expect("request should be accepted");
    assert_eq!(session.generation(), generation(7));
    assert_eq!(first.generation(), generation(7));
    let lease_debug = format!("{first:?}");
    assert!(lease_debug.contains("generation"));
    assert!(lease_debug.contains("released: false"));
    assert_eq!(session.active_requests(), 2);
    assert!(first.accepts_completion(&session));
    assert!(session.begin_closing());
    assert!(!session.begin_closing());
    assert!(matches!(
        session.begin_request(generation(7)),
        Err(MacosBridgeError::SessionNotAccepting {
            state: SessionState::Closing
        })
    ));
    session
        .mark_disconnected()
        .expect("closing session should disconnect");
    assert_eq!(session.state(), SessionState::Draining);
    session
        .mark_disconnected()
        .expect("draining disconnect should be idempotent");
    assert!(first.accepts_completion(&session));
    drop(first);
    assert_eq!(session.state(), SessionState::Draining);
    second.release();
    assert_eq!(session.state(), SessionState::Closed);
    assert_eq!(session.active_requests(), 0);
}

#[test]
fn disconnect_requires_closing_and_empty_session_closes_immediately() {
    let session = MacosExtensionSession::new(generation(1));
    assert!(matches!(
        session.mark_disconnected(),
        Err(MacosBridgeError::InvalidSessionTransition {
            from: SessionState::Accepting,
            to: SessionState::Draining
        })
    ));
    assert!(session.begin_closing());
    session
        .mark_disconnected()
        .expect("empty session should close");
    assert_eq!(session.state(), SessionState::Closed);
    session
        .mark_disconnected()
        .expect("closed transition should be idempotent");
}

#[test]
fn lease_completion_is_bound_to_exact_session_instance() {
    let session = MacosExtensionSession::new(generation(3));
    let other = MacosExtensionSession::new(generation(3));
    let lease = session
        .begin_request(generation(3))
        .expect("request should be accepted");
    assert!(lease.accepts_completion(&session));
    assert!(!lease.accepts_completion(&other));
}
