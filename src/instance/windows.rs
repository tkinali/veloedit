//! The Windows transport of the single-window handshake.
//!
//! * A **named mutex** says "a window is running". It is taken by the first process, in
//!   `main`, before anything else starts, so launches that happen at the same moment (Explorer
//!   starts one process per selected file) are told apart reliably: exactly one is the window.
//! * A **named pipe** carries the request: one JSON line, answered with one byte once the UI
//!   thread has handled it. The window creates the pipe a moment after it started (it needs
//!   the async runtime), so a launch that finds the mutex taken keeps trying to open the
//!   pipe for a few seconds.
//! * A window that does not confirm is not taken over (a pipe cannot be unlinked like a
//!   socket file): the launch simply becomes a window of its own, like `--new-window`.
//!
//! Both names carry the user name and, with `VELOEDIT_STATE_DIR`, the state directory, so
//! test/dev runs never talk to the real editor.

use std::{
    collections::VecDeque,
    ffi::c_void,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use iced::{Subscription, futures::Stream};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::windows::named_pipe::ServerOptions,
};

use super::{CONFIRM, Claim, MAX_REQUEST, OK, Request, TIMEOUT, deliver};
use crate::message::Message;

/// What `main` hands to [`serve`] when this process is the window: the pipe's name.
pub struct Listener(String);

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateMutexW(attributes: *const c_void, initial_owner: i32, name: *const u16) -> *mut c_void;
    fn GetLastError() -> u32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_PIPE_BUSY: i32 = 231;

/// FNV-1a: a hash that stays the same across Rust versions (the names must).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

fn scope() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    let state = std::env::var("VELOEDIT_STATE_DIR").unwrap_or_default();
    format!("{:016x}", fnv(&format!("{user}|{state}")))
}

fn pipe_name(scope: &str) -> String {
    format!(r"\\.\pipe\veloedit-{scope}")
}

fn mutex_name(scope: &str) -> Vec<u16> {
    format!(r"Local\veloedit-{scope}").encode_utf16().chain(Some(0)).collect()
}

/// `true` for the first process to ask for this mutex. The mutex lives as long as the
/// process (and goes away with it, so "taken" always means "a window is alive").
fn first_instance(name: &[u16]) -> bool {
    // SAFETY: `name` is NUL-terminated; the handle is deliberately never closed.
    unsafe {
        let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        // Cannot tell (no handle): behave as the window.
        handle.is_null() || GetLastError() != ERROR_ALREADY_EXISTS
    }
}

pub fn claim(paths: &[PathBuf]) -> Claim {
    let scope = scope();
    claim_named(&pipe_name(&scope), &mutex_name(&scope), paths, CONFIRM)
}

pub(super) fn claim_named(pipe: &str, mutex: &[u16], paths: &[PathBuf], confirm: Duration) -> Claim {
    if first_instance(mutex) {
        return Claim::Primary(Some(Listener(pipe.to_owned())));
    }
    // Somebody else is the window; its pipe may still be coming up.
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match OpenOptions::new().read(true).write(true).open(pipe) {
            Ok(file) => return forward(file, paths, confirm),
            Err(e)
                if Instant::now() < deadline
                    && (e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(ERROR_PIPE_BUSY)) =>
            {
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => return Claim::Primary(None),
        }
    }
}

/// Sends the paths and waits for the window's confirmation.
fn forward(mut pipe: File, paths: &[PathBuf], confirm: Duration) -> Claim {
    let sent = serde_json::to_vec(&Request { open: paths.to_vec() }).is_ok_and(|mut line| {
        line.push(b'\n');
        pipe.write_all(&line).and_then(|()| pipe.flush()).is_ok()
    });
    if !sent {
        return Claim::Primary(None);
    }
    // A blocking read has no deadline of its own: it runs on a thread and is waited for.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let _ = tx.send(matches!(pipe.read(&mut byte), Ok(1)) && byte[0] == OK);
    });
    if rx.recv_timeout(confirm) == Ok(true) { Claim::Forwarded } else { Claim::Primary(None) }
}

/// Nothing to remove: the pipe and the mutex go away with the process.
pub fn cleanup() {}

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
        if let Some(Listener(name)) = taken {
            serve_pipe(&name, &mut out).await;
        }
        std::future::pending::<()>().await
    })
}

