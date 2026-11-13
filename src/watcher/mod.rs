//! Disk watching: `notify` (inotify) turned into a debounced iced `Subscription`.
//!
//! Only the directories the user can currently see are watched (expanded
//! sidebar directories + parents of open files), non-recursively. That keeps
//! the inotify watch count proportional to what is on screen instead of the
//! size of the repository (no thousands of watches for `node_modules`/`target`).

use std::{collections::BTreeSet, path::PathBuf, time::Duration};

use iced::{
    Subscription,
    futures::{Stream, StreamExt, channel::mpsc, SinkExt},
    stream,
};
use notify::{EventKind, RecursiveMode, Watcher};

/// Quiet period after the last event before a batch is delivered. Editors and
/// build tools emit bursts (write + rename + chmod); one batch is enough.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Identity of a watcher: when the set changes, the subscription restarts.
#[derive(Clone, Debug, Hash, PartialEq, Eq, Default)]
pub struct WatchSet {
    dirs: Vec<PathBuf>,
}

impl WatchSet {
    pub fn new(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        let set: BTreeSet<PathBuf> = dirs.into_iter().collect();
        Self { dirs: set.into_iter().collect() }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.dirs.len()
    }
}

/// Batches of changed paths (files or directories), debounced.
pub fn subscription(set: WatchSet) -> Subscription<Vec<PathBuf>> {
    if set.dirs.is_empty() {
        return Subscription::none();
    }
    Subscription::run_with(set, |set| events(set.dirs.clone()))
}

pub fn events(dirs: Vec<PathBuf>) -> impl Stream<Item = Vec<PathBuf>> {
    stream::channel(16, async move |mut out| {
        let (tx, mut rx) = mpsc::unbounded::<notify::Result<notify::Event>>();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.unbounded_send(res);
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                eprintln!("veloedit: file watching unavailable: {e}");
                return std::future::pending().await;
            }
        };
        for dir in &dirs {
            // A directory that vanished meanwhile is simply not watched.
            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
        }

        // `watcher` must stay alive for as long as this loop runs.
        while let Some(first) = rx.next().await {
            let mut changed = BTreeSet::new();
            collect(first, &mut changed);
            while let Ok(Some(more)) = tokio::time::timeout(DEBOUNCE, rx.next()).await {
                collect(more, &mut changed);
            }
            if !changed.is_empty() && out.send(changed.into_iter().collect()).await.is_err() {
                return;
            }
        }
        drop(watcher);
        std::future::pending::<()>().await
    })
}

fn collect(event: notify::Result<notify::Event>, into: &mut BTreeSet<PathBuf>) {
    let Ok(event) = event else { return };
    // Reads (our own loader included) must never feed back into reloads.
    if matches!(event.kind, EventKind::Access(_)) {
        return;
    }
    into.extend(event.paths);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn reports_create_modify_delete_in_a_watched_dir_once_debounced() {
        let dir = std::env::temp_dir().join(format!("veloedit-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let mut stream = Box::pin(events(vec![dir.clone()]));
            // The watcher is set up lazily on the first poll: poll once (nothing
            // is ready yet) so it is registered before we generate events.
            assert!(tokio::time::timeout(Duration::from_millis(300), stream.next()).await.is_err());

            let file = dir.join("a.txt");
            fs::write(&file, "1").unwrap();
            fs::write(&file, "22").unwrap();
            let batch = tokio::time::timeout(Duration::from_secs(3), stream.next())
                .await
                .expect("a batch arrives")
                .unwrap();
            assert!(batch.contains(&file), "{batch:?}");

            fs::remove_file(&file).unwrap();
            let batch = tokio::time::timeout(Duration::from_secs(3), stream.next())
                .await
                .expect("delete is reported")
                .unwrap();
            assert!(batch.contains(&file), "{batch:?}");

            // Pure reads produce no batch.
            fs::write(dir.join("b.txt"), "x").unwrap();
            let _ = tokio::time::timeout(Duration::from_secs(3), stream.next()).await;
            let _ = fs::read(dir.join("b.txt")).unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(500), stream.next()).await.is_err(),
                "reading a file must not look like a change"
            );
        });
        fs::remove_dir_all(dir).ok();
    }
}
