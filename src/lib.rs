//! A small, low-level windowing library for native Wayland applications.
//!
//! `aufhebung` handles the Wayland bookkeeping that sits between an application
//! and `wl_surface`: surface roles, xdg-shell configure handshakes, toplevel
//! metadata, keyboard and pointer input, and event collection. Buffers, timing,
//! scheduling, and runtime integration remain in the application.
//!
//! It does not own your event loop. The core API is synchronous and blocking,
//! and the connection's socket is exposed directly, so it can be integrated
//! into any event loop or runtime.
//!
//! > **Early software.** The API is unstable.
//! > The library currently focuses on windows, sub-surfaces, buffers, and
//! > keyboard and pointer input.
//!
//! # A smallest application
//!
//! ```no_run
//! use aufhebung::{
//!     color::Color,
//!     display::Display,
//!     state::{Event, SurfaceEvent},
//!     surface::{SurfaceInfo, SurfaceRole},
//! };
//!
//! # fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let mut display = Display::new()?;
//!
//! let white = display.add_color(Color::WHITE);
//!
//! let id = display
//!     .add_surface(SurfaceInfo {
//!         width: 800,
//!         height: 800,
//!         role: SurfaceRole::Window {
//!             title: "hello".into(),
//!         },
//!     })
//!     .expect("no surface id available");
//!
//! loop {
//!     display.dispatch()?;
//!
//!     for event in display.events() {
//!         if let Event::SurfaceEvent {
//!             event: SurfaceEvent::Configure { width, height },
//!             ..
//!         } = event
//!         {
//!             println!("{id:?} configured to {width}x{height}");
//!             display.commit(id, &white);
//!         }
//!     }
//! }
//! # }
//! ```
//!
//! A toplevel may not carry a buffer until the compositor has configured it, so
//! the first commit has to answer a
//! [`Configure`](state::SurfaceEvent::Configure) event. That state is exposed
//! rather than hidden behind a window abstraction:
//! [`Display::is_configured`](display::Display::is_configured) reports it, and
//! [`is_configured`](surface::Surface::is_configured) asks the same of one
//! surface.
//!
//! # Modules
//!
//! - [`display`] — the connection, its surfaces, and the events it collects.
//! - [`surface`] — [`Surface`](surface::Surface) and the roles one is created
//!   with.
//! - [`state`] — the [`Event`](state::Event) values drained from the
//!   connection.
//! - [`seat`] — keyboards and pointers, and the keymap translation the
//!   keyboard's input goes through.
//! - [`color`] — colours for
//!   [`Display::add_color`](display::Display::add_color).
//!
//! # Re-exports
//!
//! The public API speaks these types, so they are re-exported here and you
//! should get them from this crate rather than declaring them yourself:
//!
//! - [`wayland_client`] — [`WlBuffer`](wayland_client::protocol::wl_buffer::WlBuffer)
//!   from [`add_color`](display::Display::add_color), plus `ConnectError`,
//!   `DispatchError`, and `ReadEventsGuard`.
//! - [`wayland_protocols`] — the protocol bindings behind [`surface`].
//! - [`kbvm`] — the [`KeysymProps`](kbvm::lookup::KeysymProps) from
//!   [`translate_key`](display::Display::translate_key).
//! - [`slotmap`] — the machinery behind [`SurfaceId`](surface::SurfaceId) and
//!   [`SeatId`](seat::SeatId), for side-tables of your own.
//!
//! Declaring `wayland-client` yourself is likely to fail: this crate depends on
//! a pinned git revision, and Cargo treats a different source as a different
//! crate, so the two `WlBuffer` types will not unify.
//! `aufhebung::wayland_client` is the same crate this library uses, so the types
//! always match.

#![cfg_attr(feature = "tokio", doc = "- [`tokio`] — the optional `tokio` feature.")]
#![deny(unused_must_use)]

pub mod color;
pub mod display;
pub(crate) mod globals;
pub(crate) mod mmap;
pub mod seat;
pub mod state;
pub mod surface;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use kbvm;
pub use slotmap;
pub use wayland_client;
pub use wayland_protocols;
