//! An image on a surface, drawn two different sizes.
//!
//! [`Display::add_pixels`] uploads real pixels through a `wl_shm` pool, and that
//! is the only way this library draws anything with detail in it: every other
//! buffer it makes is a single pixel scaled to fill a surface. The pixels here
//! are generated rather than decoded, so the example needs no image file and no
//! decoder — turning a PNG into `u32` values is the application's business, and
//! all this library ever asks for is the finished values.
//!
//! Two commits place the same buffer, and `1` and `2` switch between them,
//! which is the reason for the example:
//!
//! - `1` puts the image on a sub-surface with `commit_unscaled`, one pixel to one
//!   pixel, over a flat background. Only the image's own rectangle is damaged, so
//!   the colour behind it is left showing.
//! - `2` puts the same buffer on the window with `commit`, which stretches
//!   whatever it is given to fill the surface. That is exactly right for a
//!   colour, which is one pixel wide by design, and visibly wrong for a picture.
//!   The difference is not something the docs can be trusted to convey, so the
//!   example makes it pressable.
//!
//! Like the Cookie Clicker, this runs the blocking loop with no features enabled.

use anyhow::Result;
use aufhebung::{
    color::Color,
    display::Display,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
};

// Re-exported rather than depended on directly: declaring our own
// `wayland-client` would have to match this crate's git revision exactly, or
// the `WlBuffer` types would not unify.
use aufhebung::wayland_client::protocol::wl_buffer::WlBuffer;

const WIDTH: i32 = 520;
const HEIGHT: i32 = 380;

/// The image's own size, which is also the size the sub-surface stays: a
/// sub-surface is only ever configured once, at creation.
const IMAGE_W: i32 = 320;
const IMAGE_H: i32 = 240;

/// Which commit the last keypress asked for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The image at the size it was uploaded at, on a sub-surface, over a flat
    /// background.
    Native,
    /// The same image as the window's own buffer, stretched to the window.
    Stretched,
}

/// Hue in `0.0..=1.0`, saturation and value in `0.0..=1.0`, to a pixel packed
/// the way `add_pixels` takes them.
///
/// Every channel is below 1.0 and the alpha is opaque, so these are already
/// pre-multiplied — multiplying by an alpha of 1.0 changes nothing, which is why
/// the arithmetic never leaves floating point. A pixel with a real alpha would
/// have to be multiplied here, since the format wants it and `Color` does not
/// do it for us.
fn hsv(hue: f32, sat: f32, val: f32) -> u32 {
    // Clamped rather than trusted: a value that overshot 1.0 by rounding would
    // scale a channel past 255 and wrap it round into its neighbour.
    let val = val.clamp(0.0, 1.0);
    let sat = sat.clamp(0.0, 1.0);

    let sector = hue.rem_euclid(1.0) * 6.0;
    let f = sector - sector.floor();
    let p = val * (1.0 - sat);
    let q = val * (1.0 - sat * f);
    let t = val * (1.0 - sat * (1.0 - f));

    let (r, g, b) = match sector as u32 % 6 {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    };

    // Scaled on the way out, and this is not optional bookkeeping. These are
    // fractions of full brightness, so cast straight to a `u32` they all
    // truncate to zero and the whole picture comes out opaque black — which is
    // exactly what it looks like, and exactly what it did before this line.
    let byte = |c: f32| (c * 255.0).round() as u32;
    (0xff << 24) | (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

/// Three sine waves summed, which lands anywhere in `-3.0..=3.0`, mapped onto a
/// hue circle and brightened by how far from flat it is.
///
/// A plasma rather than anything meaningful, because the point is that there is
/// detail here at all: a single flat colour scaled up is what the rest of this
/// library can do, and a picture is the one thing it cannot fake.
fn plasma(width: i32, height: i32) -> Vec<u32> {
    let (w, h) = (width as f32, height as f32);
    let mut pixels = Vec::with_capacity((width * height) as usize);

    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let v = (fx / w * 6.0).sin() + (fy / h * 5.0).sin() + ((fx + fy) / w * 4.0).sin();
            let hue = (v + 3.0) / 6.0;
            // The square root lifts the flat parts, which is where a sum of
            // sines spends most of its range: mapped linearly the picture is
            // almost all near-black and the pattern is hard to see.
            let val = 0.25 + 0.75 * (v.abs() / 3.0).sqrt();
            pixels.push(hsv(hue, 0.8, val));
        }
    }

    pixels
}

