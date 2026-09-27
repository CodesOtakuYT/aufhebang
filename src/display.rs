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
    protocol::{wl_buffer::WlBuffer, wl_shm::Format},
};

use crate::{
    buffer::{Buffer, BufferId},
    color::Color,
    globals::Globals,
    mmap::Mmap,
    seat::SeatId,
    state::{Event, State},
    surface::{Surface, SurfaceId, SurfaceInfo},
};

/// An owned handle to the connection's socket.
///
/// It holds a handle to the connection rather than a bare descriptor number, so
/// the socket cannot be closed and recycled while this is still being polled.
/// `Send` and `Sync` follow from that instead of having to be asserted.
///
/// It does not keep the *display* alive. Everything a [`Display`] owns is gone
/// once it is dropped, so a `Socket` outliving its `Display` finds a valid,
/// readable socket attached to a connection that has no client left on it. Meant
/// for readiness APIs that need to own what they poll, such as `tokio`'s
/// `AsyncFd`, which cannot hold a [`BorrowedFd`].
///
/// ```
/// # use aufhebung::display::Display;
/// # #[cfg(feature = "tokio")]
/// # fn demo(display: &Display) -> Result<(), tokio::io::Error> {
/// let socket = tokio::io::unix::AsyncFd::new(display.socket())?;
/// # Ok(())
/// # }
/// ```
///
/// The example is compiled only with the `tokio` feature, since that is the one
/// place this crate knows the crate is called for.
#[derive(Clone, Debug)]
pub struct Socket {
    conn: Connection,
}

impl AsFd for Socket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.conn.as_fd()
    }
}

impl AsRawFd for Socket {
    fn as_raw_fd(&self) -> RawFd {
        self.conn.as_raw_fd()
    }
}

pub struct Display {
    event_queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    /// Held so [`socket`](Self::socket) can hand out a handle that keeps the
    /// connection alive, rather than a descriptor number whose validity is
    /// tied to a `Display` that may already be gone. Declared last so the queue
    /// and the state it dispatches into are dropped first.
    conn: Connection,
}

