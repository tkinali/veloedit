//! One window per user: starting `veloedit some/file` while VeloEdit is already
//! running hands the paths to the running window and exits, instead of opening a
//! second window (two windows would also fight over the one session file).
//!
//! The first instance listens (a Unix socket on Linux, a named pipe on Windows - see
//! `unix.rs` / `windows.rs`); later launches send one JSON line with their paths.
//!
//! A running window that does not answer is not trusted. The first instance confirms
//! every request with one byte - *after its UI thread has handled it* - and a launch that
//! gets no confirmation does not hand its paths to that window any more: it becomes a
//! window itself. (Without this, a window frozen on a dead network mount swallowed every
//! later launch: the paths were "delivered" into a process that never looked at them, and
//! the editor could not be started again.)

use std::{path::PathBuf, time::Duration};

use iced::futures::{SinkExt, channel::oneshot};
use serde::{Deserialize, Serialize};

use crate::message::{Handoff, Message};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub use platform::{Listener, claim, cleanup, serve, subscription};

/// A request is a few paths; anything bigger is not ours.
const MAX_REQUEST: u64 = 1 << 20;
const TIMEOUT: Duration = Duration::from_millis(800);
/// How long a launch waits for the running window to confirm it took the paths. The window
/// itself waits a little less before it gives up on its own UI thread.
const CONFIRM: Duration = Duration::from_millis(1500);
const UI_ANSWER: Duration = Duration::from_millis(1200);
/// What the running window sends back.
const OK: u8 = b'k';

#[derive(Serialize, Deserialize)]
struct Request {
    open: Vec<PathBuf>,
}

pub enum Claim {
    /// We are the window; the listener (if any) receives requests from later launches.
    Primary(Option<Listener>),
    /// A running window took the paths over: exit quietly.
    Forwarded,
}

/// Hands `paths` to the UI thread and waits until it has *handled* them: `true` then, `false`
/// when it did not answer in time (a frozen window).
async fn deliver(out: &mut iced::futures::channel::mpsc::Sender<Message>, paths: Vec<PathBuf>) -> bool {
    let (alive, answer) = oneshot::channel();
    let _ = out.send(Message::External(paths)).await;
    let _ = out.send(Message::Alive(Handoff::new(alive))).await;
    matches!(tokio::time::timeout(UI_ANSWER, answer).await, Ok(Ok(())))
}
