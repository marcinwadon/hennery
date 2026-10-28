// Browser-only harness: real host code, scripted ACP, explicit known and
// generic profiles. No production command can select a fake trusted profile.
use hennery_host::{AgentCommand, HostConfig, identity::Paired, profile::Profile};
use std::path::PathBuf;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let data = PathBuf::from(&args[1]);
    let paired = Paired::load(&data).unwrap().unwrap();
    let mut cfg = HostConfig::new(paired.collector_url, paired.host_id, paired.key, data);
    cfg.trust = paired.trust;
    cfg.home = Some(PathBuf::from(&args[2]));
    cfg.workspace_roots = vec![PathBuf::from(&args[2])];
    cfg.git = None;
    for (name, profile) in [
        ("claude", Profile::Claude),
        ("codex", Profile::Codex),
        ("custom", Profile::Generic),
    ] {
        cfg.agents.insert(
            name.into(),
            AgentCommand {
                program: args[3].clone(),
                args: vec![],
                env: vec![("HENNERY_FAKE_ACP_SCRIPT".into(), args[4].clone())],
            },
        );
        cfg.profiles.insert(name.into(), profile);
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(hennery_host::run_until(cfg, async {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
        }))
        .unwrap();
}
