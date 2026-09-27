//! Text, rasterized on the CPU and handed to [`Display::add_pixels`].
//!
//! [`add_pixels`] is the only way this library draws anything with detail in it,
//! and text is what that method is for: everything else in the crate is one pixel
//! scaled to fill a surface, and no amount of scaling turns a colour into a word.
//! The pixels go in the same way — a `Vec<u32>` of pre-multiplied ARGB — so the
//! only thing a text example adds is how those `u32`s are made.
//!
//! Which is the part worth showing, because [`add_pixels`] is an *upload*, not a
//! draw. It does not composite: hand it a buffer and the compositor takes it
//! exactly as written. A text rasterizer cannot help with that, since none of
//! them produce finished pixels — [`ab_glyph`] hands back a coverage value per
//! pixel, `swash` an alpha bitmap, `cosmic-text` a list of positioned quads. So
//! the coverage has to be blended into something before it can be uploaded, and
//! [`over`](over) plus the few lines in [`rasterize`] are that step. About thirty
//! lines, once, which is why this crate has no text feature: a feature that
//! re-exported somebody else's rasterizer would be a larger thing than the thing
//! it replaced.
//!
//! Three commits, switched with `1`, `2` and `3`, and the reason for the example
//! is what they differ on:
//!
//! - `1` puts 32px text on a sub-surface with `commit_unscaled`, one pixel to one
//!   pixel, over a flat background. Correct, and the only way to draw text.
//! - `2` puts *the same buffer* on a sub-surface with `commit`, which stretches
//!   whatever it is given to fill the surface. Right for a colour, which is one
//!   pixel wide by design; visibly wrong for text, because a text raster has
//!   antialiased edges that are correct for exactly one size. This is the same
//!   pair of commits as `pixels.rs`, where the stretch is merely soft. Here it is
//!   the mistake, and pressing `1` after `2` is the lesson.
//! - `3` goes back to `commit_unscaled` with the same string rasterized at 64px.
//!   Compare it with `1`: sharper, and the buffer is twice the size. This is what
//!   to do instead of `2` — rasterize at the size the text will be shown at, and
//!   upload that.
//!
//! The surface in `2` is a sub-surface rather than the window, and the difference
//! is not cosmetic. A text raster is transparent around its ink, so committing
//! one to the window makes the *window* transparent, and what you end up looking
//! at is the see-through rather than the softness. Over a background colour the
//! same stretch is unmistakably what is wrong.
//!
//! Nothing here scales a text buffer, which is the point, and it is also why
//! there is no key that changes the size: the sizes that exist are the sizes
//! that were uploaded, because [`add_pixels`] is a one-shot upload whose pixels
//! are never written again afterwards.
//!
//! ## What this does not do
//!
//! [`ab_glyph`] outlines and rasterizes a glyph at a position you give it, and
//! nothing else. There is no shaping, so no kerning, no ligatures, and no
//! complex-script or bidirectional reordering — Latin in a plain left-to-right
//! run is the whole range it covers well. There is no hinting either, so glyph
//! stems are wherever the outline puts them rather than snapped to the pixel
//! grid. The font in `assets/ascii.ttf` is a subset covering printable ASCII, so
//! anything outside that range comes out as whatever `.notdef` is.
//!
//! A real application wants [`swash`](https://docs.rs/swash) or
//! [`cosmic-text`](https://docs.rs/cosmic-text) for that, which is why the
//! comparison of them is a note in the README rather than a choice baked in
//! here: the integration above does not change whichever one is used, because
//! they all hand back coverage.
//!
//! Like the Cookie Clicker, this runs the blocking loop with no features enabled.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use anyhow::Result;
use aufhebung::{
    buffer::BufferId,
    color::Color,
    display::Display,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
};

const WIDTH: i32 = 520;
const HEIGHT: i32 = 300;

/// The three lines drawn, laid out one per line. Three rather than one so the
/// raster is not so much wider than it is tall that `2` distorts the aspect as
/// well as softening the edges, which would be a second mistake on top of the
/// one the mode is about.
const LINES: [&str; 3] = ["aufhebung", "rasterize at", "final size"];

/// Both sizes are rasterized up front, at startup, and both are uploaded then
/// too — `add_pixels` is a one-shot upload, so a size the user cannot reach is a
/// size that costs nothing and one they can reach is a buffer per frame.
const SMALL: f32 = 32.0;
const LARGE: f32 = 64.0;

