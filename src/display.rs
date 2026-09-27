//! The compositor connection: [`Display`], the surfaces it owns, and the
//! events collected from them.
//!
//! # Driving the connection yourself
//!
//! [`Display::dispatch`] is a complete blocking event loop in one call. An
//! application that would rather wait on the socket alongside its own work
//! uses the four pieces it is made of, in this order:
//!
//! 1. wait until the socket is readable, through [`Display::connection_fd`] or
//!    [`Display::socket`];
//! 2. [`Display::prepare_read`], then read through the guard it returns;
//! 3. [`Display::dispatch_pending`] for everything that was read;
//! 4. [`Display::flush`] to write out the requests this pass queued.
//!
//! The order carries two constraints that are easy to satisfy by accident.
//!
//! **The read has to be claimed.** The compositor can write between the
//! readiness check and the read, so the read is announced first and the claim
//! held until it happens; two readers interleaving lose events. A `None` guard
//! means a read is already in flight — dispatch what has been read, then try
//! again — and the guard must reach [`ReadEventsGuard::read`] before it is
//! dropped, or the connection stays locked.
//!
//! **The flush has to come before the wait.** The compositor cannot answer a
//! surface it has not been told about, so a window whose creation request is
//! still buffered would wait for a configure that is never coming.

#![cfg_attr(
    feature = "tokio",
    doc = "With the `tokio` feature, [`Reactor::recv`](crate::tokio::Reactor::recv) \
           does all four in the right order."
)]

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

use kbvm::Keycode;
use wayland_client::{
    ConnectError, Connection, DispatchError, EventQueue, NoopIgnore, QueueHandle,
    backend::{ReadEventsGuard, WaylandError},
    protocol::wl_buffer::WlBuffer,
};

use crate::{
    color::Color,
    globals::{Globals, GlobalsError},
    seat::SeatId,
    state::{Event, State},
    surface::{Surface, SurfaceId, SurfaceInfo},
};

/// An owned, copyable handle to the connection's socket.
///
/// The descriptor belongs to the connection, not to the `Display`, so this does
/// not borrow it — but it is only valid while that `Display` is alive, and is
/// closed with it. Meant for readiness APIs that need to own what they poll,
/// such as `tokio`'s `AsyncFd`, which cannot hold a [`BorrowedFd`].
///
/// ```
/// # use aufhebung::display::Display;
/// # fn demo(display: &Display) -> Result<(), tokio::io::Error> {
/// let socket = tokio::io::unix::AsyncFd::new(display.socket())?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Socket {
    fd: RawFd,
}

// SAFETY: the descriptor is owned by the connection and is only ever read, so
// sharing a copy of the handle cannot cause two threads to write to it.
unsafe impl Send for Socket {}
unsafe impl Sync for Socket {}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: `fd` is a live descriptor owned by the connection, and the
        // borrow is tied to `&self`, so it cannot outlive the handle.
        unsafe { BorrowedFd::borrow_raw(self.fd) }
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

pub struct Display {
    event_queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
}

