//! The events drained from the connection.
//!
//! Surface and seat events are pushed into a queue as the compositor's events
//! are handled, and are taken out by
//! [`Display::events`](crate::display::Display::events), which drains everything
//! queued since the last call.
//!
//! # Seats
//!
//! A seat is bound when the compositor advertises it, which is not necessarily
//! at startup: a compositor that gains an input device can add one later, and
//! withdraws the seat again when it goes away. Nothing has to be set up for
//! either, but it means a [`SeatId`] only ever names a seat for as long as the
//! compositor keeps it.
//!
//! A seat is not required to have a keyboard, a pointer, or both, and each is
//! used only once the compositor has said the seat has it.
//!
//! [`translate_char`](crate::display::Display::translate_char) and
//! [`translate_key`](crate::display::Display::translate_key) resolve a key
//! through one seat's keymap and modifier state, so they take a [`SeatId`].
//! That id is the `id` on the [`SeatEvent`] carrying the key, so translating a
//! key and handling the event it arrived on are the same step. A seat that has
//! been withdrawn in the meantime answers `None` rather than failing:
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
//!
//! # Pointers
//!
//! A [`PointerEvent`] names the surface the pointer is on and the position on
//! that surface, so a click needs no geometry of the application's own: the id
//! is the answer, and for a sub-surface it is the id of the sub-surface, not of
//! its parent.
//!
//! The `x` and `y` on a [`Button`](PointerEvent::Button) are the position from
//! the last `Enter` or `Motion`, because the protocol's own button event names
//! no coordinates:
//!
//! ```no_run
//! # use aufhebung::{
//! #     display::Display,
//! #     state::{Button, Event, PointerEvent, SeatEvent},
//! #     surface::SurfaceId,
//! # };
//! # fn demo(display: &mut Display, cookie: SurfaceId) {
//! for event in display.events() {
//!     if let Event::SeatEvent { event: SeatEvent::Pointer(pointer), .. } = event {
//!         match pointer {
//!             PointerEvent::Button { surface, button: Button::Left, pressed: true, x, y, .. }
//!                 if surface == cookie =>
//!             {
//!                 println!("clicked the cookie at {x}, {y}");
//!             }
//!
//!             _ => {}
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
    seat::{Keyboard, Pointer, Seat, SeatId},
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
    /// Something the seat's pointer did.
    ///
    /// Only reported once the compositor has advertised a pointer for the seat.
    /// See [`PointerEvent`].
    Pointer(PointerEvent),
}

/// What a pointer did.
///
/// Every variant that names a surface reports it in that surface's *own*
/// coordinates: a sub-surface reports positions relative to itself, so an
/// application does not have to convert them to the parent to know what was
/// hit. Positions are the compositor's own fixed-point values widened to `f64`,
/// so a fraction of a pixel is preserved and rounding is the application's
/// choice.
#[derive(Debug)]
pub enum PointerEvent {
    /// The pointer moved onto a surface, at the position given.
    Enter { surface: SurfaceId, x: f64, y: f64 },
    /// The pointer left the surface it was on.
    Leave { surface: SurfaceId },
    /// The pointer moved while over a surface.
    Motion { surface: SurfaceId, x: f64, y: f64 },
    /// A button went down or up.
    ///
    /// The position is the one from the most recent [`Enter`](Self::Enter) or
    /// [`Motion`](Self::Motion). The protocol's button event names no
    /// coordinates of its own, so this is the only way to know where a click
    /// landed.
    Button {
        surface: SurfaceId,
        time: u32,
        x: f64,
        y: f64,
        button: Button,
        pressed: bool,
    },
    /// A scroll wheel, or a finger, moved.
    ///
    /// The value is in surface units: positive is away from the user for a
    /// vertical wheel. At the seat version this crate binds, a wheel reports
    /// whole steps and a finger reports fractions, which are not otherwise
    /// distinguishable here.
    Axis { time: u32, axis: Axis, value: f64 },
}

/// A pointer button.
///
/// The codes are the ones the Linux input layer uses, which is what the
/// compositor reports. Only the three a mouse has are named; anything else
/// arrives as [`Other`](Self::Other) with the code intact, so no button is
/// unrepresentable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Right,
    Middle,
    Other(u32),
}

impl Button {
    /// The button a raw code names, or [`Other`](Self::Other) if it names none
    /// of the three a mouse has.
    ///
    /// ```
    /// use aufhebung::state::Button;
    ///
    /// assert_eq!(Button::from_raw(0x110), Button::Left);
    /// assert_eq!(Button::from_raw(0x113), Button::Other(0x113));
    /// ```
    pub const fn from_raw(code: u32) -> Self {
        match code {
            0x110 => Self::Left,
            0x111 => Self::Right,
            0x112 => Self::Middle,
            code => Self::Other(code),
        }
    }

    /// The code this button arrives as on the wire.
    pub const fn as_raw(self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
            Self::Other(code) => code,
        }
    }
}

/// Which way a scroll wheel or finger moved.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    VerticalScroll,
    HorizontalScroll,
}

#[derive(Debug)]
pub enum Event {
    SurfaceEvent { id: SurfaceId, event: SurfaceEvent },
    SeatEvent { id: SeatId, event: SeatEvent },
}

/// Everything the connection dispatches into.
///
/// Owned by [`Display`](crate::display::Display), which is the only way to
/// reach it, and named as the state parameter of every `Dispatch` impl in the
/// crate. The event queue drives it; nothing outside sees it.
pub(crate) struct State {
    pub(crate) globals: Globals,
    pub(crate) surfaces: SlotMap<SurfaceId, Surface>,
    pub(crate) xkb_ctx: kbvm::xkb::Context,
    pub(crate) events: VecDeque<Event>,
}

impl State {
    /// `None` if the seat is gone, or if the compositor has not told us about it
    /// yet.
    pub(crate) fn seat(&self, seat: SeatId) -> Option<&Seat> {
        self.globals.seats.get(seat)?.as_ref()
    }

    pub(crate) fn seat_mut(&mut self, seat: SeatId) -> Option<&mut Seat> {
        self.globals.seats.get_mut(seat)?.as_mut()
    }

    /// `None` if the seat is gone or exposes no keyboard.
    pub(crate) fn keyboard(&self, seat: SeatId) -> Option<&Keyboard> {
        self.seat(seat)?.keyboard.as_ref()
    }

    pub(crate) fn keyboard_mut(&mut self, seat: SeatId) -> Option<&mut Keyboard> {
        self.seat_mut(seat)?.keyboard.as_mut()
    }

    /// `None` if the seat is gone or exposes no pointer.
    pub(crate) fn pointer(&self, seat: SeatId) -> Option<&Pointer> {
        self.seat(seat)?.pointer.as_ref()
    }

    pub(crate) fn pointer_mut(&mut self, seat: SeatId) -> Option<&mut Pointer> {
        self.seat_mut(seat)?.pointer.as_mut()
    }
}