/// Near-white, on the dark background. [`Color`] carries straight alpha and
/// pre-multiplies it on the way out, which is exactly the step a text raster
/// needs and exactly the one `add_pixels` refuses to do for us.
const INK: u32 = 0xf0_f2_f7;

/// Empty space around the ink, so no antialiased edge lands on the buffer's
/// boundary and gets clipped.
const PAD: i32 = 4;

/// Space between lines, as a multiple of the em.
///
/// The font asks for none of it: `line_gap` is `0`, and `ascent - descent` comes
/// out as exactly one em, because `ab_glyph` divides both by the font's own
/// line box so that the pair always sums to the nominal size. One em is tight
/// enough that a descender sits close under the next line's ascenders, and real
/// text layout engines add leading on top of the font's metrics — so this does
/// too, and the 1.25 it lands on is an ordinary body-text line height.
const LEADING: f32 = 0.25;

/// A finished upload, ready for `add_pixels`.
struct Raster {
    width: i32,
    height: i32,
    pixels: Vec<u32>,
}

/// Puts a pre-multiplied source pixel over a pre-multiplied destination pixel.
///
/// The two early returns are not an optimisation: they are the cases where the
/// arithmetic below would divide by nothing useful, and a source that is fully
/// transparent has to leave the destination alone rather than scale it.
///
/// Note what is *not* here. `over` does not composite onto an opaque
/// background — the destination is a pre-multiplied pixel like the source, alpha
/// included, and it comes back pre-multiplied. The window's own colour buffer
/// supplies the background, which is why the text's own background is left
/// transparent rather than being filled in with the colour it will be seen on.
fn over(src: u32, dst: u32) -> u32 {
    let src_a = src >> 24;
    match src_a {
        0 => return dst,
        0xff => return src,
        _ => {}
    }
    let inv = 0xff - src_a;

    // Rounded rather than truncated, on both halves. Antialiasing lives in the
    // low bits of this: a coverage of 0.4 truncated to 0 or to 1 depending on the
    // pixel is the difference between a smooth edge and a staircase.
    let mix = |s: u32, d: u32| (s + (d * inv + 0x7f) / 0xff).min(0xff);
    let a = (src_a + ((dst >> 24) * inv + 0x7f) / 0xff).min(0xff);

    (a << 24)
        | (mix(src >> 16 & 0xff, dst >> 16 & 0xff) << 16)
        | (mix(src >> 8 & 0xff, dst >> 8 & 0xff) << 8)
        | mix(src & 0xff, dst & 0xff)
}

