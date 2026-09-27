use slotmap::SlotMap;

use crate::{
    globals::Globals,
    seat::{Keyboard, Seat, SeatId},
    surface::{Surface, SurfaceId},
};

pub struct State {
    pub globals: Globals,
    pub surfaces: SlotMap<SurfaceId, Surface>,
    pub xkb_ctx: kbvm::xkb::Context,
}

impl State {
    pub fn seat_mut(&mut self, seat: SeatId) -> &mut Seat {
        self.globals.seats.get_mut(seat).unwrap().as_mut().unwrap()
    }

    pub fn keyboard_mut(&mut self, seat: SeatId) -> &mut Keyboard {
        self.seat_mut(seat).keyboard.as_mut().unwrap()
    }
}
