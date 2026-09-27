//! Keyboards, and the keymap translation the compositor's input goes through.
//!
//! A seat's keyboard exists only once the compositor has advertised one, and the
//! [`SeatId`] to resolve its keys with is the one its [`SeatEvent`]s carry. See
//! [the `state` module docs](crate::state#seats).
//!
//! Neither the seat nor its keyboard outlives the compositor's own: a seat
//! advertised after startup is bound as it arrives, and one withdrawn takes its
//! keymap and focus with it.

use std::os::fd::AsFd;

use kbvm::{GroupIndex, ModifierMask, lookup::LookupTable, xkb::diagnostic::WriteToLog};
use slotmap::new_key_type;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_keyboard::{KeymapFormat, WlKeyboard},
        wl_seat::{self, WlSeat},
    },
};

use crate::{
    mmap::Mmap,
    state::{Event, SeatEvent, State},
    surface::SurfaceId,
};

pub(crate) struct Keyboard {
    /// The only form of the keymap this crate keeps. A kbvm `Keymap` owns a
    /// parsed xkb tree, and `LookupTable` is a flattened, self-contained copy of
    /// the parts of it that a lookup reads, so there is nothing to keep the
    /// tree alive for and nothing to borrow from it.
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
    /// The name the compositor advertised this seat's global under, kept so
    /// that withdrawing that global can find the seat again.
    pub(crate) global_name: u32,
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
            wayland_client::protocol::wl_keyboard::Event::Keymap { format, fd, size } => {
                // `NoKeymap` means the seat has no mapping at all, and
                // carries no fd worth mapping.
                if format == KeymapFormat::NoKeymap {
                    return;
                }

                // The mapping outlives the fd, so `fd` is closed as soon as this
                // scope ends.
                //
                // A keymap that cannot be mapped or compiled leaves whatever was
                // already installed in place, so one the compositor gets wrong
                // costs the application its input rather than its process. The
                // reason is already on the log: `WriteToLog` is the diagnostic
                // sink for the compile that just failed, and a map that cannot
                // be mapped is not a keymap problem to report here either.
                let Ok(mmap) = Mmap::new(fd.as_fd(), size as usize) else {
                    return;
                };
                let Ok(keymap) = state
                    .xkb_ctx
                    .keymap_from_bytes(WriteToLog, None, mmap.as_slice())
                else {
                    return;
                };
                // The tree itself is dropped here: `build_lookup_table` copies
                // everything a lookup needs out of it.
                let lookup_table = keymap.to_builder().build_lookup_table();
                let Some(keyboard) = state.keyboard_mut(*self) else {
                    return;
                };
                keyboard.lookup_table = Some(lookup_table);
            }
            wayland_client::protocol::wl_keyboard::Event::Enter { surface, .. } => {
                // Every surface in this connection is created with a `SurfaceId`
                // as its user data, so a name that does not carry one is the
                // compositor violating the protocol. Ignoring the event is
                // better than aborting the process over it, and it leaves the
                // previous focus in place rather than inventing one.
                let Some(surface_id) = surface.data::<SurfaceId>() else {
                    return;
                };
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
