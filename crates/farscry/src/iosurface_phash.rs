#![cfg(target_os = "macos")]

use farscry_core::StateId;
use std::os::raw::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

extern "C" {
    fn farscry_stream_start(
        display: u32,
        out_w: usize,
        out_h: usize,
        fps_limit: u32,
        callback: unsafe extern "C" fn(base: *const c_void, bpr: usize, ctx: *mut c_void),
        ctx: *mut c_void,
    ) -> *mut c_void;

    fn farscry_stream_stop(handle: *mut c_void);
}

/// A CGDisplayStream that delivers the primary display at 32×32 to a callback.
/// Frames are rate-limited to `fps_limit` per second by the ObjC layer.
/// The latest pHash is stored atomically and can be read from any thread.
pub struct DisplayStream {
    handle: *mut c_void,
    pub latest_hash: Arc<AtomicU64>,
    _ctx: Box<StreamCtx>,
}

unsafe impl Send for DisplayStream {}
unsafe impl Sync for DisplayStream {}

struct StreamCtx {
    latest_hash: Arc<AtomicU64>,
}

impl DisplayStream {
    /// Start a 32×32 display stream on the primary display.
    /// `fps_limit`: maximum pHash computations per second (default 2, enough for 1 FPS daemon).
    pub fn start(fps_limit: u32) -> Option<Self> {
        let latest_hash = Arc::new(AtomicU64::new(0));
        let ctx = Box::new(StreamCtx {
            latest_hash: latest_hash.clone(),
        });
        let ctx_ptr = &*ctx as *const StreamCtx as *mut c_void;

        let handle =
            unsafe { farscry_stream_start(0, 32, 32, fps_limit.max(1), frame_callback, ctx_ptr) };

        if handle.is_null() {
            return None;
        }

        Some(Self {
            handle,
            latest_hash,
            _ctx: ctx,
        })
    }

    /// Read the most recent pHash delivered by the stream.
    /// Returns None if no frame has arrived yet.
    pub fn latest_phash(&self) -> Option<StateId> {
        let bits = self.latest_hash.load(Ordering::Relaxed);
        if bits == 0 {
            None
        } else {
            Some(StateId::from_bits(bits))
        }
    }
}

impl Drop for DisplayStream {
    fn drop(&mut self) {
        unsafe { farscry_stream_stop(self.handle) };
    }
}

unsafe extern "C" fn frame_callback(base: *const c_void, bpr: usize, ctx: *mut c_void) {
    let sc = &*(ctx as *const StreamCtx);
    if base.is_null() || bpr == 0 {
        return;
    }
    let pixels = std::slice::from_raw_parts(base as *const u8, 32 * bpr);
    let hash = sample_and_phash(pixels, 32, 32, bpr);
    sc.latest_hash.store(hash.to_bits(), Ordering::Relaxed);
}

/// Sample 32×32 from a BGRA IOSurface and compute DCT pHash.
///
/// Uses `(i + 0.5) * src_size / 32` sampling to match
/// `image::imageops::FilterType::Nearest` so hashes are consistent
/// with the heap-copy reference implementation.
///
/// Total allocation: 1024-byte gray Vec + ~12 KB inside farscry_core::phash_image.
fn sample_and_phash(pixels: &[u8], w: usize, h: usize, bpr: usize) -> StateId {
    let mut gray = vec![0u8; 1024];
    for row in 0..32usize {
        let sy = ((row as f64 + 0.5) * h as f64 / 32.0) as usize;
        let sy = sy.min(h.saturating_sub(1));
        for col in 0..32usize {
            let sx = ((col as f64 + 0.5) * w as f64 / 32.0) as usize;
            let sx = sx.min(w.saturating_sub(1));
            let px = sy * bpr + sx * 4;
            if px + 2 < pixels.len() {
                let b = pixels[px] as f32;
                let g = pixels[px + 1] as f32;
                let r = pixels[px + 2] as f32;
                gray[row * 32 + col] = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
            }
        }
    }
    let luma = image::GrayImage::from_raw(32, 32, gray)
        .map(image::DynamicImage::ImageLuma8)
        .expect("32×32 GrayImage");
    farscry_core::phash_image(&luma)
}

use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowListExcludeDesktopElements, kCGWindowListOptionAll,
    kCGWindowNumber, kCGWindowOwnerPID, CGWindowID,
};

/// Walk the ancestor process tree from `start_pid` and return the first PID
/// that owns a visible GUI window in the CGWindow list.
pub fn find_terminal_window(start_pid: u32) -> Option<CGWindowID> {
    let mut pid = start_pid;
    for _ in 0..8 {
        if pid <= 1 {
            break;
        }
        if let Some(wid) = window_for_pid(pid) {
            return Some(wid);
        }
        pid = ppid(pid)?;
    }
    None
}

/// Return the first CGWindowID owned by `target_pid`.
fn window_for_pid(target_pid: u32) -> Option<CGWindowID> {
    #[allow(improper_ctypes)]
    extern "C" {
        fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
        fn CFNumberGetValue(n: *const c_void, t: i32, v: *mut c_void) -> bool;
    }
    const SINT32: i32 = 3;

    let wins = copy_window_info(
        kCGWindowListOptionAll | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;

    for raw in wins.get_all_values() {
        unsafe {
            let pid_cf = CFDictionaryGetValue(raw, kCGWindowOwnerPID as *const c_void);
            if pid_cf.is_null() {
                continue;
            }
            let mut owner: i32 = 0;
            if !CFNumberGetValue(pid_cf, SINT32, &mut owner as *mut i32 as *mut c_void) {
                continue;
            }
            if owner as u32 != target_pid {
                continue;
            }
            let wid_cf = CFDictionaryGetValue(raw, kCGWindowNumber as *const c_void);
            if wid_cf.is_null() {
                continue;
            }
            let mut wid: i32 = 0;
            if CFNumberGetValue(wid_cf, SINT32, &mut wid as *mut i32 as *mut c_void) {
                return Some(wid as CGWindowID);
            }
        }
    }
    None
}

/// Return the parent PID of `pid` via ps(1).
pub fn ppid(pid: u32) -> Option<u32> {
    let out = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid="])
        .output()
        .ok()?;
    std::str::from_utf8(&out.stdout)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
}

pub fn daemon_pid_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".farscry")
        .join("daemon.pid")
}

pub fn daemon_sock_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".farscry")
        .join("daemon.sock")
}

/// Return true if a process with this PID is running.
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}
