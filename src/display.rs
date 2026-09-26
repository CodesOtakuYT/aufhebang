use wayland_client::{
    ConnectError, Connection, DispatchError, EventQueue, NoopIgnore, QueueHandle,
    protocol::wl_buffer::WlBuffer,
};

use crate::{
    globals::{Globals, GlobalsError},
    state::State,
    surface::{Surface, SurfaceError, SurfaceId, SurfaceInfo},
};

pub struct Display {
    conn: Connection,
    event_queue: EventQueue<State>,
    qh: QueueHandle<State>,
    globals: Globals,
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
        let state = State::default();
        Ok(Self {
            conn,
            event_queue,
            qh,
            state,
            globals,
        })
    }

    pub fn add_surface(&mut self, info: SurfaceInfo) -> Result<SurfaceId, SurfaceError> {
        Surface::new(&self.globals, &mut self.state.surfaces, &self.qh, info)
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
        self.globals
            .spbm
            .create_u32_rgba_buffer(r, g, b, a, &self.qh, NoopIgnore)
    }

    pub fn dispatch(&mut self) -> Result<usize, DispatchError> {
        self.event_queue.blocking_dispatch(&mut self.state)
    }

    pub fn is_window(&self, id: SurfaceId) -> Option<bool> {
        self.surface(id).map(|s| s.is_window())
    }
}
