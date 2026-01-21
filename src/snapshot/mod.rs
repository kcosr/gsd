use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::config::{Config, TargetConfig};
use crate::git::{
    commit_all, ensure_repo_initialized, has_changes, is_detached_head, is_git_available,
    list_changed_files, GitError,
};

#[derive(Debug)]
struct TargetState {
    config: TargetConfig,
    in_flight: bool,
    task_handle: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub struct SnapshotService {
    config: Config,
    config_path: Option<PathBuf>,
    targets: Arc<RwLock<HashMap<String, TargetState>>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    shutdown_rx: Option<mpsc::Receiver<()>>,
}

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum SnapshotError {
    #[error("git is not available")]
    GitNotAvailable,

    #[error("git error: {0}")]
    Git(#[from] GitError),

    #[error("target initialization failed for {id}: {message}")]
    TargetInitFailed { id: String, message: String },
}

fn format_commit_message(files: &[String], max_files: usize) -> String {
    let visible: Vec<&str> = files.iter().take(max_files).map(|s| s.as_str()).collect();
    let remaining = files.len().saturating_sub(max_files);

    if remaining > 0 {
        format!("{} +{} more", visible.join(", "), remaining)
    } else {
        visible.join(", ")
    }
}

impl SnapshotService {
    pub fn new(config: Config, config_path: Option<PathBuf>) -> Self {
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>(1);
        Self {
            config,
            config_path,
            targets: Arc::new(RwLock::new(HashMap::new())),
            shutdown_tx: Some(shutdown_tx),
            shutdown_rx: Some(shutdown_rx),
        }
    }

    /// Get a sender that can be used to trigger shutdown from another task
    pub fn get_shutdown_sender(&self) -> mpsc::Sender<()> {
        self.shutdown_tx.clone().expect("shutdown_tx should exist")
    }

    pub async fn initialize(&mut self) -> Result<(), SnapshotError> {
        if !is_git_available().await {
            return Err(SnapshotError::GitNotAvailable);
        }

        let mut initialized_count = 0;
        let mut skipped_count = 0;

        for target in &self.config.targets {
            if !target.enabled {
                info!(target = %target.name(), "Target is disabled, skipping");
                skipped_count += 1;
                continue;
            }

            // Merge ignore patterns
            let mut all_patterns = self.config.git.default_ignore_patterns.clone();
            all_patterns.extend(target.ignore_patterns.clone());

            match ensure_repo_initialized(
                &target.path,
                &self.config.git.author_name,
                &self.config.git.author_email,
                &all_patterns,
            )
            .await
            {
                Ok(()) => {
                    info!(
                        target = %target.name(),
                        path = %target.path.display(),
                        interval_seconds = target.interval_seconds,
                        "Initialized target"
                    );

                    let mut targets = self.targets.write().await;
                    targets.insert(
                        target.path.to_string_lossy().to_string(),
                        TargetState {
                            config: target.clone(),
                            in_flight: false,
                            task_handle: None,
                        },
                    );
                    initialized_count += 1;
                }
                Err(e) => {
                    warn!(
                        target = %target.name(),
                        path = %target.path.display(),
                        error = %e,
                        "Failed to initialize target"
                    );
                    skipped_count += 1;
                }
            }
        }

        info!(
            initialized = initialized_count,
            skipped = skipped_count,
            "Snapshot service initialized"
        );

        if initialized_count == 0 && !self.config.targets.is_empty() {
            warn!("No targets were successfully initialized");
        }

        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), SnapshotError> {
        let mut shutdown_rx = self.shutdown_rx.take().expect("shutdown_rx should exist");

        // Initial commit check for all targets
        self.commit_all_targets().await;

        // Start timer tasks for all targets
        self.start_all_target_tasks().await;

        // Set up config file watcher
        let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);
        let _watcher = self.setup_config_watcher(reload_tx);

        info!("Snapshot service running, waiting for shutdown signal");

        // Main event loop
        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received, stopping tasks");
                    break;
                }
                _ = reload_rx.recv() => {
                    info!("Config change detected, reloading");
                    if let Err(e) = self.reload_config().await {
                        warn!(error = %e, "Failed to reload config");
                    }
                }
            }
        }

        // Cancel all tasks
        self.stop_all_target_tasks().await;

        Ok(())
    }

    /// Start timer tasks for all targets
    async fn start_all_target_tasks(&self) {
        let mut targets = self.targets.write().await;
        for (id, state) in targets.iter_mut() {
            if state.task_handle.is_some() {
                continue; // Already running
            }
            let handle = self.spawn_target_task(id.clone(), state.config.clone());
            state.task_handle = Some(handle);
        }
    }

    /// Stop all target tasks
    async fn stop_all_target_tasks(&self) {
        let mut targets = self.targets.write().await;
        for state in targets.values_mut() {
            if let Some(handle) = state.task_handle.take() {
                handle.abort();
            }
        }
    }

    /// Spawn a timer task for a single target
    fn spawn_target_task(&self, target_id: String, config: TargetConfig) -> JoinHandle<()> {
        let interval = Duration::from_secs(config.interval_seconds);
        let targets_ref = Arc::clone(&self.targets);
        let path = config.path.clone();

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);
            interval_timer.tick().await; // Skip immediate first tick

            loop {
                interval_timer.tick().await;
                Self::commit_target_static(&targets_ref, &target_id, &path).await;
            }
        })
    }

    /// Set up config file watcher
    fn setup_config_watcher(&self, reload_tx: mpsc::Sender<()>) -> Option<RecommendedWatcher> {
        let config_path = self.config_path.as_ref()?;

        let path_for_handler = config_path.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            if let Ok(event) = res {
                // Only trigger on modify/create events for our config file
                if (event.kind.is_modify() || event.kind.is_create())
                    && event.paths.iter().any(|p| p == &path_for_handler)
                {
                    let _ = reload_tx.blocking_send(());
                }
            }
        })
        .ok()?;

        // Watch the parent directory (more reliable for editors that do atomic saves)
        if let Some(parent) = config_path.parent() {
            if watcher.watch(parent, RecursiveMode::NonRecursive).is_err() {
                warn!("Failed to watch config directory for changes");
                return None;
            }
        }

        info!(path = %config_path.display(), "Watching config file for changes");
        Some(watcher)
    }

    /// Reload config and reconcile targets
    async fn reload_config(&mut self) -> Result<(), SnapshotError> {
        let config_path = match &self.config_path {
            Some(p) => p.clone(),
            None => return Ok(()), // No config path, nothing to reload
        };

        // Small delay to let editors finish writing
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Load new config
        let new_config = match Config::load_from_sources(Some(&config_path)) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "Failed to parse config, keeping current");
                return Ok(());
            }
        };

        // Build set of new target paths
        let new_target_paths: std::collections::HashSet<_> = new_config
            .targets
            .iter()
            .filter(|t| t.enabled)
            .map(|t| t.path.to_string_lossy().to_string())
            .collect();

        // Build set of current target paths
        let current_paths: Vec<String> = {
            let targets = self.targets.read().await;
            targets.keys().cloned().collect()
        };

        // Remove targets that are no longer in config or disabled
        for path in &current_paths {
            if !new_target_paths.contains(path) {
                self.remove_target(path).await;
            }
        }

        // Add or update targets
        for target in &new_config.targets {
            if !target.enabled {
                continue;
            }

            let path_key = target.path.to_string_lossy().to_string();
            let needs_restart = {
                let targets = self.targets.read().await;
                if let Some(state) = targets.get(&path_key) {
                    // Check if interval changed
                    state.config.interval_seconds != target.interval_seconds
                } else {
                    false
                }
            };

            if needs_restart {
                // Interval changed, restart the task
                self.remove_target(&path_key).await;
            }

            // Add if not present
            let exists = {
                let targets = self.targets.read().await;
                targets.contains_key(&path_key)
            };

            if !exists {
                self.add_target(target.clone()).await;
            }
        }

        self.config = new_config;
        info!("Config reloaded successfully");
        Ok(())
    }

    /// Add a new target at runtime
    async fn add_target(&self, target: TargetConfig) {
        let path_key = target.path.to_string_lossy().to_string();

        // Initialize the repo
        let mut all_patterns = self.config.git.default_ignore_patterns.clone();
        all_patterns.extend(target.ignore_patterns.clone());

        if let Err(e) = ensure_repo_initialized(
            &target.path,
            &self.config.git.author_name,
            &self.config.git.author_email,
            &all_patterns,
        )
        .await
        {
            warn!(
                target = %target.name(),
                error = %e,
                "Failed to initialize new target"
            );
            return;
        }

        // Spawn task and add to targets
        let handle = self.spawn_target_task(path_key.clone(), target.clone());

        let mut targets = self.targets.write().await;
        targets.insert(
            path_key.clone(),
            TargetState {
                config: target.clone(),
                in_flight: false,
                task_handle: Some(handle),
            },
        );

        info!(target = %target.name(), "Added target");
    }

    /// Remove a target at runtime
    async fn remove_target(&self, path_key: &str) {
        let mut targets = self.targets.write().await;
        if let Some(mut state) = targets.remove(path_key) {
            if let Some(handle) = state.task_handle.take() {
                handle.abort();
            }
            info!(target = %path_key, "Removed target");
        }
    }

    async fn commit_all_targets(&self) {
        let targets = self.targets.read().await;

        for (id, state) in targets.iter() {
            Self::commit_target_static(&self.targets, id, &state.config.path).await;
        }
    }

    async fn commit_target_static(
        targets: &Arc<RwLock<HashMap<String, TargetState>>>,
        target_id: &str,
        path: &Path,
    ) {
        // Check and set in_flight
        {
            let mut targets_write = targets.write().await;
            if let Some(state) = targets_write.get_mut(target_id) {
                if state.in_flight {
                    debug!(target = %target_id, "Commit already in progress, skipping");
                    return;
                }
                state.in_flight = true;
            } else {
                return;
            }
        }

        // Do the actual commit work
        let result = Self::do_commit(target_id, path).await;

        // Clear in_flight
        {
            let mut targets_write = targets.write().await;
            if let Some(state) = targets_write.get_mut(target_id) {
                state.in_flight = false;
            }
        }

        if let Err(e) = result {
            warn!(target = %target_id, error = %e, "Failed to commit");
        }
    }

    async fn do_commit(target_id: &str, path: &Path) -> Result<(), GitError> {
        // Check for detached HEAD
        if is_detached_head(path).await? {
            warn!(
                target = %target_id,
                "Detached HEAD detected, skipping commit"
            );
            return Err(GitError::DetachedHead {
                path: path.to_path_buf(),
            });
        }

        // Check for changes
        if !has_changes(path).await? {
            debug!(target = %target_id, "No changes to commit");
            return Ok(());
        }

        // Get changed files for commit message
        let changed_files = list_changed_files(path).await?;
        let message = format_commit_message(&changed_files, 10);

        info!(
            target = %target_id,
            files = changed_files.len(),
            message = %message,
            "Committing changes"
        );

        commit_all(path, &message).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GSD_DIR;
    use tempfile::TempDir;
    use tokio::fs;

    #[test]
    fn test_format_commit_message() {
        let files = vec!["a.txt".to_string(), "b.txt".to_string()];
        assert_eq!(format_commit_message(&files, 10), "a.txt, b.txt");

        let files: Vec<String> = (0..15).map(|i| format!("file{}.txt", i)).collect();
        let msg = format_commit_message(&files, 10);
        assert!(msg.ends_with("+5 more"));
    }

    #[tokio::test]
    async fn test_service_initialization() {
        let temp = TempDir::new().unwrap();
        let target_path = temp.path().join("target1");
        fs::create_dir_all(&target_path).await.unwrap();

        let config = Config {
            targets: vec![crate::config::TargetConfig {
                path: target_path.clone(),
                interval_seconds: 60,
                ignore_patterns: vec![],
                enabled: true,
            }],
            ..Default::default()
        };

        let mut service = SnapshotService::new(config, None);
        service.initialize().await.unwrap();

        // Check that our snapshot repo was initialized (not .git)
        assert!(target_path.join(GSD_DIR).exists());
        assert!(!target_path.join(".git").exists());
    }

    #[tokio::test]
    async fn test_service_coexists_with_regular_git() {
        let temp = TempDir::new().unwrap();
        let target_path = temp.path().join("project");
        fs::create_dir_all(&target_path).await.unwrap();

        // Create a regular git repo first
        crate::git::run_git(&target_path, &["init"], None)
            .await
            .unwrap();
        assert!(target_path.join(".git").exists());

        let config = Config {
            targets: vec![crate::config::TargetConfig {
                path: target_path.clone(),
                interval_seconds: 60,
                ignore_patterns: vec![],
                enabled: true,
            }],
            ..Default::default()
        };

        let mut service = SnapshotService::new(config, None);
        service.initialize().await.unwrap();

        // Both repos should exist
        assert!(target_path.join(".git").exists());
        assert!(target_path.join(GSD_DIR).exists());

        // Target should be tracked
        let targets = service.targets.read().await;
        assert_eq!(targets.len(), 1);
    }
}