/// Re-exported so it stays nameable now that the `globals` module is private:
/// this is the type of [`DisplayError::GlobalsError`].
pub use crate::globals::GlobalsError;

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
        let event_queue = conn.new_event_queue();
        let qh = event_queue.handle();
        let globals = Globals::new(&conn, &qh)?;
        let state = State {
            globals,
            surfaces: Default::default(),
            buffers: Default::default(),
            xkb_ctx: Default::default(),
            events: Default::default(),
        };
        Ok(Self {
            conn,
            event_queue,
            qh,
            state,
        })
    }

    /// Create a surface with the given role.
    ///
    /// Returns `None` when the role cannot be satisfied: a
    /// [`Subsurface`](crate::surface::SurfaceRole::Subsurface) whose parent is
    /// no longer live, or one on a compositor that has no `wl_subcompositor` at
    /// all.
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

    /// A buffer by id, for its size or its own id.
    ///
    /// `None` if the id is not one this connection created. There is no way to
    /// make a `BufferId` that did not come from one of the `add_*` functions,
    /// so in practice this does not fail — it is here so a `BufferId` stored
    /// somewhere of your own can be resolved.
    pub fn buffer(&self, id: BufferId) -> Option<&Buffer> {
        self.state.buffers.get(id)
    }

    /// Every buffer created so far, in creation order.
    ///
    /// Never shrinks: a buffer the compositor may still be reading cannot be
    /// forgotten, and nothing here is destroyed. Iterating this is a way to see
    /// what a program has allocated, not a way to collect it.
    pub fn buffers(&self) -> impl Iterator<Item = BufferId> {
        self.state.buffers.keys()
    }

    /// A 1x1 buffer of the given colour, meant to be scaled to a surface's size
    /// by [`Surface::commit`].
    ///
    /// ```no_run
    /// use aufhebung::color::Color;
    ///
    /// # fn demo(display: &mut aufhebung::display::Display) {
    /// let dark = display.add_color(Color::rgb(0x18, 0x1a, 0x22));
    /// let red = display.add_color(Color::hex(0xff_0000));
    /// # }
    /// ```
    ///
    /// The buffer is 1x1, which is what makes
    /// [`commit`](Self::commit) the right way to place it: a single pixel
    /// scaled to fill a surface.
    pub fn add_color(&mut self, color: Color) -> BufferId {
        let (r, g, b, a) = color.channels();
        let proxy = self
            .state
            .globals
            .spbm
            .create_u32_rgba_buffer(r, g, b, a, &self.qh, NoopIgnore);
        // 1x1 rather than 0x0: a colour buffer is one pixel by construction, and
        // `commit_unscaled` on one of these would place a single pixel at the
        // top-left corner.
        self.insert_buffer(proxy, 1, 1)
    }

    /// Records a buffer against a fresh id and returns it.
    ///
    /// Every buffer in this library is created here, so this is the one place
    /// that hands out a [`BufferId`].
    fn insert_buffer(&mut self, proxy: WlBuffer, width: i32, height: i32) -> BufferId {
        // `insert_with_key` so the buffer is built holding the id it is about to
        // be filed under, rather than a placeholder that has to be patched.
        self.state
            .buffers
            .insert_with_key(|id| Buffer::new(id, proxy, width, height))
    }

    /// A buffer holding real pixels, for an image rather than a flat colour.
    ///
    /// `pixels` is `width * height` values in row-major order, each one pixel
    /// packed as `0xAARRGGBB` in the machine's native byte order — so on a
    /// little-endian machine the bytes in memory are B, G, R, A. Alpha is
    /// pre-multiplied, which is the format's own requirement; [`Color`] is
    /// *not* pre-multiplied, so a picture built out of it has to be multiplied
    /// on the way in. Rows are packed with no padding, so the stride is
    /// `width * 4`.
    ///
    /// A buffer from here is a real image, so
    /// [`commit_unscaled`](Self::commit_unscaled) places it at its own size
    /// rather than stretching it, the way [`commit`](Self::commit) does a colour.
    ///
    /// ```no_run
    /// use aufhebung::display::Display;
    ///
    /// # fn demo(display: &mut Display) {
    /// // A 2x2 image: opaque red, opaque green, opaque blue, opaque white.
    /// let pixels = [0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff];
    /// let Some(buffer) = display.add_pixels(2, 2, &pixels) else {
    ///     return;
    /// };
    /// # }
    /// ```
    ///
    /// # What this is not for
    ///
    /// **The pixels must not be changed once they are committed.** The
    /// compositor may read them at any point after the commit, and says when it
    /// has stopped with a `wl_buffer.release` event — which this library does not
    /// watch for. Writing to a committed buffer's memory is a protocol violation
    /// whose effect is undefined surface contents, and it tends to show up as
    /// occasional garbage rather than as a clean failure.
    ///
    /// Each call also creates a fresh pool and buffer that live until the
    /// connection closes, since nothing here is ever destroyed. So this is for
    /// uploading an image once and committing it as often as you like, not for
    /// a frame loop: a redraw that needs new pixels wants a different design.
    ///
    /// `None` means the image cannot be made into a buffer at all, which is
    /// worth reporting rather than working around: a non-positive `width` or
    /// `height`, fewer than `width * height` values in `pixels`, or a pool larger
    /// than the protocol's own limit. Each of those is a mistake that would
    /// otherwise end the connection rather than the call. A `pixels` slice longer
    /// than needed is fine, and the tail is ignored.
    pub fn add_pixels(&mut self, width: i32, height: i32, pixels: &[u32]) -> Option<BufferId> {
        // Every one of these is checked before anything is allocated or sent,
        // because the failure each describes is a protocol error rather than a
        // refused request.
        if width <= 0 || height <= 0 {
            return None;
        }
        let count = (width as usize).checked_mul(height as usize)?;
        if pixels.len() < count {
            return None;
        }

        let bytes = count.checked_mul(4)?;
        // The pool's size is an `i32` on the wire, and a pool that cannot be
        // described is a pool that cannot be created.
        let Ok(size) = i32::try_from(bytes) else {
            return None;
        };

        // A memory file rather than one on disk. The compositor gets its own
        // descriptor when the request below is queued, so this one is closed
        // again immediately afterwards.
        let Ok(file) = rustix::fs::memfd_create(c"aufhebung", rustix::fs::MemfdFlags::CLOEXEC)
        else {
            return None;
        };
        // Given a size before it is mapped rather than left to be grown by
        // writing: the compositor maps the same file when the request arrives,
        // and a zero-length file has nothing to map.
        if rustix::fs::ftruncate(&file, bytes as u64).is_err() {
            return None;
        }

        let Ok(mut map) = Mmap::writable_shared(file.as_fd(), bytes) else {
            return None;
        };
        // The same `count * 4` bytes on both sides: the caller's pixels, and the
        // mapping. Both lengths were checked above and a `u32` has no padding,
        // so this is a copy of exactly the bytes the format wants, in the
        // machine's own order.
        let src = unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), bytes) };
        map.as_mut_slice()[..bytes].copy_from_slice(src);
        // Unmapped before the request goes out: nothing reads the mapping again,
        // and keeping it would hold the file for nothing.
        drop(map);

        let pool = self
            .state
            .globals
            .shm
            .create_pool(file.as_fd(), size, &self.qh, NoopIgnore);
        drop(file);

        // The size is recorded, so `commit_unscaled` can use it without being
        // told. Every check above has already passed by this point, so `width`
        // and `height` are the same positive values the copy used.
        let proxy = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            Format::Argb8888,
            &self.qh,
            NoopIgnore,
        );
        Some(self.insert_buffer(proxy, width, height))
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

    /// The same descriptor as [`connection_fd`](Self::connection_fd), but held
    /// by a handle that owns the connection behind it, for readiness APIs that
    /// need to own what they poll. See [`Socket`].
    pub fn socket(&self) -> Socket {
        Socket {
            conn: self.conn.clone(),
        }
    }

    /// Commit a buffer to a surface by id, scaled to the surface's current size.
    ///
    /// Shorthand for looking the surface up and committing, which most mutation
    /// is. Returns `false` if either id is not one this connection created,
    /// having changed nothing.
    ///
    /// Committing to a toplevel before the compositor has configured it is a
    /// protocol error, not a failed commit; see
    /// [`is_configured`](Self::is_configured).
    pub fn commit(&self, id: SurfaceId, buffer: BufferId) -> bool {
        let (Some(surface), Some(buffer)) = (self.surface(id), self.buffer(buffer)) else {
            return false;
        };
        surface.commit(buffer);
        true
    }

    /// [`Surface::commit_unscaled`](crate::surface::Surface::commit_unscaled) by
    /// id: attach `buffer` at its own size and damage only that much.
    ///
    /// This is how a buffer from [`add_pixels`](Self::add_pixels) is placed
    /// without being stretched, which is what leaves a colour behind it visible.
    /// The size comes from the buffer rather than from an argument, so it cannot
    /// be wrong. Returns `false` if either id is not one this connection
    /// created, having changed nothing.
    pub fn commit_unscaled(&self, id: SurfaceId, buffer: BufferId) -> bool {
        let (Some(surface), Some(buffer)) = (self.surface(id), self.buffer(buffer)) else {
            return false;
        };
        surface.commit_unscaled(buffer);
        true
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
