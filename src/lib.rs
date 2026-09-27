//! A small, low-level windowing library for native Wayland applications.
//!
//! `aufhebung` handles the Wayland bookkeeping that sits between an application
//! and `wl_surface`: surface roles, xdg-shell configure handshakes, toplevel
//! metadata, keyboard and pointer input, and event collection. What to draw,
//! timing, scheduling, and runtime integration remain in the application.
//!
//! It does not own your event loop. The core API is synchronous and blocking,
//! and the connection's socket is exposed directly, so it can be integrated
//! into any event loop or runtime.
//!
//! > **Early software.** The API is unstable.
//! > The library currently focuses on windows, sub-surfaces, colour and pixel
//! > buffers, and keyboard and pointer input.
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
//!             display.commit(id, white);
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
//! # Pixels
//!
//! [`add_color`](display::Display::add_color) makes a single pixel that the
//! compositor scales to fill a surface, which is the right shape for a flat
//! colour and the only thing that can be drawn with one.
//! [`add_pixels`](display::Display::add_pixels) makes a real image instead —
//! `width * height` `u32` values, row-major, each `0xAARRGGBB` in the machine's
//! own byte order, uploaded through a `wl_shm` pool — and
//! [`commit_unscaled`](surface::Surface::commit_unscaled) places it one pixel to
//! one pixel instead of stretching it, at the buffer's own size — which the
//! buffer knows, so there is no size to pass and none to get wrong.
//!
//! Loading a file is one step beyond that, and is what the `image` feature is
//! for: `Display::add_image` takes a `DynamicImage` instead of finished numbers
//! and does the conversion, including the pre-multiplication that `image`
//! deliberately does not do for you. Decoding stays the application's — the
//! feature brings in `jpeg` and `png` and little else, and a program that wants
//! another format depends on `image` itself with the features it needs, which
//! Cargo unifies with this one.
//!
//! That is an upload, not a canvas. The compositor may still be reading a
//! committed buffer's pixels, and this library does not watch for the
//! `wl_buffer.release` that says it has stopped, so the memory must not be
//! written again; each upload also leaves a pool and buffer alive until the
//! connection closes. An application that redraws by uploading new pixels needs
//! a pool of its own that tracks releases, which this library does not provide.
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
//! - [`buffer`] — [`BufferId`](buffer::BufferId), which is what
//!   [`add_color`](display::Display::add_color) and
//!   [`add_pixels`](display::Display::add_pixels) hand back.
//! - [`color`] — colours for
//!   [`Display::add_color`](display::Display::add_color).
//!
//! # Re-exports
//!
//! The public API speaks these types, so they are re-exported here and you
//! should get them from this crate rather than declaring them yourself:
//!
//! - [`wayland_client`] — `ConnectError`, `DispatchError`, and
//!   `ReadEventsGuard`.
//! - [`wayland_protocols`] — the protocol bindings behind [`surface`].
//! - [`kbvm`] — the [`KeysymProps`](kbvm::lookup::KeysymProps) from
//!   [`translate_key`](display::Display::translate_key).
//! - [`slotmap`] — the machinery behind [`SurfaceId`](surface::SurfaceId),
//!   [`SeatId`](seat::SeatId), and [`BufferId`](buffer::BufferId), for
//!   side-tables of your own.
//!
//! Declaring `wayland-client` yourself is likely to fail: this crate depends on
//! a pinned git revision, and Cargo treats a different source as a different
//! crate, so the two types will not unify. `aufhebung::wayland_client` is the
//! same crate this library uses, so the types always match.
//!
//! The public API does *not* speak `WlBuffer`: a buffer is a
//! [`BufferId`](buffer::BufferId), and a `wl_buffer` is a proxy that whoever
//! holds it can destroy. See [`buffer`] for why.

#![cfg_attr(feature = "tokio", doc = "- [`tokio`] — the optional `tokio` feature.")]
#![cfg_attr(
    feature = "image",
    doc = "- [`image`] — the optional `image` feature, for `Display::add_image` and the decoding in `crate::decode`."
)]
#![deny(unused_must_use)]

pub mod buffer;
pub mod color;
#[cfg(feature = "image")]
pub mod decode;
pub mod display;
pub(crate) mod globals;
pub(crate) mod mmap;
pub mod seat;
pub mod state;
pub mod surface;
#[cfg(feature = "tokio")]
pub mod tokio;

#[cfg(feature = "image")]
pub use image;
pub use kbvm;
pub use slotmap;
pub use wayland_client;
pub use wayland_protocols;
