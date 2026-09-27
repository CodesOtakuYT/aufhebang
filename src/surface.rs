//! Surfaces: [`Surface`], the roles one is created with, and the events it
//! reports.
//!
//! # Damage
//!
//! [`Surface::commit`] damages the whole surface, in surface coordinates, on
//! every commit. This is deliberate. Damage is double-buffered state that a
//! commit applies, and a commit that damages nothing tells the compositor there
//! is nothing to repaint — so a surface that only ever swaps between buffers of
//! the same size, at the same position, can keep showing stale pixels. That
//! failure is quiet: the protocol is satisfied, nothing errors, and the surface
//! simply stops updating.
//!
//! The cost is that nothing here can repaint part of a surface, so a consumer
//! animating a small area of a large one redraws all of it. The damage rectangle
//! is not a parameter of `commit` for that reason; it would have to grow into a
//! separate request alongside [`Surface::set_position`].
//!
//! # Subsurface synchronization
//!
//! A sub-surface created with `sync: true` — the protocol default — is
//! *synchronized*: a change to it, be it position, buffer, or damage, is cached
//! until the parent is committed, so every change needs a second commit on the
//! parent to become visible. Committing only the sub-surface does nothing on
//! screen.
//!
//! `sync: false` is *desynchronized*: each commit applies on its own, halving
//! the work per change at the cost of no longer being atomic with a change to
//! the parent, so a frame where both must move together can tear.
//!
//! Prefer `true` unless you have measured otherwise. KWin was observed not to
//! apply a sub-surface's cached position from a desynchronized commit alone, so
//! with `false` and no parent commit the sub-surface stays where it was
//! created.
//!
//! The mode is only read when the sub-surface is first committed, which
//! [`Display::add_surface`](crate::display::Display::add_surface) has already
//! done by the time it returns. That is why it is a creation parameter rather
//! than a setter.

use std::convert::Infallible;

use slotmap::{SlotMap, new_key_type};
use wayland_client::{
    Connection, Dispatch, NoopIgnore, Proxy, QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};
use wayland_protocols::{
    wp::viewporter::client::wp_viewport::WpViewport,
    xdg::{
        decoration::zv1::client::zxdg_toplevel_decoration_v1::{Mode, ZxdgToplevelDecorationV1},
        shell::client::{xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel},
    },
};

use crate::{
    globals::Globals,
    state::{Event, State, SurfaceEvent},
};

new_key_type! {
    pub struct SurfaceId;
}

/// The role to give a new surface.
pub enum SurfaceRole {
    Window {
        title: String,
    },
    Subsurface {
        parent: SurfaceId,
        x: i32,
        y: i32,
        /// Whether the sub-surface waits for its parent to be committed before
        /// its changes show. See
        /// [the module docs](self#subsurface-synchronization) before changing
        /// this.
        sync: bool,
    },
    None,
}

/// Everything needed to create a surface. Buffers are deliberately absent: this
/// library owns the handshake, the consumer owns the pixels.
pub struct SurfaceInfo {
    /// Starting size. A toplevel is resized by the compositor and reports the
    /// result through [`SurfaceEvent::Configure`].
    pub width: i32,
    pub height: i32,
    pub role: SurfaceRole,
}

enum SurfaceRoleObject {
    Window {
        xdg_surface: XdgSurface,
        toplevel: XdgToplevel,
        deco: ZxdgToplevelDecorationV1,
        title: String,
        should_close: bool,
        /// Whether the compositor has sent a configure yet. Until it has, the
        /// surface may not carry a buffer. See [`Surface::is_configured`].
        configured: bool,
    },
    Subsurface {
        subsurface: WlSubsurface,
    },
    None,
}

pub struct Surface {
    surface: WlSurface,
    viewport: WpViewport,
    width: i32,
    height: i32,
    role: SurfaceRoleObject,
}

