//! Keyboards and pointers, and the keymap translation the compositor's input
//! goes through.
//!
//! A seat's keyboard and pointer exist only once the compositor has advertised
//! them, and the [`SeatId`] to resolve its keys with is the one its
//! [`SeatEvent`]s carry. See
//! [the `state` module docs](crate::state#seats).
//!
//! Neither the seat nor its devices outlive the compositor's own: a seat
//! advertised after startup is bound as it arrives, and one withdrawn takes its
//! keymap, focus and pointer position with it.

use std::os::fd::AsFd;

use kbvm::{GroupIndex, ModifierMask, lookup::LookupTable, xkb::diagnostic::WriteToLog};
use slotmap::new_key_type;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_keyboard::{KeymapFormat, WlKeyboard},
        wl_pointer::{self, WlPointer},
        wl_seat::{self, WlSeat},
    },
};

use crate::{
    mmap::Mmap,
    state::{Axis, Button, Event, PointerEvent, SeatEvent, State},
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

/// A seat's pointer.
///
/// The position is what earns this struct its keep: the protocol's `button` and
/// `axis` events name no coordinates, so the position from the most recent
/// `enter` or `motion` is the only way left to say where a click landed.
#[derive(Default)]
pub(crate) struct Pointer {
    /// The surface the pointer is over, if it is one of ours. A pointer is
    /// often over something else entirely — a panel, a menu, another window —
    /// and events for those are not ours to report.
    focused_surface: Option<SurfaceId>,
    x: f64,
    y: f64,
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
    pub(crate) pointer: Option<Pointer>,
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

        let Some(seat) = state.seat_mut(*self) else {
            return;
        };

        // A seat is not required to have both devices, so each is handled on its
        // own. Asking for a capability the seat does not advertise is
        // `wl_seat.error.missing_capability` and takes the connection down, so
        // the check has to come first; and creating a second device for the same
        // seat would leak the first and throw away its state, so only ever do
        // this once.
        //
        // A capability the compositor withdraws takes the device's state with
        // it: the spec makes this event the authoritative set, and the
        // compositor destroys the object itself, so there is nothing to send.
        if capabilities.contains(wl_seat::Capability::Keyboard) {
            if seat.keyboard.is_none() {
                seat.keyboard = Some(Keyboard {
                    lookup_table: Default::default(),
                    focused_surface: Default::default(),
                    group: Default::default(),
                    mods: Default::default(),
                });

                // The proxy is deliberately not kept: nothing needs to send
                // requests on it, and dropping a client-side proxy sends no
                // request, so the compositor keeps the keyboard alive.
                let _ = proxy.get_keyboard(qh, *self);
            }
        } else {
            seat.keyboard = None;
        }

        if capabilities.contains(wl_seat::Capability::Pointer) {
            if seat.pointer.is_none() {
                seat.pointer = Some(Pointer::default());

                // Likewise for the pointer: this crate never sends a request on
                // it — no cursor surface, no pointer constraints — so holding it
                // would only be a way to keep it alive a moment longer.
                let _ = proxy.get_pointer(qh, *self);
            }
        } else {
            seat.pointer = None;
        }
    }
}

/// Records where the pointer is and reports it against the surface it is on.
///
/// `on` is the surface the protocol named, or `None` to stay on the one the
/// pointer is already on — which is how the events that name no surface of their
/// own are handled.
fn pointer_moved(
    state: &mut State,
    seat: SeatId,
    on: Option<SurfaceId>,
    x: f64,
    y: f64,
    make: impl FnOnce(SurfaceId, f64, f64) -> PointerEvent,
) {
    let Some(pointer) = state.pointer_mut(seat) else {
        return;
    };
    if let Some(surface) = on {
        pointer.focused_surface = Some(surface);
    }
    // The pointer is over something that is not ours, so there is no surface the
    // application could do anything with.
    let Some(surface) = pointer.focused_surface else {
        return;
    };
    pointer.x = x;
    pointer.y = y;
    state.events.push_back(Event::SeatEvent {
        id: seat,
        event: SeatEvent::Pointer(make(surface, x, y)),
    });
}

/// Reports against where the pointer already is, without moving it.
///
/// The protocol's `button` and `axis` events carry no coordinates, so this is
/// where the position cached by the last `enter` or `motion` gets used.
fn pointer_here(
    state: &mut State,
    seat: SeatId,
    make: impl FnOnce(SurfaceId, f64, f64) -> PointerEvent,
) {
    let Some(pointer) = state.pointer(seat) else {
        return;
    };
    let Some(surface) = pointer.focused_surface else {
        return;
    };
    let (x, y) = (pointer.x, pointer.y);
    state.events.push_back(Event::SeatEvent {
        id: seat,
        event: SeatEvent::Pointer(make(surface, x, y)),
    });
}

impl Dispatch<WlPointer, State> for SeatId {
    fn event(
        &self,
        state: &mut State,
        _proxy: &WlPointer,
        event: <WlPointer as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                // Every surface in this connection is created with a `SurfaceId`
                // as its user data, so a name that does not carry one is the
                // compositor violating the protocol. Ignoring the event is
                // better than aborting the process over it, and it leaves the
                // pointer where it was rather than inventing a surface.
                let Some(surface) = surface.data::<SurfaceId>().copied() else {
                    return;
                };
                // The position rides along on `enter`, so entering a surface is
                // also the seed for the position that a later `button` reports.
                pointer_moved(
                    state,
                    *self,
                    Some(surface),
                    surface_x,
                    surface_y,
                    |surface, x, y| PointerEvent::Enter { surface, x, y },
                );
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                pointer_moved(state, *self, None, surface_x, surface_y, |surface, x, y| {
                    PointerEvent::Motion { surface, x, y }
                });
            }
            wl_pointer::Event::Leave { surface, .. } => {
                let Some(surface) = surface.data::<SurfaceId>().copied() else {
                    return;
                };
                if let Some(pointer) = state.pointer_mut(*self) {
                    pointer.focused_surface = None;
                }
                state.events.push_back(Event::SeatEvent {
                    id: *self,
                    event: SeatEvent::Pointer(PointerEvent::Leave { surface }),
                });
            }
            wl_pointer::Event::Button {
                time,
                button,
                state: button_state,
                ..
            } => {
                // No position comes with a button, so this is the one case
                // where the cached one is all there is.
                pointer_here(state, *self, |surface, x, y| PointerEvent::Button {
                    surface,
                    time,
                    x,
                    y,
                    button: Button::from_raw(button),
                    pressed: button_state == wl_pointer::ButtonState::Pressed,
                });
            }
            wl_pointer::Event::Axis { time, axis, value } => {
                if state.pointer(*self).is_none() {
                    return;
                }
                // The protocol names exactly these two directions, and the
                // compositor may not send an event above the version the seat
                // was bound at, so an unrecognised value is not one this crate
                // has a name for and has nothing to report.
                let axis = if axis == wl_pointer::Axis::VerticalScroll {
                    Axis::VerticalScroll
                } else if axis == wl_pointer::Axis::HorizontalScroll {
                    Axis::HorizontalScroll
                } else {
                    return;
                };
                state.events.push_back(Event::SeatEvent {
                    id: *self,
                    event: SeatEvent::Pointer(PointerEvent::Axis { time, axis, value }),
                });
            }
            // The rest of the pointer protocol needs a seat version this crate
            // does not bind: `frame`, `axis_source`, `axis_stop` and
            // `axis_discrete` are all version 5 or later, and the compositor may
            // not send an event above the version the seat was bound at.
            _ => {}
        }
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
