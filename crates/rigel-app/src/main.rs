// Hide the console window in release builds. Debug keeps a console the caller already owns, so
// `cargo run` still shows logs, but hides one Windows made for this process alone.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// A debug build is a console binary. Launched from Explorer, or by a test runner, Windows gives
/// it a fresh console window that would otherwise sit on the desktop showing only `eprintln!`s
/// (the control API's first line was the visible one). Hide only a console this process owns;
/// a terminal the user launched us from stays visible.
#[cfg(all(windows, debug_assertions))]
fn hide_orphan_console() {
    use core::ffi::c_void;
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut c_void;
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn ShowWindow(window: *mut c_void, command: i32) -> i32;
    }
    // SAFETY: these read this process's console and its holder list, then hide only a window that
    // belongs to this process alone. A null handle is the documented "no console" answer.
    unsafe {
        let mut holders = [0u32; 2];
        if GetConsoleProcessList(holders.as_mut_ptr(), 2) > 1 {
            return;
        }
        let window = GetConsoleWindow();
        if !window.is_null() {
            ShowWindow(window, 0); // SW_HIDE
        }
    }
}

fn main() {
    #[cfg(all(windows, debug_assertions))]
    hide_orphan_console();
    // The deferred-replace helper is a headless second copy of this binary: wait for the app to
    // exit, swap the executable, start the new build. It must run before Tauri starts.
    if let Some(code) = rigel_app::update_helper_mode(std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    // `rigel-app startup ...` is the headless readiness probe used by the cold-start test. It
    // runs the same path as the visible launch and exits instead of opening a window.
    if let Some(code) = rigel_app::startup_probe_mode(std::env::args().skip(1)) {
        std::process::exit(code);
    }
    // `rigel-app mcp ...` is the stdio server Claude calls back into; anything else opens the
    // window. A packaged app has no CLI on `PATH`, so the window binary hosts it itself.
    if let Some(code) = rigel_app::mcp_mode(std::env::args().skip(1)) {
        std::process::exit(code);
    }
    rigel_app::run();
}
