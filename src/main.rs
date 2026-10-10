//! VELA - a fast, minimal media player.
//!
//! One process, one window, one GL context:
//!   winit window -> glutin GL context -> libmpv render API draws the video ->
//!   egui draws the overlay on top -> swap. Frames are produced only when mpv
//!   has a new picture or the UI changed, so an idle/paused player uses ~0% CPU.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![recursion_limit = "512"]

mod app;
mod fonts;
mod ipc;
#[cfg(target_os = "macos")]
mod macos;
mod mpv;
mod platform;
mod player;
mod store;
mod thumbs;
mod ui;

use std::time::Duration;

fn absolutize(arg: &str) -> String {
    if platform::is_url(arg) {
        return arg.to_string();
    }
    std::path::absolute(arg).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| arg.to_string())
}

fn main() {
    platform::START.get_or_init(std::time::Instant::now);
    platform::init_log();
    std::panic::set_hook(Box::new(|info| logf!("panic: {info}")));

    // macOS passes "-psn_…" when launched from Finder; ignore any flags.
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).map(|a| absolutize(&a)).collect();
    let request = if args.is_empty() { "open".to_string() } else { format!("open\n{}", args.join("\n")) };

    // Single instance: hand our files to the running player, if there is one.
    let mut server = ipc::claim();
    if server.is_none() {
        // Explorer gave *this* process the right to take the foreground; pass
        // it on so the running player can come to the front with the file.
        platform::allow_foreground_handoff();
    }
    let mut tries = 0;
    while server.is_none() {
        if ipc::send(&request, Duration::from_secs(3)).is_some() {
            return;
        }
        tries += 1;
        if tries > 40 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        server = ipc::claim();
    }

    let event_loop = match winit::event_loop::EventLoop::<app::UserEvent>::with_user_event().build() {
        Ok(el) => el,
        Err(e) => {
            logf!("event loop: {e}");
            return;
        }
    };
    #[cfg(target_os = "macos")]
    {
        let proxy = event_loop.create_proxy();
        macos::install_open_handler(move |paths| {
            let _ = proxy.send_event(app::UserEvent::Open(paths));
        });
    }
    let mut app = app::App::new(event_loop.create_proxy(), args);
    if let Some(s) = server {
        app.spawn_ipc(s);
    }
    if let Err(e) = event_loop.run_app(&mut app) {
        logf!("event loop exited: {e}");
    }
}