impl Surface {
    /// Returns `None` if the role names a subsurface parent that is not live.
    pub(crate) fn insert(
        globals: &Globals,
        surfaces: &mut SlotMap<SurfaceId, Surface>,
        qh: &QueueHandle<State>,
        info: SurfaceInfo,
    ) -> Option<SurfaceId> {
        let SurfaceInfo {
            width,
            height,
            role,
        } = info;

        // Resolve the parent before creating anything, so that a dead parent
        // rejects the request instead of panicking halfway through.
        let parent = match &role {
            SurfaceRole::Subsurface { parent, .. } => Some(surfaces.get(*parent)?.surface.clone()),
            _ => None,
        };

        let id = surfaces
            .try_insert_with_key(|id: SurfaceId| {
                let surface = globals.compositor.create_surface(qh, id);
                let viewport = globals.viewporter.get_viewport(&surface, qh, NoopIgnore);

                let role = match role {
                    SurfaceRole::Window { title } => {
                        let xdg_surface = globals.wm_base.get_xdg_surface(&surface, qh, id);
                        let toplevel = xdg_surface.get_toplevel(qh, id);
                        // This revision of wayland-protocols takes string
                        // arguments by value, so keep our own copy to hand back
                        // from `Surface::title`.
                        toplevel.set_title(title.clone());
                        let deco = globals
                            .deco_mgr
                            .get_toplevel_decoration(&toplevel, qh, NoopIgnore);
                        deco.set_mode(Mode::ServerSide);
                        SurfaceRoleObject::Window {
                            xdg_surface,
                            toplevel,
                            deco,
                            title,
                            should_close: false,
                            configured: false,
                        }
                    }
                    SurfaceRole::Subsurface { x, y, sync, .. } => {
                        let subsurface = globals.subcompositor.get_subsurface(
                            &surface,
                            parent
                                .as_ref()
                                .expect("subsurface role always resolves a parent"),
                            qh,
                            NoopIgnore,
                        );
                        subsurface.set_position(x, y);
                        if sync {
                            subsurface.set_sync();
                        } else {
                            subsurface.set_desync();
                        }
                        SurfaceRoleObject::Subsurface { subsurface }
                    }
                    SurfaceRole::None => SurfaceRoleObject::None,
                };

                // The initial commit carries no buffer. For a toplevel this is
                // what makes the compositor send the first configure; for a
                // subsurface it is simply an empty starting point.
                surface.commit();

                // A full slot map panics inside slotmap, so the error type here
                // is never actually produced.
                Ok::<_, Infallible>(Self {
                    surface,
                    role,
                    viewport,
                    width,
                    height,
                })
            })
            .ok()?;

        Some(id)
    }

    /// Attaches `buffer`, scales it to the current size, damages the whole
    /// surface, and commits.
    ///
    /// A toplevel must be committed in response to [`SurfaceEvent::Configure`],
    /// which is emitted only after the configure has been acked; committing one
    /// before its first configure is a protocol error. Subsurfaces get no
    /// configure events, so they may be committed whenever. A single buffer may
    /// back any number of surfaces.
    pub fn commit(&self, buffer: &WlBuffer) {
        self.surface.attach(Some(buffer), 0, 0);
        // Surface coordinates, so the viewport's scaling needs no accounting:
        // `set_destination` below makes the surface exactly `width` x `height`.
        self.surface.damage(0, 0, self.width, self.height);
        self.viewport.set_destination(self.width, self.height);
        self.surface.commit();
    }

    /// The size last set by the compositor, or the size given at creation for a
    /// surface that has not been configured yet.
    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Move a sub-surface, in the parent surface's coordinate system.
    ///
    /// The position is double-buffered, so it does not take effect until a
    /// commit cycle completes: call this before [`commit`](Self::commit) on the
    /// sub-surface, and — while it is in the default synchronized mode — commit
    /// the parent as well, since that is what applies the cached state.
    ///
    /// Negative coordinates are allowed, and a sub-surface is *not* clipped to
    /// the parent's area, so this can place one outside it. Returns `false`,
    /// having changed nothing, for any role other than a sub-surface.
    pub fn set_position(&self, x: i32, y: i32) -> bool {
        match &self.role {
            SurfaceRoleObject::Subsurface { subsurface } => {
                subsurface.set_position(x, y);
                true
            }
            _ => false,
        }
    }

    /// Constrain the sizes the compositor may configure this toplevel to.
    ///
    /// `None` leaves that bound to the compositor, which is what never having
    /// called this means; passing the same size as both bounds is how a window
    /// is made fixed. Both bounds are in window geometry coordinates — the
    /// region `set_window_geometry` describes, which this library anchors at the
    /// top-left corner — and a bound of zero means no opinion rather than no
    /// size.
    ///
    /// Like [`set_position`](Self::set_position) this is double-buffered: it
    /// takes effect on the next [`commit`](Self::commit), so call it before the
    /// commit meant to carry it. The compositor may ignore either bound.
    ///
    /// Returns `false`, having changed nothing, for any role other than a window.
    pub fn set_size_limits(&self, min: Option<(i32, i32)>, max: Option<(i32, i32)>) -> bool {
        let SurfaceRoleObject::Window { toplevel, .. } = &self.role else {
            return false;
        };
        // Zero is the protocol's "no opinion", so an absent bound is a zero.
        let (min_width, min_height) = min.unwrap_or((0, 0));
        let (max_width, max_height) = max.unwrap_or((0, 0));
        toplevel.set_min_size(min_width, min_height);
        toplevel.set_max_size(max_width, max_height);
        true
    }

