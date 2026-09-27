//! A picture loaded from a file, drawn two different sizes.
//!
//! [`Display::add_pixels`] is the whole of what this library can draw with
//! detail in it, and it wants finished `u32` values. That is a fine interface
//! for a program that has them and a poor one for a program holding a JPEG, so
//! the `image` feature adds [`Display::add_image`], which takes a decoded
//! [`DynamicImage`] instead and does the conversion.
//!
//! The asset is Hokusai's *Great Wave off Kanagawa*, public domain and
//! committed alongside the examples rather than downloaded, so this runs with no
//! network. See `assets/README.md` for where it came from.
//!
//! The two keys are the point of the example, and the same pair
//! `examples/pixels.rs` uses on a generated picture, so the two can be read
//! side by side:
//!
//! - `1` puts the image on a sub-surface with `commit_unscaled`, one pixel to
//!   one pixel, over a flat background. Only the image's own rectangle is
//!   damaged, so the colour behind it is left showing.
//! - `2` puts the same buffer on the window with `commit`, which stretches
//!   whatever it is given to fill the surface. That is exactly right for a
//!   colour, which is one pixel wide by design, and visibly wrong for a
//!   picture.
//!
//! Decoding is still the application's: `image::open` is this file's own call,
//! and any other way of producing a `DynamicImage` would do. The feature turns
//! on `jpeg` and `png` and little else — for a format this crate does not
//! bundle an asset for, depend on `image` yourself with the feature you want
//! and Cargo will unify the two.
//!
//! Run it with:
//!
//! ```text
//! cargo run --features image --example photo
//! ```

use anyhow::Result;
use aufhebung::{
    color::Color,
    display::Display,
    image::DynamicImage,
    state::{Event, SeatEvent, SurfaceEvent},
    surface::{SurfaceId, SurfaceInfo, SurfaceRole},
};

// Re-exported rather than depended on directly, for the same reason
// `wayland-client` is: a second declaration of the same crate at a different
// version gives back a different `DynamicImage`, and `add_image` would not take
// it.
use aufhebung::wayland_client::protocol::wl_buffer::WlBuffer;

/// Relative to the crate root, which is where `cargo run` puts us. An example
/// that fetched its own asset would need a network, and a demo that fails
/// without one is a demo nobody runs twice.
const ASSET: &str = "assets/great-wave.jpg";

/// Blank space left around the picture, so the background is visible in the
/// native mode and the stretch has somewhere to go.
const MARGIN: i32 = 20;

/// Which commit the last keypress asked for.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// The image at the size it was decoded at, on a sub-surface, over a flat
    /// background.
    Native,
    /// The same image as the window's own buffer, stretched to the window.
    Stretched,
}

struct Picture {
    window: SurfaceId,
    image: SurfaceId,
    /// The decoded file, kept because the two commits below need its size and
    /// it is the only thing that knows it.
    ///
    /// It is also worth holding onto for a resize: `image::imageops::resize`
    /// makes a new `DynamicImage`, and that is the size that should then be
    /// uploaded and committed at. The window below is sized from the image for
    /// the same reason — there is no reason for the two to disagree.
    file: DynamicImage,
    image_size: (i32, i32),
    /// The upload, at the size the image was decoded at.
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
        let (image_w, image_h) = self.image_size;
        if let Some(image) = display.surface(self.image) {
            image.set_position((width - image_w) / 2, (height - image_h) / 2);
        }
    }

    /// Commits the sub-surface, then the window.
    ///
    /// The order matters and not only for tidiness: the sub-surface is
    /// synchronized, so its contents are only cached until the parent is
    /// committed after it. Without that second commit the window would go on
    /// showing what it showed before.
    fn paint(&self, display: &Display) {
        let (image_w, image_h) = self.image_size;
        match self.mode {
            Mode::Native => {
                // The *image's* size, not the sub-surface's. They are the same
                // here because the sub-surface was created at this size, which
                // is not a coincidence: a sub-surface keeps the size it was
                // created with, so the upload has to be made at the size it will
                // stay.
                display.commit_unscaled(self.image, &self.photo, image_w, image_h);
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
            Mode::Native => "photo: one pixel to one pixel - 1:1, 2: stretch, q: quit",
            Mode::Stretched => "photo: stretched to the window - 1: 1:1, 2: stretch, q: quit",
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

    // The application's own decoding, and the only place the file format is
    // mentioned. This crate asks for a `DynamicImage` and has no opinion how one
    // arrived.
    let file = aufhebung::image::open(ASSET).map_err(|e| anyhow::anyhow!("{ASSET}: {e}"))?;

    // Taken from the image rather than hardcoded, so the two sizes that have to
    // agree — the buffer's and the sub-surface's — cannot drift apart.
    let image_size = (file.width() as i32, file.height() as i32);
    let (image_w, image_h) = image_size;
    let width = image_w + MARGIN * 2;
    let height = image_h + MARGIN * 2;

    // Uploaded once and committed as many times as we like, which is what
    // `add_image` is for. Its pixels are not written again afterwards: the
    // compositor may still be reading them, and nothing in this library would
    // say when it had stopped.
    let Some(photo) = display.add_image(&file) else {
        anyhow::bail!("{ASSET} does not fit in a shared memory pool");
    };

    let window = display
        .add_surface(SurfaceInfo {
            width,
            height,
            role: SurfaceRole::Window {
                title: "photo".into(),
            },
        })
        .expect("surface id space exhausted");

    // The image gets its own surface, because a surface carries one buffer and
    // the background needs one too. Created at the image's own size, which is
    // the size `commit_unscaled` will later ask for.
    let image = display
        .add_surface(SurfaceInfo {
            width: image_w,
            height: image_h,
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
        file,
        image_size,
        photo,
        bg: display.add_color(Color::hex(0x18_1a_22)),
        clear: display.add_color(Color::TRANSPARENT),
        mode: Mode::Native,
    };
    display.set_title(window, picture.title());

    // A fixed-size window, because the image is a fixed size and its position is
    // relative to the window. The compositor may ignore the request, so the
    // configure below still reads the size it actually gave.
    display.set_size_limits(window, Some((width, height)), Some((width, height)));

    // Push the creation requests before waiting: the compositor cannot configure
    // a surface it has not received yet.
    display.flush()?;

    println!("{ASSET} at {image_w}x{image_h}; 1 for its own size, 2 stretched, q to quit");

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

    // The decoded file outlives the upload: it is dropped here, and not before,
    // which is worth noticing only because the pixels it produced are still in
    // a shared memory pool the compositor can still be reading. Those are two
    // separate copies, and the pool's is the one that matters.
    drop(picture.file);

    Ok(())
}
