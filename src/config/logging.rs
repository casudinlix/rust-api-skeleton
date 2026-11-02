use std::{path::PathBuf, str::FromStr};

use anyhow::{bail, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogOutput {
    Stdout,
    File,
    Both,
}

impl FromStr for LogOutput {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stdout" => Ok(Self::Stdout),
            "file" => Ok(Self::File),
            "both" => Ok(Self::Both),
            _ => bail!("LOG_OUTPUT must be stdout, file, or both"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogRotation {
    Never,
    Hourly,
    Daily,
}

impl FromStr for LogRotation {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "never" => Ok(Self::Never),
            "hourly" => Ok(Self::Hourly),
            "daily" => Ok(Self::Daily),
            _ => bail!("LOG_ROTATION must be never, hourly, or daily"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoggingConfig {
    pub output: LogOutput,
    pub directory: PathBuf,
    pub file_prefix: String,
    pub rotation: LogRotation,
    pub max_files: usize,
    pub channel_capacity: usize,
}

impl LoggingConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.directory.as_os_str().is_empty(),
            "LOG_FILE_DIR must not be empty"
        );
        ensure!(
            !self.file_prefix.is_empty()
                && self.file_prefix.len() <= 64
                && self
                    .file_prefix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            "LOG_FILE_PREFIX must contain only ASCII letters, numbers, '-' or '_' and be at most 64 bytes"
        );
        ensure!(
            (1..=366).contains(&self.max_files),
            "LOG_MAX_FILES must be 1..366"
        );
        ensure!(
            (128..=1_048_576).contains(&self.channel_capacity),
            "LOG_CHANNEL_CAPACITY must be 128..1048576"
        );
        Ok(())
    }

    pub fn writes_file(&self) -> bool {
        matches!(self.output, LogOutput::File | LogOutput::Both)
    }

    pub fn writes_stdout(&self) -> bool {
        matches!(self.output, LogOutput::Stdout | LogOutput::Both)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_log_modes() {
        assert_eq!("stdout".parse::<LogOutput>().unwrap(), LogOutput::Stdout);
        assert_eq!("both".parse::<LogOutput>().unwrap(), LogOutput::Both);
        assert_eq!("daily".parse::<LogRotation>().unwrap(), LogRotation::Daily);
        assert!("weekly".parse::<LogRotation>().is_err());
    }

    #[test]
    fn validates_prefix_retention_and_buffer() {
        let mut config = LoggingConfig {
            output: LogOutput::Stdout,
            directory: "storage/logs".into(),
            file_prefix: "app".into(),
            rotation: LogRotation::Daily,
            max_files: 14,
            channel_capacity: 8_192,
        };
        config.validate().unwrap();
        config.file_prefix = "bad prefix".into();
        assert!(config.validate().is_err());
    }
}
