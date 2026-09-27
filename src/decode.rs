//! Turning a decoded picture into a buffer.
//!
//! [`add_pixels`](crate::display::Display::add_pixels) takes finished numbers,
//! which is the right interface for a program that has them and the wrong one
//! for a program holding a JPEG. [`Display::add_image`] is the translation
//! between the two, and it is what the `image` feature is for.
//!
//! It decodes nothing itself. It wants a [`DynamicImage`], so
//! [`image::open`], a decoder of the application's own choosing, and a raster
//! that arrived from somewhere else entirely are all equally acceptable inputs.
//!
//! What it does have to change is the pixels, because the two sides disagree
//! about alpha and the disagreement is invisible until it is not.
//! [`DynamicImage`] carries *straight* alpha — a colour channel as it was
//! written down — while `argb8888` wants it *pre-multiplied*, with the colour
//! already scaled by the alpha it will be composited through. Left straight, the
//! colour is at full strength in the buffer and the compositor scales it by the
//! same alpha again on the way out, so a translucent pixel lands darker than it
//! was meant to. An opaque image is unaffected either way, which is why this is
//! the sort of thing that ships working and is only noticed once something
//! translucent reaches the screen.

use std::borrow::Cow;

use image::{DynamicImage, RgbaImage};
use wayland_client::protocol::wl_buffer::WlBuffer;

use crate::display::Display;

/// One `u32` per pixel, in the layout [`add_pixels`] uploads.
///
/// [`add_pixels`]: crate::display::Display::add_pixels
fn pack(raster: &RgbaImage) -> Vec<u32> {
    // `as_raw` is the whole raster with no padding between rows, which is the
    // `width * 4` stride the buffer is created with, so this walks the two
    // layouts in step. The leftover is always empty, the length being
    // `width * height * 4` by construction, and the count comes from the slice
    // rather than from `width * height` so the two cannot disagree.
    let (pixels, _) = raster.as_raw().as_chunks::<4>();
    let mut out = Vec::with_capacity(pixels.len());

    for &[r, g, b, a] in pixels {
        // Straight alpha in, pre-multiplied out. An opaque pixel takes the
        // identity: this is every pixel of a JPEG, and most of the rest, and
        // multiplying by 255 and dividing by it again is not free.
        let channel = |c: u8| {
            if a == 0xff {
                c as u32
            } else {
                c as u32 * a as u32 / 0xff
            }
        };

        // Built as a number and handed over as a number, never byte-swapped.
        // `add_pixels` copies the `u32`s in the machine's own order, so this is
        // the same on either endianness — and lands as `0xAARRGGBB` read
        // natively, which is B, G, R, A in memory on a little-endian machine.
        out.push((a as u32) << 24 | channel(r) << 16 | channel(g) << 8 | channel(b));
    }

    out
}

/// An 8-bit RGBA view of `image`, borrowed where the image already is one.
fn as_rgba8(image: &DynamicImage) -> Cow<'_, RgbaImage> {
    // A decoder hands back `ImageRgba8`, so this is the branch that runs in
    // practice, and borrowing it avoids a copy of the whole raster. Everything
    // else is some other colour type and has to be converted regardless.
    match image {
        DynamicImage::ImageRgba8(raster) => Cow::Borrowed(raster),
        other => Cow::Owned(other.to_rgba8()),
    }
}