struct Picture {
    window: SurfaceId,
    image: SurfaceId,
    /// The upload, at the size it was uploaded at.
    photo: WlBuffer,
    /// The background the image sits on, in the native mode.
    bg: WlBuffer,
    /// Fully transparent, and what takes the sub-surface out of the way in the
    /// stretched mode. Scaled over the sub-surface it composites to nothing,
    /// which is the one thing a colour buffer is genuinely good at.
    clear: WlBuffer,
    mode: Mode,
}

impl Picture {
    /// Centres the image in whatever size the compositor actually gave, so a
    /// window it refused to keep at the requested size still looks deliberate.
    fn layout(&self, display: &Display, width: i32, height: i32) {
        if let Some(image) = display.surface(self.image) {
            image.set_position((width - IMAGE_W) / 2, (height - IMAGE_H) / 2);
        }
    }

    /// Commits the sub-surface, then the window.
    ///
    /// The order matters and not only for tidiness: the sub-surface is
    /// synchronized, so its contents are only cached until the parent is
    /// committed after it. Without that second commit the window would go on
    /// showing what it showed before.
    fn paint(&self, display: &Display) {
        match self.mode {
            Mode::Native => {
                display.commit_unscaled(self.image, &self.photo, IMAGE_W, IMAGE_H);
                display.commit(self.window, &self.bg);
            }
            Mode::Stretched => {
                display.commit(self.image, &self.clear);
                display.commit(self.window, &self.photo);
            }
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Native => "pixels: one pixel to one pixel - 1:1, 2: stretch, q: quit",
            Mode::Stretched => "pixels: stretched to the window - 1: 1:1, 2: stretch, q: quit",
        }
    }

    /// Switches modes and repaints. Returns whether anything changed, so a key
    /// that was already the current mode costs no repaint.
    fn set_mode(&mut self, display: &mut Display, mode: Mode) -> bool {
        if mode == self.mode {
            return false;
        }

        self.mode = mode;
        display.set_title(self.window, self.title());
        self.paint(display);
        true
    }
}

fn main() -> Result<()> {
    let mut display = Display::new()?;

    // Uploaded once and committed as many times as we like, which is what
    // `add_pixels` is for. Its pixels are not written again afterwards: the
    // compositor may still be reading them, and nothing in this library would
    // say when it had stopped.
    let plasma = plasma(IMAGE_W, IMAGE_H);
    let Some(photo) = display.add_pixels(IMAGE_W, IMAGE_H, &plasma) else {
        anyhow::bail!(
            "{}x{} does not fit in a shared memory pool",
            IMAGE_W,
            IMAGE_H
        );
    };

    let window = display
        .add_surface(SurfaceInfo {
            width: WIDTH,
            height: HEIGHT,
            role: SurfaceRole::Window {
                title: "pixels".into(),
            },
        })
        .expect("surface id space exhausted");

    // The image gets its own surface, because a surface carries one buffer and
    // the background needs one too. Its size is whatever it was created with, so
    // the image is uploaded at the size it will stay.
    let image = display
        .add_surface(SurfaceInfo {
            width: IMAGE_W,
            height: IMAGE_H,
            role: SurfaceRole::Subsurface {
                parent: window,
                x: 0,
                y: 0,
                sync: true,
            },
        })
        .expect("a compositor without a subcompositor cannot host an image");

    let mut picture = Picture {
        window,
        image,
        photo,
        bg: display.add_color(Color::hex(0x18_1a_22)),
        clear: display.add_color(Color::TRANSPARENT),
        mode: Mode::Native,
    };
    display.set_title(window, picture.title());

    // A fixed-size window, because the image is a fixed size and its position is
    // relative to the window. The compositor may ignore the request, so the
    // configure below still reads the size it actually gave.
    display.set_size_limits(window, Some((WIDTH, HEIGHT)), Some((WIDTH, HEIGHT)));

    // Push the creation requests before waiting: the compositor cannot configure
    // a surface it has not received yet.
    display.flush()?;

    println!("1 for the image at its own size, 2 stretched over the window, q to quit");

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

                    picture.layout(&display, width, height);

                    // The configure has arrived, so the first buffer commit is
                    // now legal.
                    picture.paint(&display);
                }

                Event::SeatEvent { id: seat, event } => match event {
                    SeatEvent::Key {
                        surface,
                        pressed,
                        key,
                        ..
                    } => {
                        // This window's own keys only: another surface can have
                        // the focus while this one is still on screen.
                        if surface != window || !pressed {
                            continue;
                        }

                        match display.translate_char(seat, key) {
                            Some('1') => {
                                picture.set_mode(&mut display, Mode::Native);
                            }
                            Some('2') => {
                                picture.set_mode(&mut display, Mode::Stretched);
                            }
                            Some('q') => return Ok(()),
                            _ => {}
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