#[derive(thiserror::Error, Debug)]
pub enum DisplayError {
    #[error(transparent)]
    ConnectError(#[from] ConnectError),
    #[error(transparent)]
    GlobalsError(#[from] GlobalsError),
}

impl Display {
    pub fn new() -> Result<Self, DisplayError> {
        let conn = unsafe { Connection::connect_to_env() }?;
        // The queue holds its own handle to the connection, so the local one can
        // be dropped here.
        let event_queue = conn.new_event_queue();
        let qh = event_queue.handle();
        let globals = Globals::new(&conn, &qh)?;
        let state = State {
            globals,
            surfaces: Default::default(),
            xkb_ctx: Default::default(),
            events: Default::default(),
        };
        Ok(Self {
            event_queue,
            qh,
            state,
        })
    }

    /// Create a surface with the given role.
    ///
    /// Returns `None` if the role names a subsurface parent that is not live.
    pub fn add_surface(&mut self, info: SurfaceInfo) -> Option<SurfaceId> {
        Surface::insert(
            &self.state.globals,
            &mut self.state.surfaces,
            &self.qh,
            info,
        )
    }

    /// Remove a surface, sending the `destroy` requests for its protocol
    /// objects when the returned [`Surface`] is dropped.
    ///
    /// The surface is returned rather than destroyed here so its details can
    /// still be read — `title`, say. Returns `None` for an id that is not live;
    /// a `SurfaceId` is never reused, so a stale one names no later surface.
    ///
    /// Removing a parent does not remove its children, and does not stop the
    /// compositor from drawing the children that are still live. Remove the
    /// children first, or a sub-surface outlives the window it was placed in.
    pub fn remove_surface(&mut self, id: SurfaceId) -> Option<Surface> {
        self.state.surfaces.remove(id)
    }

    pub fn should_close(&self, id: SurfaceId) -> bool {
        self.surface(id)
            .is_some_and(|surface| surface.should_close())
    }

    pub fn surface(&self, id: SurfaceId) -> Option<&Surface> {
        self.state.surfaces.get(id)
    }

    pub fn surfaces(&self) -> impl Iterator<Item = SurfaceId> {
        self.state.surfaces.keys()
    }

    /// A 1x1 buffer of the given colour, meant to be scaled to a surface's size
    /// by [`Surface::commit`].
    ///
    /// ```no_run
    /// use aufhebung::color::Color;
    ///
    /// # fn demo(display: &aufhebung::display::Display) {
    /// let dark = display.add_color(Color::rgb(0x18, 0x1a, 0x22));
    /// let red = display.add_color(Color::hex(0xff_0000));
    /// # }
    /// ```
    pub fn add_color(&self, color: Color) -> WlBuffer {
        let (r, g, b, a) = color.channels();
        self.state
            .globals
            .spbm
            .create_u32_rgba_buffer(r, g, b, a, &self.qh, NoopIgnore)
    }

    pub fn dispatch(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.blocking_dispatch(&mut self.state)
    }

    /// The socket, for an external event loop to wait on.
    ///
    /// Hand this to whatever readiness API you already have — `tokio`'s
    /// `AsyncFd`, an epoll loop, `poll`, or a thread that blocks on it. The
    /// library has no runtime of its own and never reads from the socket
    /// except through [`prepare_read`](Self::prepare_read), so the loop stays
    /// in control of when reading happens.
    ///
    /// The fd is valid for as long as the `Display` is, and is closed with it.
    pub fn connection_fd(&self) -> BorrowedFd<'_> {
        self.event_queue.as_fd()
    }

    /// The same descriptor as [`connection_fd`](Self::connection_fd), but
    /// detached from the `Display`'s lifetime, for readiness APIs that need to
    /// own what they poll. See [`Socket`].
    pub fn socket(&self) -> Socket {
        Socket {
            fd: self.event_queue.as_fd().as_raw_fd(),
        }
    }

    /// Commit a buffer to a surface by id, scaled to the surface's current size.
    ///
    /// Shorthand for looking the surface up and committing, which most mutation
    /// is. Returns `false` if the id is no longer live, having changed nothing.
    pub fn commit(&self, id: SurfaceId, buffer: &WlBuffer) -> bool {
        self.surface(id).is_some_and(|surface| {
            surface.commit(buffer);
            true
        })
    }

    /// Whether a buffer may be committed to this surface yet.
    ///
    /// `false` only for a toplevel the compositor has not configured yet. Use
    /// this to guard a commit rather than tracking the first
    /// [`SurfaceEvent::Configure`](crate::state::SurfaceEvent::Configure)
    /// yourself. Returns `false` if the id is no longer live.
    pub fn is_configured(&self, id: SurfaceId) -> bool {
        self.surface(id)
            .is_some_and(|surface| surface.is_configured())
    }

    /// Forward to [`Surface::set_title`]. `false` if `id` is not a window.
    pub fn set_title(&mut self, id: SurfaceId, title: &str) -> bool {
        self.state
            .surfaces
            .get_mut(id)
            .is_some_and(|surface| surface.set_title(title))
    }

    /// Forward to [`Surface::set_size_limits`]. `false` if `id` is not a window.
    pub fn set_size_limits(
        &self,
        id: SurfaceId,
        min: Option<(i32, i32)>,
        max: Option<(i32, i32)>,
    ) -> bool {
        self.surface(id)
            .is_some_and(|surface| surface.set_size_limits(min, max))
    }

    /// Claim the right to read from the socket. `None` means a read is already
    /// in flight. See [the module docs](self#driving-the-connection-yourself)
    /// for why the claim has to be held across the read.
    pub fn prepare_read(&self) -> Option<ReadEventsGuard> {
        self.event_queue.prepare_read()
    }

    /// Run the handlers for everything already read from the socket, and return
    /// how many events that was. Never touches the socket.
    pub fn dispatch_pending(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.dispatch_pending(&mut self.state)
    }

    /// Write out the requests queued since the last flush.
    ///
    /// [`dispatch`](Self::dispatch) does this for you. An event loop that waits
    /// on the socket has to call it itself, and *before* waiting — see
    /// [the module docs](self#driving-the-connection-yourself).
    pub fn flush(&self) -> Result<(), WaylandError> {
        self.event_queue.flush()
    }

    pub fn is_window(&self, id: SurfaceId) -> bool {
        self.surface(id).is_some_and(|s| s.is_window())
    }

    /// The keysyms `key` produces on `seat`, most preferred first.
    ///
    /// `None` if the seat exposes no keyboard or has not sent its keymap yet.
    pub fn translate_key(&self, seat: SeatId, key: u32) -> Option<Vec<kbvm::lookup::KeysymProps>> {
        let keyboard = self.state.keyboard(seat)?;
        let lookup_table = keyboard.lookup_table.as_ref()?;
        Some(
            lookup_table
                .lookup(keyboard.group, keyboard.mods, Keycode::from_evdev(key))
                .into_iter()
                .collect(),
        )
    }

    /// The character `key` produces on `seat`, or `None` if it produces none.
    ///
    /// Takes the most preferred keysym from [`translate_key`](Self::translate_key)
    /// and asks kbvm for its character, falling back to a table for keys that
    /// are not characters: the cursor keys, Return, Escape, Tab, and Backspace.
    /// The arrows are a convention of this crate, not a character the keyboard
    /// produced; use [`translate_key`](Self::translate_key) for the raw keysym.
    pub fn translate_char(&self, seat: SeatId, key: u32) -> Option<char> {
        let props = self.translate_key(seat, key)?.into_iter().next()?;
        props.char().or_else(|| non_char_keysym(props.keysym().0))
    }

    /// Drain every event queued since the last call.
    pub fn events(&mut self) -> Vec<Event> {
        self.state.events.drain(..).collect()
    }
}

/// Keysyms that stand for a key rather than for a character, which kbvm's own
/// `char()` has no entry for.
fn non_char_keysym(keysym: u32) -> Option<char> {
    Some(match keysym {
        0xff51 => '\u{2190}',    // Left
        0xff52 => '\u{2191}',    // Up
        0xff53 => '\u{2192}',    // Right
        0xff54 => '\u{2193}',    // Down
        0xff0d | 0xff8d => '\r', // Return, KP_Enter
        0xff1b => '\u{1b}',      // Escape
        0xff08 => '\u{8}',       // Backspace
        0xff09 => '\t',          // Tab
        _ => return None,
    })
}
