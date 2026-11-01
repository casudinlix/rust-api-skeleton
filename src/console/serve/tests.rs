use std::{fs, process::Stdio};

use nix::sys::signal::kill;
use tokio::{
    sync::mpsc,
    time::{Instant, sleep, timeout},
};

use super::*;

impl SignalSource for mpsc::UnboundedReceiver<StopSignal> {
    async fn recv(&mut self) -> StopSignal {
        self.recv().await.unwrap_or(StopSignal::Terminate)
    }
}

fn fixture(root: &Path, name: &str, script: &str) -> (String, Command) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script, "artisan-test"])
        .arg(root)
        .arg(name)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    (name.to_owned(), command)
}

const COOPERATIVE: &str = r#"
trap 'printf INT > "$1/$2.stopped"; exit 0' INT
trap 'printf TERM > "$1/$2.stopped"; exit 0' TERM
printf '%s' "$$" > "$1/$2.pid"
while :; do sleep 1; done
"#;

async fn wait_for(path: &Path) {
    timeout(Duration::from_secs(5), async {
        loop {
            if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0) {
                return;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture never wrote {}", path.display()));
}

fn fixture_pid(root: &Path, name: &str) -> Pid {
    Pid::from_raw(
        fs::read_to_string(root.join(format!("{name}.pid")))
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    )
}

fn is_running(pid: Pid) -> bool {
    if kill(pid, None::<Signal>) == Err(Errno::ESRCH) {
        return false;
    }
    // A killed grandchild is reparented for reaping by the host init process.
    // Some CI containers do not reap promptly; a zombie cannot execute work.
    #[cfg(target_os = "linux")]
    if matches!(linux_process_state(pid), Some('Z' | 'X')) {
        return false;
    }
    true
}

#[cfg(target_os = "linux")]
fn linux_process_state(pid: Pid) -> Option<char> {
    fn state(status: &str) -> Option<char> {
        status
            .lines()
            .find_map(|line| line.strip_prefix("State:"))?
            .trim_start()
            .chars()
            .next()
    }
    let self_pid = fs::read_link("/proc/self").ok()?;
    if self_pid.to_string_lossy() == std::process::id().to_string() {
        return state(&fs::read_to_string(format!("/proc/{pid}/status")).ok()?);
    }
    // Sandboxes can mount a host /proc inside a nested PID namespace. Match the
    // namespace and NSpid instead of confusing the child's ID with a host PID.
    let namespace = fs::read_link("/proc/self/ns/pid").ok()?;
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        let path = entry.path();
        let Ok(status) = fs::read_to_string(path.join("status")) else {
            continue;
        };
        let inner_pid = status
            .lines()
            .find_map(|line| line.strip_prefix("NSpid:"))
            .and_then(|ids| ids.split_whitespace().last())
            .and_then(|id| id.parse::<i32>().ok());
        if inner_pid == Some(pid.as_raw())
            && fs::read_link(path.join("ns/pid")).ok().as_ref() == Some(&namespace)
        {
            return state(&status);
        }
    }
    None
}

async fn assert_stopped(pid: Pid) {
    timeout(Duration::from_secs(3), async {
        while is_running(pid) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture process {pid} is still executing"));
}

async fn check_graceful_signal(signal: StopSignal, expected: &str) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![
        fixture(root, "api", COOPERATIVE),
        fixture(root, "worker", COOPERATIVE),
    ];
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("api.pid")).await;
        wait_for(&root.join("worker.pid")).await;
        sender.send(signal).unwrap();
    };
    let (result, ()) = tokio::join!(
        supervise(commands, &mut receiver, Duration::from_secs(2)),
        control
    );
    result.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("api.stopped")).unwrap(),
        expected
    );
    assert_eq!(
        fs::read_to_string(root.join("worker.stopped")).unwrap(),
        expected
    );
    assert_eq!(
        kill(fixture_pid(root, "api"), None::<Signal>),
        Err(Errno::ESRCH)
    );
    assert_eq!(
        kill(fixture_pid(root, "worker"), None::<Signal>),
        Err(Errno::ESRCH)
    );
}

#[tokio::test]
async fn term_is_forwarded_and_both_children_are_reaped() {
    check_graceful_signal(StopSignal::Terminate, "TERM").await;
}

#[tokio::test]
async fn interrupt_is_forwarded_to_both_children() {
    check_graceful_signal(StopSignal::Interrupt, "INT").await;
}

