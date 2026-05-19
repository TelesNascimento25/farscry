use anyhow::{Context, Result};
#[cfg(not(target_os = "linux"))]
use crossbeam_channel::bounded;
use farscry_core::vasf::VasfWriter;
use farscry_core::StateId;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct RecordOpts {
    pub output: PathBuf,
    pub fps: u32,
    pub daemon: bool,
    pub threshold: u8,
    #[allow(dead_code)]
    pub silent: bool,
    pub window_pid: Option<u32>,
    pub global: bool,
    pub shell_pid: Option<u32>,
}

pub fn record(opts: RecordOpts) -> Result<()> {
    #[cfg(unix)]
    if opts.daemon && opts.global {
        let shell_pid = opts
            .shell_pid
            .unwrap_or_else(std::os::unix::process::parent_id);
        return crate::commands::daemon::connect_and_register(shell_pid);
    }
    if opts.daemon {
        return daemonize(opts);
    }
    run_capture_loop(opts)
}

fn daemonize(opts: RecordOpts) -> Result<()> {
    #[cfg(unix)]
    let pid_hint = opts
        .window_pid
        .unwrap_or_else(std::os::unix::process::parent_id);

    let exe = std::env::current_exe()?;
    let mut args: Vec<String> = vec![
        "record".to_string(),
        "--output".to_string(),
        opts.output.to_string_lossy().into_owned(),
        "--fps".to_string(),
        opts.fps.to_string(),
        "--threshold".to_string(),
        opts.threshold.to_string(),
    ];

    #[cfg(unix)]
    {
        args.push("--window-pid".to_string());
        args.push(pid_hint.to_string());
    }

    args.push("--silent".to_string());

    let child = std::process::Command::new(&exe)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("failed to spawn record daemon")?;
    let pid_path = pid_file_path();
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&pid_path, child.id().to_string()).ok();
    Ok(())
}

fn pid_file_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".farscry")
        .join("sessions")
        .join(".current.pid")
}

fn run_capture_loop(opts: RecordOpts) -> Result<()> {
    #[cfg(target_os = "linux")]
    return capture_loop_linux(opts);

    #[cfg(not(target_os = "linux"))]
    capture_loop_default(opts)
}

