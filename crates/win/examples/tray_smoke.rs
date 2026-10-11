//! GUI smoke test of `lastdrop_win::tray`: adds a real notification icon for about 12 s, changes
//! its image and tooltip once, reads its rectangle, then removes it and exits. Nothing takes
//! focus and nothing else on the desktop is touched.
//!
//! `cargo run -p lastdrop-win --example tray_smoke` (exit code 0 = every call succeeded).
#![expect(
    clippy::print_stdout,
    reason = "a developer smoke tool reports to its console; it prints no secrets"
)]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lastdrop_win::backdrop::allow_dark_menus;
use lastdrop_win::tray::{Tray, TrayImage, TrayInput};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetSystemMetrics, MSG, PM_REMOVE, PeekMessageW, SM_CXSMICON, TranslateMessage,
};

/// How long the icon stays (the smoke budget is 20 s).
const SHOW_FOR: Duration = Duration::from_secs(12);

/// A filled disc of `rgb` with a soft edge, straight BGRA.
fn disc(size: u32, rgb: [u8; 3]) -> TrayImage {
    let n = size as usize;
    let mut bgra = vec![0u8; n * n * 4];
    let c = size as f32 / 2.0;
    let r = c - 1.0;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let a = (r + 0.5 - d).clamp(0.0, 1.0);
            let i = (y * n + x) * 4;
            bgra[i..i + 4].copy_from_slice(&[rgb[2], rgb[1], rgb[0], (a * 255.0).round() as u8]);
        }
    }
    TrayImage { size, bgra }
}

fn main() {
    // SAFETY: process-wide DPI awareness, set before any window exists.
    let aware =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if let Err(e) = aware {
        println!("DPI awareness not set: {e}");
    }
    allow_dark_menus();
    // SAFETY: plain metric read.
    let size = u32::try_from(unsafe { GetSystemMetrics(SM_CXSMICON) })
        .ok()
        .filter(|&s| s > 0)
        .unwrap_or(16);

    let inputs = Rc::new(RefCell::new(Vec::<TrayInput>::new()));
    let sink = Rc::clone(&inputs);
    let tray = match Tray::create(
        &disc(size, [0x8E, 0x3A, 0xA7]),
        "Lastdrop smoke test",
        Box::new(move |input| sink.borrow_mut().push(input)),
    ) {
        Ok(tray) => tray,
        Err(e) => {
            println!("Tray::create failed: {e}");
            std::process::exit(1);
        }
    };
    println!("icon added at {size} px, hidden window {:#x}", tray.hwnd());

    let mut ok = true;
    let start = Instant::now();
    let mut changed = false;
    while start.elapsed() < SHOW_FOR {
        let mut msg = MSG::default();
        // SAFETY: a standard message pump on this thread (the tray's window lives here).
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if !changed && start.elapsed() > Duration::from_secs(2) {
            changed = true;
            match tray.icon_rect() {
                Some(r) => println!(
                    "icon rect: ({}, {}) {} x {}",
                    r.left,
                    r.top,
                    r.width(),
                    r.height()
                ),
                None => println!("icon rect: unavailable (icon in the overflow?)"),
            }
            if let Err(e) = tray.set_image(&disc(size, [0xD5, 0x8A, 0xE8])) {
                println!("set_image failed: {e}");
                ok = false;
            }
            if let Err(e) = tray.set_tooltip(&format!("Lastdrop smoke test {}", "·".repeat(200))) {
                println!("set_tooltip failed: {e}");
                ok = false;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    drop(tray);
    println!("inputs seen: {:?}", inputs.borrow());
    println!("icon removed; {}", if ok { "OK" } else { "FAILED" });
    std::process::exit(if ok { 0 } else { 1 });
}
