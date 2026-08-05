use portix_lib::core::{Config, Engine};
use std::path::PathBuf;

fn fixture_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/proc")
        .join(name)
}

/// A dual-stack service should appear as ONE row with `dual_stack = true`,
/// even though `/proc/net/tcp` and `/proc/net/tcp6` both contain a listener.
#[test]
fn test_dual_stack_fixture_collapses_to_one_service() {
    let root = fixture_root("dual-stack-node");
    let mut engine = Engine::new(Some(&root), Config::default());
    let snapshot = engine.scan();

    assert_eq!(snapshot.services.len(), 1, "expected one merged service");
    let service = &snapshot.services[0];

    assert_eq!(service.socket.port, 3000);
    assert!(service.socket.dual_stack, "dual_stack must be true");
    assert!(service.socket.protocol.is_v6(), "survivor should be IPv6");
    assert_eq!(service.socket.state, portix_lib::model::TcpState::Listen);
    assert_eq!(service.socket.inode, 486312);

    let process = service.process.as_ref().expect("process must resolve");
    assert_eq!(process.pid, 48213);
    assert_eq!(process.name, "node");
    assert_eq!(process.cmdline, vec!["node", "server.js"]);
    assert!(process.user.is_some(), "user must resolve from status");

    let project = service.project.as_ref().expect("project must resolve");
    assert_eq!(project.name, "@acme/web");
    assert_eq!(
        project.package_manager,
        Some(portix_lib::model::PackageManager::Npm)
    );
    assert_eq!(project.language.as_deref(), Some("JavaScript"));
}

/// A hostile `comm` containing parentheses and spaces must not shift field
/// parsing across the full Engine pipeline.
#[test]
fn test_hostile_comm_survives_full_scan() {
    let root = fixture_root("comm-with-parens");
    let mut engine = Engine::new(Some(&root), Config::default());
    let snapshot = engine.scan();

    assert_eq!(snapshot.services.len(), 1);
    let service = &snapshot.services[0];
    let process = service.process.as_ref().expect("process must resolve");

    assert_eq!(process.pid, 12345);
    assert_eq!(process.name, "(evil) 1 2 3");
    assert_eq!(process.ppid, 12340);
    assert_eq!(process.threads, 20);
}

/// A docker-proxy row is rewritten into its destination container, and the
/// proxy's own project/framework detection is skipped: the proxy's cwd points
/// at a real project, but the row must show the container — not a project the
/// proxy never belonged to.
#[test]
fn test_docker_proxy_row_skips_project_detection() {
    let root = fixture_root("docker-proxy");
    let mut engine = Engine::new(Some(&root), Config::default());
    let snapshot = engine.scan();

    assert_eq!(snapshot.services.len(), 1);
    let service = &snapshot.services[0];

    assert!(service.docker_proxy_rewritten);
    assert!(
        service.project.is_none(),
        "proxy rows show the container, not the proxy's cwd project"
    );
    assert!(service.framework.is_none());

    let container = service.container.as_ref().expect("container must resolve");
    assert_eq!(container.runtime, portix_lib::model::Runtime::Docker);
    assert_eq!(container.id, "172.17.0.2");

    // The original host process is preserved for the UI.
    let process = service.process.as_ref().expect("process must resolve");
    assert_eq!(process.pid, 54321);
    assert_eq!(process.name, "docker-proxy");
}
