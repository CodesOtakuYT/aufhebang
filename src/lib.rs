//! A small, low-level windowing library for native Wayland applications.
//!
//! `aufhebung` handles the Wayland bookkeeping that sits between an application
//! and `wl_surface`: surface roles, xdg-shell configure handshakes, toplevel
//! metadata, keyboard state, and event collection. Buffers, timing,
//! scheduling, and runtime integration remain in the application.
//!
//! It does not own your event loop. The core API is synchronous and blocking,
//! and the connection's socket is exposed directly, so it can be integrated
//! into any event loop or runtime.
//!
//! > **Early software.** The API is unstable.
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
//! - [`seat`] — keyboards and keymap translation.
//! - [`color`] — colours for
//!   [`Display::add_color`](display::Display::add_color).

#![cfg_attr(feature = "tokio", doc = "- [`tokio`] — the optional `tokio` feature.")]
#![deny(unused_must_use)]

pub mod color;
pub mod display;
pub mod globals;
pub mod mmap;
pub mod seat;
pub mod state;
pub mod surface;
#[cfg(feature = "tokio")]
pub mod tokio;
