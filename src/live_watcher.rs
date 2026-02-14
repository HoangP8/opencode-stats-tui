use log::{error, info};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::{
    path::PathBuf,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

/// Real-time file watcher with instant updates via channel-based wake
pub struct LiveWatcher {
    watcher: RecommendedWatcher,
    storage_path: PathBuf,
    last_flush: Arc<Mutex<Instant>>,
    first_pending: Arc<Mutex<Option<Instant>>>,
    changed_files: Arc<Mutex<Vec<PathBuf>>>,
    on_change: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>,
}

impl LiveWatcher {
    pub fn new(
        storage_path: PathBuf,
        on_change: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>,
        wake_tx: mpsc::Sender<()>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let changed_files = Arc::new(Mutex::new(Vec::new()));
        let changed_files_clone = changed_files.clone();

        let config = Config::default().with_poll_interval(Duration::from_millis(50));

        let watcher = RecommendedWatcher::new(
            move |res: Result<Event, _>| {
                if let Ok(event) = res {
                    if event.kind.is_modify() || event.kind.is_create() || event.kind.is_remove() {
                        let mut any_added = false;
                        for path in event.paths {
                            if let Some(path_str) = path.to_str() {
                                if path_str.contains(".swp")
                                    || path_str.contains(".tmp")
                                    || path_str.contains("~")
                                    || path_str.contains("4913")
                                {
                                    continue;
                                }

                                let is_json = path.extension().is_some_and(|e| e == "json");
                                // SQLite mode writes mostly hit opencode.db-wal/opencode.db-shm
                                // (and sometimes opencode.db). Include these so live refresh stays reliable.
                                let is_sqlite_file =
                                    path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                                        n == "opencode.db"
                                            || n == "opencode.db-wal"
                                            || n == "opencode.db-shm"
                                    });

                                if is_json || is_sqlite_file || event.kind.is_remove() {
                                    let mut files = changed_files_clone.lock();
                                    if !files.contains(&path) {
                                        files.push(path.clone());
                                        any_added = true;
                                    }
                                }
                            }
                        }
                        if any_added {
                            let _ = wake_tx.send(());
                        }
                    }
                } else if let Err(e) = res {
                    error!("File watcher error: {:?}", e);
                }
            },
            config,
        )?;

        Ok(Self {
            watcher,
            storage_path,
            last_flush: Arc::new(Mutex::new(Instant::now() - Duration::from_millis(100))),
            first_pending: Arc::new(Mutex::new(None)),
            changed_files,
            on_change,
        })
    }

    /// Start watching
    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.watcher
            .watch(&self.storage_path, RecursiveMode::Recursive)?;
        info!(
            "Watching directory for live updates: {}",
            self.storage_path.display()
        );
        Ok(())
    }

    /// Process pending changes with bounded coalesce (no silence requirement).
    /// Flushes immediately if rate limit allows, or after bounded wait.
    pub fn process_changes(&self) {
        let mut files = self.changed_files.lock();

        if files.is_empty() {
            return;
        }

        let now = Instant::now();

        {
            let mut fp = self.first_pending.lock();
            if fp.is_none() {
                *fp = Some(now);
            }
        }

        let last_flush = *self.last_flush.lock();
        let first_pending = *self.first_pending.lock();

        let should_flush = now.duration_since(last_flush) >= Duration::from_millis(30)
            || first_pending.is_some_and(|t| now.duration_since(t) >= Duration::from_millis(50))
            || files.len() >= 25;

        if !should_flush {
            return;
        }

        let changed = std::mem::take(&mut *files);
        *self.last_flush.lock() = now;
        *self.first_pending.lock() = None;

        (self.on_change)(changed);
    }

    /// Check if there are pending changes
    #[allow(dead_code)]
    pub fn has_pending_changes(&self) -> bool {
        !self.changed_files.lock().is_empty()
    }
}