/// Draws `lines` at `size` pixels and returns a tightly cropped, pre-multiplied
/// raster on a transparent background.
///
/// Measured before it is drawn, because the widest line decides the width and
/// the glyph positions need to be known before anything is written: a `Vec`
/// sized from a guess that the real extents then exceed is a panic, and
/// `ab_glyph` is happy to hand back a `min` that is negative.
fn rasterize(font: &FontRef, lines: &[&str], size: f32, ink: Color) -> Raster {
    let scale = PxScale::from(size);
    let scaled = ab_glyph::PxScaleFont {
        font: font.clone(),
        scale,
    };

    // Rounded up before anything is placed against it, so that a baseline can
    // never land on a fraction and two lines cannot accumulate a half-pixel
    // drift into a visibly uneven gap.
    let ascent = scaled.ascent().ceil() as i32;
    let descent = (-scaled.descent()).ceil() as i32;
    let leading = (LEADING * size).round() as i32;
    let line_height = ascent + descent + leading;

    let mut advance = 0.0f32;
    for line in lines {
        let mut pen = 0.0f32;
        for c in line.chars() {
            pen += scaled.h_advance(scaled.glyph_id(c));
        }
        advance = advance.max(pen);
    }

    let width = advance.ceil() as i32 + 2 * PAD;
    let height = ascent + descent + (lines.len() as i32 - 1) * line_height + 2 * PAD;
    let mut pixels = vec![0u32; (width * height) as usize];

    // `Color::channels` pre-multiplies by alpha on its way out, in the
    // protocol's percentage scale. Narrowing with a shift gives each channel
    // back as the 8-bit value, already scaled by the ink's alpha — so coverage
    // only has to cover the rest of the distance, and the same arithmetic the
    // crate already does inside `Color` is the same arithmetic the buffer wants.
    let (r, g, b, ink_a) = ink.channels();
    let (r, g, b) = (r >> 24, g >> 24, b >> 24);
    let ink_a = ink_a >> 24;

    for (row, line) in lines.iter().enumerate() {
        // Relative to the first baseline rather than to the canvas, so that
        // `bounds.min` — which is measured from the baseline and runs negative
        // above it — needs one ascent of correction, applied once, below.
        let baseline = (row as i32 * line_height) as f32;
        let mut pen = 0.0f32;

        for c in line.chars() {
            let id = font.glyph_id(c);
            let placed = id.with_scale_and_position(scale, ab_glyph::point(pen, baseline));
            pen += scaled.h_advance(id);

            // A space has no outline, which is a normal outcome and not an error.
            let Some(outline) = font.outline_glyph(placed) else {
                continue;
            };
            // Rounded to the nearest pixel rather than cast, which truncates.
            // Truncation rounds towards zero, and `min.y` is negative for every
            // glyph above its baseline — so it would push the whole string up to
            // a pixel off the position the outline asked for, one way, on every
            // line.
            let bounds = outline.px_bounds();
            let (bx, by) = (bounds.min.x.round() as i32, bounds.min.y.round() as i32);

            outline.draw(|x, y, coverage| {
                // `draw` hands back bitmap coordinates counting from zero, and
                // `px_bounds` is where that bitmap sits relative to the baseline.
                // Adding them is the whole of the placement, and forgetting the
                // `PAD` on both axes is invisible until the ink touches an edge.
                let dx = x as i32 + bx + PAD;
                let dy = y as i32 + by + ascent + PAD;
                if dx < 0 || dy < 0 || dx >= width || dy >= height {
                    return;
                }

                // Coverage in, one pre-multiplied pixel out.
                let a = (coverage * ink_a as f32).round().min(255.0) as u32;
                if a == 0 {
                    return;
                }
                let channel = |c: u32| ((c as f32 * coverage).round().min(255.0)) as u32;
                let src = (a << 24) | (channel(r) << 16) | (channel(g) << 8) | channel(b);

                let at = (dy as usize) * width as usize + dx as usize;
                pixels[at] = over(src, pixels[at]);
            });
        }
    }

    Raster {
        width,
        height,
        pixels,
    }
}

/// Which commit the last keypress asked for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// 32px text at the size it was rasterized at, on a sub-surface, over a flat
    /// background.
    Native,
    /// The same 32px raster, on a surface of the window's size, scaled up to fill
    /// it — over the same background, so all that changes is the text.
    Stretched,
    /// The same string again, rasterized at 64px and committed the same way.
    Large,
}

struct Sheet {
    window: SurfaceId,
    /// Sized to [`SMALL`], so `commit_unscaled` puts the ink in it exactly.
    small: SurfaceId,
    /// Sized to [`LARGE`], and therefore never resized for the small text.
    large: SurfaceId,
    /// Sized to the window, and what the stretched mode stretches into.
    fill: SurfaceId,
    /// The 32px upload, at the size it was uploaded at.
    small_pixels: BufferId,
    /// The 64px upload of the same string.
    large_pixels: BufferId,
    /// The background the text sits on, in all three modes.
    bg: BufferId,
    /// Fully transparent, and what takes an unused sub-surface out of the way. A
    /// single pixel stretched over the whole sub-surface composites to nothing,
    /// which is the one thing a colour buffer is genuinely good at.
    clear: BufferId,
    mode: Mode,
}

impl Sheet {
    /// Centres the two text sub-surfaces in whatever size the compositor actually
    /// gave, so a window it refused to keep at the requested size still looks
    /// deliberate. `fill` is not centred: it is meant to cover the window, and it
    /// is created at the requested size because a sub-surface cannot be resized
    /// to match one the compositor chose instead.
    fn layout(&self, display: &Display, width: i32, height: i32) {
        for (id, pixels) in [
            (self.small, self.small_pixels),
            (self.large, self.large_pixels),
        ] {
            // Both halves are ids this struct was handed, so a miss would be a
            // bug rather than a race — but `layout` runs on a configure, and a
            // `continue` keeps a surprise there from ending the example.
            let (Some(raster), Some(surface)) = (display.buffer(pixels), display.surface(id))
            else {
                continue;
            };
            surface.set_position((width - raster.width()) / 2, (height - raster.height()) / 2);
        }
    }

