use kbvm::{
    GroupIndex, Keycode, ModifierMask,
    lookup::LookupTable,
    xkb::{Keymap, diagnostic::WriteToLog},
};
use slotmap::new_key_type;
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_keyboard::{self, WlKeyboard},
        wl_seat::WlSeat,
    },
};

use crate::{
    globals::GlobalData,
    mmap::Mmap,
    state::{Event, SeatEvent, State},
    surface::SurfaceId,
};

pub struct Keyboard {
    keyboard: WlKeyboard,
    keymap: Option<Keymap>,
    pub(crate) lookup_table: Option<LookupTable>,
    focused_surface: Option<SurfaceId>,
    pub(crate) group: GroupIndex,
    pub(crate) mods: ModifierMask,
}

new_key_type! {
    pub struct SeatId;
}

pub struct Seat {
    pub seat: WlSeat,
    pub keyboard: Option<Keyboard>,
}

impl Dispatch<WlSeat, State> for SeatId {
    fn event(
        &self,
        state: &mut State,
        proxy: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        let keyboard = proxy.get_keyboard(qh, *self);
        let seat = state.seat_mut(*self);
        seat.keyboard = Some(Keyboard {
            keyboard,
            keymap: Default::default(),
            lookup_table: Default::default(),
            focused_surface: Default::default(),
            group: Default::default(),
            mods: Default::default(),
        });
    }
}

impl Dispatch<WlKeyboard, State> for SeatId {
    fn event(
        &self,
        state: &mut State,
        proxy: &WlKeyboard,
        event: <WlKeyboard as wayland_client::Proxy>::Event,
        conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        match event {
            wayland_client::protocol::wl_keyboard::Event::Keymap { format, fd, size } => {
                let keymap = {
                    let mmap = Mmap::new(fd, size as usize).unwrap();
                    state
                        .xkb_ctx
                        .keymap_from_bytes(WriteToLog, None, mmap.as_slice())
                }
                .unwrap();
                let lookup_table = keymap.to_builder().build_lookup_table();
                let keyboard = state.keyboard_mut(*self);
                keyboard.keymap = Some(keymap);
                keyboard.lookup_table = Some(lookup_table);
            }
            wayland_client::protocol::wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => {
                let surface_id = surface.data::<SurfaceId>().unwrap();
                let keyboard = state.keyboard_mut(*self);
                keyboard.focused_surface = Some(*surface_id);
            }
            wayland_client::protocol::wl_keyboard::Event::Leave { serial, surface } => {
                let keyboard = state.keyboard_mut(*self);
                keyboard.focused_surface = None;
            }
            wayland_client::protocol::wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                let keyboard = state.keyboard(*self);
                state.events.push_back(Event::SeatEvent {
                    id: *self,
                    event: SeatEvent::Key {
                        surface: keyboard.focused_surface.unwrap(),
                        time,
                        key,
                        group: keyboard.group,
                        mods: keyboard.mods,
                    },
                });
            }
            wayland_client::protocol::wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                let keyboard = state.keyboard_mut(*self);
                keyboard.group = GroupIndex(group);
                keyboard.mods = ModifierMask(mods_depressed | mods_latched | mods_locked);
            }
            wayland_client::protocol::wl_keyboard::Event::RepeatInfo { rate, delay } => {}
            _ => {}
        }
    }
}
