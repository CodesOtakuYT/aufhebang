//! Tokio integration, behind the `tokio` feature.
//!
//! [`Reactor`] registers the connection socket with tokio and folds the
//! per-wakeup cycle — wait, read, dispatch, flush — into a single
//! [`recv`](Reactor::recv) call. It is a convenience, not a dependency: every
//! hook it uses is public on [`Display`], so any other executor, or a
//! hand-written one on tokio, needs only the same four calls described in
//! [the `display` module docs](crate::display#driving-the-connection-yourself).
//!
//! # Why `recv` is ordered the way it is
//!
//! Three things in the cycle are easy to get wrong by hand:
//!
//! - **One read per wakeup.** The compositor can write between the readiness
//!   check and the read, so the read has to be announced with
//!   [`Display::prepare_read`] and the claim held until it happens. Looping
//!   "until the read returns nothing" spins forever, because `AsyncFd` reports
//!   the socket readable for as long as the level is set and every read after
//!   the first returns 0.
//! - **Clearing readiness last.** An event arriving during the read sets the
//!   level again, and clearing before reading would swallow it.
//! - **Flushing.** The compositor cannot answer a request it has not been
//!   sent, so a window whose creation is still buffered would wait forever for a
//!   configure that is never coming.
//!
//! # Using this in a `select!`
//!
//! The future borrows the `Display` mutably for as long as it is awaited, so
//! no other branch of the same `select!` may touch it. Record which branch woke
//! and do that work after:
//!
//! ```no_run
//! # use aufhebung::display::Display;
//! # use aufhebung::tokio::Reactor;
//! # async fn demo(display: &mut Display, reactor: &mut Reactor)
//! #     -> Result<(), Box<dyn std::error::Error>> {
//! # let mut ticker = tokio::time::interval(std::time::Duration::from_millis(180));
//! loop {
//!     let from_compositor = tokio::select! {
//!         biased;
//!         read = reactor.recv(display) => { read?; true }
//!         _ = ticker.tick() => false,
//!     };
//!     if !from_compositor {
//!         // Safe to borrow the display again: the future is gone.
//!         // display.step();
//!     }
//! }
//! # }
//! ```
//!
//! For a loop that has to await several *other* things while the display sits
//! idle, drop the future instead and keep [`Reactor::socket`]: that borrows the
//! `Reactor` alone, so a branch may use the display freely.

use std::io;

use ::tokio::io::unix::AsyncFd;
use wayland_client::{DispatchError, backend::WaylandError};

use crate::display::{Display, Socket};

#[derive(thiserror::Error, Debug)]
pub enum ReactorError {
    #[error("the connection socket cannot be watched by tokio: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Dispatch(#[from] DispatchError),
    #[error(transparent)]
    Wayland(#[from] WaylandError),
}

/// A tokio registration for the compositor's socket.
///
/// Made once from a [`Display`] and then reused, rather than registering the fd
/// again on each wakeup. It borrows nothing: the descriptor belongs to the
/// connection, so the `Display` may be borrowed mutably while this is awaited.
/// The registration goes away with the `Reactor`, and the descriptor itself goes
/// away with the `Display`, which must therefore outlive it.
pub struct Reactor {
    socket: AsyncFd<Socket>,
}

impl Reactor {
    /// Register the connection's socket with the current tokio reactor.
    pub fn new(display: &Display) -> Result<Self, ReactorError> {
        Ok(Self {
            socket: AsyncFd::new(display.socket())?,
        })
    }

    /// Wait for the compositor, read what it sent, run the handlers, and flush,
    /// returning the number of events handled.
    ///
    /// This is the whole per-wakeup cycle in one call, and the order within it
    /// is not something the caller can get wrong. See [the module docs](self)
    /// for what it does and why.
    pub async fn recv(&mut self, display: &mut Display) -> Result<usize, ReactorError> {
        // Readiness is awaited before anything is read. `readable` borrows only
        // the socket, so the display is untouched until the wait is over.
        let mut ready = self.socket.readable().await?;

        // One read, and the claim taken and released around it. A `None` here
        // means a read is already in flight elsewhere, which cannot happen
        // through this type alone — the only way to reach the socket is this
        // method — so the events already in the queue are all there is to
        // dispatch.
        let read = match display.prepare_read() {
            Some(guard) => guard.read(),
            None => Ok(0),
        };

        // Cleared after the read, so an event that arrived during it is not lost.
        ready.clear_ready();

        read?;
        let events = display.dispatch_pending()?;
        display.flush()?;
        Ok(events)
    }

    /// The registration itself, for a loop that needs to await the socket and
    /// something else at the same time.
    ///
    /// Awaiting this borrows the `Reactor` and not the `Display`, so unlike
    /// [`recv`](Self::recv) it composes with branches that mutate the display.
    /// The caller then has to do the read, dispatch and flush, in that order,
    /// which is the arrangement [`recv`](Self::recv) exists to make unnecessary.
    pub fn socket(&mut self) -> &mut AsyncFd<Socket> {
        &mut self.socket
    }
}
