//! The Unix transport of the single-window handshake: a Unix socket
//! (`$XDG_RUNTIME_DIR/veloedit.sock`, or inside `$VELOEDIT_STATE_DIR` when that is set, so
//! test/dev runs with their own state never talk to the real editor). A stale socket left
//! by a crash is detected (nobody answers) and taken over; so is a window that does not
//! confirm (it is unlinked and replaced). The request is one line of JSON, the end of it is
//! the client shutting down its write side.

use std::{
    io::{Read, Write},
    net::Shutdown,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use iced::{Subscription, futures::Stream};

use super::{CONFIRM, Claim, MAX_REQUEST, OK, Request, TIMEOUT, deliver};
use crate::message::Message;

/// What `main` hands to [`serve`] when this process is the window.
pub type Listener = UnixListener;

pub fn socket_path() -> PathBuf {
    let wanted = if let Some(dir) = std::env::var_os("VELOEDIT_STATE_DIR") {
        PathBuf::from(dir).join("veloedit.sock")
    } else if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("veloedit.sock")
    } else {
        // No runtime dir (rare): a per-user name in /tmp.
        let uid = std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m)).unwrap_or(0);
        std::env::temp_dir().join(format!("veloedit-{uid}.sock"))
    };
    short_enough(wanted)
}

/// A Unix socket path must fit `sun_path` (108 bytes on Linux). Longer ones (deep
/// state directories) get a short stand-in in the temp dir, still unique per path.
fn short_enough(path: PathBuf) -> PathBuf {
    use std::hash::{Hash, Hasher};
    if path.as_os_str().len() < 100 {
        return path;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::env::temp_dir().join(format!("veloedit-{:016x}.sock", h.finish()))
}

pub fn claim(paths: &[PathBuf]) -> Claim {
    claim_at(&socket_path(), paths)
}

pub fn claim_at(socket: &Path, paths: &[PathBuf]) -> Claim {
    claim_waiting(socket, paths, CONFIRM)
}

fn claim_waiting(socket: &Path, paths: &[PathBuf], confirm: Duration) -> Claim {
    if let Some(dir) = socket.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    for _ in 0..3 {
        match UnixStream::connect(socket) {
            Ok(mut stream) => {
                let _ = stream.set_write_timeout(Some(TIMEOUT));
                let req = Request { open: paths.to_vec() };
                let sent = serde_json::to_vec(&req).is_ok_and(|mut line| {
                    line.push(b'\n');
                    stream.write_all(&line).and_then(|()| stream.flush()).is_ok()
                });
                if sent && taken_over_by_the_window(&mut stream, confirm) {
                    return Claim::Forwarded;
                }
                // Connected but the other side is not listening any more, or does not
                // react (a frozen window): take over.
                let _ = std::fs::remove_file(socket);
            }
            Err(_) => {
                // Nobody there (or a stale file): become the listener.
                let _ = std::fs::remove_file(socket);
                match UnixListener::bind(socket) {
                    Ok(listener) => {
                        let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600));
                        remember_ownership(socket);
                        return Claim::Primary(Some(listener));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {} // lost a race: connect again
                    Err(_) => break,
                }
            }
        }
    }
    Claim::Primary(None) // no socket possible: just run standalone
}

/// Did the running window take the request over? It says so with one byte once its UI has
/// handled the paths. A window of an older version only closes the connection (no byte, but
/// an answer all the same); a window that says nothing at all is frozen.
fn taken_over_by_the_window(stream: &mut UnixStream, confirm: Duration) -> bool {
    let _ = stream.shutdown(Shutdown::Write); // the request is complete
    let _ = stream.set_read_timeout(Some(confirm));
    let mut byte = [0u8; 1];
    match stream.read(&mut byte) {
        Ok(1) => byte[0] == OK,
        Ok(_) => true, // closed without a word: an older window, alive
        Err(_) => false,
    }
}

/// The socket files this process created, as `(path, device, inode)`.
static OWNED: Mutex<Vec<(PathBuf, u64, u64)>> = Mutex::new(Vec::new());

