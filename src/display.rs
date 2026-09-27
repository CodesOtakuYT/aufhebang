use std::os::fd::{AsFd, BorrowedFd};

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
    /// except through [`prepare_read`](Self::prepare_read), so the loop stays in
    /// control of when reading happens.
    ///
    /// The fd is valid for as long as the `Display` is, and is closed with it.
    pub fn connection_fd(&self) -> BorrowedFd<'_> {
        self.event_queue.as_fd()
    }

    /// Claim the right to read from the socket.
    ///
    /// This is the part of a non-blocking dispatch that cannot be folded into a
    /// single call. The compositor can write between your readiness check and
    /// your read, so the read has to be announced first and the claim held until
    /// it happens; otherwise two readers can interleave and lose events.
    ///
    /// `None` means a read is already in flight. Dispatch what has been read
    /// with [`dispatch_pending`](Self::dispatch_pending), then try again.
    ///
    /// The guard must reach [`read`](ReadEventsGuard::read) before it is
    /// dropped, or the connection stays locked.
    pub fn prepare_read(&self) -> Option<ReadEventsGuard> {
        self.event_queue.prepare_read()
    }

    /// Run the handlers for everything already read from the socket.
    ///
    /// Returns how many events were handled. This never touches the socket, so
    /// anything the compositor has sent but that has not been read yet is not
    /// seen here — that is what [`prepare_read`](Self::prepare_read) is for.
    pub fn dispatch_pending(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.dispatch_pending(&mut self.state)
    }

    /// Write out the requests queued since the last flush.
    ///
    /// [`dispatch`](Self::dispatch) does this for you. An event loop that waits
    /// on the socket has to call it itself, and it has to call it *before*
    /// waiting: the compositor cannot answer a surface it has not been told
    /// about, so a window whose creation request is still buffered would wait
    /// for a configure that is never coming.
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

    /// Drain every event queued since the last call.
    pub fn events(&mut self) -> Vec<Event> {
        self.state.events.drain(..).collect()
    }
}
