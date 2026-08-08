//! The plugin's own patch, through the `clap.state` extension.
//!
//! What a parameter list cannot carry: the patch chosen in Surge XT's browser,
//! a wavetable, a sample path. CLAP hands it over as a byte stream the plugin
//! writes into and reads back — so the host provides the stream and keeps the
//! bytes.
//!
//! Reached through the same raw `clap_plugin` pointer the editor uses, and the
//! same shared cell: once the instance is gone, saving reports nothing instead
//! of touching freed memory.

use std::ffi::c_void;

use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::plugin::clap_plugin;
use clap_sys::stream::{clap_istream, clap_ostream};

use crate::editor::SharedGui;

/// A growing buffer the plugin writes its state into.
struct OutBuf {
    data: Vec<u8>,
}

/// A buffer the plugin reads its state out of, with a cursor.
struct InBuf<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn write_cb(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the `OutBuf` this host put there, alive for the call.
    let out = unsafe { &mut *((*stream).ctx as *mut OutBuf) };
    let bytes = unsafe { std::slice::from_raw_parts(buffer as *const u8, size as usize) };
    out.data.extend_from_slice(bytes);
    size as i64
}

unsafe extern "C" fn read_cb(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the `InBuf` this host put there, alive for the call.
    let inp = unsafe { &mut *((*stream).ctx as *mut InBuf) };
    let n = (inp.data.len() - inp.pos).min(size as usize);
    if n > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(inp.data[inp.pos..].as_ptr(), buffer as *mut u8, n)
        };
        inp.pos += n;
    }
    n as i64
}

pub struct ClapState {
    shared: SharedGui,
}

impl ClapState {
    pub fn new(shared: SharedGui) -> Self {
        Self { shared }
    }

    /// The `clap.state` vtable of a live plugin.
    ///
    /// # Safety
    /// `plugin` must be a live `clap_plugin` whose `get_extension` is callable.
    unsafe fn extension(plugin: *const clap_plugin) -> Option<*const clap_plugin_state> {
        let get = unsafe { (*plugin).get_extension }?;
        let ext = unsafe { get(plugin, CLAP_EXT_STATE.as_ptr()) } as *const clap_plugin_state;
        (!ext.is_null()).then_some(ext)
    }
}

impl choz_ports::PluginState for ClapState {
    fn save(&self) -> Option<Vec<u8>> {
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let cell = guard.as_ref()?;
        // SAFETY: the cell is `Some` only while the instance lives.
        let ext = unsafe { Self::extension(cell.plugin) }?;
        let save = unsafe { (*ext).save }?;

        let mut out = OutBuf { data: Vec::new() };
        let stream = clap_ostream {
            ctx: &mut out as *mut OutBuf as *mut c_void,
            write: Some(write_cb),
        };
        // SAFETY: the plugin writes through the stream for the duration of the
        // call and nowhere else.
        let ok = unsafe { save(cell.plugin, &stream) };
        (ok && !out.data.is_empty()).then_some(out.data)
    }

    fn restore(&self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let guard = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let Some(cell) = guard.as_ref() else { return };
        // SAFETY: as above.
        let Some(ext) = (unsafe { Self::extension(cell.plugin) }) else { return };
        let Some(load) = (unsafe { (*ext).load }) else { return };

        let mut inp = InBuf { data, pos: 0 };
        let stream =
            clap_istream { ctx: &mut inp as *mut InBuf as *mut c_void, read: Some(read_cb) };
        unsafe { load(cell.plugin, &stream) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two stream callbacks are the whole host side of `clap.state`, and
    /// they can be exercised without a plugin: they are what a plugin calls.
    #[test]
    fn the_streams_carry_the_bytes_both_ways() {
        let mut out = OutBuf { data: Vec::new() };
        let ostream =
            clap_ostream { ctx: &mut out as *mut OutBuf as *mut c_void, write: Some(write_cb) };
        let payload = b"patch-bytes";
        // SAFETY: the stream points at a live `OutBuf`.
        let n = unsafe { write_cb(&ostream, payload.as_ptr() as *const c_void, payload.len() as u64) };
        assert_eq!(n, payload.len() as i64);
        assert_eq!(out.data, payload);

        let mut inp = InBuf { data: &out.data, pos: 0 };
        let istream =
            clap_istream { ctx: &mut inp as *mut InBuf as *mut c_void, read: Some(read_cb) };
        let mut buf = [0u8; 5];
        // A short read is normal: the plugin asks again until it gets 0.
        let n = unsafe { read_cb(&istream, buf.as_mut_ptr() as *mut c_void, 5) };
        assert_eq!((n, &buf), (5, b"patch"));
        let mut rest = [0u8; 64];
        let n = unsafe { read_cb(&istream, rest.as_mut_ptr() as *mut c_void, 64) };
        assert_eq!(n, 6);
        assert_eq!(&rest[..6], b"-bytes");
        // And it stops at the end instead of running past it.
        assert_eq!(unsafe { read_cb(&istream, rest.as_mut_ptr() as *mut c_void, 64) }, 0);
    }
}
