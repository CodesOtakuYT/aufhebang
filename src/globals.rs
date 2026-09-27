use slotmap::SlotMap;
use wayland_client::{
    Connection, Dispatch, NoopIgnore, Proxy, QueueHandle,
    globals::{BindError, Global, GlobalError, GlobalList, GlobalListHandler},
    protocol::{
        wl_compositor::WlCompositor, wl_seat::WlSeat, wl_shm::WlShm,
        wl_subcompositor::WlSubcompositor,
    },
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

use crate::{
    seat::{Seat, SeatId},
    state::State,
};

/// The bound singletons, and the seats the compositor advertised.
///
/// Reachable only through [`Display`](crate::display::Display), which owns the
/// [`State`](crate::state::State) holding it.
pub(crate) struct Globals {
    pub(crate) compositor: WlCompositor,
    pub(crate) viewporter: WpViewporter,
    pub(crate) spbm: WpSinglePixelBufferManagerV1,

    /// Where the pixels of a real image come from.
    ///
    /// Bound at version 1, so [`release`](WlShm::release) — a version 2 request —
    /// is not available. Nothing here needs it: the pool outlives the request
    /// that created it on the compositor's side, and the connection going away
    /// is what ends both.
    pub(crate) shm: WlShm,

    /// `None` on a compositor with no sub-surfaces. Only
    /// [`SurfaceRole::Subsurface`](crate::surface::SurfaceRole::Subsurface)
    /// needs it.
    pub(crate) subcompositor: Option<WlSubcompositor>,

    pub(crate) wm_base: XdgWmBase,
    /// `None` on a compositor that does not implement xdg-decoration. Nothing
    /// is lost by its absence: server-side decoration is the protocol default,
    /// and that is all this crate ever asked for.
    pub(crate) deco_mgr: Option<ZxdgDecorationManagerV1>,

    pub(crate) seats: SlotMap<SeatId, Option<Seat>>,
}

#[derive(thiserror::Error, Debug)]
pub enum GlobalsError {
    #[error(transparent)]
    GlobalError(#[from] GlobalError),
    #[error(transparent)]
    BindError(#[from] BindError),
}

pub(crate) struct GlobalData;

impl Globals {
    pub(crate) fn new(conn: &Connection, qh: &QueueHandle<State>) -> Result<Self, GlobalsError> {
        let global_list = GlobalList::init(conn, qh)?;

        let compositor = global_list.bind_singleton::<WlCompositor, _, _>(1..=1, qh, NoopIgnore)?;
        let viewporter = global_list.bind_singleton::<WpViewporter, _, _>(1..=1, qh, NoopIgnore)?;
        let spbm = global_list.bind_singleton::<WpSinglePixelBufferManagerV1, _, _>(
            1..=1,
            qh,
            NoopIgnore,
        )?;
        // A core protocol, so a compositor without it is not one this library can
        // talk to. The `format` events it sends are not collected: every
        // compositor has to support `argb8888`, which is the only format
        // `add_pixels` asks for, so there is nothing to negotiate.
        let shm = global_list.bind_singleton::<WlShm, _, _>(1..=1, qh, NoopIgnore)?;

        let wm_base = global_list.bind_singleton::<XdgWmBase, _, _>(1..=1, qh, GlobalData)?;

        // The extensions a compositor is free not to have. `viewporter` and
        // `spbm` are deliberately not among them: without the viewport a 1x1
        // buffer cannot fill a surface, so a connection missing either could
        // not draw anything at all, and refusing to start beats a window that
        // shows a single pixel.
        let subcompositor = bind_optional::<WlSubcompositor, _>(&global_list, qh, NoopIgnore)?;
        let deco_mgr = bind_optional::<ZxdgDecorationManagerV1, _>(&global_list, qh, NoopIgnore)?;

        let mut seats = SlotMap::<SeatId, Option<Seat>>::default();
        // The global names come back separately, because the id has to be minted
        // by the binding closure and only the registry event knows which global
        // it is for.
        let mut names = Vec::new();
        let bound = global_list.bind_all::<WlSeat, _, _>(1..=1, qh, |global| {
            names.push(global.name);
            seats.insert(None)
        })?;
        for (seat, global_name) in bound.into_iter().zip(names) {
            fill_seat(&mut seats, global_name, seat);
        }

        Ok(Self {
            compositor,
            viewporter,
            spbm,
            shm,
            subcompositor,
            wm_base,
            deco_mgr,
            seats,
        })
    }
}

/// Binds a singleton the compositor may not have, reporting absence as `None`.
///
/// `NotPresent` is the only failure a compositor can cause here: these are
/// version 1 protocols requested at version 1, so there is no lower version to
/// fall short of. Any other error stays an error rather than becoming an absent
/// feature the application only discovers when it asks for one.
fn bind_optional<I, U>(
    global_list: &GlobalList,
    qh: &QueueHandle<State>,
    udata: U,
) -> Result<Option<I>, GlobalsError>
where
    I: Proxy + 'static,
    U: Dispatch<I, State> + Send + Sync + 'static,
{
    match global_list.bind_singleton::<I, _, _>(1..=1, qh, udata) {
        Ok(proxy) => Ok(Some(proxy)),
        Err(BindError::NotPresent(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Records a freshly bound seat in the slot its user data already reserved.
///
/// The id has to be minted by the binding closure, before a proxy exists to ask
/// for one, which is why the slot is reserved empty and filled in here. A proxy
/// whose user data is not a [`SeatId`] is ignored: that would mean a seat bound
/// without going through this crate, which is not a state the rest of it can
/// represent.
fn fill_seat(seats: &mut SlotMap<SeatId, Option<Seat>>, global_name: u32, seat: WlSeat) {
    let Some(id) = seat.data::<SeatId>().copied() else {
        return;
    };
    let Some(slot) = seats.get_mut(id) else {
        return;
    };
    *slot = Some(Seat {
        global_name,
        seat,
        keyboard: None,
        pointer: None,
    });
}

impl GlobalListHandler for State {
    /// Binds a seat the compositor advertises after startup.
    ///
    /// Seats are the one global here that can turn up late: a compositor adds
    /// one when an input device appears, and a client that ignored the registry
    /// event would have no way to reach that device's keyboard at all.
    fn runtime_add_global(
        &mut self,
        globals: &GlobalList,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        global: &Global,
    ) {
        if global.interface != WlSeat::interface().name {
            return;
        }

        // Bound by name, because the registry event is what identified it.
        let id = self.globals.seats.insert(None);
        let Ok(seat) = globals.bind_specific::<WlSeat, State, _>(global.name, 1..=1, qh, id) else {
            self.globals.seats.remove(id);
            return;
        };

        fill_seat(&mut self.globals.seats, global.name, seat);
    }

    /// Forgets a seat whose global the compositor has withdrawn, along with the
    /// keymap and focus it held.
    ///
    /// Nothing is sent on the way out: the global's removal already destroyed
    /// the seat on the compositor's side, and dropping a proxy sends no request.
    /// A [`SeatId`](crate::seat::SeatId) that was already handed out through a
    /// [`SeatEvent`](crate::state::SeatEvent) now names nothing, and
    /// [`translate_key`](crate::display::Display::translate_key) answers `None`
    /// for it rather than failing.
    fn runtime_remove_global(
        &mut self,
        _globals: &GlobalList,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        global: &Global,
    ) {
        if global.interface != WlSeat::interface().name {
            return;
        }

        let withdrawn = self
            .globals
            .seats
            .iter()
            .find(|(_, seat)| {
                seat.as_ref()
                    .is_some_and(|seat| seat.global_name == global.name)
            })
            .map(|(id, _)| id);

        if let Some(id) = withdrawn {
            self.globals.seats.remove(id);
        }
    }
}

impl Dispatch<XdgWmBase, State> for GlobalData {
    fn event(
        &self,
        _state: &mut State,
        proxy: &XdgWmBase,
        event: <XdgWmBase as wayland_client::Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_wm_base::Event;
        if let Event::Ping { serial } = event {
            proxy.pong(serial);
        }
    }
}
