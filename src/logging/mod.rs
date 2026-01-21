use std::path::PathBuf;

use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::EnvFilter;

use crate::config::LoggingConfig;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LoggingSettings {
    pub level: Level,
    pub directory: Option<PathBuf>,
    pub console: bool,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum LoggingError {
    #[error("invalid log level: {0}")]
    InvalidLevel(String),

    #[error("failed to create logging directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to initialize tracing subscriber")]
    InitFailed,
}

fn parse_level(value: &str) -> Result<Level, LoggingError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "trace" => Ok(Level::TRACE),
        "debug" => Ok(Level::DEBUG),
        "info" => Ok(Level::INFO),
        "warn" | "warning" => Ok(Level::WARN),
        "error" => Ok(Level::ERROR),
        other => Err(LoggingError::InvalidLevel(other.to_string())),
    }
}

impl LoggingSettings {
    pub fn from_config(cfg: &LoggingConfig) -> Result<Self, LoggingError> {
        let level = parse_level(&cfg.level)?;
        Ok(Self {
            level,
            directory: cfg.directory.clone(),
            console: cfg.console,
        })
    }

    pub fn init_tracing(&self) -> Result<(), LoggingError> {
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new(self.level.as_str()));

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_span_events(FmtSpan::CLOSE)
            .with_target(true);

        if self.console {
            subscriber.with_ansi(true).init();
        } else {
            subscriber.with_ansi(false).init();
        }

        Ok(())
    }
}