impl Display {
    /// A buffer holding `image` at the image's own size.
    ///
    /// Decoding is the caller's business, so this takes something already
    /// decoded and leaves the file format to it:
    ///
    /// ```no_run
    /// use aufhebung::display::Display;
    ///
    /// # fn demo(display: &Display) {
    /// // Through this crate's re-export, not a dependency of its own: see the
    /// // re-exports section of the crate docs for why that matters.
    /// let file = aufhebung::image::open("assets/great-wave.jpg").unwrap();
    ///
    /// // The size to commit at is the *image's*, not the surface's.
    /// let (width, height) = (file.width() as i32, file.height() as i32);
    /// let Some(buffer) = display.add_image(&file) else {
    ///     return;
    /// };
    /// # let _ = (width, height, buffer);
    /// # }
    /// ```
    ///
    /// # Sizing it
    ///
    /// A picture is placed with
    /// [`commit_unscaled`](crate::surface::Surface::commit_unscaled), and that
    /// takes the *buffer's* size — the image's `width` and `height`, which are
    /// not the surface's. A source rectangle reaching past the buffer is
    /// `out_of_buffer`, which ends the connection rather than drawing something
    /// wrong, so the two sizes are worth keeping straight.
    ///
    /// To draw the image at another size, resize it first. That is the
    /// application's choice rather than this function's: `image::imageops::resize`
    /// for a decoded image, or whatever the decoder has. Scaling the *buffer*
    /// with [`commit`](crate::display::Display::commit) instead is the other
    /// option, and is the wrong shape for a picture.
    ///
    /// # What it is not for
    ///
    /// Everything [`add_pixels`](Self::add_pixels) says about the buffer's
    /// lifetime applies unchanged, because this is that function with the
    /// conversion in front of it. In short: the pixels must not be written
    /// again once committed, and each call leaves a pool and buffer alive
    /// until the connection closes. Decode once, commit often.
    ///
    /// `None` means the image is too large to describe as a buffer, which for
    /// any real photograph means a resolution no display has. As with
    /// `add_pixels`, that is worth reporting rather than working around.
    pub fn add_image(&self, image: &DynamicImage) -> Option<WlBuffer> {
        let raster = as_rgba8(image);
        let (width, height) = (raster.width(), raster.height());

        // An image wider than `i32` has to be refused here rather than handed
        // to `add_pixels`, which takes `i32` and would wrap it into a small
        // positive number and upload the wrong rectangle of the wrong picture.
        let (Ok(width), Ok(height)) = (i32::try_from(width), i32::try_from(height)) else {
            return None;
        };

        self.add_pixels(width, height, &pack(&raster))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    /// A one-row raster from whole pixels, so the tests read as pictures rather
    /// than as offsets.
    fn row(pixels: &[[u8; 4]]) -> RgbaImage {
        RgbaImage::from_raw(u32::try_from(pixels.len()).unwrap(), 1, pixels.concat())
            .expect("a non-empty row is a valid raster")
    }

    #[test]
    fn opaque_pixels_pack_as_0xaarrggbb() {
        for (rgba, expected, name) in [
            ([0xff, 0, 0, 0xff], 0xffff_0000, "red"),
            ([0, 0xff, 0, 0xff], 0xff00_ff00, "green"),
            ([0, 0, 0xff, 0xff], 0xff00_00ff, "blue"),
            ([0xff, 0xff, 0xff, 0xff], 0xffff_ffff, "white"),
            ([0, 0, 0, 0xff], 0xff00_0000, "black"),
        ] {
            assert_eq!(pack(&row(&[rgba])), [expected], "{name}");
        }
    }

    #[test]
    fn alpha_is_premultiplied() {
        // Half-transparent. Left straight, the colour sits in the buffer at full
        // strength and the compositor scales it by the alpha a second time, so
        // the pixel arrives darker than intended — the opposite of what leaving
        // it alone looks like it would do, and invisible on anything opaque.
        let packed = pack(&row(&[[0x80, 0x40, 0x20, 0x80]]));
        assert_eq!(packed, [0x8040_2010]);
    }

    #[test]
    fn transparent_is_all_zero() {
        assert_eq!(pack(&row(&[[0x12, 0x34, 0x56, 0x00]])), [0x0000_0000]);
        // Even a white pixel goes to nothing once it cannot be seen.
        assert_eq!(pack(&row(&[[0xff, 0xff, 0xff, 0x00]])), [0x0000_0000]);
    }

    #[test]
    fn pixels_keep_their_order() {
        let packed = pack(&row(&[
            [0xff, 0, 0, 0xff],
            [0, 0xff, 0, 0xff],
            [0, 0, 0xff, 0xff],
        ]));
        assert_eq!(packed, [0xffff_0000, 0xff00_ff00, 0xff00_00ff]);
    }

    #[test]
    fn the_length_matches_the_raster() {
        let raster = RgbaImage::from_raw(7, 5, vec![0x20; 7 * 5 * 4]).unwrap();
        assert_eq!(pack(&raster).len(), 35);
    }

    /// The claim `add_pixels` makes is that the bytes in memory are B, G, R, A
    /// on a little-endian machine. Getting red and blue the wrong way round
    /// still produces a picture, just not *this* one, so it is worth pinning.
    #[cfg(target_endian = "little")]
    #[test]
    fn memory_is_bgra_on_a_little_endian_machine() {
        let packed = pack(&row(&[[0xff, 0, 0, 0xff]]));
        assert_eq!(packed[0].to_ne_bytes(), [0x00, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn an_rgba_image_is_borrowed_and_anything_else_is_converted() {
        let rgba = DynamicImage::ImageRgba8(row(&[[1, 2, 3, 0xff]]));
        assert!(matches!(as_rgba8(&rgba), Cow::Borrowed(_)));

        // An RGB image has no alpha channel of its own, so the conversion has to
        // invent an opaque one. A luma image has to invent the colour too.
        let rgb = DynamicImage::ImageRgb8(RgbImage::from_raw(1, 1, vec![10, 20, 30]).unwrap());
        assert_eq!(as_rgba8(&rgb).as_raw(), &[10, 20, 30, 0xff]);

        let luma = DynamicImage::ImageLuma8(image::GrayImage::from_raw(1, 1, vec![90]).unwrap());
        assert_eq!(as_rgba8(&luma).as_raw(), &[90, 90, 90, 0xff]);
    }

    /// The asset `examples/photo.rs` loads, checked as far as is worth checking.
    ///
    /// Deliberately not the colours: those belong to one artwork, and swapping it
    /// would fail a test with nothing to say about this code. What is checked is
    /// that the file survived being committed, that the decode and the packing
    /// agree on its size, and — the one that earns the test — that the result is
    /// not a flat field.
    ///
    /// A conversion that zeroed every channel produces a perfectly well-formed
    /// buffer of the right length, the right stride, and 635520 identical
    /// pixels. Nothing about it fails; the only symptom is a black window.
    #[test]
    fn the_bundled_asset_decodes_to_a_real_picture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/great-wave.jpg");
        // Through the crate's own re-export, which is the path the example takes.
        let file = crate::image::open(path).expect("the bundled asset decodes");

        let raster = as_rgba8(&file);
        let (width, height) = (raster.width(), raster.height());
        assert_eq!((width, height), (960, 662), "not the asset this expects");

        let packed = pack(&raster);
        assert_eq!(packed.len(), (width * height) as usize, "one u32 per pixel");

        // A JPEG has no alpha channel, so the conversion has to invent an opaque
        // one. Anything else would mean it invented a wrong one, and every
        // colour would then be scaled towards the wrong value.
        assert!(
            packed.iter().all(|p| p >> 24 == 0xff),
            "the asset is opaque and nothing else is"
        );

        let mut lo = 0xff_u32;
        let mut hi = 0;
        for &p in &packed {
            let luma = (p >> 16) & 0xff;
            lo = lo.min(luma);
            hi = hi.max(luma);
        }
        assert!(lo < 0x20, "nothing is dark: {lo:#04x}");
        assert!(hi > 0xe0, "nothing is light: {hi:#04x}");
    }
}