    /// Commits the sub-surfaces, then the window.
    ///
    /// The order matters and not only for tidiness: a sub-surface is
    /// synchronized, so its contents are only cached until the parent is
    /// committed after it. Without that second commit the window would go on
    /// showing what it showed before.
    fn paint(&self, display: &Display) {
        match self.mode {
            Mode::Native => {
                display.commit(self.fill, self.clear);
                display.commit(self.large, self.clear);
                display.commit_unscaled(self.small, self.small_pixels);
                display.commit(self.window, self.bg);
            }
            Mode::Stretched => {
                // The same 32px buffer, and the same surface it went to in the
                // native mode, but committed with `commit` — so the compositor
                // scales a text raster up to fill it.
                //
                // The window keeps its colour rather than taking this buffer,
                // and that is the point of the sub-surface here. A text raster is
                // transparent around the ink, so committing it to the *window*
                // makes the window transparent too: what is on screen becomes the
                // stretching rather than the fact that the text is stretched,
                // which is the one thing this mode has to show. It is a real trap
                // rather than a quirk of this example — the same buffer is
                // correct in one commit and wrong in the other.
                display.commit(self.small, self.clear);
                display.commit(self.large, self.clear);
                display.commit(self.fill, self.small_pixels);
                display.commit(self.window, self.bg);
            }
            Mode::Large => {
                display.commit(self.fill, self.clear);
                display.commit(self.small, self.clear);
                display.commit_unscaled(self.large, self.large_pixels);
                display.commit(self.window, self.bg);
            }
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Native => "text: 32px, one pixel to one pixel - 1, 2, 3, q",
            Mode::Stretched => "text: 32px raster stretched - 1, 2, 3, q",
            Mode::Large => "text: 64px raster, one to one - 1, 2, 3, q",
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

    // Borrowed straight out of the binary rather than copied into a `FontVec`:
    // `include_bytes!` is already a `&'static [u8]`, which is what `FontRef`
    // wants, so loading the font allocates nothing and copies nothing.
    let font = FontRef::try_from_slice(include_bytes!("../assets/ascii.ttf"))
        .map_err(|_| anyhow::anyhow!("assets/ascii.ttf is not a font"))?;

    // Both rasters are built and uploaded once, before the window exists, so
    // their sizes are known and the sub-surfaces can be created at them: a
    // sub-surface is configured once, at creation, and never resized.
    let small = rasterize(&font, &LINES, SMALL, Color::hex(INK));
    let large = rasterize(&font, &LINES, LARGE, Color::hex(INK));

    let (Some(small_pixels), Some(large_pixels)) = (
        display.add_pixels(small.width, small.height, &small.pixels),
        display.add_pixels(large.width, large.height, &large.pixels),
    ) else {
        anyhow::bail!("the text does not fit in a shared memory pool");
    };

    let window = display
        .add_surface(SurfaceInfo {
            width: WIDTH,
            height: HEIGHT,
            role: SurfaceRole::Window {
                title: "text".into(),
            },
        })
        .expect("surface id space exhausted");

    // One sub-surface per raster, each at its own size, because a surface
    // carries one buffer and carries it at the size the surface was created
    // with. The transparent background around the ink is the point of the
    // sub-surface: it lets the window's own colour show through.
    let mut subsurface = |width: i32, height: i32| {
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
            .expect("a compositor without a subcompositor cannot host text")
    };

    let mut sheet = Sheet {
        window,
        small: subsurface(small.width, small.height),
        large: subsurface(large.width, large.height),
        // The size the window was asked for, which the size limits below hold it
        // to. This is the only surface the stretched mode scales the text into.
        fill: subsurface(WIDTH, HEIGHT),
        small_pixels,
        large_pixels,
        bg: display.add_color(Color::hex(0x18_1a_22)),
        clear: display.add_color(Color::TRANSPARENT),
        mode: Mode::Native,
    };
    display.set_title(window, sheet.title());

    // A fixed-size window, because the sub-surface positions are relative to the
    // window. The compositor may ignore the request, so the configure below
    // still reads the size it actually gave.
    display.set_size_limits(window, Some((WIDTH, HEIGHT)), Some((WIDTH, HEIGHT)));

    // Push the creation requests before waiting: the compositor cannot configure
    // a surface it has not received yet.
    display.flush()?;

    println!("1 for 32px as rasterized, 2 stretched, 3 for 64px, q to quit");

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

                    sheet.layout(&display, width, height);

                    // The configure has arrived, so the first buffer commit is
                    // now legal.
                    sheet.paint(&display);
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
                                sheet.set_mode(&mut display, Mode::Native);
                            }
                            Some('2') => {
                                sheet.set_mode(&mut display, Mode::Stretched);
                            }
                            Some('3') => {
                                sheet.set_mode(&mut display, Mode::Large);
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
