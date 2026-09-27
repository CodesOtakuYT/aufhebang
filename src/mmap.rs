//! Mapping a file into memory.

use std::{ffi::c_void, io, os::fd::BorrowedFd};

use rustix::mm::{MapFlags, ProtFlags};

/// A private mapping of a file.
///
/// Read-only, unless made with [`Mmap::writable_shared`], and used both to read
/// the keymap the compositor sends on a `wl_keyboard.keymap` fd and to fill the
/// pool behind a buffer of real pixels.
///
/// The file descriptor is only borrowed for the duration of the constructor: the
/// caller is free to close it as soon as the mapping exists, since the mapping
/// keeps its own reference to the underlying file.
pub(crate) struct Mmap {
    size: usize,
    ptr: *mut c_void,
}

impl Mmap {
    /// A read-only private mapping, for reading a file that will not change.
    pub(crate) fn new(fd: BorrowedFd<'_>, size: usize) -> io::Result<Self> {
        Self::map(fd, size, ProtFlags::READ, MapFlags::PRIVATE)
    }

    /// A writable shared mapping, for filling in a file another process reads.
    ///
    /// The pool behind a `wl_shm` buffer is exactly that case: the compositor
    /// maps the same file to show the pixels, so this has to be `SHARED` for it
    /// to see anything written here, and writing past the end of the file grows
    /// it. Callers that want a predictable size anyway should `ftruncate` first.
    pub(crate) fn writable_shared(fd: BorrowedFd<'_>, size: usize) -> io::Result<Self> {
        Self::map(
            fd,
            size,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
        )
    }

    fn map(fd: BorrowedFd<'_>, size: usize, prot: ProtFlags, flags: MapFlags) -> io::Result<Self> {
        let ptr = unsafe { rustix::mm::mmap(std::ptr::null_mut(), size, prot, flags, fd, 0) }?;
        Ok(Self { size, ptr })
    }

    /// The mapping as bytes, for reading the contents of a read-only mapping.
    pub(crate) fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.size) }
    }

    /// The mapping as mutable bytes, for filling in a writable one.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr as *mut u8, self.size) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        _ = unsafe { rustix::mm::munmap(self.ptr, self.size) };
    }
}