    /// Whether a buffer may be committed to this surface yet.
    ///
    /// `false` only for a toplevel the compositor has not configured yet, where
    /// attaching a buffer is `xdg_surface.error.unconfigured_buffer`; a timer
    /// or a socket that becomes ready early is the usual way to lose that race.
    /// Surfaces without a toplevel role are always safe to commit.
    pub fn is_configured(&self) -> bool {
        match &self.role {
            SurfaceRoleObject::Window { configured, .. } => *configured,
            _ => true,
        }
    }

    /// The title of a toplevel, or `None` for any other role.
    pub fn title(&self) -> Option<&str> {
        match &self.role {
            SurfaceRoleObject::Window { title, .. } => Some(title),
            _ => None,
        }
    }

    /// Change a toplevel's title. `false`, having changed nothing, for any role
    /// other than a window.
    ///
    /// Unlike [`set_position`](Self::set_position) and
    /// [`set_size_limits`](Self::set_size_limits) this is *not* double-buffered:
    /// the compositor shows the new title as soon as it reads the request, so it
    /// is safe to call as often as the title should change — every score, say.
    /// It takes `&mut self` because [`title`](Self::title) reports the stored
    /// value and should not go stale behind a `&self`.
    pub fn set_title(&mut self, title: &str) -> bool {
        let SurfaceRoleObject::Window {
            toplevel,
            title: current,
            ..
        } = &mut self.role
        else {
            return false;
        };
        current.clear();
        current.push_str(title);
        // This revision of wayland-protocols takes string arguments by value, so
        // the request needs its own copy of the title.
        toplevel.set_title(title.to_owned());
        true
    }

    /// Whether the compositor asked this toplevel to close.
    pub fn should_close(&self) -> bool {
        matches!(
            &self.role,
            SurfaceRoleObject::Window {
                should_close: true,
                ..
            }
        )
    }

    pub fn is_window(&self) -> bool {
        matches!(&self.role, SurfaceRoleObject::Window { .. })
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        match &self.role {
            SurfaceRoleObject::Window {
                xdg_surface,
                toplevel,
                deco,
                ..
            } => {
                deco.destroy();
                toplevel.destroy();
                xdg_surface.destroy();
            }
            SurfaceRoleObject::Subsurface { subsurface } => subsurface.destroy(),
            SurfaceRoleObject::None => {}
        }
        self.viewport.destroy();
        self.surface.destroy();
    }
}

impl Dispatch<WlSurface, State> for SurfaceId {
    fn event(
        &self,
        _state: &mut State,
        _proxy: &WlSurface,
        _event: <WlSurface as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<XdgSurface, State> for SurfaceId {
    fn event(
        &self,
        state: &mut State,
        proxy: &XdgSurface,
        event: <XdgSurface as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_surface::Event as XdgSurfaceEvent;

        // `xdg_surface` defines no event other than `configure`.
        if let XdgSurfaceEvent::Configure { serial } = event {
            // The toplevel configure carrying the new size always precedes this
            // one, so `width`/`height` are already latched. Ack first: the
            // protocol requires the ack to reach the compositor before the commit
            // that answers it, and the consumer only learns about the configure
            // through the event pushed below.
            proxy.ack_configure(serial);

            let Some(surface) = state.surfaces.get(*self) else {
                return;
            };
            let (width, height) = (surface.width, surface.height);
            state.events.push_back(Event::SurfaceEvent {
                id: *self,
                event: SurfaceEvent::Configure { width, height },
            });
        }
    }
}

impl Dispatch<XdgToplevel, State> for SurfaceId {
    fn event(
        &self,
        state: &mut State,
        _proxy: &XdgToplevel,
        event: <XdgToplevel as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_toplevel::Event as XdgToplevelEvent;

        if let Some(surface) = state.surfaces.get_mut(*self) {
            match event {
                XdgToplevelEvent::Configure { width, height, .. } => {
                    if let SurfaceRoleObject::Window { configured, .. } = &mut surface.role {
                        *configured = true;
                    }
                    // Zero means the compositor has no opinion, so keep whatever
                    // size we already had.
                    if width != 0 {
                        surface.width = width;
                    }
                    if height != 0 {
                        surface.height = height;
                    }
                    match &surface.role {
                        SurfaceRoleObject::Window { xdg_surface, .. } => {
                            xdg_surface.set_window_geometry(0, 0, surface.width, surface.height)
                        }
                        _ => unreachable!("an xdg_toplevel is always the window role"),
                    }
                }
                XdgToplevelEvent::Close => {
                    if let SurfaceRoleObject::Window { should_close, .. } = &mut surface.role {
                        *should_close = true;
                    }
                }
                _ => {}
            }
        }
    }
}
