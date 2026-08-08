//! POSIX shared memory: the block of bytes the two processes both see.
//!
//! `shm_open` + `ftruncate` + `mmap`. The process that creates the region
//! unlinks the name as soon as the child has attached, so a crash on either
//! side leaves nothing behind in `/dev/shm`.
//!
//! Not realtime: mapping happens once, at spawn. The audio thread only ever
//! touches the mapped bytes through [`crate::bridge`].

use std::ffi::CString;
use std::os::raw::c_void;

use anyhow::{Context, Result, bail};

/// A mapped shared-memory region.
pub struct Shm {
    name: CString,
    ptr: *mut c_void,
    len: usize,
    /// Only the creator unlinks the name; the attacher just unmaps.
    owner: bool,
}

// SAFETY: the whole point of the region is that two processes touch it. Which
// bytes each side may write is the bridge's contract, not this type's — and
// `Shm` itself only ever hands out the base pointer, so sharing the handle
// between threads adds nothing the two processes were not already doing.
unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

impl Shm {
    /// Create a region called `name` (`/choz-…`, no other slashes) of `len`
    /// bytes, zeroed.
    pub fn create(name: &str, len: usize) -> Result<Self> {
        Self::open_inner(name, len, true)
    }

    /// Attach to a region someone else created.
    pub fn attach(name: &str, len: usize) -> Result<Self> {
        Self::open_inner(name, len, false)
    }

    fn open_inner(name: &str, len: usize, create: bool) -> Result<Self> {
        let cname = CString::new(name).context("shm name has an interior NUL")?;
        let flags = if create { libc::O_CREAT | libc::O_EXCL | libc::O_RDWR } else { libc::O_RDWR };
        // 0o600: both processes run as the same user, nobody else needs it.
        let fd = unsafe { libc::shm_open(cname.as_ptr(), flags, 0o600) };
        if fd < 0 {
            bail!("shm_open({name}): {}", std::io::Error::last_os_error());
        }
        if create && unsafe { libc::ftruncate(fd, len as libc::off_t) } != 0 {
            let e = std::io::Error::last_os_error();
            unsafe {
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
            }
            bail!("ftruncate({name}, {len}): {e}");
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        // The mapping keeps the region alive; the descriptor has done its job.
        unsafe { libc::close(fd) };
        if ptr == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error();
            if create {
                unsafe { libc::shm_unlink(cname.as_ptr()) };
            }
            bail!("mmap({name}, {len}): {e}");
        }
        Ok(Self { name: cname, ptr: ptr as *mut c_void, len, owner: create })
    }

    /// Base of the mapping. Valid for `len` bytes while this handle lives.
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr as *mut u8
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drop the name from the filesystem while keeping the mapping. Call it
    /// once the other side has attached: from then on nothing can leak, because
    /// the region dies with the last mapping.
    pub fn unlink(&mut self) {
        if self.owner {
            unsafe { libc::shm_unlink(self.name.as_ptr()) };
            self.owner = false;
        }
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr, self.len);
            if self.owner {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }
}

/// A region name unique to this process and `tag`.
pub fn unique_name(tag: &str) -> String {
    format!("/choz-{}-{}", std::process::id(), tag)
}
