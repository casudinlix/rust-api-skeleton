use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, BufReader},
    process::{Child, Command},
    signal::unix::{SignalKind, signal},
    time::{Instant, interval, sleep_until, timeout, timeout_at},
};

use super::Options;

#[derive(Clone, Copy, Debug)]
enum StopSignal {
    Interrupt,
    Terminate,
}

impl StopSignal {
    fn os_signal(self) -> Signal {
        match self {
            Self::Interrupt => Signal::SIGINT,
            Self::Terminate => Signal::SIGTERM,
        }
    }
}

trait SignalSource {
    async fn recv(&mut self) -> StopSignal;
}

struct ProcessSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl ProcessSignals {
    fn install() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt()).context("install SIGINT handler")?,
            terminate: signal(SignalKind::terminate()).context("install SIGTERM handler")?,
        })
    }
}

impl SignalSource for ProcessSignals {
    async fn recv(&mut self) -> StopSignal {
        tokio::select! {
            _ = self.interrupt.recv() => StopSignal::Interrupt,
            _ = self.terminate.recv() => StopSignal::Terminate,
        }
    }
}

/// Each direct child starts its own process group. The guard also covers future
/// cancellation and partial startup, where normal async cleanup cannot finish.
struct ManagedChild {
    name: String,
    process: Child,
    group: Pid,
    status: Option<ExitStatus>,
    group_cleaned: bool,
}

impl ManagedChild {
    fn spawn(name: String, mut command: Command) -> Result<Self> {
        command.process_group(0).kill_on_drop(true);
        let process = command
            .spawn()
            .with_context(|| format!("start {name} process"))?;
        let id = process.id().context("spawned child has no process ID")?;
        let group = Pid::from_raw(i32::try_from(id).context("child process ID is out of range")?);
        ensure!(group.as_raw() > 0, "child process group must be positive");
        Ok(Self {
            name,
            process,
            group,
            status: None,
            group_cleaned: false,
        })
    }

    fn signal_group(&self, signal: Signal) -> Result<()> {
        match killpg(self.group, signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error).with_context(|| format!("signal {} process group", self.name)),
        }
    }

    fn poll(&mut self) -> Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = self
                .process
                .try_wait()
                .with_context(|| format!("reap {} process", self.name))?;
        }
        Ok(self.status)
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if !self.group_cleaned {
            let _ = self.signal_group(Signal::SIGKILL);
        }
        // Tokio also kills the direct child and schedules best-effort reaping
        // on drop. The process-group signal above covers its descendants.
    }
}