fn remember_ownership(socket: &Path) {
    if let (Ok(meta), Ok(mut owned)) = (std::fs::metadata(socket), OWNED.lock()) {
        owned.retain(|(p, ..)| p != socket);
        owned.push((socket.to_path_buf(), meta.dev(), meta.ino()));
    }
}

/// Removes the socket file - but only if it is still ours: a frozen window that wakes up
/// later must not delete the socket of the window that replaced it.
pub fn cleanup() {
    cleanup_at(&socket_path());
}

fn cleanup_at(socket: &Path) {
    let ours = OWNED.lock().ok().and_then(|owned| owned.iter().find(|(p, ..)| p == socket).map(|&(_, dev, ino)| (dev, ino)));
    let now = std::fs::metadata(socket).ok().map(|m| (m.dev(), m.ino()));
    if ours.is_some() && ours == now {
        let _ = std::fs::remove_file(socket);
    }
}

// ------------------------------------------------------------------- server

static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// Hands the listener to the subscription (called once, from `main`).
pub fn serve(listener: Listener) {
    if let Ok(mut slot) = LISTENER.lock() {
        *slot = Some(listener);
    }
}

pub fn subscription() -> Subscription<Message> {
    Subscription::run(requests)
}

fn requests() -> impl Stream<Item = Message> {
    iced::stream::channel(4, async move |mut out| {
        let taken = LISTENER.lock().ok().and_then(|mut slot| slot.take());
        let Some(std_listener) = taken else {
            return std::future::pending::<()>().await;
        };
        let _ = std_listener.set_nonblocking(true);
        let Ok(listener) = tokio::net::UnixListener::from_std(std_listener) else {
            return std::future::pending::<()>().await;
        };
        // Connections whose launch did not get its confirmation are kept open for a while,
        // so that it keeps waiting (a closed connection would read as "answered").
        let mut unanswered = std::collections::VecDeque::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let mut line = Vec::new();
            let mut limited = tokio::io::AsyncReadExt::take(stream, MAX_REQUEST);
            let read = tokio::time::timeout(TIMEOUT, tokio::io::AsyncReadExt::read_to_end(&mut limited, &mut line)).await;
            if read.is_err() {
                continue;
            }
            let mut stream = limited.into_inner();
            let Ok(req) = serde_json::from_slice::<Request>(&line) else { continue };
            // The confirmation comes from the UI thread itself, after it handled the paths.
            if deliver(&mut out, req.open).await {
                let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &[OK]).await;
            } else {
                unanswered.push_back(stream);
                if unanswered.len() > 4 {
                    unanswered.pop_front();
                }
            }
        }
    })
}

