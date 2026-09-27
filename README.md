# aufhebung

A small, low-level windowing library for native Wayland applications.

`aufhebung` handles the Wayland bookkeeping that sits between an application and
`wl_surface`: surface roles, xdg-shell configure handshakes, toplevel metadata,
keyboard state, and event collection.

It does **not** own your event loop.

Buffers, timing, scheduling, and runtime integration remain in the application.
The core API is synchronous and blocking; the underlying Wayland socket is also
available directly, so an application can integrate it with any event loop or
runtime it wants.

> **Early software.** The API is unstable. The library currently focuses on
> windows, sub-surfaces, buffers, and keyboard input.

## Example

![Snake example](assets/snake.png)

```sh
cargo run --features tokio --example snake
```

The Snake example is intentionally built from the library's low-level pieces:

* the window is one `wl_surface`;
* every snake segment and the food are sub-surfaces;
* each colour is a tiny `wl_buffer` scaled to fill its surface;
* normal movement reuses the tail surface instead of allocating a new one;
* movement is driven by a Tokio timer;
* compositor events and timer ticks are handled by the same `tokio::select!`.

Use `w/a/s/d`, `h/j/k/l`, or the arrow keys to steer. Press `p` or Space to
pause and `q` to quit.

After a collision, the board stays still while the title flashes. Once the
flash ends, any key starts another round.

The example is also a demonstration of async integration: `aufhebung` exposes
the Wayland connection without requiring a runtime, while the application
decides how that socket participates in its event loop.

## Usage

The smallest application can use the blocking dispatcher directly:

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
            role: SurfaceRole::Window {
                title: "hello".into(),
            },
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
                println!("{id:?} configured to {width}x{height}");
                display.surface(id).unwrap().commit(&white);
            }
        }
    }
}
```

A toplevel must receive its first `configure` before the application commits
its first buffer. `aufhebung` exposes that state directly rather than hiding
the handshake behind a window abstraction.

## API

### `Display`

The main entry point.

* `new` connects to the Wayland compositor.
* `add_surface` and `remove_surface` manage surfaces by `SurfaceId`.
* `surface` and `surfaces` provide access to existing surfaces.
* `dispatch` runs the complete blocking event loop.
* `connection_fd`, `socket`, `prepare_read`, `dispatch_pending`, and `flush`
  expose the pieces needed to integrate the connection into another event loop.
* `events` drains events collected by the library.
* `translate_key` resolves a key through the compositor's keyboard map.
* `translate_char` resolves a key to a character and provides character
  mappings for navigation keys.
* `add_color` creates a buffer containing a solid colour.

### `Surface`

A handle to a Wayland surface.

* `commit` attaches a buffer, fills the surface with it, and damages the full
  surface.
* `set_position` moves a sub-surface.
* `set_size_limits` constrains a toplevel's requested size.
* `is_configured` reports whether a toplevel has completed its initial
  configure handshake.
* `title`, `set_title`, and `should_close` expose toplevel state.

### `SurfaceRole`

A surface can be created with one of three roles:

```rust
SurfaceRole::Window {
    title: "hello".into(),
}
```

A toplevel window.

```rust
SurfaceRole::Subsurface {
    parent,
    x,
    y,
    sync,
}
```

A positioned child surface.

```rust
SurfaceRole::None
```

A bare `wl_surface`.

### `Color`

`Color` represents an RGBA colour with 8 bits per channel and provides
convenient conversions from common colour literals.

`Display::add_color` turns a colour into a buffer that can be committed to a
surface.

## Tokio

Tokio support is optional:

```toml
[dependencies]
aufhebung = { version = "0.1", features = ["tokio"] }
```

The `tokio` feature provides `Reactor`, which registers the Wayland connection
socket with Tokio and combines waiting, reading, dispatching, and flushing into
one operation:

```rust
let mut reactor = Reactor::new(&display)?;

let from_compositor = tokio::select! {
    biased;

    read = reactor.recv(&mut display) => {
        read?;
        true
    }

    _ = stepper.tick() => false,
};
```

The feature is disabled by default.

`Reactor` does not introduce a second abstraction around the application event
loop. It builds on the same public `Display` operations that are available
without Tokio.

Other runtimes can integrate directly with the connection through
`Display::connection_fd` and the lower-level read/dispatch methods.

## Why not winit?

For most applications, [winit] is the right choice. It is mature,
cross-platform, widely used, and provides facilities that `aufhebung` does not
attempt to provide.

`aufhebung` exists for applications that specifically want a smaller,
Wayland-native layer.

|                         | winit 0.30                               | aufhebung                         |
| ----------------------- | ---------------------------------------- | --------------------------------- |
| Platforms               | Windows, macOS, Linux, Android, iOS, web | Wayland                           |
| Window abstraction      | `Window`, `ApplicationHandler`           | `wl_surface`, roles, configs      |
| Pointer input           | Yes                                      | No                                |
| Text input              | Yes                                      | No                                |
| Clipboard               | Yes                                      | No                                |
| Monitor/output handling | Yes                                      | No                                |
| Sub-surfaces            | Not directly                             | Yes                               |
| Event loop              | Owns the application loop                | Application owns the socket       |
| Async integration       | External integration required            | Direct socket integration         |
| Scope                   | Cross-platform windowing                 | Wayland surface/window primitives |

There are a few situations where `aufhebung` may be a better fit:

**Wayland-only applications.**
If the application does not need X11, Windows, macOS, or other platforms,
there is no need to carry those abstractions.

**Sub-surface-heavy applications.**
Sub-surfaces are first-class objects rather than something the application has
to reach around the window abstraction to obtain.

**Application-owned event loops.**
The library does not insist on owning the main loop. The Wayland socket can be
one source in an existing `select!`, poller, reactor, or runtime.

**A small dependency surface.**
The Wayland protocol and keyboard handling are implemented without depending
on the system Wayland or X11 libraries.

[winit]: https://github.com/rust-windowing/winit

## System libraries

The crate does not require shared Wayland, X11, or xkbcommon libraries on the
target system.

`wayland-client`'s `system` and `dlopen` features are disabled, so the
connection uses its Rust backend and communicates with the compositor socket
directly.

Keyboard maps are handled by [kbvm], a Rust implementation of the relevant
xkb functionality, rather than by `libxkbcommon`.

No external `xkeyboard-config` data files are required: the keyboard map is
received from the compositor as part of `wl_keyboard.keymap` and compiled in
memory.

Protocol bindings are generated at compile time by the Wayland protocol
machinery.

[kbvm]: https://crates.io/crates/kbvm

## Limitations

This is deliberately a small API. Currently it does **not** provide:

* pointer input;
* text input;
* clipboard support;
* `wl_output` / monitor enumeration;
* frame callbacks;
* client-side decorations;
* decoration mode reporting.

Keyboard handling currently exposes both translated characters and raw
keysyms. Keys that do not correspond to a character, beyond the navigation
keys handled by `translate_char`, are available through `translate_key`.

## Requirements

A Wayland compositor is the only runtime requirement.

The keyboard map is supplied by the compositor through the Wayland keyboard
protocol, so `xkeyboard-config` data files are not required at runtime.

The repository contains a `rust-toolchain.toml` that currently pins nightly
for development. The crate itself does not intentionally require nightly
language features.
