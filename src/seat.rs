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

use crate::{globals::GlobalData, mmap::Mmap, state::State, surface::SurfaceId};

pub struct Keyboard {
    keyboard: WlKeyboard,
    keymap: Option<Keymap>,
    lookup_table: Option<LookupTable>,
    focused_surface: Option<SurfaceId>,
    group: GroupIndex,
    mods: ModifierMask,
}

pub struct Seat {
    keyboard: Keyboard,
}

impl Dispatch<WlSeat, State> for GlobalData {
    fn event(
        &self,
        state: &mut State,
        proxy: &WlSeat,
        event: <WlSeat as wayland_client::Proxy>::Event,
        conn: &Connection,
        qh: &QueueHandle<State>,
    ) {
        let keyboard = proxy.get_keyboard(qh, GlobalData);
        state.seat = Some(Seat {
            keyboard: Keyboard {
                keyboard,
                keymap: None,
                lookup_table: None,
                focused_surface: None,
                group: Default::default(),
                mods: Default::default(),
            },
        });
    }
}

impl Dispatch<WlKeyboard, State> for GlobalData {
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
                let keyboard = &mut state.seat.as_mut().unwrap().keyboard;
                keyboard.keymap = Some(keymap);
                keyboard.lookup_table = Some(lookup_table);
            }
            wayland_client::protocol::wl_keyboard::Event::Enter {
                serial,
                surface,
                keys,
            } => {
                let surface_id = surface.data::<SurfaceId>().unwrap();
                let keyboard = &mut state.seat.as_mut().unwrap().keyboard;
                keyboard.focused_surface = Some(*surface_id);
            }
            wayland_client::protocol::wl_keyboard::Event::Leave { serial, surface } => {
                let keyboard = &mut state.seat.as_mut().unwrap().keyboard;
                keyboard.focused_surface = None;
            }
            wayland_client::protocol::wl_keyboard::Event::Key {
                serial,
                time,
                key,
                state: key_state,
            } => {
                let keyboard = &mut state.seat.as_mut().unwrap().keyboard;
                for keysym in keyboard.lookup_table.as_ref().unwrap().lookup(
                    keyboard.group,
                    keyboard.mods,
                    Keycode::from_evdev(key),
                ) {
                    println!("{keysym:#?}");
                }
            }
            wayland_client::protocol::wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            } => {
                let keyboard = &mut state.seat.as_mut().unwrap().keyboard;
                keyboard.group = GroupIndex(group);
                keyboard.mods = ModifierMask(mods_depressed | mods_latched | mods_locked);
            }
            wayland_client::protocol::wl_keyboard::Event::RepeatInfo { rate, delay } => {}
            _ => {}
        }
    }
}
