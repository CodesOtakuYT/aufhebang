//! Mapping a file into memory.

use std::{ffi::c_void, io, os::fd::BorrowedFd};

use rustix::mm::{MapFlags, ProtFlags};

/// A read-only private mapping of a file.
///
/// Only used to read the keymap the compositor sends on a `wl_keyboard.keymap`
/// fd, and only while that fd is open, so the mapping is not worth exposing.
///
/// The file descriptor is only borrowed for the duration of [`Mmap::new`]: the
/// caller is free to close it as soon as the mapping exists, since the mapping
/// keeps its own reference to the underlying file.
pub(crate) struct Mmap {
    size: usize,
    ptr: *mut c_void,
}

impl Mmap {
    pub(crate) fn new(fd: BorrowedFd<'_>, size: usize) -> io::Result<Self> {
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                size,
                ProtFlags::READ,
                MapFlags::PRIVATE,
                fd,
                0,
            )
        }?;
        Ok(Self { size, ptr })
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.size) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        _ = unsafe { rustix::mm::munmap(self.ptr, self.size) };
    }
}
