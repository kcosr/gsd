use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

use crate::config::LoggingConfig;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LoggingSettings {
    pub level: Level,
    pub directory: Option<PathBuf>,
    pub max_bytes: u64,
    pub max_files: usize,
    pub console: bool,
}

pub struct LoggingGuards {
    shutdown_tx: Option<mpsc::Sender<LogMessage>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for LoggingGuards {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(LogMessage::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum LoggingError {
    #[error("invalid log level for {field}: {value}")]
    InvalidLevel { field: &'static str, value: String },

    #[error("invalid logging config for {field}: {message}")]
    InvalidConfig {
        field: &'static str,
        message: String,
    },

    #[error("failed to create logging directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to initialize global tracing subscriber: {0}")]
    InitFailed(#[from] Box<dyn std::error::Error + Send + Sync>),
}

fn parse_level(value: &str) -> Result<Level, LoggingError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => Ok(Level::TRACE),
        "debug" => Ok(Level::DEBUG),
        "info" => Ok(Level::INFO),
        "warn" | "warning" => Ok(Level::WARN),
        "error" => Ok(Level::ERROR),
        other => Err(LoggingError::InvalidLevel {
            field: "level",
            value: other.to_string(),
        }),
    }
}

impl LoggingSettings {
    pub fn from_config(cfg: &LoggingConfig) -> Result<Self, LoggingError> {
        let level = parse_level(&cfg.level)?;

        if cfg.directory.is_some() {
            if cfg.max_bytes == 0 {
                return Err(LoggingError::InvalidConfig {
                    field: "max_bytes",
                    message: "must be > 0 when logging.directory is set".to_string(),
                });
            }
            if cfg.max_files == 0 {
                return Err(LoggingError::InvalidConfig {
                    field: "max_files",
                    message: "must be > 0 when logging.directory is set".to_string(),
                });
            }
        }

        Ok(Self {
            level,
            directory: cfg.directory.clone(),
            max_bytes: cfg.max_bytes,
            max_files: cfg.max_files,
            console: cfg.console,
        })
    }

    pub fn init_tracing(&self) -> Result<LoggingGuards, LoggingError> {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(self.level.as_str()));

        let (writer, guards) = self.build_writer()?;
        let ansi = self.console && self.directory.is_none();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_span_events(FmtSpan::CLOSE)
            .with_target(true)
            .with_ansi(ansi)
            .with_writer(writer);

        subscriber.try_init().map_err(LoggingError::InitFailed)?;

        Ok(guards)
    }

    fn build_writer(
        &self,
    ) -> Result<(impl Fn() -> NonBlockingWriter, LoggingGuards), LoggingError> {
        let (tx, rx) = mpsc::channel::<LogMessage>();
        let mut file_writer = None;

        if let Some(directory) = &self.directory {
            fs::create_dir_all(directory).map_err(|source| LoggingError::CreateDir {
                path: directory.clone(),
                source,
            })?;
            let path = directory.join("gsd.log");
            file_writer = Some(
                RotatingFileWriter::new(path, self.max_bytes, self.max_files)
                    .map_err(|e| LoggingError::InitFailed(Box::new(e)))?,
            );
        }

        let console = self.console;
        let worker = thread::spawn(move || {
            let mut writer = LogWriter::new(file_writer, console);
            while let Ok(msg) = rx.recv() {
                match msg {
                    LogMessage::Data(data) => {
                        let _ = writer.write_all(&data);
                    }
                    LogMessage::Flush => {
                        let _ = writer.flush();
                    }
                    LogMessage::Shutdown => break,
                }
            }
            let _ = writer.flush();
        });

        let writer = NonBlockingWriter { sender: tx.clone() };
        let make_writer = move || writer.clone();

        Ok((
            make_writer,
            LoggingGuards {
                shutdown_tx: Some(tx),
                worker: Some(worker),
            },
        ))
    }
}

#[derive(Clone)]
struct NonBlockingWriter {
    sender: mpsc::Sender<LogMessage>,
}

impl Write for NonBlockingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sender
            .send(LogMessage::Data(buf.to_vec()))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "logging worker stopped"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sender
            .send(LogMessage::Flush)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "logging worker stopped"))?;
        Ok(())
    }
}

