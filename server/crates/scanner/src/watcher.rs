//! Filesystem watcher: debounced events -> targeted rescans.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use reel_core::{Error, Result};
use sqlx::SqlitePool;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, warn};

use crate::{
    parser::{extension, is_subtitle_ext, is_video_ext},
    scan::{Scanner, library_roots},
};

/// Keeps watches alive; dropping it stops watching and the rescan task.
pub struct LibraryWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    task: JoinHandle<()>,
}

impl Drop for LibraryWatcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Only video/subtitle files and extension-less paths (directories) matter.
fn relevant(p: &Path) -> bool {
    match p.file_name().and_then(|n| n.to_str()) {
        Some(n) if n.starts_with('.') => false,
        Some(n) => match extension(n) {
            Some(e) => is_video_ext(&e) || is_subtitle_ext(&e),
            None => true,
        },
        None => false,
    }
}

impl LibraryWatcher {
    /// Watch every path of every library currently in the DB. Restart the
    /// watcher after libraries are added/changed.
    pub async fn start(scanner: Scanner, pool: SqlitePool, debounce: Duration) -> Result<Self> {
        let roots = library_roots(&pool).await?;
        Self::start_paths(scanner, pool, roots, debounce)
    }

    /// Watch explicit paths.
    pub fn start_paths(
        scanner: Scanner,
        pool: SqlitePool,
        paths: Vec<PathBuf>,
        debounce: Duration,
    ) -> Result<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<PathBuf>>();
        let mut debouncer =
            new_debouncer(debounce, None, move |res: DebounceEventResult| match res {
                Ok(events) => {
                    let paths: Vec<PathBuf> = events
                        .into_iter()
                        .filter(|e| !matches!(e.kind, EventKind::Access(_)))
                        .flat_map(|e| e.event.paths)
                        .filter(|p| relevant(p))
                        .collect();
                    if !paths.is_empty() {
                        let _ = tx.send(paths);
                    }
                }
                Err(errs) => warn!("watch errors: {errs:?}"),
            })
            .map_err(|e| Error::Other(e.to_string()))?;
        for p in &paths {
            if let Err(e) = debouncer.watch(p, RecursiveMode::Recursive) {
                warn!(path = %p.display(), "cannot watch: {e}");
            }
        }
        let task = tokio::spawn(async move {
            while let Some(first) = rx.recv().await {
                let mut batch: BTreeSet<PathBuf> = first.into_iter().collect();
                while let Ok(more) = rx.try_recv() {
                    batch.extend(more);
                }
                for p in batch {
                    debug!(path = %p.display(), "watch-triggered rescan");
                    match scanner.scan_path(&pool, &p).await {
                        Ok(_) | Err(Error::NotFound) => {}
                        Err(e) => warn!(path = %p.display(), "rescan failed: {e}"),
                    }
                }
            }
        });
        Ok(Self {
            _debouncer: debouncer,
            task,
        })
    }
}
