use wayland_client::{
    Connection, Dispatch, NoopIgnore, QueueHandle,
    globals::{BindError, GlobalError, GlobalList, GlobalListHandler},
    protocol::{wl_compositor::WlCompositor, wl_subcompositor::WlSubcompositor},
};
use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        viewporter::client::wp_viewporter::WpViewporter,
    },
    xdg::{
        decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        shell::client::xdg_wm_base::XdgWmBase,
    },
};

use crate::state::State;

pub struct Globals {
    pub compositor: WlCompositor,
    pub viewporter: WpViewporter,
    pub spbm: WpSinglePixelBufferManagerV1,

    pub subcompositor: WlSubcompositor,

    pub wm_base: XdgWmBase,
    pub deco_mgr: ZxdgDecorationManagerV1,
}

#[derive(thiserror::Error, Debug)]
pub enum GlobalsError {
    #[error(transparent)]
    GlobalError(#[from] GlobalError),
    #[error(transparent)]
    BindError(#[from] BindError),
}

struct GlobalData;

impl Globals {
    pub fn new(conn: &Connection, qh: &QueueHandle<State>) -> Result<Self, GlobalsError> {
        let global_list = GlobalList::init(conn, qh)?;

        let compositor = global_list.bind_singleton::<WlCompositor, _, _>(1..=1, qh, NoopIgnore)?;
        let viewporter = global_list.bind_singleton::<WpViewporter, _, _>(1..=1, qh, NoopIgnore)?;
        let spbm = global_list.bind_singleton::<WpSinglePixelBufferManagerV1, _, _>(
            1..=1,
            qh,
            NoopIgnore,
        )?;

        let subcompositor =
            global_list.bind_singleton::<WlSubcompositor, _, _>(1..=1, qh, NoopIgnore)?;

        let wm_base = global_list.bind_singleton::<XdgWmBase, _, _>(1..=1, qh, GlobalData)?;
        let deco_mgr =
            global_list.bind_singleton::<ZxdgDecorationManagerV1, _, _>(1..=1, qh, NoopIgnore)?;

        Ok(Self {
            compositor,
            viewporter,
            spbm,
            subcompositor,
            wm_base,
            deco_mgr,
        })
    }
}

impl GlobalListHandler for State {}

impl Dispatch<XdgWmBase, State> for GlobalData {
    fn event(
        &self,
        state: &mut State,
        proxy: &XdgWmBase,
        event: <XdgWmBase as wayland_client::Proxy>::Event,
        conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_wm_base::Event;
        match event {
            Event::Ping { serial } => proxy.pong(serial),
            _ => {}
        }
    }
}
