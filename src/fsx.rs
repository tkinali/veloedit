//! Filesystem access that cannot take the whole editor down with it.
//!
//! A network mount that stops answering (SFTP / sshfs / NFS through the file manager, a VPN
//! that drops) makes a plain `stat` or `read` block for minutes - or for good. Three rules
//! keep that away from the editor:
//!
//! * The UI thread never asks the filesystem a question without a deadline: [`is_dir`],
//!   [`is_file`] and [`canonical`] run the call on a helper thread and give up
//!   after [`PROBE`] (the answer is `None` - "unknown").
//! * Slow work (reading, writing, listing) runs on a thread of its own, [`off_thread`],
//!   never on the async runtime's few workers - a read that never returns pins its own
//!   thread only, so timers, the watcher and the single-window socket keep running.
//! * [`timeout`] puts a deadline on such work where the caller can go on without the answer.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use iced::futures::{
    channel::oneshot,
    future::{Either, select},
    pin_mut,
};

/// How long the UI thread waits for the answer to one filesystem question.
const PROBE: Duration = Duration::from_millis(300);
/// Questions that have not come back yet. Past this many, nobody is asked any more (the
/// filesystem is evidently stuck) instead of piling up one blocked thread per click.
const MAX_IN_FLIGHT: usize = 8;

/// The questions of the whole process that have not been answered yet.
fn in_flight() -> &'static Arc<AtomicUsize> {
    static COUNT: OnceLock<Arc<AtomicUsize>> = OnceLock::new();
    COUNT.get_or_init(Arc::default)
}

/// Runs `f` on a thread of its own and returns its answer as a future. `None`: the thread
/// could not be started or died. Nothing is held by the async runtime while `f` runs.
pub fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl Future<Output = Option<T>> {
    let (tx, rx) = oneshot::channel();
    let started = std::thread::Builder::new().name("veloedit-io".into()).spawn(move || {
        let _ = tx.send(f());
    });
    async move {
        started.ok()?;
        rx.await.ok()
    }
}

/// Waits for `fut` at most `limit`. `None`: it did not finish in time (what it was doing
/// keeps going if it cannot be stopped, but nobody waits for it).
pub async fn timeout<T>(limit: Duration, fut: impl Future<Output = T>) -> Option<T> {
    let clock = off_thread(move || std::thread::sleep(limit));
    pin_mut!(fut, clock);
    match select(fut, clock).await {
        Either::Left((value, _)) => Some(value),
        Either::Right(_) => None,
    }
}

/// `f` with a deadline, for callers that cannot wait (the UI thread). `None`: no answer.
fn bounded<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    bounded_in(in_flight(), MAX_IN_FLIGHT, limit, f)
}

fn bounded_in<T: Send + 'static>(
    count: &Arc<AtomicUsize>,
    max: usize,
    limit: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    if count.load(Ordering::Relaxed) >= max {
        return None;
    }
    count.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let counter = Arc::clone(count);
    let started = std::thread::Builder::new().name("veloedit-probe".into()).spawn(move || {
        let answer = f();
        counter.fetch_sub(1, Ordering::Relaxed);
        let _ = tx.send(answer);
    });
    if started.is_err() {
        count.fetch_sub(1, Ordering::Relaxed);
        return None;
    }
    rx.recv_timeout(limit).ok()
}

/// `path.is_dir()`, or `None` when the filesystem does not answer in time.
pub fn is_dir(path: &Path) -> Option<bool> {
    let path = path.to_path_buf();
    bounded(PROBE, move || path.is_dir())
}

/// `path.is_file()`, or `None` when the filesystem does not answer in time.
pub fn is_file(path: &Path) -> Option<bool> {
    let path = path.to_path_buf();
    bounded(PROBE, move || path.is_file())
}

/// The canonical form of `path`; when that is not available (the file does not exist yet,
/// or the filesystem does not answer) the absolute form, else the path as it is.
pub fn canonical(path: &Path) -> PathBuf {
    let owned = path.to_path_buf();
    match bounded(PROBE, move || std::fs::canonicalize(&owned)) {
        Some(Ok(real)) => real,
        _ => std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::executor::block_on;

    /// A path that answers like a dead mount: the call never comes back.
    fn hang() {
        std::thread::sleep(Duration::from_secs(5));
    }

    #[test]
    fn probes_answer_quickly_for_ordinary_paths() {
        let dir = std::env::temp_dir();
        assert_eq!(is_dir(&dir), Some(true));
        assert_eq!(is_file(&dir), Some(false));
        assert_eq!(is_dir(&dir.join("veloedit-surely-not-here")), Some(false));
        assert_eq!(canonical(&dir), std::fs::canonicalize(&dir).unwrap());
        // Not there yet: the absolute form, never an error.
        let ghost = canonical(Path::new("veloedit-ghost/file.txt"));
        assert!(ghost.is_absolute() && ghost.ends_with("veloedit-ghost/file.txt"), "{ghost:?}");
    }

    #[test]
    fn a_question_the_filesystem_never_answers_gives_up_after_the_deadline() {
        let t = std::time::Instant::now();
        let got = bounded(Duration::from_millis(100), hang);
        assert!(got.is_none());
        assert!(t.elapsed() < Duration::from_secs(2), "waited {:?}", t.elapsed());
    }

    #[test]
    fn work_that_hangs_pins_its_own_thread_and_the_deadline_still_fires() {
        let t = std::time::Instant::now();
        let hung = block_on(timeout(Duration::from_millis(150), async { off_thread(hang).await }));
        assert!(hung.is_none(), "no answer in time");
        assert!(t.elapsed() < Duration::from_secs(2));
        // Work that finishes is returned as it is.
        let done = block_on(timeout(Duration::from_secs(3), async { off_thread(|| 7).await }));
        assert_eq!(done, Some(Some(7)));
    }

    #[test]
    fn stuck_questions_are_not_piled_up() {
        // A gate of its own: the process-wide one is shared with every other test.
        let gate = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            assert!(bounded_in(&gate, 3, Duration::from_millis(5), hang).is_none());
        }
        assert_eq!(gate.load(Ordering::Relaxed), 3, "three questions are still waiting");
        // Past the limit nobody is asked and nobody waits.
        let t = std::time::Instant::now();
        assert!(bounded_in(&gate, 3, Duration::from_secs(3), || true).is_none());
        assert!(t.elapsed() < Duration::from_millis(100));
        assert_eq!(gate.load(Ordering::Relaxed), 3, "no thread was started");
    }
}
