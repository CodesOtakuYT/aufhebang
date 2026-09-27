//! Cookie clicker: a cookie, an oven, and a pointer.
//!
//! The cookie is a coloured rectangle and so is the oven, both of them
//! sub-surfaces of the window, which is the background behind them. Nothing is
//! drawn: every surface carries one solid colour scaled to fill it, and the game
//! is played by committing different ones.
//!
//! This is the one example that does not use Tokio. Nothing here is driven by a
//! timer — the game only moves when the pointer does — so the blocking
//! `Display::dispatch` loop is the whole runtime, and this is what the library
//! looks like with no features enabled at all.
//!
//! A click arrives as a `PointerEvent::Button` naming the surface it landed on.
//! Because a sub-surface reports positions relative to itself, deciding that a
//! click hit the cookie needs no geometry at all: the surface's id is the
//! answer, and this example never reads the `x` and `y` the event also carries.
//! The `q` key is handled through `Display::translate_char`, the same way the
//! Snake example does it.

use anyhow::Result;
use aufhebung::{
    color::Color,
    display::Display,
    state::{Button, Event, PointerEvent, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
};

// Re-exported rather than depended on directly: declaring our own
// `wayland-client` would have to match this crate's git revision exactly, or
// the `WlBuffer` types would not unify.
use aufhebung::wayland_client::protocol::wl_buffer::WlBuffer;

const WIDTH: i32 = 520;
const HEIGHT: i32 = 380;

/// A sub-surface's size is fixed when it is created — only a toplevel is ever
/// configured again — so these are what they stay.
const COOKIE: i32 = 220;
const OVEN_W: i32 = 210;
const OVEN_H: i32 = 92;
const GAP: i32 = 30;

/// How many steps of browning the cookie palette has, and so how many ovens there
/// are to buy before it stops getting any darker.
const SHADES: usize = 12;

struct Inks {
    /// One buffer per shade, for the cookie at rest.
    dough: Vec<WlBuffer>,
    /// The same shades lightened, for the cookie under the pointer.
    dough_lit: Vec<WlBuffer>,
    /// The oven, bright when its price is affordable and dim when it is not.
    oven_hot: WlBuffer,
    oven_cold: WlBuffer,
}

/// What the next oven costs: ten cookies, doubling. That is the whole reason to
/// come back to a clicker.
fn price(ovens: u32) -> u32 {
    10 << ovens.min(12)
}

/// Moves one channel along the shade ramp. `level` runs 0 to `SHADES - 1` and
/// works in either direction, so this lightens as well as darkens.
fn mix(from: u8, to: u8, level: usize) -> u8 {
    let step = level as i32 * 255 / (SHADES - 1) as i32;
    let from = from as i32;
    (from + (to as i32 - from) * step / 255).clamp(0, 255) as u8
}

/// Pale dough browned to a well-baked cookie, or the same lightened because the
/// pointer is on it.
fn dough(level: usize, lit: bool) -> Color {
    let (r, g, b) = if lit {
        (0xff, 0xf6, 0xd6)
    } else {
        (0xc8, 0x93, 0x50)
    };

    Color::rgb(
        mix(0xf2, r, level),
        mix(0xe4, g, level),
        mix(0xc8, b, level),
    )
}

struct Clicker {
    window: SurfaceId,
    cookie: SurfaceId,
    oven: SurfaceId,
    bg: WlBuffer,
    inks: Inks,

    cookies: u32,
    per_click: u32,
    ovens: u32,

    /// Whether the pointer is on the cookie. The oven needs no hover state of
    /// its own: whether it is affordable is already a colour.
    lit: bool,
}

impl Clicker {
    /// The shade for the current upgrade, or the last one once the oven ladder
    /// runs out.
    fn level(&self) -> usize {
        (self.ovens as usize).min(SHADES - 1)
    }

    fn affordable(&self) -> bool {
        self.cookies >= price(self.ovens)
    }

    /// Centres the children in whatever size the compositor actually gave, so a
    /// window it refused to keep at the requested size still looks deliberate.
    fn layout(&self, display: &Display, width: i32, height: i32) {
        let top = ((height - COOKIE - GAP - OVEN_H) / 2).max(0);

        if let Some(cookie) = display.surface(self.cookie) {
            cookie.set_position(((width - COOKIE) / 2).max(0), top);
        }

        if let Some(oven) = display.surface(self.oven) {
            oven.set_position(((width - OVEN_W) / 2).max(0), top + COOKIE + GAP);
        }
    }

    /// The title doubles as the score, and starts as the one instruction a new
    /// player needs.
    fn title(&self) -> String {
        if self.cookies == 0 && self.ovens == 0 {
            return "click the cookie".into();
        }

        format!(
            "{} cookies - {}/click - next oven {}",
            self.cookies,
            self.per_click,
            price(self.ovens)
        )
    }

    /// Commits both children, then the background.
    ///
    /// The children are synchronized sub-surfaces, so their contents are only
    /// cached until the parent is committed after them. Without that last
    /// commit the window would go on showing what it showed before.
    fn paint(&self, display: &Display) {
        let dough = if self.lit {
            &self.inks.dough_lit
        } else {
            &self.inks.dough
        };

        display.commit(self.cookie, &dough[self.level()]);
        display.commit(
            self.oven,
            if self.affordable() {
                &self.inks.oven_hot
            } else {
                &self.inks.oven_cold
            },
        );
        display.commit(self.window, &self.bg);
    }

    /// Bakes, or buys. Returns whether anything changed, so that a click on the
    /// bare window costs no repaint.
    fn click(&mut self, display: &mut Display, surface: SurfaceId) -> bool {
        if surface == self.cookie {
            self.cookies += self.per_click;
        } else if surface == self.oven {
            // The colour already says an unaffordable oven is not for sale, but
            // the check still has to be here: a click is a click.
            if !self.affordable() {
                return false;
            }

            self.cookies -= price(self.ovens);
            self.ovens += 1;
            self.per_click += 1;
        } else {
            return false;
        }

        display.set_title(self.window, &self.title());
        self.paint(display);
        true
    }

    /// Takes the pointer's surface, or `None` where it has left ours. Returns
    /// whether the hover state moved, which is the only thing here that can
    /// need a repaint of its own.
    fn hover(&mut self, display: &Display, over: Option<SurfaceId>) -> bool {
        let lit = over == Some(self.cookie);
        if lit == self.lit {
            return false;
        }

        self.lit = lit;
        self.paint(display);
        true
    }
}

fn main() -> Result<()> {
    let mut display = Display::new()?;

    // A palette rather than a colour per state: one buffer is committed to many
    // surfaces, and changing shade is a commit rather than a new buffer. Two
    // ramps, because the cookie also lightens under the pointer.
    let mut palette = Vec::new();
    let mut palette_lit = Vec::new();
    for level in 0..SHADES {
        palette.push(display.add_color(dough(level, false)));
        palette_lit.push(display.add_color(dough(level, true)));
    }

    let inks = Inks {
        dough: palette,
        dough_lit: palette_lit,
        oven_hot: display.add_color(Color::hex(0xe8_7a_3c)),
        oven_cold: display.add_color(Color::hex(0x4a_3a_33)),
    };

    let bg = display.add_color(Color::hex(0x18_1a_22));

    let window = display
        .add_surface(SurfaceInfo {
            width: WIDTH,
            height: HEIGHT,
            role: SurfaceRole::Window {
                title: "cookie clicker".into(),
            },
        })
        .expect("surface id space exhausted");

    // The children need the configured size to be positioned, which is not known
    // yet, and cannot be committed until the configure makes that legal. They
    // are created now so the requests are already queued when it arrives, and
    // committed from the configure below.
    let child = |display: &mut Display, width: i32, height: i32| {
        display
            .add_surface(SurfaceInfo {
                width,
                height,
                role: SurfaceRole::Subsurface {
                    parent: window,
                    x: 0,
                    y: 0,
                    sync: true,
                },
            })
            .expect("a compositor without a subcompositor cannot host a cookie")
    };

    let cookie = child(&mut display, COOKIE, COOKIE);
    let oven = child(&mut display, OVEN_W, OVEN_H);

    let mut clicker = Clicker {
        window,
        cookie,
        oven,
        bg,
        inks,
        cookies: 0,
        per_click: 1,
        ovens: 0,
        lit: false,
    };

    // A fixed-size window, because the layout is a fixed arrangement of
    // rectangles. The compositor may ignore the request, so the configure below
    // still reads the size it actually gave.
    display.set_size_limits(window, Some((WIDTH, HEIGHT)), Some((WIDTH, HEIGHT)));

    // Push the creation requests before waiting: the compositor cannot configure
    // a surface it has not received yet.
    display.flush()?;

    println!("click the cookie to bake, the oven to buy one that bakes more, q to quit");

    loop {
        display.dispatch()?;

        for event in display.events() {
            match event {
                Event::SurfaceEvent {
                    id,
                    event: SurfaceEvent::Configure { width, height },
                } => {
                    if id != window {
                        continue;
                    }

                    clicker.layout(&display, width, height);

                    // The configure has arrived, so the first buffer commit is
                    // now legal — and the window is mapped from here, so this is
                    // also the earliest a pointer event can arrive.
                    clicker.paint(&display);
                }

                Event::SeatEvent { id: seat, event } => match event {
                    SeatEvent::Pointer(PointerEvent::Button {
                        surface,
                        button: Button::Left,
                        pressed: true,
                        ..
                    }) => {
                        clicker.click(&mut display, surface);
                    }

                    // Hover needs the surface the pointer is over rather than a
                    // direction, and both the events that carry a position
                    // supply it.
                    SeatEvent::Pointer(
                        PointerEvent::Enter { surface, .. } | PointerEvent::Motion { surface, .. },
                    ) => {
                        clicker.hover(&display, Some(surface));
                    }

                    SeatEvent::Pointer(PointerEvent::Leave { .. }) => {
                        clicker.hover(&display, None);
                    }

                    SeatEvent::Key {
                        surface,
                        pressed,
                        key,
                        ..
                    } => {
                        // This window's own keys only: another surface can have
                        // the focus while this one is still on screen.
                        if surface == window
                            && pressed
                            && display.translate_char(seat, key) == Some('q')
                        {
                            return Ok(());
                        }
                    }

                    SeatEvent::Pointer(_) | SeatEvent::RepeatInfo { .. } => {}
                },
            }
        }

        if display.should_close(window) {
            break;
        }
    }

    Ok(())
}