pub(super) async fn run(root: &Path, options: Options) -> Result<()> {
    ensure!(
        !options.shutdown_timeout.is_zero(),
        "shutdown timeout must be positive"
    );
    let mut signals = ProcessSignals::install()?;
    let mut binaries = vec!["api", "worker"];
    if options.scheduler {
        binaries.push("scheduler");
    }
    eprintln!("Building {}...", binaries.join(" + "));
    let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    build
        .current_dir(root)
        .args([
            "build",
            "--locked",
            "--message-format=json-render-diagnostics",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    for binary in &binaries {
        build.args(["--bin", binary]);
    }
    let Some(artifacts) = build_command(
        build,
        root,
        &binaries,
        &mut signals,
        options.shutdown_timeout,
    )
    .await?
    else {
        return Ok(());
    };

    let mut commands = Vec::with_capacity(binaries.len());
    for name in binaries {
        let executable = artifacts
            .get(name)
            .with_context(|| format!("Cargo did not report an executable for {name}"))?;
        let mut command = Command::new(executable);
        command
            .current_dir(root)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        commands.push((name.to_owned(), command));
    }
    supervise(commands, &mut signals, options.shutdown_timeout).await
}

enum BuildEvent {
    Stopped(StopSignal),
    Exited(std::io::Result<ExitStatus>),
    Output(Result<BTreeMap<String, PathBuf>>),
}

async fn build_command(
    command: Command,
    root: &Path,
    binaries: &[&str],
    signals: &mut impl SignalSource,
    grace: Duration,
) -> Result<Option<BTreeMap<String, PathBuf>>> {
    let mut child = ManagedChild::spawn("cargo".to_owned(), command)?;
    let stdout = child
        .process
        .stdout
        .take()
        .context("Cargo stdout must be piped")?;
    let mut reader = Box::pin(read_artifacts(BufReader::new(stdout), root, binaries));
    let mut artifacts = None;
    let status = loop {
        let event = tokio::select! {
            stop = signals.recv() => BuildEvent::Stopped(stop),
            result = child.process.wait() => BuildEvent::Exited(result),
            result = &mut reader, if artifacts.is_none() => BuildEvent::Output(result),
        };
        match event {
            BuildEvent::Stopped(stop) => {
                eprintln!("Stopping build...");
                drop(reader);
                stop_children(std::slice::from_mut(&mut child), signals, stop, grace).await?;
                return Ok(None);
            }
            BuildEvent::Output(Ok(output)) => artifacts = Some(output),
            BuildEvent::Exited(Ok(status)) => break status,
            BuildEvent::Output(Err(error)) => {
                drop(reader);
                let cleanup = stop_children(
                    std::slice::from_mut(&mut child),
                    signals,
                    StopSignal::Terminate,
                    grace,
                )
                .await;
                return Err(with_cleanup(error, cleanup));
            }
            BuildEvent::Exited(Err(error)) => {
                drop(reader);
                let cleanup = stop_children(
                    std::slice::from_mut(&mut child),
                    signals,
                    StopSignal::Terminate,
                    grace,
                )
                .await;
                return Err(with_cleanup(error.into(), cleanup));
            }
        }
    };
    child.status = Some(status);
    // Observe Cargo exit independently of stdout EOF: build-script descendants
    // can retain the pipe after Cargo exits. Stop those groups before draining.
    child.signal_group(Signal::SIGKILL)?;
    child.group_cleaned = true;
    let artifacts = match artifacts {
        Some(artifacts) => artifacts,
        None => tokio::select! {
            _ = signals.recv() => return Ok(None),
            result = timeout(REAP_TIMEOUT, &mut reader) => {
                result.context("Cargo output did not close after its build process exited")??
            }
        },
    };
    ensure!(status.success(), "Cargo build failed with {status}");
    for name in binaries {
        ensure!(
            artifacts.contains_key(*name),
            "Cargo did not report an executable for {name}"
        );
    }
    Ok(Some(artifacts))
}

async fn read_artifacts(
    reader: impl AsyncBufRead + Unpin,
    root: &Path,
    requested: &[&str],
) -> Result<BTreeMap<String, PathBuf>> {
    let mut artifacts = BTreeMap::new();
    let mut lines = reader.lines();
    let manifest = root.join("Cargo.toml");
    while let Some(line) = lines.next_line().await.context("read Cargo output")? {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            // Procedural macros can emit non-JSON output even in Cargo JSON mode.
            eprintln!("{line}");
            continue;
        };
        if message["reason"] == "compiler-message" {
            if let Some(rendered) = message["message"]["rendered"].as_str() {
                eprint!("{rendered}");
            }
            continue;
        }
        if message["reason"] != "compiler-artifact"
            || message["manifest_path"].as_str().map(Path::new) != Some(manifest.as_path())
            || !message["target"]["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
        {
            continue;
        }
        if let (Some(name), Some(path)) = (
            message["target"]["name"].as_str(),
            message["executable"].as_str(),
        ) && requested.contains(&name)
        {
            let path = PathBuf::from(path);
            artifacts.insert(
                name.to_owned(),
                if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                },
            );
        }
    }
    Ok(artifacts)
}

async fn add_child(
    children: &mut Vec<ManagedChild>,
    name: String,
    command: Command,
    signals: &mut impl SignalSource,
    grace: Duration,
) -> Result<()> {
    match ManagedChild::spawn(name, command) {
        Ok(child) => {
            eprintln!("Started {} (PID {}).", child.name, child.group);
            children.push(child);
            Ok(())
        }
        Err(error) => {
            let cleanup = stop_children(children, signals, StopSignal::Terminate, grace).await;
            Err(with_cleanup(error, cleanup))
        }
    }
}

async fn supervise(
    commands: Vec<(String, Command)>,
    signals: &mut impl SignalSource,
    grace: Duration,
) -> Result<()> {
    ensure!(!commands.is_empty(), "no services were selected");
    let mut children = Vec::with_capacity(commands.len());
    for (name, command) in commands {
        add_child(&mut children, name, command, signals, grace).await?;
    }
    eprintln!("Services are running. Press Ctrl+C to stop them together.");
    let mut poll = interval(Duration::from_millis(25));
    let failure = loop {
        tokio::select! {
            stop = signals.recv() => {
                eprintln!("Stopping services...");
                return stop_children(&mut children, signals, stop, grace).await;
            }
            _ = poll.tick() => {
                let mut failure = None;
                for child in &mut children {
                    match child.poll() {
                        Ok(None) => {}
                        Ok(Some(status)) => {
                            failure = Some(anyhow!("{} exited unexpectedly with {status}", child.name));
                            break;
                        }
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
                if let Some(failure) = failure {
                    break failure;
                }
            }
        }
    };
    let cleanup = stop_children(&mut children, signals, StopSignal::Terminate, grace).await;
    Err(with_cleanup(failure, cleanup))
}

const REAP_TIMEOUT: Duration = Duration::from_secs(5);

async fn stop_children(
    children: &mut [ManagedChild],
    signals: &mut impl SignalSource,
    stop: StopSignal,
    grace: Duration,
) -> Result<()> {
    let mut errors = Vec::new();
    for child in children.iter() {
        if let Err(error) = child.signal_group(stop.os_signal()) {
            errors.push(format!("{error:#}"));
        }
    }
    let deadline = Instant::now() + grace;
    let mut poll = interval(Duration::from_millis(25));
    let forced = loop {
        let mut all_stopped = true;
        for child in children.iter_mut() {
            match child.poll() {
                Ok(Some(_)) => {}
                Ok(None) => all_stopped = false,
                Err(error) => {
                    errors.push(format!("{error:#}"));
                    all_stopped = false;
                }
            }
        }
        if all_stopped {
            break false;
        }
        if !errors.is_empty() {
            break true;
        }
        tokio::select! {
            _ = sleep_until(deadline) => break true,
            stop = signals.recv() => {
                // Make can forward TERM after the original process-group TERM.
                // Repeated termination requests keep the original deadline;
                // Ctrl+C during a drain is the explicit request to force it.
                if matches!(stop, StopSignal::Interrupt) {
                    break true;
                }
            },
            _ = poll.tick() => {}
        }
    };
    if forced {
        eprintln!("Forcing remaining process groups to stop.");
    }
    // Even a reaped direct child may have descendants. Sweep every group after
    // cooperative children exit, or when the common grace period expires.
    for child in children.iter_mut() {
        match child.signal_group(Signal::SIGKILL) {
            Ok(()) => child.group_cleaned = true,
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    let reap_deadline = Instant::now() + REAP_TIMEOUT;
    for child in children.iter_mut().filter(|child| child.status.is_none()) {
        match timeout_at(reap_deadline, child.process.wait()).await {
            Ok(Ok(status)) => child.status = Some(status),
            Ok(Err(error)) => errors.push(format!("could not reap {}: {error}", child.name)),
            Err(_) => errors.push(format!("timed out reaping {} after SIGKILL", child.name)),
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("; "));
    }
    Ok(())
}

fn with_cleanup(error: anyhow::Error, cleanup: Result<()>) -> anyhow::Error {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) => anyhow!("{error:#}; cleanup also failed: {cleanup:#}"),
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
