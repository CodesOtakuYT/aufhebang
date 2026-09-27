//! Keyboards, and the keymap translation the compositor's input goes through.
//!
//! A seat's keyboard exists only once the compositor has advertised one, and the
//! [`SeatId`] to resolve its keys with is the one its [`SeatEvent`]s carry. See
//! [the `state` module docs](crate::state#seats).

use std::os::fd::AsFd;

use kbvm::{
    GroupIndex, ModifierMask,
    lookup::LookupTable,
    xkb::{Keymap, diagnostic::WriteToLog},
};
use slotmap::new_key_type;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_keyboard::WlKeyboard,
        wl_seat::{self, WlSeat},
    },
};

use crate::{
    mmap::Mmap,
    state::{Event, SeatEvent, State},
    surface::SurfaceId,
};

pub(crate) struct Keyboard {
    keymap: Option<Keymap>,
    pub(crate) lookup_table: Option<LookupTable>,
    focused_surface: Option<SurfaceId>,
    pub(crate) group: GroupIndex,
    pub(crate) mods: ModifierMask,
}

new_key_type! {
    pub struct SeatId;
}

/// A seat and, once the compositor has advertised one, its keyboard.
///
/// Reachable only through the [`SeatId`] that a
/// [`SeatEvent`](crate::state::SeatEvent) carries, and the ids themselves only
/// from those events.
pub(crate) struct Seat {
    // Never read, but held so the proxy outlives the seat's events — the same
    // reasoning as the keyboard proxy below. `pub` used to hide this from
    // dead_code.
    #[allow(dead_code)]
    pub(crate) seat: WlSeat,
    pub(crate) keyboard: Option<Keyboard>,
}

impl Dispatch<WlSeat, State> for SeatId {
    fn event(
        &self,
        state: &mut State,
        proxy: &WlSeat,
        event: <WlSeat as Proxy>::Event,
        _conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        // Any other seat event is not interesting, and must not be allowed to
        // run the body below a second time.
        let wl_seat::Event::Capabilities { capabilities } = event else {
            return;
        };

        // Creating a second keyboard for the same seat would leak the first one
        // and throw away its keymap and focus, so only ever do this once.
        if !capabilities.contains(wl_seat::Capability::Keyboard) {
            return;
        }
        let Some(seat) = state.seat_mut(*self) else {
            return;
        };
        if seat.keyboard.is_some() {
            return;
        }

        seat.keyboard = Some(Keyboard {
            keymap: Default::default(),
            lookup_table: Default::default(),
            focused_surface: Default::default(),
            group: Default::default(),
            mods: Default::default(),
        });

        // The proxy is deliberately not kept: nothing needs to send requests on
        // it, and dropping a client-side proxy sends no request, so the
        // compositor keeps the keyboard alive.
        let _ = proxy.get_keyboard(qh, *self);
    }
}

impl Dispatch<WlKeyboard, State> for SeatId {
    fn event(
        &self,
        state: &mut State,
        _proxy: &WlKeyboard,
        event: <WlKeyboard as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        match event {
            wayland_client::protocol::wl_keyboard::Event::Keymap {
                format: _,
                fd,
                size,
            } => {
                // The mapping outlives the fd, so `fd` is closed as soon as this
                // scope ends.
                let mmap = Mmap::new(fd.as_fd(), size as usize).unwrap();
                let keymap = state
                    .xkb_ctx
                    .keymap_from_bytes(WriteToLog, None, mmap.as_slice())
                    .unwrap();
                let lookup_table = keymap.to_builder().build_lookup_table();
                let Some(keyboard) = state.keyboard_mut(*self) else {
                    return;
                };
                keyboard.keymap = Some(keymap);
                keyboard.lookup_table = Some(lookup_table);
            }
            wayland_client::protocol::wl_keyboard::Event::Enter { surface, .. } => {
                let surface_id = surface.data::<SurfaceId>().unwrap();
                let Some(keyboard) = state.keyboard_mut(*self) else {
                    return;
                };
                keyboard.focused_surface = Some(*surface_id);
            }
            wayland_client::protocol::wl_keyboard::Event::Leave { .. } => {
                let Some(keyboard) = state.keyboard_mut(*self) else {
                    return;
                };
                keyboard.focused_surface = None;
            }
            wayland_client::protocol::wl_keyboard::Event::Key {
                time,
                key,
                state: key_state,
                ..
            } => {
                let pressed = key_state == wayland_client::protocol::wl_keyboard::KeyState::Pressed;
                let Some(keyboard) = state.keyboard(*self) else {
                    return;
                };
                // Keys can only be delivered while a surface has focus, but a
                // release can still race a `leave`, so report what we know.
                let Some(surface) = keyboard.focused_surface else {
                    return;
                };
                state.events.push_back(Event::SeatEvent {
                    id: *self,
                    event: SeatEvent::Key {
                        surface,
                        time,
                        key,
                        pressed,
                        group: keyboard.group,
                        mods: keyboard.mods,
                    },
                });
            }
            wayland_client::protocol::wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let Some(keyboard) = state.keyboard_mut(*self) else {
                    return;
                };
                keyboard.group = GroupIndex(group);
                keyboard.mods = ModifierMask(mods_depressed | mods_latched | mods_locked);
            }
            wayland_client::protocol::wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.events.push_back(Event::SeatEvent {
                    id: *self,
                    event: SeatEvent::RepeatInfo { rate, delay },
                });
            }
            _ => {}
        }
    }
}
