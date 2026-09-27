use std::collections::VecDeque;

use kbvm::{
    GroupIndex, Keysym, ModifierMask,
    lookup::{KeysymProps, Lookup},
};
use slotmap::SlotMap;

use crate::{
    globals::Globals,
    seat::{Keyboard, Seat, SeatId},
    surface::{Surface, SurfaceId},
};

#[derive(Debug)]
pub enum SurfaceEvent {
    Configure { width: i32, height: i32 },
}

#[derive(Debug)]
pub enum SeatEvent {
    Key {
        surface: SurfaceId,
        time: u32,
        key: u32,
        group: GroupIndex,
        mods: ModifierMask,
    },
}

#[derive(Debug)]
pub enum Event {
    SurfaceEvent { id: SurfaceId, event: SurfaceEvent },
    SeatEvent { id: SeatId, event: SeatEvent },
}

pub struct State {
    pub globals: Globals,
    pub surfaces: SlotMap<SurfaceId, Surface>,
    pub xkb_ctx: kbvm::xkb::Context,
    pub events: VecDeque<Event>,
}

impl State {
    pub fn seat(&self, seat: SeatId) -> &Seat {
        self.globals.seats.get(seat).unwrap().as_ref().unwrap()
    }

    pub fn seat_mut(&mut self, seat: SeatId) -> &mut Seat {
        self.globals.seats.get_mut(seat).unwrap().as_mut().unwrap()
    }

    pub fn keyboard(&self, seat: SeatId) -> &Keyboard {
        self.seat(seat).keyboard.as_ref().unwrap()
    }

    pub fn keyboard_mut(&mut self, seat: SeatId) -> &mut Keyboard {
        self.seat_mut(seat).keyboard.as_mut().unwrap()
    }
}
