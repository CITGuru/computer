use computer::testing::{ScriptedEngine, ScriptedRemote};
use computer::{EngineMachine, SystemEngine};
use computer_server::runtimes::{self, Runtimes, Tuning};
use computer_server::{AppState, routes};
use std::sync::Arc;

#[tokio::test]
async fn test_a_daemon_can_be_given_its_runtimes_in_code() {
    let runtimes = Runtimes::default();

    runtimes.add(runtimes::engine(
        "docker",
        Arc::new(EngineMachine::new(Arc::new(ScriptedEngine::new()))),
    ));

    let mut hardened = runtimes::engine(
        "hardened",
        Arc::new(EngineMachine::new(Arc::new(
            SystemEngine::new("docker").before(["--context", "gpu-1"]),
        ))),
    );
    hardened.tuning = Tuning {
        memory: Some("8g".to_string()),
        isolation: Some("runsc".to_string()),
        ..Tuning::default()
    };
    runtimes.add(hardened);

    runtimes.add(runtimes::remote(
        "fleet".to_string(),
        Arc::new(ScriptedRemote::new()),
        Tuning {
            max_lifetime_secs: Some(24 * 60 * 60),
            lifetime_secs: Some(4 * 60 * 60),
            ..Tuning::default()
        },
    ));

    runtimes.prefer("hardened").expect("a name it has");

    let state = Arc::new(AppState::default().with(runtimes));
    let _ = routes::router(Arc::clone(&state));

    assert_eq!(state.runtimes.all().len(), 3);
    assert_eq!(
        state.runtimes.resolve(None).expect("a default").name,
        "hardened",
        "a daemon of somebody else's says which runtime a box lands on"
    );

    let fleet = state.runtimes.get("fleet").expect("the vendor");
    assert_eq!(
        fleet.can.max_lifetime_secs,
        Some(24 * 60 * 60),
        "what the operator knows about their account is theirs to state"
    );
    assert_eq!(fleet.lifetime(), 4 * 60 * 60);
    assert_eq!(fleet.view(0).fields["max_lifetime_secs"], 24 * 60 * 60);
}
