# aufhebung

A windowing library for native Wayland applications. It wraps the parts of xdg-shell that
every app has to get right — the configure handshake, surface roles, toplevel metadata, seat
and keyboard state — and hands you events. You keep your own buffers and your own event
loop, and you draw.

> **Status: early.** The API is unstable and will change. Pointer input, frame callbacks,
> clipboard, text input, and `wl_output` are not implemented yet. See [Known gaps](#known-gaps).

## Model

The split is deliberate: **the library owns the protocol bookkeeping, the app owns content.**

- The library creates surfaces, assigns roles, tracks configure/resize state, and acks every
  `xdg_surface.configure` exactly once.
- Your app supplies buffers and decides when to commit them.

So the loop is:

1. `Display::add_surface` — creates the surface and commits it with **no** buffer, which is
   what makes the compositor send the first configure.
2. The compositor configures. The library acks and queues a `SurfaceEvent::Configure`.
3. `Display::events` yields it; you call `Surface::commit(&buffer)`, which attaches, scales
   to the current size via `wp_viewporter`, and commits.

Committing a toplevel before its first configure is a protocol error. Subsurfaces get no
configure events, so they may be committed whenever you like.

`Display::add_color` hands out 1×1 `wl_buffer`s from
`wp_single_pixel_buffer_manager_v1`, which `Surface::commit` scales to fill the surface. It
exists so the example runs without a buffer pipeline. Real drawing wants your own shm or
dma-buf buffers — the library never inspects a buffer it is given, and the same buffer may
back any number of surfaces.

`Display::dispatch` blocks until something arrives. Driving it from a thread, a `poll`, or
an async runtime is your call; the library runs no loop of its own and spawns no threads.

## Example

```rust
use aufhebung::{
    display::Display,
    state::{Event, SurfaceEvent},
    surface::{SurfaceInfo, SurfaceRole},
};

let mut display = Display::new()?;
let white = display.add_color(u32::MAX, u32::MAX, u32::MAX, u32::MAX);

let id = display
    .add_surface(SurfaceInfo {
        width: 800,
        height: 800,
        role: SurfaceRole::Window { title: "hello".into() },
    })
    .expect("subsurface parent is not live");

loop {
    display.dispatch()?;
    for event in display.events() {
        match event {
            Event::SurfaceEvent { id, event: SurfaceEvent::Configure { width, height } } => {
                println!("{id:?} is now {width}x{height}");
                display.surface(id).unwrap().commit(&white);
            }
            Event::SeatEvent { .. } => {}
        }
    }
}
```

See `examples/demo.rs` for subsurfaces, key translation, and close handling.

## Windows and decorations

`SurfaceRole::Window` takes a title, and `Surface::title` reads it back. The library asks
for `zxdg_toplevel_decoration_v1` in `server_side` mode, meaning the compositor draws the
title bar and the app's buffer covers only the client area. There is no way to select
client-side decoration, so an app that draws its own chrome cannot use it as-is.

## Keyboard

`Display::translate_key` runs the compositor's keymap through [kbvm] and returns the keysyms
for a key, most preferred first. It is `None` until the seat has sent its keymap, so a
keypress that races startup is skipped rather than panicking.

`SeatEvent::Key` carries `pressed`, so presses and releases are distinguishable. The
compositor does not send auto-repeat key events, so repeating is your job: it arrives as
`SeatEvent::RepeatInfo { rate, delay }`, where `rate` is keys per second (`0` disables
repeat) and `delay` is the milliseconds before the first one. Start a timer on a press, stop
it on the matching release, and retune it if `RepeatInfo` changes.

Modifiers and layout group come from `wl_keyboard.modifiers`. kbvm's own state machine is
not used, so a group switch that a keymap performs internally is not tracked.

[kbvm]: https://crates.io/crates/kbvm

## Dependencies

`wayland-client` and `wayland-protocols` are pinned to a git revision rather than a release,
because the staging and unstable protocols this crate needs are not in a published crate
yet. Expect churn there.

## Building

```sh
cargo build
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`rust-toolchain.toml` pins nightly, which is what this was last built with — but nothing in
the crate actually requires it. Edition 2024 needs stable 1.85 or newer, and the source uses
no `#![feature]` attributes, so switching the channel to `stable` should work.

```sh
cargo run --example demo
```

## Known gaps

Ordered roughly by how much they block a real application.

- **No pointer input.** No `wl_pointer`, no `wl_seat.pointer` capability handling. A
  windowing library without the mouse is not yet usable as one.
- **No frame callbacks.** There is no way to learn when the compositor is done with the
  previous frame, so redraws cannot be paced to vsync.
- **No text input or clipboard.** No `zwp_text_input_v*`, no `wl_data_device`, so no IME and
  no copy/paste.
- **No `wl_output` binding.** Monitors are never enumerated, so multi-monitor placement has
  nothing to work from.
- **No popups, cursors, or DnD protocols.** `SurfaceRole` covers toplevel, subsurface, and
  unassigned only — no `xdg_popup`, so no menus, tooltips, or context menus.
- **No `set_app_id`.** `xdg_toplevel.app_id` is never sent, which some compositors and
  desktop-environment rules need.
- **No surface removal cascade.** Removing a surface does not remove its subsurfaces.
- **No runtime global handling.** `GlobalListHandler` has no `runtime_add_global`
  implementation, so a global appearing after startup is ignored.
- **Unvalidated keymap.** `wl_keyboard.keymap.format` is not checked, and a keymap that
  fails to parse panics rather than surfacing an error.
