//! Colour values for [`Display::add_color`](crate::display::Display::add_color).

/// A colour with 8 bits per channel and straight (non-pre-multiplied) alpha.
///
/// `wp_single_pixel_buffer_manager_v1` takes each channel as a percentage over
/// the whole `u32` range, which makes passing raw values easy to get wrong:
/// `0xff` is not white, it is a quarter of a percent of white, and nothing about
/// the call site hints at that. This type takes the 8-bit values you actually
/// have — from a hex literal, a design tool, a palette — and widens them on the
/// way out.
///
/// ```
/// use aufhebung::color::Color;
///
/// assert_eq!(Color::hex(0xff_8000).channels(), (0xffff_ffff, 0x8080_8080, 0, 0xffff_ffff));
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Color {
    r: u8,
    g: u8,
    b: u8,
    a: u8,
}

/// Widens one channel to the protocol's percentage scale, pre-multiplied by
/// alpha as the protocol requires.
const fn channel(c: u8, a: u8) -> u32 {
    (c as u32 * a as u32 / 0xff) * 0x0101_0101
}

impl Color {
    /// Fully transparent. Useful as a base for [`with_alpha`](Self::with_alpha).
    pub const TRANSPARENT: Self = Self::rgba(0, 0, 0, 0);
    pub const BLACK: Self = Self::rgb(0, 0, 0);
    pub const WHITE: Self = Self::rgb(0xff, 0xff, 0xff);

    /// Opaque, from 8-bit channels.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }

    /// From 8-bit channels, alpha included. `0` is transparent, `0xff` opaque.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Opaque, from a packed `0xRRGGBB`.
    ///
    /// ```
    /// use aufhebung::color::Color;
    ///
    /// assert_eq!(Color::hex(0x3c_d0_78), Color::rgb(0x3c, 0xd0, 0x78));
    /// ```
    pub const fn hex(value: u32) -> Self {
        Self {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
            a: 0xff,
        }
    }

    /// The same colour at a different opacity.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    /// The four channels in the percentage scale the protocol expects.
    ///
    /// Each is `0` for none of the component and `u32::MAX` for all of it. The
    /// colour channels are pre-multiplied by alpha, which is what
    /// `wp_single_pixel_buffer_manager_v1` asks for; for an opaque colour that
    /// is the identity.
    pub const fn channels(self) -> (u32, u32, u32, u32) {
        (
            channel(self.r, self.a),
            channel(self.g, self.a),
            channel(self.b, self.a),
            self.a as u32 * 0x0101_0101,
        )
    }
}

impl From<u32> for Color {
    /// Reads a packed `0xRRGGBB`, so `Color::from(0xff_8000)` works where a
    /// colour is expected.
    fn from(value: u32) -> Self {
        Self::hex(value)
    }
}

impl From<(u8, u8, u8)> for Color {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self::rgb(r, g, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPAQUE: u32 = u32::MAX;

    #[test]
    fn opaque_channels_reach_the_8bit_value() {
        assert_eq!(Color::WHITE.channels(), (OPAQUE, OPAQUE, OPAQUE, OPAQUE));
        assert_eq!(Color::BLACK.channels(), (0, 0, 0, OPAQUE));
    }

    #[test]
    fn widening_preserves_the_8bit_value() {
        for v in [0x00, 0x01, 0x7f, 0x80, 0xfe, 0xff] {
            let (r, _, _, _) = Color::rgb(v, 0, 0).channels();
            assert_eq!(r as u8, v, "channel {v:#x} did not survive widening");
        }
    }

    #[test]
    fn hex_splits_into_channels() {
        assert_eq!(Color::hex(0x3c_d0_78), Color::rgb(0x3c, 0xd0, 0x78));
        assert_eq!(Color::from(0x00_00_00), Color::BLACK);
    }

    #[test]
    fn alpha_is_premultiplied() {
        // Half-transparent red. Pre-multiplication scales the colour channel
        // down along with alpha; leaving it at full would brighten the pixel
        // instead of fading it.
        let (r, g, b, a) = Color::rgba(0xff, 0, 0, 0x80).channels();
        assert_eq!(a, 0x80 * 0x0101_0101);
        assert_eq!(r, 0x80 * 0x0101_0101);
        assert_eq!((g, b), (0, 0));
    }

    #[test]
    fn fully_transparent_is_all_zero() {
        assert_eq!(Color::TRANSPARENT.channels(), (0, 0, 0, 0));
        // Even a bright colour goes to nothing once it is invisible.
        assert_eq!(Color::WHITE.with_alpha(0).channels(), (0, 0, 0, 0));
    }
}
