# aufhebung

A windowing library for native Wayland applications.

It handles the xdg-shell bookkeeping — the configure handshake, surface roles, toplevel
metadata, keyboard state — and hands you events. Buffers and the event loop stay yours.

> **Early.** The API is unstable. No pointer input, frame callbacks, text input, clipboard, or
> `wl_output` yet.

## Example

<img src="assets/snake.png" width="300" alt="The snake example running: a near-black window holding a grid of tiles, a green snake with a paler green head, and a single red food tile" />

```sh
cargo run --features tokio --example snake
```

`w/a/s/d` or `h/j/k/l` to steer, `q` to quit.

A game that moves by itself, on a `tokio::select!` over the Wayland socket and a timer — the
library has no runtime dependency, the example does. Every tile is its own sub-surface, and
each step moves the tail surface to the front instead of destroying and recreating one, so
the number of surfaces tracks the snake's length for the whole game. Every colour is a 1×1
`wl_buffer` that the compositor scales to fill.

## Usage

```rust
use aufhebung::{
    color::Color,
    display::Display,
    state::{Event, SurfaceEvent},
    surface::{SurfaceInfo, SurfaceRole},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut display = Display::new()?;
    let white = display.add_color(Color::WHITE);

    let id = display
        .add_surface(SurfaceInfo {
            width: 800,
            height: 800,
            role: SurfaceRole::Window { title: "hello".into() },
        })
        .expect("no surface id available");

    loop {
        display.dispatch()?;
        for event in display.events() {
            if let Event::SurfaceEvent {
                event: SurfaceEvent::Configure { width, height },
                ..
            } = event
            {
                println!("{id:?} resized to {width}x{height}");
                display.surface(id).unwrap().commit(&white);
            }
        }
    }
}
```

Commit a toplevel's first buffer in response to its configure event, not before — the
compositor has to say how big the window is first.

## API

- **`Display`** — connect, and drive the connection either blocking (`dispatch`) or from any
  readiness API (`connection_fd`, `prepare_read`, `dispatch_pending`, `flush`).
- **`SurfaceRole`** — `Window { title }` for a toplevel, `Subsurface { parent, x, y, sync }`
  for a positioned sub-surface, or `None` for a bare `wl_surface`.
- **`Surface`** — `commit` attaches a buffer, scales it to the surface, and damages it in
  full; `set_position` moves a sub-surface; `set_size_limits` constrains how far a toplevel
  may be resized; `is_configured` reports whether a toplevel may carry a buffer yet.
- **`Display::translate_char`** — a key as a character, including the cursor keys.
- **`Color`** — 8 bits per channel. `add_color` turns one into a `wl_buffer` that fills a
  surface, which is enough to draw something before you have a buffer pipeline.
- **`translate_key`** — the compositor's keymap, resolved to keysyms.

No threads, no timers, no runtime dependency.

## tokio

Off by default, and optional in the only sense that matters: with it off, the crate does not
depend on tokio at all. With it on, the crate still does not *require* a runtime — every hook
`Reactor` uses is public on `Display`.

The event loop is four calls, and all four are public: `connection_fd` to get the socket,
`prepare_read` to claim the right to read, `read` to read, then `dispatch_pending` and `flush`.
What `Reactor` adds is the order, which is easy to get wrong in three separate ways:

```rust,ignore
let mut reactor = Reactor::new(&display)?;
let from_compositor = tokio::select! {
    biased;
    read = reactor.recv(&mut display) => { read?; true }
    _ = stepper.tick() => false,
};
```

`recv` reads the socket once, runs the handlers, and flushes. Note that its future borrows the
display for as long as it is awaited, so no other branch of the same `select!` may touch the
display — hence the `from_compositor` flag and the work after. For a loop that must await other
things while the display sits idle, `Reactor::socket` borrows the reactor alone and composes
with anything.

## Why not winit?

For most projects, [winit] is the right tool and you should use it. It is mature, it is
cross-platform, and it has pointer input, text input, clipboard, and monitor enumeration —
none of which exist here yet. This crate is not trying to beat it.

[winit]: https://github.com/rust-windowing/winit
[#1199]: https://github.com/rust-windowing/winit/issues/1199
[#3506]: https://github.com/rust-windowing/winit/issues/3506

| | winit 0.30 | aufhebung 0.1 |
|---|---|---|
| Platforms | Windows, macOS, Linux (X11 + Wayland), Android, iOS, web | Wayland only |
| Pointer, text input, clipboard, monitors | yes | none of these |
| Sub-surfaces | no, [open issue][#3506] | yes |
| Maturity | widely depended on | one example |
| System libraries | `libwayland-dev` and `pkg-config` to build, `libxkbcommon` at runtime | [none](#system-libraries) |
| Event loop | blocking `run`, normally the main thread, no native async | you own the socket and write the `select!` |
| Abstraction level | `Window`, `ApplicationHandler` | `wl_surface`, roles, configs, buffers |

Four reasons to reach for this instead:

- **You are Wayland-only** and want no X11, Windows, or macOS code in the tree.
- **You need sub-surfaces.** winit is toplevel-only, so the usual workaround is to run
  `wayland-client` alongside it and manage the second connection yourself. The snake example
  is built entirely out of sub-surfaces.
- **You want async to be the default rather than a port.** winit has no native async story —
  it is a blocking loop you call `run` on, usually pinned to the main thread, and the
  conventional fix is to run tokio on other threads and ferry events over a channel. That
  integration is still an [open question upstream][#1199]. This crate has no loop to run: you
  hold the socket, so the event loop is one branch of a `select!` and the runtime is optional.
- **You would rather not ship system libraries.** No `libwayland` or `libxkbcommon` to install
  on the target, and no C toolchain to cross-compile with. See
  [System libraries](#system-libraries).

If none of those four apply, winit is the better tool.

## System libraries

No shared-library dependencies. No `libwayland`, no `libxkbcommon`, no `libX11`.

- **`wayland-client`'s `system` and `dlopen` features are off**, so it uses the pure-Rust
  rustix backend and talks to the compositor socket directly.
- **Keymaps go through [kbvm]**, a Rust implementation of xkbcommon, rather than libxkbcommon.
- **No C compiler runs.** Protocol code is generated at compile time by the `wayland-scanner`
  proc-macro. The one `cc` build-dependency in the graph belongs to `wayland-backend`'s
  logging shim, behind a feature this crate does not enable.

Two things are still expected of the system, neither a library: the `xkeyboard-config` data
files that kbvm compiles keymaps against, which every desktop ships and which its build script
falls back to `/usr/share/X11/xkb` for when `pkg-config` cannot locate them; and a Wayland
compositor to connect to.

[kbvm]: https://crates.io/crates/kbvm

## Limitations

- No pointer input, frame callbacks, text input, clipboard, or `wl_output`.
- Decoration is server-side only, and the mode the compositor settles on is not read back.
- `translate_key` returns keysyms. `translate_char` is the convenient form, but a key that is
  neither a character nor one of the eight it knows about is still only reachable as a keysym.

## Requirements

A Wayland compositor, and `wayland-client`. `rust-toolchain.toml` pins nightly; nothing in the
crate requires it.
