//! Buffers, and the [`BufferId`] that names one.
//!
//! Every buffer this library makes goes through a `wl_shm` pool or the
//! single-pixel manager, and every one of them is something the application
//! commits to a surface and the library keeps track of. A [`BufferId`] is that
//! bookkeeping, in the same shape as [`SurfaceId`](crate::surface::SurfaceId) and
//! [`SeatId`](crate::seat::SeatId), and it is what
//! [`add_color`](crate::display::Display::add_color) and
//! [`add_pixels`](crate::display::Display::add_pixels) hand back.
//!
//! # Why not the protocol object
//!
//! A `wl_buffer` is a [`Proxy`](wayland_client::Proxy), and a proxy can be
//! destroyed by whoever holds it. An application holding one could destroy a
//! buffer the library still counts, and nothing would stop it. A [`Buffer`] is
//! not a proxy: it can be cloned, read, and committed, and there is nothing on
//! it that can be sent.
//!
//! A [`BufferId`] is the other half. Everything on
//! [`Display`](crate::display::Display) takes one, because a commit needs a
//! surface *and* a buffer and `Display` owns both of them; a [`Buffer`] is what
//! you get back when you ask for one by id, and is what
//! [`Surface::commit`](crate::surface::Surface::commit) takes directly.
//!
//! # Why the size travels with it
//!
//! [`commit_unscaled`](crate::surface::Surface::commit_unscaled) used to take
//! the buffer's width and height as arguments, which is a protocol error rather
//! than a wrong picture if they are wrong — the source rectangle is checked
//! against the buffer, and one reaching past it raises `out_of_buffer` and ends
//! the connection. The surface's own size is the likely wrong answer, and the
//! library already knew the right one. So the size is looked up from the buffer
//! instead of taken on trust, and the arguments are gone.

use slotmap::new_key_type;
use wayland_client::protocol::wl_buffer::WlBuffer;

new_key_type! {
    /// Names a buffer this library created.
    ///
    /// Only ever obtained from [`add_color`](crate::display::Display::add_color)
    /// and the two functions beside it, or from [`Buffer::id`], and resolved
    /// with [`Display::buffer`](crate::display::Display::buffer).
    pub struct BufferId;
}

/// A buffer the library owns, and the size it was created at.
///
/// Cloneable, and carrying no way to send a request: the protocol object inside
/// is not reachable. That is the whole reason this type exists in place of the
/// `wl_buffer` it wraps.
#[derive(Clone, Debug)]
pub struct Buffer {
    id: BufferId,
    /// The protocol object. `pub(crate)` and not private because
    /// [`Surface::commit`](crate::surface::Surface::commit) attaches it, and
    /// `pub(crate)` rather than handed out so nothing outside this crate can
    /// send a request through it.
    pub(crate) proxy: WlBuffer,
    width: i32,
    height: i32,
}

impl Buffer {
    /// Only reached through [`SlotMap::insert_with_key`](slotmap::SlotMap::insert_with_key),
    /// so a buffer is never built without the id it is filed under.
    pub(crate) fn new(id: BufferId, proxy: WlBuffer, width: i32, height: i32) -> Buffer {
        Buffer {
            id,
            proxy,
            width,
            height,
        }
    }

    /// This buffer's id, for a [`Display::commit`](crate::display::Display::commit)
    /// or a later [`Display::buffer`](crate::display::Display::buffer).
    ///
    /// The bridge between the two ways of naming a buffer: a `Buffer` is what
    /// [`Surface::commit`](crate::surface::Surface::commit) takes, and a
    /// `BufferId` is what everything on [`Display`](crate::display::Display)
    /// takes.
    pub fn id(&self) -> BufferId {
        self.id
    }

    /// The width it was created at, which is what
    /// [`commit_unscaled`](crate::surface::Surface::commit_unscaled) uses for
    /// the source rectangle.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// The height it was created at.
    pub fn height(&self) -> i32 {
        self.height
    }
}
