use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::Duration,
};

use chrono::Utc;
use tracing_subscriber::{
    EnvFilter, Layer,
    filter,
    fmt::MakeWriter,
    layer::SubscriberExt,
    util::SubscriberInitExt,
};

use crate::config::{
    Settings,
    logging::{LogOutput, LogRotation, LoggingConfig},
};

#[derive(Clone)]
struct AsyncMakeWriter {
    sender: SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
}

struct EventWriter {
    sender: SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
    buffer: Vec<u8>,
}

impl EventWriter {
    fn submit(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        let bytes = std::mem::take(&mut self.buffer);
        match self.sender.try_send(bytes) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Drop for EventWriter {
    fn drop(&mut self) {
        self.submit();
    }
}

impl Write for EventWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.submit();
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for AsyncMakeWriter {
    type Writer = EventWriter;

    fn make_writer(&'a self) -> Self::Writer {
        EventWriter {
            sender: self.sender.clone(),
            dropped: Arc::clone(&self.dropped),
            buffer: Vec::with_capacity(512),
        }
    }
}

pub struct LogGuard {
    stop: Option<Arc<AtomicBool>>,
    done: Option<Receiver<()>>,
    thread: Option<JoinHandle<()>>,
    dropped: Arc<AtomicU64>,
}

impl LogGuard {
    fn stdout_only() -> Self {
        Self {
            stop: None,
            done: None,
            thread: None,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Release);
        }

        let finished = self
            .done
            .take()
            .is_none_or(|done| done.recv_timeout(Duration::from_secs(2)).is_ok());
        if finished {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        } else {
            // Drop the JoinHandle to detach the logger. Process exit remains the
            // hard bound if the filesystem itself is stalled.
            self.thread.take();
            eprintln!("file logger did not flush within 2 seconds");
        }

        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            eprintln!("logging queue dropped {dropped} event(s)");
        }
    }
}

pub fn init(settings: &Settings, process: &str) -> anyhow::Result<LogGuard> {
    settings.logging.validate()?;
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));

    if !settings.logging.writes_file() {
        init_stdout(settings.app.log_format.as_str(), env_filter)?;
        return Ok(LogGuard::stdout_only());
    }

    let (writer, guard) = file_writer(&settings.logging, process)?;
    match (settings.app.log_format.as_str(), settings.logging.output) {
        ("json", LogOutput::File) => tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize JSON file tracing"))?,
        ("pretty", LogOutput::File) => tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize file tracing"))?,
        ("json", LogOutput::Both) => tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer)
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize JSON tracing"))?,
        ("pretty", LogOutput::Both) => tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_filter(filter::filter_fn(safe_target)))
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize tracing"))?,
        _ => anyhow::bail!("unsupported logging configuration"),
    }
    Ok(guard)
}

fn init_stdout(format: &str, env_filter: EnvFilter) -> anyhow::Result<()> {
    if format == "json" {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(filter::filter_fn(safe_target)),
            )
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize JSON tracing"))
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_filter(filter::filter_fn(safe_target)))
            .try_init()
            .map_err(|_| anyhow::anyhow!("could not initialize tracing"))
    }
}

fn safe_target(metadata: &tracing::Metadata<'_>) -> bool {
    !metadata.target().starts_with("apalis_core")
        && !metadata.target().starts_with("apalis_redis")
}

fn file_writer(config: &LoggingConfig, process: &str) -> anyhow::Result<(AsyncMakeWriter, LogGuard)> {
    fs::create_dir_all(&config.directory)
        .map_err(|_| anyhow::anyhow!("could not create log directory"))?;
    let metadata = fs::symlink_metadata(&config.directory)
        .map_err(|_| anyhow::anyhow!("could not inspect log directory"))?;
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "LOG_FILE_DIR must be a real directory, not a symlink"
    );

    let prefix = format!("{}-{process}", config.file_prefix);
    let mut rotating = RotatingFile::open(
        config.directory.clone(),
        prefix,
        config.rotation,
        config.max_files,
    )?;
    rotating.prune()?;

    let (sender, receiver) = mpsc::sync_channel(config.channel_capacity);
    let (done_sender, done_receiver) = mpsc::channel();
    let dropped = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name(format!("log-writer-{process}"))
        .spawn(move || {
            writer_loop(&mut rotating, receiver, &worker_stop);
            let _ = done_sender.send(());
        })
        .map_err(|_| anyhow::anyhow!("could not start log writer thread"))?;

    let writer = AsyncMakeWriter {
        sender,
        dropped: Arc::clone(&dropped),
    };
    let guard = LogGuard {
        stop: Some(stop),
        done: Some(done_receiver),
        thread: Some(thread),
        dropped,
    };
    Ok((writer, guard))
}

