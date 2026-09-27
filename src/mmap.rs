use std::{ffi::c_void, io, os::fd::OwnedFd};

use rustix::mm::{MapFlags, ProtFlags};

pub struct Mmap {
    fd: OwnedFd,
    size: usize,
    ptr: *mut c_void,
}

impl Mmap {
    pub fn new(fd: OwnedFd, size: usize) -> io::Result<Self> {
        let ptr = unsafe {
            rustix::mm::mmap(
                std::ptr::null_mut(),
                size,
                ProtFlags::READ,
                MapFlags::PRIVATE,
                &fd,
                0,
            )
        }?;
        Ok(Self { fd, size, ptr })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.size) }
    }
}

impl Drop for Mmap {
    fn drop(&mut self) {
        _ = unsafe { rustix::mm::munmap(self.ptr, self.size) };
    }
}