#[tokio::test]
async fn successful_unexpected_exit_stops_its_sibling_and_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![
        fixture(root, "worker", COOPERATIVE),
        fixture(
            root,
            "api",
            r#"while [ ! -f "$1/release" ]; do sleep 0.05; done; exit 0"#,
        ),
    ];
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("worker.pid")).await;
        fs::write(root.join("release"), "exit").unwrap();
    };
    let (result, ()) = tokio::join!(
        supervise(commands, &mut receiver, Duration::from_secs(2)),
        control
    );
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("api") && error.contains("unexpectedly"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(root.join("worker.stopped")).unwrap(),
        "TERM"
    );
}

#[tokio::test]
async fn artifact_paths_follow_cargo_output_and_ignore_other_packages() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let executable = root.join("custom-target/debug/api");
    let message = |manifest: PathBuf, path: PathBuf| {
        serde_json::json!({
            "reason": "compiler-artifact",
            "manifest_path": manifest,
            "target": { "name": "api", "kind": ["bin"] },
            "executable": path,
        })
    };
    let lines = format!(
        "{}\n{}\n",
        message(root.join("Cargo.toml"), executable.clone()),
        message(root.join("other/Cargo.toml"), root.join("wrong-api"))
    );
    let artifacts = read_artifacts(lines.as_bytes(), root, &["api", "worker"])
        .await
        .unwrap();
    assert_eq!(artifacts.get("api"), Some(&executable));
    assert!(!artifacts.contains_key("worker"));
}

#[tokio::test]
async fn empty_process_set_is_rejected() {
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let started = Instant::now();
    assert!(
        supervise(vec![], &mut receiver, Duration::from_secs(1))
            .await
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

const STUBBORN_TREE: &str = r#"
trap '' INT TERM
/bin/sh -c 'trap "" INT TERM; printf "%s" "$$" > "$1/descendant.pid"; while :; do sleep 1; done' child "$1" &
printf '%s' "$$" > "$1/$2.pid"
wait
"#;

#[tokio::test]
async fn deadline_kills_stubborn_children_and_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![fixture(root, "worker", STUBBORN_TREE)];
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("worker.pid")).await;
        wait_for(&root.join("descendant.pid")).await;
        sender.send(StopSignal::Terminate).unwrap();
    };
    let started = Instant::now();
    let (result, ()) = tokio::join!(
        supervise(commands, &mut receiver, Duration::from_millis(100)),
        control
    );
    result.unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        kill(fixture_pid(root, "worker"), None::<Signal>),
        Err(Errno::ESRCH)
    );
    assert_stopped(fixture_pid(root, "descendant")).await;
}

async fn check_interrupt_escalation(first: StopSignal) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![fixture(root, "worker", STUBBORN_TREE)];
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("worker.pid")).await;
        wait_for(&root.join("descendant.pid")).await;
        sender.send(first).unwrap();
        sleep(Duration::from_millis(50)).await;
        sender.send(StopSignal::Interrupt).unwrap();
    };
    let started = Instant::now();
    let (result, ()) = tokio::join!(
        supervise(commands, &mut receiver, Duration::from_secs(30)),
        control
    );
    result.unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_stopped(fixture_pid(root, "descendant")).await;
}

#[tokio::test]
async fn interrupt_escalates_either_initial_shutdown_signal() {
    check_interrupt_escalation(StopSignal::Interrupt).await;
    check_interrupt_escalation(StopSignal::Terminate).await;
}

#[tokio::test]
async fn repeated_termination_signals_allow_a_slow_graceful_drain() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let script = r#"
trap 'printf started > "$1/$2.draining"; sleep 0.8; printf finished > "$1/$2.drained"; exit 0' TERM
printf '%s' "$$" > "$1/$2.pid"
while :; do sleep 1; done
"#;
    let commands = vec![fixture(root, "worker", script)];
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("worker.pid")).await;
        sender.send(StopSignal::Terminate).unwrap();
        wait_for(&root.join("worker.draining")).await;
        // A delayed parent wrapper can forward TERM after the original group
        // signal. This must remain graceful even beyond a debounce interval.
        sleep(Duration::from_millis(350)).await;
        sender.send(StopSignal::Terminate).unwrap();
        sleep(Duration::from_millis(200)).await;
        sender.send(StopSignal::Terminate).unwrap();
    };
    let (result, ()) = tokio::join!(
        supervise(commands, &mut receiver, Duration::from_secs(2)),
        control
    );
    result.unwrap();
    assert_eq!(
        fs::read_to_string(root.join("worker.drained")).unwrap(),
        "finished"
    );
    assert_eq!(
        kill(fixture_pid(root, "worker"), None::<Signal>),
        Err(Errno::ESRCH)
    );
}

