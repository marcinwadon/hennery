//! An agent-override variable the host inherited never reaches an adapter;
//! one the agent's own configuration sets does (plan 7b, A1). Its own test
//! binary: the variables are set in this process before any thread exists.

use hennery_host::adapter::{Adapter, AgentCommand, INHERITED_OVERRIDE_VARS};

#[test]
fn inherited_override_variables_are_dropped_and_configured_ones_pass() {
    for var in INHERITED_OVERRIDE_VARS {
        // SAFETY: the only test in this binary, before its runtime starts:
        // no other thread reads the environment.
        unsafe { std::env::set_var(var, "/inherited/elsewhere") };
    }
    unsafe { std::env::set_var("HENNERY_ADAPTER_ENV_CONTROL", "kept") };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("env.txt");
        let cmd = AgentCommand {
            program: "sh".into(),
            args: vec!["-c".into(), format!("env > {}", out.display())],
            env: vec![("CLAUDE_CODE_EXECUTABLE".into(), "/configured/claude".into())],
        };
        let (mut adapter, _io) = Adapter::spawn(&cmd, dir.path()).unwrap();
        adapter.exited().await;
        let env = std::fs::read_to_string(&out).unwrap();
        assert!(env.contains("HENNERY_ADAPTER_ENV_CONTROL=kept"), "{env}");
        assert!(env.contains("CLAUDE_CODE_EXECUTABLE=/configured/claude"), "{env}");
        assert!(!env.contains("/inherited/elsewhere"), "{env}");
    });
}