fn writer_loop(file: &mut RotatingFile, receiver: Receiver<Vec<u8>>, stop: &AtomicBool) {
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(bytes) => {
                if file.write(&bytes).is_err() {
                    eprintln!("file logging write failed");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if stop.load(Ordering::Acquire) {
            while let Ok(bytes) = receiver.try_recv() {
                if file.write(&bytes).is_err() {
                    eprintln!("file logging write failed");
                    break;
                }
            }
            let _ = file.flush();
            break;
        }
    }
}

struct RotatingFile {
    directory: PathBuf,
    prefix: String,
    rotation: LogRotation,
    max_files: usize,
    key: String,
    file: File,
}

impl RotatingFile {
    fn open(
        directory: PathBuf,
        prefix: String,
        rotation: LogRotation,
        max_files: usize,
    ) -> anyhow::Result<Self> {
        let key = rotation_key(rotation);
        let file = open_file(&directory, &prefix, rotation, &key)?;
        Ok(Self {
            directory,
            prefix,
            rotation,
            max_files,
            key,
            file,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let key = rotation_key(self.rotation);
        if key != self.key {
            self.file.flush()?;
            self.file = open_file(&self.directory, &self.prefix, self.rotation, &key)?;
            self.key = key;
            if self.prune().is_err() {
                eprintln!("log retention cleanup failed");
            }
        }
        self.file.write_all(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn prune(&self) -> anyhow::Result<()> {
        if self.rotation == LogRotation::Never {
            return Ok(());
        }

        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.directory)
            .map_err(|_| anyhow::anyhow!("could not scan log directory"))?
        {
            let entry = entry.map_err(|_| anyhow::anyhow!("could not inspect log file"))?;
            let file_type = entry
                .file_type()
                .map_err(|_| anyhow::anyhow!("could not inspect log file type"))?;
            if file_type.is_symlink() || !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with(&format!("{}.", self.prefix)) && name.ends_with(".log") {
                candidates.push((name.to_owned(), entry.path()));
            }
        }

        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let excess = candidates.len().saturating_sub(self.max_files);
        for (_, path) in candidates.into_iter().take(excess) {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
}

fn rotation_key(rotation: LogRotation) -> String {
    match rotation {
        LogRotation::Never => String::new(),
        LogRotation::Hourly => Utc::now().format("%Y-%m-%d-%H").to_string(),
        LogRotation::Daily => Utc::now().format("%Y-%m-%d").to_string(),
    }
}

fn open_file(
    directory: &Path,
    prefix: &str,
    rotation: LogRotation,
    key: &str,
) -> anyhow::Result<File> {
    let name = if rotation == LogRotation::Never {
        format!("{prefix}.log")
    } else {
        format!("{prefix}.{key}.log")
    };
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(name))
        .map_err(|_| anyhow::anyhow!("could not open log file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::logging::LoggingConfig;

    #[test]
    fn rotating_file_prunes_only_matching_process_files() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "app-api.2026-10-29.log",
            "app-api.2026-10-30.log",
            "app-api.2026-10-31.log",
            "app-worker.2026-10-29.log",
            "unrelated.txt",
        ] {
            fs::write(root.path().join(name), b"x").unwrap();
        }
        let rotating = RotatingFile::open(
            root.path().to_path_buf(),
            "app-api".into(),
            LogRotation::Daily,
            2,
        )
        .unwrap();
        rotating.prune().unwrap();

        assert!(!root.path().join("app-api.2026-10-29.log").exists());
        assert!(root.path().join("app-worker.2026-10-29.log").exists());
        assert!(root.path().join("unrelated.txt").exists());
    }

    #[test]
    fn process_prefixes_keep_multi_process_logs_separate() {
        let config = LoggingConfig {
            output: LogOutput::File,
            directory: "storage/logs".into(),
            file_prefix: "app".into(),
            rotation: LogRotation::Daily,
            max_files: 14,
            channel_capacity: 8_192,
        };
        let api = format!("{}-{}", config.file_prefix, "api");
        let worker = format!("{}-{}", config.file_prefix, "worker");
        assert_ne!(api, worker);
    }
}
