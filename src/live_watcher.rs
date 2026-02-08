use log::{error, info};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

/// Real-time file watcher with instant updates
pub struct LiveWatcher {
    watcher: RecommendedWatcher,
    storage_path: PathBuf,
    /// Debounce timer to prevent excessive updates
    last_change: Arc<Mutex<Option<Instant>>>,
    last_event: Arc<Mutex<Option<Instant>>>,
    /// Files that changed but haven't been processed
    changed_files: Arc<Mutex<Vec<PathBuf>>>,
    /// Callback for when files change (receives list of changed files)
    on_change: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>,
}

impl LiveWatcher {
    pub fn new(
        storage_path: PathBuf,
        on_change: Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let changed_files = Arc::new(Mutex::new(Vec::new()));
        let changed_files_clone = changed_files.clone();
        let last_change = Arc::new(Mutex::new(None));
        let last_event = Arc::new(Mutex::new(None));
        let last_event_clone = last_event.clone();

        // Create watcher with 100ms poll interval for faster responsiveness
        let config = Config::default().with_poll_interval(Duration::from_millis(100));

        let watcher = RecommendedWatcher::new(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                *last_event_clone.lock() = Some(Instant::now());
                if event.kind.is_modify() || event.kind.is_create() {
                    for path in event.paths {
                        if let Some(path_str) = path.to_str() {
                            // Skip temporary files
                            if path_str.contains(".swp")
                                || path_str.contains(".tmp")
                                || path_str.contains("~")
                                || path_str.contains("4913")
                            {
                                continue;
                            }

                            // Only process JSON files
                            if path.extension().is_some_and(|e| e == "json") {
                                let mut files = changed_files_clone.lock();
                                if !files.contains(&path) {
                                    files.push(path.clone());
                                    // debug!("File changed: {}", path_str);
                                }
                            }
                        }
                    }
                }
            } else if let Err(e) = res {
                error!("File watcher error: {:?}", e);
            }
        }, config)?;

        Ok(Self {
            watcher,
            storage_path,
            last_change,
            last_event,
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

    /// Process pending changes with 80ms debounce for faster live updates
    pub fn process_changes(&self) {
        let mut files = self.changed_files.lock();

        if files.is_empty() {
            return;
        }

        let now = Instant::now();

        // Debounce: only process if no new events in 80ms for faster updates
        if let Some(last_event) = *self.last_event.lock() {
            if now.duration_since(last_event) < Duration::from_millis(80) {
                return;
            }
        }

        // Process the changes
        let changed = std::mem::take(&mut *files);
        *self.last_change.lock() = Some(now);

        (self.on_change)(changed);
    }

    /// Check if there are pending changes
    #[allow(dead_code)]
    pub fn has_pending_changes(&self) -> bool {
        !self.changed_files.lock().is_empty()
    }
}