#[tokio::test]
async fn repeated_termination_signals_do_not_extend_the_original_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![fixture(root, "worker", STUBBORN_TREE)];
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("worker.pid")).await;
        wait_for(&root.join("descendant.pid")).await;
        let requested = Instant::now();
        sender.send(StopSignal::Terminate).unwrap();
        for _ in 0..10 {
            sleep(Duration::from_millis(50)).await;
            sender.send(StopSignal::Terminate).unwrap();
        }
        requested
    };
    let supervisor = async {
        let result = supervise(commands, &mut receiver, Duration::from_millis(150)).await;
        (result, Instant::now())
    };
    let ((result, finished), requested) = tokio::join!(supervisor, control);
    result.unwrap();
    assert!(finished.duration_since(requested) < Duration::from_millis(400));
    assert_stopped(fixture_pid(root, "descendant")).await;
}

#[tokio::test]
async fn partial_spawn_failure_stops_already_started_children() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (name, command) = fixture(root, "api", COOPERATIVE);
    let mut children = vec![ManagedChild::spawn(name, command).unwrap()];
    wait_for(&root.join("api.pid")).await;
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let result = add_child(
        &mut children,
        "worker".to_owned(),
        Command::new(root.join("missing-executable")),
        &mut receiver,
        Duration::from_secs(2),
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("start worker"));
    assert_eq!(
        fs::read_to_string(root.join("api.stopped")).unwrap(),
        "TERM"
    );
    assert_eq!(
        kill(fixture_pid(root, "api"), None::<Signal>),
        Err(Errno::ESRCH)
    );
}

#[tokio::test]
async fn canceled_build_stops_its_process_group() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (_, mut command) = fixture(root, "cargo", STUBBORN_TREE);
    command.stdout(Stdio::piped());
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let control = async {
        wait_for(&root.join("cargo.pid")).await;
        wait_for(&root.join("descendant.pid")).await;
        sender.send(StopSignal::Terminate).unwrap();
    };
    let (result, ()) = tokio::join!(
        build_command(
            command,
            root,
            &["api", "worker"],
            &mut receiver,
            Duration::from_millis(100)
        ),
        control
    );
    assert!(result.unwrap().is_none());
    assert_eq!(
        kill(fixture_pid(root, "cargo"), None::<Signal>),
        Err(Errno::ESRCH)
    );
    assert_stopped(fixture_pid(root, "descendant")).await;
}

#[tokio::test]
async fn failed_build_does_not_return_service_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (_, mut command) = fixture(root, "cargo", "exit 7");
    command.stdout(Stdio::piped());
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let result = build_command(
        command,
        root,
        &["api"],
        &mut receiver,
        Duration::from_secs(1),
    )
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Cargo build failed")
    );
}

#[tokio::test]
async fn completed_build_cleans_descendant_holding_stdout_before_draining() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let script = r#"
/bin/sh -c 'trap "" INT TERM; printf "%s" "$$" > "$1/descendant.pid"; while :; do sleep 1; done' child "$1" &
while [ ! -s "$1/descendant.pid" ]; do sleep 0.01; done
printf '%s\n' "$BUILD_ARTIFACT"
exit 0
"#;
    let (_, mut command) = fixture(root, "cargo", script);
    let executable = root.join("custom-target/debug/api");
    command.stdout(Stdio::piped()).env(
        "BUILD_ARTIFACT",
        serde_json::json!({
            "reason": "compiler-artifact",
            "manifest_path": root.join("Cargo.toml"),
            "target": { "name": "api", "kind": ["bin"] },
            "executable": executable,
        })
        .to_string(),
    );
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let result = timeout(
        Duration::from_secs(2),
        build_command(
            command,
            root,
            &["api"],
            &mut receiver,
            Duration::from_secs(1),
        ),
    )
    .await
    .expect("Cargo exited, but inherited stdout kept the supervisor waiting");
    assert_eq!(result.unwrap().unwrap().get("api"), Some(&executable));
    assert_stopped(fixture_pid(root, "descendant")).await;
}

#[tokio::test]
async fn canceled_supervisor_future_still_kills_owned_groups() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let commands = vec![fixture(root, "worker", STUBBORN_TREE)];
    let (_sender, mut receiver) = mpsc::unbounded_channel();
    let cancellation = async {
        wait_for(&root.join("worker.pid")).await;
        wait_for(&root.join("descendant.pid")).await;
    };
    tokio::select! {
        result = supervise(commands, &mut receiver, Duration::from_secs(1)) => {
            panic!("supervisor unexpectedly finished: {result:?}");
        }
        () = cancellation => {}
    }
    assert_stopped(fixture_pid(root, "worker")).await;
    assert_stopped(fixture_pid(root, "descendant")).await;
}