/// Test helper: reads one request from a std listener.
#[cfg(test)]
fn read_request(listener: &UnixListener) -> Vec<PathBuf> {
    use std::io::Read;
    let (stream, _) = listener.accept().unwrap();
    let mut buf = Vec::new();
    stream.take(MAX_REQUEST).read_to_end(&mut buf).unwrap();
    serde_json::from_slice::<Request>(&buf).unwrap().open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("veloedit-inst-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_launch_listens_second_forwards_and_exits() {
        let d = dir("fwd");
        let sock = d.join("a.sock");
        let Claim::Primary(Some(listener)) = claim_at(&sock, &[]) else { panic!("first launch is the window") };
        assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600, "private socket");

        let paths = vec![PathBuf::from("/tmp/a.php"), PathBuf::from("/tmp/dir with space")];
        let handle = std::thread::spawn({
            let (sock, paths) = (sock.clone(), paths.clone());
            move || matches!(claim_at(&sock, &paths), Claim::Forwarded)
        });
        assert_eq!(read_request(&listener), paths);
        assert!(handle.join().unwrap(), "the second launch forwards and leaves");
        std::fs::remove_dir_all(d).ok();
    }

    /// A "window" that takes the request, does nothing with it and never answers or hangs up
    /// - what a window frozen on a dead network mount looks like from outside.
    fn frozen_window(listener: UnixListener) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let _held = {
                let (stream, _) = listener.accept().unwrap();
                let mut buf = Vec::new();
                (&stream).take(MAX_REQUEST).read_to_end(&mut buf).unwrap();
                stream
            };
            std::thread::sleep(Duration::from_secs(3));
        })
    }

    #[test]
    fn a_frozen_window_does_not_swallow_the_next_launch() {
        let d = dir("frozen");
        let sock = d.join("f.sock");
        let Claim::Primary(Some(old)) = claim_at(&sock, &[]) else { panic!("first launch is the window") };
        let frozen = frozen_window(old);

        // The second launch gets no confirmation: it does not leave, it becomes the window.
        let started = std::time::Instant::now();
        let paths = vec![PathBuf::from("/tmp/a.php")];
        let Claim::Primary(Some(listener)) = claim_waiting(&sock, &paths, Duration::from_millis(300)) else {
            panic!("a window that never confirms must be replaced")
        };
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());

        // ... and later launches reach the new window.
        let handle = std::thread::spawn({
            let sock = sock.clone();
            move || matches!(claim_waiting(&sock, &[PathBuf::from("/tmp/b.php")], Duration::from_millis(300)), Claim::Forwarded)
        });
        assert_eq!(read_request(&listener), vec![PathBuf::from("/tmp/b.php")]);
        assert!(handle.join().unwrap());
        drop(frozen);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_window_that_confirms_takes_the_paths_and_the_launch_leaves() {
        let d = dir("confirm");
        let sock = d.join("c.sock");
        let Claim::Primary(Some(window)) = claim_at(&sock, &[]) else { panic!() };
        let server = std::thread::spawn(move || {
            let (mut stream, _) = window.accept().unwrap();
            let mut buf = Vec::new();
            (&stream).take(MAX_REQUEST).read_to_end(&mut buf).unwrap();
            stream.write_all(&[OK]).unwrap();
            serde_json::from_slice::<Request>(&buf).unwrap().open
        });
        let paths = vec![PathBuf::from("/tmp/x.php")];
        assert!(matches!(claim_waiting(&sock, &paths, Duration::from_secs(3)), Claim::Forwarded));
        assert_eq!(server.join().unwrap(), paths);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_window_that_woke_up_late_leaves_the_socket_of_its_replacement_alone() {
        let d = dir("owner");
        let sock = d.join("o.sock");
        let Claim::Primary(Some(first)) = claim_at(&sock, &[]) else { panic!() };
        // The replacement unlinks the file and binds its own.
        std::fs::remove_file(&sock).unwrap();
        let second = UnixListener::bind(&sock).unwrap();
        remember_ownership(&sock);
        drop(first);
        // Nothing records the *first* window's file any more: its cleanup must not remove this one.
        OWNED.lock().unwrap().retain(|(p, ..)| p != &sock);
        cleanup_at(&sock);
        assert!(sock.exists(), "somebody else's socket is not removed");
        remember_ownership(&sock);
        cleanup_at(&sock);
        assert!(!sock.exists(), "our own is");
        drop(second);
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn a_stale_socket_from_a_crash_is_taken_over() {
        let d = dir("stale");
        let sock = d.join("b.sock");
        drop(UnixListener::bind(&sock).unwrap()); // the file stays, nobody listens
        assert!(sock.exists());
        assert!(matches!(claim_at(&sock, &[]), Claim::Primary(Some(_))));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn long_paths_get_a_short_unique_stand_in() {
        let long = PathBuf::from(format!("/{}/veloedit.sock", "d".repeat(150)));
        let short = short_enough(long.clone());
        assert!(short.as_os_str().len() < 100 && short != long);
        assert_eq!(short, short_enough(long), "stable for the same path");
        assert_ne!(short, short_enough(PathBuf::from(format!("/{}/veloedit.sock", "e".repeat(150)))));
        let ok = PathBuf::from("/run/user/1000/veloedit.sock");
        assert_eq!(short_enough(ok.clone()), ok);
    }

    #[test]
    fn garbage_in_the_socket_path_degrades_to_standalone() {
        let d = dir("bad");
        // A socket path that can never be created (parent is a file).
        let file = d.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(claim_at(&file.join("c.sock"), &[]), Claim::Primary(None)));
        std::fs::remove_dir_all(d).ok();
    }
}
