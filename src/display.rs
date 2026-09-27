use kbvm::Keycode;
use wayland_client::{
    ConnectError, Connection, DispatchError, EventQueue, NoopIgnore, QueueHandle,
    protocol::wl_buffer::WlBuffer,
};

use crate::{
    globals::{Globals, GlobalsError},
    seat::SeatId,
    state::{Event, State},
    surface::{Surface, SurfaceError, SurfaceId, SurfaceInfo},
};

pub struct Display {
    conn: Connection,
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
            conn,
            event_queue,
            qh,
            state,
        })
    }

    pub fn add_surface(&mut self, info: SurfaceInfo) -> Result<SurfaceId, SurfaceError> {
        Surface::new(
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

    pub fn add_color(&self, r: u32, g: u32, b: u32, a: u32) -> WlBuffer {
        self.state
            .globals
            .spbm
            .create_u32_rgba_buffer(r, g, b, a, &self.qh, NoopIgnore)
    }

    pub fn dispatch(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.blocking_dispatch(&mut self.state)
    }

    pub fn is_window(&self, id: SurfaceId) -> Option<bool> {
        self.surface(id).map(|s| s.is_window())
    }

    pub fn translate_key(&self, seat: SeatId, key: u32) -> Vec<kbvm::lookup::KeysymProps> {
        let keyboard = self.state.keyboard(seat);
        keyboard
            .lookup_table
            .as_ref()
            .unwrap()
            .lookup(keyboard.group, keyboard.mods, Keycode::from_evdev(key))
            .into_iter()
            .collect()
    }

    pub fn events(&mut self) -> Vec<Event> {
        self.state.events.drain(..).collect()
    }
}