enum LogMessage {
    Data(Vec<u8>),
    Flush,
    Shutdown,
}

struct LogWriter {
    file: Option<RotatingFileWriter>,
    console: bool,
}

impl LogWriter {
    fn new(file: Option<RotatingFileWriter>, console: bool) -> Self {
        Self { file, console }
    }
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.console {
            let mut stderr = io::stderr();
            stderr.write_all(buf)?;
        }

        if let Some(writer) = &mut self.file {
            writer.write_all(buf)?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.console {
            let mut stderr = io::stderr();
            stderr.flush()?;
        }

        if let Some(writer) = &mut self.file {
            writer.flush()?;
        }

        Ok(())
    }
}

struct RotatingFileWriter {
    base_path: PathBuf,
    max_bytes: u64,
    max_files: usize,
    file: File,
    size: u64,
}

impl RotatingFileWriter {
    fn new(base_path: PathBuf, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&base_path)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);

        let mut writer = Self {
            base_path,
            max_bytes,
            max_files,
            file,
            size,
        };

        if writer.max_bytes > 0 && writer.size >= writer.max_bytes {
            writer.rotate()?;
        }

        Ok(writer)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;

        if self.max_files == 0 {
            self.file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.base_path)?;
            self.size = 0;
            return Ok(());
        }

        let oldest = self.rotated_path(self.max_files);
        let _ = fs::remove_file(&oldest);

        for idx in (1..self.max_files).rev() {
            let from = self.rotated_path(idx);
            let to = self.rotated_path(idx + 1);
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }

        if self.base_path.exists() {
            let _ = fs::rename(&self.base_path, self.rotated_path(1));
        }

        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.base_path)?;
        self.size = 0;
        Ok(())
    }

    fn rotated_path(&self, index: usize) -> PathBuf {
        let name = self
            .base_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "gsd.log".to_string());
        let file_name = format!("{}.{}", name, index);
        let mut path = self.base_path.clone();
        path.set_file_name(file_name);
        path
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.max_bytes > 0 && self.size.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }

        let written = self.file.write(buf)?;
        self.size = self.size.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_logging_settings_rejects_zero_max_bytes() {
        let temp = TempDir::new().unwrap();
        let cfg = LoggingConfig {
            level: "info".to_string(),
            directory: Some(temp.path().to_path_buf()),
            max_bytes: 0,
            max_files: 1,
            console: true,
        };

        assert!(matches!(
            LoggingSettings::from_config(&cfg),
            Err(LoggingError::InvalidConfig {
                field: "max_bytes",
                ..
            })
        ));
    }

    #[test]
    fn test_logging_settings_rejects_zero_max_files() {
        let temp = TempDir::new().unwrap();
        let cfg = LoggingConfig {
            level: "info".to_string(),
            directory: Some(temp.path().to_path_buf()),
            max_bytes: 10,
            max_files: 0,
            console: true,
        };

        assert!(matches!(
            LoggingSettings::from_config(&cfg),
            Err(LoggingError::InvalidConfig {
                field: "max_files",
                ..
            })
        ));
    }

    #[test]
    fn test_rotating_file_writer_rotates_on_size() {
        let temp = TempDir::new().unwrap();
        let base_path = temp.path().join("gsd.log");

        let mut writer = RotatingFileWriter::new(base_path.clone(), 5, 2).unwrap();
        writer.write_all(b"hello").unwrap();
        writer.write_all(b"!").unwrap();
        writer.flush().unwrap();
        drop(writer);

        let current = fs::read_to_string(&base_path).unwrap();
        assert_eq!(current, "!");

        let rotated = fs::read_to_string(temp.path().join("gsd.log.1")).unwrap();
        assert_eq!(rotated, "hello");
        assert!(!temp.path().join("gsd.log.2").exists());
    }
}
