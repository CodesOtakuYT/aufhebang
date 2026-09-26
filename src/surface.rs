use slotmap::{SlotMap, new_key_type};
use wayland_client::{
    Dispatch, NoopIgnore, QueueHandle,
    protocol::{wl_buffer::WlBuffer, wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};
use wayland_protocols::{
    wp::viewporter::client::wp_viewport::WpViewport,
    xdg::{
        decoration::zv1::client::zxdg_toplevel_decoration_v1::{Mode, ZxdgToplevelDecorationV1},
        shell::client::{xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel},
    },
};

use crate::{globals::Globals, state::State};

new_key_type! {
    pub struct SurfaceId;
}

pub enum SurfaceRole {
    Window { title: String },
    Subsurface { parent: SurfaceId, x: i32, y: i32 },
    None,
}

enum SurfaceRoleObject {
    Window {
        xdg_surface: XdgSurface,
        toplevel: XdgToplevel,
        deco: ZxdgToplevelDecorationV1,
        should_close: bool,
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
    buffer: Option<WlBuffer>,
    role: SurfaceRoleObject,
}

#[derive(thiserror::Error, Debug)]
pub enum SurfaceError {}

pub struct SurfaceInfo {
    pub width: i32,
    pub height: i32,
    pub buffer: Option<WlBuffer>,
    pub role: SurfaceRole,
}

impl Surface {
    pub fn new(
        globals: &Globals,
        surfaces: &mut SlotMap<SurfaceId, Surface>,
        qh: &QueueHandle<State>,
        info: SurfaceInfo,
    ) -> Result<SurfaceId, SurfaceError> {
        let subsurface_parent = match info.role {
            SurfaceRole::Subsurface { parent, .. } => {
                let surface = surfaces.get(parent).map(|s| s.surface.clone());
                surface
            }
            _ => None,
        };

        surfaces.try_insert_with_key(|id: SurfaceId| {
            let surface = globals.compositor.create_surface(qh, id);
            let viewport = globals.viewporter.get_viewport(&surface, qh, NoopIgnore);

            let role = match info.role {
                SurfaceRole::Window { title } => {
                    let xdg_surface = globals.wm_base.get_xdg_surface(&surface, qh, id);
                    let toplevel = xdg_surface.get_toplevel(qh, id);
                    toplevel.set_title(title);
                    let deco = globals
                        .deco_mgr
                        .get_toplevel_decoration(&toplevel, qh, NoopIgnore);
                    deco.set_mode(Mode::ServerSide);
                    SurfaceRoleObject::Window {
                        xdg_surface,
                        toplevel,
                        deco,
                        should_close: false,
                    }
                }
                SurfaceRole::Subsurface { parent, x, y } => {
                    let subsurface = globals.subcompositor.get_subsurface(
                        &surface,
                        &subsurface_parent.unwrap(),
                        qh,
                        NoopIgnore,
                    );
                    subsurface.set_position(x, y);
                    viewport.set_destination(info.width, info.height);
                    surface.attach(info.buffer.as_ref(), 0, 0);
                    SurfaceRoleObject::Subsurface { subsurface }
                }
                SurfaceRole::None => SurfaceRoleObject::None,
            };

            surface.commit();

            Ok::<_, SurfaceError>(Self {
                surface,
                role,
                viewport,
                buffer: info.buffer,
                width: info.width,
                height: info.height,
            })
        })
    }

    pub fn should_close(&self) -> bool {
        match &self.role {
            SurfaceRoleObject::Window {
                xdg_surface,
                toplevel,
                deco,
                should_close,
            } => *should_close,
            _ => false,
        }
    }

    pub fn is_window(&self) -> bool {
        match &self.role {
            SurfaceRoleObject::Window {
                xdg_surface,
                toplevel,
                deco,
                should_close,
            } => true,
            _ => false,
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        match &self.role {
            SurfaceRoleObject::Window {
                xdg_surface,
                toplevel,
                deco,
                should_close,
            } => {
                deco.destroy();
                toplevel.destroy();
                xdg_surface.destroy();
            }
            SurfaceRoleObject::Subsurface { subsurface } => {
                subsurface.destroy();
            }
            SurfaceRoleObject::None => {}
        }
        self.viewport.destroy();
        self.surface.destroy();
    }
}

impl Dispatch<WlSurface, State> for SurfaceId {
    fn event(
        &self,
        state: &mut State,
        proxy: &WlSurface,
        event: <WlSurface as wayland_client::Proxy>::Event,
        conn: &wayland_client::Connection,
        qh: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<XdgSurface, State> for SurfaceId {
    fn event(
        &self,
        state: &mut State,
        proxy: &XdgSurface,
        event: <XdgSurface as wayland_client::Proxy>::Event,
        conn: &wayland_client::Connection,
        qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_surface::Event;

        if let Some(surface) = state.surfaces.get(*self) {
            match event {
                Event::Configure { serial } => {
                    proxy.ack_configure(serial);
                    surface.surface.attach(surface.buffer.as_ref(), 0, 0);
                    surface
                        .viewport
                        .set_destination(surface.width, surface.height);
                    surface.surface.commit();
                }
                _ => todo!(),
            }
        }
    }
}

impl Dispatch<XdgToplevel, State> for SurfaceId {
    fn event(
        &self,
        state: &mut State,
        proxy: &XdgToplevel,
        event: <XdgToplevel as wayland_client::Proxy>::Event,
        conn: &wayland_client::Connection,
        qh: &QueueHandle<State>,
    ) {
        use wayland_protocols::xdg::shell::client::xdg_toplevel::Event;
        if let Some(surface) = state.surfaces.get_mut(*self) {
            match event {
                Event::Configure {
                    width,
                    height,
                    states,
                } => {
                    if width != 0 {
                        surface.width = width;
                    }
                    if height != 0 {
                        surface.height = height;
                    }
                    match &surface.role {
                        SurfaceRoleObject::Window {
                            xdg_surface,
                            toplevel,
                            deco,
                            should_close,
                        } => xdg_surface.set_window_geometry(0, 0, surface.width, surface.height),
                        _ => unreachable!(),
                    }
                }
                Event::Close => match &mut surface.role {
                    SurfaceRoleObject::Window {
                        xdg_surface,
                        toplevel,
                        deco,
                        should_close,
                    } => *should_close = true,
                    _ => unreachable!(),
                },
                _ => {}
            }
        }
    }
}