/// Accepts launches one after the other until the pipe cannot be served any more.
pub(super) async fn serve_pipe(name: &str, out: &mut iced::futures::channel::mpsc::Sender<Message>) {
    // Not ours to serve (another process owns the pipe): this window just runs on its own.
    let Ok(mut server) = ServerOptions::new().first_pipe_instance(true).create(name) else { return };
    // Connections whose launch did not get its confirmation are kept open for a while, so that
    // it keeps waiting (a closed pipe would read as "answered").
    let mut unanswered = VecDeque::new();
    loop {
        if server.connect().await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        // The next instance exists before this one is handled: a second launch never finds the pipe busy.
        let Ok(next) = ServerOptions::new().create(name) else { return };
        let connected = std::mem::replace(&mut server, next);

        let mut reader = BufReader::new(connected.take(MAX_REQUEST));
        let mut line = Vec::new();
        let read = tokio::time::timeout(TIMEOUT, reader.read_until(b'\n', &mut line)).await;
        let mut stream = reader.into_inner().into_inner();
        if !matches!(read, Ok(Ok(n)) if n > 0) {
            continue;
        }
        let Ok(req) = serde_json::from_slice::<Request>(&line) else { continue };
        if deliver(out, req.open).await {
            // Dropping the pipe afterwards is fine: the launch still reads the byte.
            let _ = stream.write_all(&[OK]).await;
            let _ = stream.flush().await;
        } else {
            unanswered.push_back(stream);
            if unanswered.len() > 4 {
                unanswered.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::futures::{StreamExt, channel::mpsc};

    fn unique(tag: &str) -> (String, Vec<u16>) {
        let scope = format!("test-{}-{tag}", std::process::id());
        (pipe_name(&scope), mutex_name(&scope))
    }

    /// Runs the real pipe server on a thread of its own; returns what its UI side receives.
    fn window(pipe: &str) -> mpsc::Receiver<Message> {
        let (mut tx, rx) = mpsc::channel(4);
        let pipe = pipe.to_owned();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(serve_pipe(&pipe, &mut tx));
        });
        rx
    }

    #[test]
    fn names_are_stable_and_distinguish_state_directories() {
        assert_eq!(fnv(""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv("a|x"), fnv("a|y"));
        assert!(pipe_name("ab").starts_with(r"\\.\pipe\veloedit-"));
        assert_eq!(mutex_name("ab").last(), Some(&0));
    }

    #[test]
    fn the_first_launch_is_the_window_and_the_second_forwards_to_it() {
        let (pipe, mutex) = unique("fwd");
        assert!(matches!(claim_named(&pipe, &mutex, &[], CONFIRM), Claim::Primary(Some(_))));

        let mut ui = window(&pipe);
        let paths = vec![PathBuf::from(r"C:\a.php"), PathBuf::from(r"C:\dir with space")];
        let client = std::thread::spawn({
            let (pipe, mutex, paths) = (pipe.clone(), mutex.clone(), paths.clone());
            move || claim_named(&pipe, &mutex, &paths, Duration::from_secs(5))
        });
        // The UI thread: handle the paths, then say it is alive.
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            match ui.next().await {
                Some(Message::External(got)) => assert_eq!(got, paths),
                _ => panic!("the paths come first"),
            }
            match ui.next().await {
                Some(Message::Alive(ack)) => {
                    let _ = ack.take().unwrap().send(());
                }
                _ => panic!("then the liveness question"),
            }
        });
        assert!(matches!(client.join().unwrap(), Claim::Forwarded), "the second launch forwards and leaves");
    }

    #[test]
    fn a_window_that_never_confirms_does_not_swallow_the_launch() {
        let (pipe, mutex) = unique("frozen");
        assert!(matches!(claim_named(&pipe, &mutex, &[], CONFIRM), Claim::Primary(Some(_))));
        let _frozen = window(&pipe); // nobody answers the liveness question
        let started = Instant::now();
        let claim = claim_named(&pipe, &mutex, &[PathBuf::from(r"C:\x.txt")], Duration::from_millis(400));
        assert!(matches!(claim, Claim::Primary(None)), "the launch becomes a window of its own");
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    }
}
