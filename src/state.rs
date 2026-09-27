//! The events drained from the connection.
//!
//! Surface and seat events are pushed into a queue as the compositor's events
//! are handled, and are taken out by
//! [`Display::events`](crate::display::Display::events), which drains everything
//! queued since the last call.
//!
//! # Seats
//!
//! [`translate_char`](crate::display::Display::translate_char) and
//! [`translate_key`](crate::display::Display::translate_key) resolve a key
//! through one seat's keymap and modifier state, so they take a [`SeatId`].
//! That id is the `id` on the [`SeatEvent`] carrying the key, so translating a
//! key and handling the event it arrived on are the same step:
//!
//! ```no_run
//! # use aufhebung::{display::Display, state::{Event, SeatEvent}};
//! # fn demo(display: &mut Display) {
//! for event in display.events() {
//!     if let Event::SeatEvent { id: seat, event: SeatEvent::Key { key, .. } } = event {
//!         if let Some(c) = display.translate_char(seat, key) {
//!             println!("{c}");
//!         }
//!     }
//! }
//! # }
//! ```

use std::collections::VecDeque;

use kbvm::{GroupIndex, ModifierMask};
use slotmap::SlotMap;

use crate::{
    globals::Globals,
    seat::{Keyboard, Seat, SeatId},
    surface::{Surface, SurfaceId},
};

/// Emitted when the compositor configures a surface.
///
/// The dimensions are the ones latched by the preceding toplevel configure, so
/// they are ready to be used by the time this event is drained.
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
        /// `true` for a press, `false` for a release. The compositor never
        /// sends auto-repeat key events; use
        /// [`RepeatInfo`](Self::RepeatInfo) to implement repeating.
        pressed: bool,
        group: GroupIndex,
        mods: ModifierMask,
    },
    /// The compositor's key repeat configuration, sent when it changes.
    ///
    /// Implement repeating from this: on a press, wait `delay` milliseconds,
    /// then emit a press every `1000 / rate` milliseconds until the key is
    /// released or `rate` becomes 0.
    RepeatInfo { rate: i32, delay: i32 },
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
    /// `None` if the seat is gone, or if the compositor has not told us about it
    /// yet.
    pub fn seat(&self, seat: SeatId) -> Option<&Seat> {
        self.globals.seats.get(seat)?.as_ref()
    }

    pub fn seat_mut(&mut self, seat: SeatId) -> Option<&mut Seat> {
        self.globals.seats.get_mut(seat)?.as_mut()
    }

    /// `None` if the seat is gone or exposes no keyboard.
    pub fn keyboard(&self, seat: SeatId) -> Option<&Keyboard> {
        self.seat(seat)?.keyboard.as_ref()
    }

    pub fn keyboard_mut(&mut self, seat: SeatId) -> Option<&mut Keyboard> {
        self.seat_mut(seat)?.keyboard.as_mut()
    }
}