#[cfg(target_os = "linux")]
fn capture_loop_linux(opts: RecordOpts) -> Result<()> {
    use scrap::{Capturer, Display};

    if let Some(parent) = opts.output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let display =
        Display::primary().map_err(|e| anyhow::anyhow!("no display (is DISPLAY set?): {e}"))?;
    let mut capturer =
        Capturer::new(display).map_err(|e| anyhow::anyhow!("capturer init failed: {e}"))?;
    let img_w = capturer.width() as u32;
    let img_h = capturer.height() as u32;

    let writer = Arc::new(Mutex::new(VasfWriter::create(&opts.output)?));
    let mut last_hash: Option<StateId> = None;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = stop.clone();
    ctrlc::set_handler(move || stop_c.store(true, Ordering::SeqCst)).ok();

    let fps = opts.fps.max(1);
    let interval = Duration::from_millis(1000 / fps as u64);
    let threshold = opts.threshold;

    loop {
        thread::sleep(interval);
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let hash_opt = loop {
            match capturer.frame() {
                Ok(frame) => {
                    let h = farscry_core::phash_from_bgra(&frame, img_w, img_h);
                    break Some(h);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break None,
            }
        };

        let Some(hash) = hash_opt else { continue };
        let ts = crate::util::now_ms();
        let is_new = last_hash
            .map(|prev| hash.hamming(prev) > threshold)
            .unwrap_or(true);

        if let Ok(mut w) = writer.lock() {
            if is_new {
                w.append_state(hash, "").ok();
                last_hash = Some(hash);
            } else {
                w.append_timeline(ts, hash).ok();
            }
        }
    }

    if let Ok(mut w) = writer.lock() {
        w.finalize().ok();
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn capture_loop_default(opts: RecordOpts) -> Result<()> {
    if let Some(parent) = opts.output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let capture_pid = effective_capture_pid(opts.window_pid);

    let writer = Arc::new(Mutex::new(VasfWriter::create(&opts.output)?));
    let (cap_tx, cap_rx) = bounded::<image::DynamicImage>(3);
    let (ocr_tx, ocr_rx) = bounded::<(image::DynamicImage, StateId)>(1);

    let stop = Arc::new(AtomicBool::new(false));
    let stop_ctrlc = stop.clone();
    ctrlc::set_handler(move || {
        stop_ctrlc.store(true, Ordering::SeqCst);
    })
    .ok();

    let threshold = opts.threshold;
    let writer_phash = writer.clone();
    let t_phash = thread::spawn(move || phash_thread(cap_rx, ocr_tx, writer_phash, threshold));

    let pipeline = crate::pipeline::get_or_build_pipeline()?;
    let writer_ocr = writer.clone();
    let t_ocr = thread::spawn(move || ocr_thread(ocr_rx, pipeline, writer_ocr));

    let fps = opts.fps.max(1);
    let interval = Duration::from_millis(1000 / fps as u64);

    loop {
        thread::sleep(interval);
        if stop.load(Ordering::SeqCst) {
            break;
        }
        if let Some(img) = capture_screen(capture_pid) {
            cap_tx.try_send(img).ok();
        }
    }

    drop(cap_tx);
    t_phash.join().ok();
    t_ocr.join().ok();
    if let Ok(mut w) = writer.lock() {
        w.finalize().ok();
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn effective_capture_pid(hint: Option<u32>) -> Option<u32> {
    #[cfg(target_os = "macos")]
    {
        let start = hint.unwrap_or_else(std::os::unix::process::parent_id);
        crate::iosurface_phash::find_terminal_window(start)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = hint;
        None
    }
}

#[cfg(not(target_os = "linux"))]
fn phash_thread(
    rx: crossbeam_channel::Receiver<image::DynamicImage>,
    ocr_tx: crossbeam_channel::Sender<(image::DynamicImage, StateId)>,
    writer: Arc<Mutex<VasfWriter>>,
    threshold: u8,
) {
    let mut last_hash: Option<StateId> = None;
    for img in rx {
        let hash = farscry_core::phash_image(&img);
        let ts = crate::util::now_ms();
        let is_new = last_hash
            .map(|prev| hash.hamming(prev) > threshold)
            .unwrap_or(true);
        if is_new {
            ocr_tx.try_send((img, hash)).ok();
            last_hash = Some(hash);
        } else {
            drop(img);
            if let Ok(mut w) = writer.lock() {
                w.append_timeline(ts, hash).ok();
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn ocr_thread(
    rx: crossbeam_channel::Receiver<(image::DynamicImage, StateId)>,
    pipeline: std::sync::Arc<farscry_core::Pipeline>,
    writer: Arc<Mutex<VasfWriter>>,
) {
    for (img, hash) in rx {
        let (w, h) = (img.width(), img.height());
        let ts = crate::util::now_ms();
        if let Ok(vasp) = pipeline.process(img) {
            let vasp_text = farscry_formatter::VaspFormatter::format_vasp(&vasp, "screen", w, h);
            drop(vasp);
            if let Ok(mut wr) = writer.lock() {
                wr.append_state(hash, &vasp_text).ok();
                wr.append_timeline(ts, hash).ok();
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn capture_screen(window_id: Option<u32>) -> Option<image::DynamicImage> {
    if let Some(wid) = window_id {
        if let Some(img) = capture_window(wid) {
            return Some(img);
        }
    }
    capture_full_screen()
}

#[cfg(target_os = "macos")]
fn capture_window(window_id: u32) -> Option<image::DynamicImage> {
    use core_graphics::window::{
        create_image, kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow,
    };
    let bounds = unsafe { core_graphics::display::CGRectNull };
    let cg_img = create_image(
        bounds,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBoundsIgnoreFraming,
    )?;
    cgimage_to_dynamic(cg_img)
}

#[cfg(target_os = "macos")]
fn capture_full_screen() -> Option<image::DynamicImage> {
    use core_graphics::display::CGDisplay;
    let image = CGDisplay::main().image()?;
    cgimage_to_dynamic(image)
}

#[cfg(target_os = "macos")]
fn cgimage_to_dynamic(image: core_graphics::image::CGImage) -> Option<image::DynamicImage> {
    let width = image.width() as u32;
    let height = image.height() as u32;
    let bpr = image.bytes_per_row();
    let bytes = {
        let data = image.data();
        data.bytes().to_vec()
    };
    drop(image);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row = (y as usize) * bpr;
        for x in 0..width {
            let px = row + (x as usize) * 4;
            if px + 3 < bytes.len() {
                rgba.push(bytes[px + 2]);
                rgba.push(bytes[px + 1]);
                rgba.push(bytes[px]);
                rgba.push(bytes[px + 3]);
            }
        }
    }
    drop(bytes);
    image::RgbaImage::from_raw(width, height, rgba).map(image::DynamicImage::ImageRgba8)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn capture_screen(window_pid: Option<u32>) -> Option<image::DynamicImage> {
    let _ = window_pid;
    use scrap::{Capturer, Display};
    let display = Display::primary().ok()?;
    let mut capturer = Capturer::new(display).ok()?;
    let (w, h) = (capturer.width(), capturer.height());
    loop {
        match capturer.frame() {
            Ok(frame) => {
                let mut rgba = Vec::with_capacity(w * h * 4);
                for chunk in frame.chunks(4) {
                    if chunk.len() == 4 {
                        rgba.push(chunk[2]);
                        rgba.push(chunk[1]);
                        rgba.push(chunk[0]);
                        rgba.push(255);
                    }
                }
                return image::RgbaImage::from_raw(w as u32, h as u32, rgba)
                    .map(image::DynamicImage::ImageRgba8);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(16));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "linux"))]
    use super::*;
    use crossbeam_channel::bounded;

    #[cfg(all(unix, not(target_os = "linux")))]
    fn rss_kib() -> u64 {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &std::process::id().to_string()])
            .output()
        else {
            return 0;
        };
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .unwrap_or(0)
    }

    #[test]
    fn test_ring_buffer_drops_when_full() {
        let (tx, rx) = bounded::<image::DynamicImage>(3);
        for _ in 0..3 {
            assert!(tx
                .try_send(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)))
                .is_ok());
        }
        assert!(
            tx.try_send(image::DynamicImage::ImageRgba8(image::RgbaImage::new(4, 4)))
                .is_err(),
            "must drop frame when full, never block"
        );
        drop(rx);
    }

    #[test]
    #[cfg(all(unix, not(target_os = "linux")))]
    fn test_rss_bounded_after_60_frame_simulation() {
        let rss_before = rss_kib();

        let tmp = std::env::temp_dir().join("_farscry_mem_test.vasf");
        let writer = Arc::new(Mutex::new(VasfWriter::create(&tmp).unwrap()));
        let (cap_tx, cap_rx) = bounded::<image::DynamicImage>(3);
        let (ocr_tx, ocr_rx) = bounded::<(image::DynamicImage, StateId)>(1);

        let t_ocr: thread::JoinHandle<()> = thread::spawn(move || {
            for (img, _) in &ocr_rx {
                drop(img);
            }
        });

        let w2 = writer.clone();
        let t_phash = thread::spawn(move || phash_thread(cap_rx, ocr_tx, w2, 10));

        for _ in 0..60 {
            let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(1920, 1080));
            cap_tx.try_send(img).ok();
        }

        drop(cap_tx);
        t_phash.join().unwrap();
        t_ocr.join().unwrap();

        let rss_after = rss_kib();
        let delta_kib = rss_after.saturating_sub(rss_before);
        assert!(
            delta_kib < 128 * 1024,
            "RSS grew {delta_kib}KiB after 60-frame simulation (limit: 128MiB)",
        );

        if let Ok(mut w) = writer.lock() {
            w.finalize().ok();
        }
        std::fs::remove_file(&tmp).ok();
    }
}
