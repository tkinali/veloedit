//! VeloEdit - entry point. See ARCHITECTURE.md for the module map.

mod app;
mod detect;
mod editor;
mod fileops;
mod find;
mod fsx;
mod git;
mod instance;
mod message;
mod palette;
mod search;
mod project_search;
mod session;
mod settings;
mod syntax;
mod ui;
mod watcher;
mod workspace;

use std::path::PathBuf;

use iced::{Size, window};

/// The window icon (X11 `_NET_WM_ICON`: taskbar, Alt+Tab). Wayland takes the icon
/// from the `.desktop` file instead - see `packaging/`. Decoding 128x128 takes
/// well under a millisecond.
fn window_icon() -> Option<window::Icon> {
    const PNG: &[u8] = include_bytes!("../assets/icon-128.png");
    let img = image::load_from_memory_with_format(PNG, image::ImageFormat::Png).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    window::icon::from_rgba(img.into_raw(), w, h).ok()
}

fn main() -> iced::Result {
    app::init_timing();

    // `veloedit [--new-window] [FOLDER|FILE]...` - folders become workspace roots, files open as tabs.
    let mut new_window = false;
    let mut args: Vec<PathBuf> = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.to_str() {
            Some("--new-window" | "-n") => new_window = true,
            Some("--version" | "-V") => {
                println!("veloedit {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--help" | "-h") => {
                println!(
                    "VeloEdit {}\n\nUsage: veloedit [OPTIONS] [FOLDER|FILE]...\n\n  \
                     -n, --new-window   open a separate window even if VeloEdit is already running\n  \
                     -V, --version      print the version\n  -h, --help         print this help",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            _ => args.push(PathBuf::from(arg)),
        }
    }
    // Paths are relative to *this* process: make them absolute before they travel.
    // (With a deadline: a path on a mount that does not answer must not keep the window from opening.)
    let args: Vec<PathBuf> = args.iter().map(|p| fsx::canonical(p)).collect();

    // One window: if VeloEdit is already running, it takes the paths and we leave.
    if !new_window {
        match instance::claim(&args) {
            instance::Claim::Forwarded => return Ok(()),
            instance::Claim::Primary(Some(listener)) => instance::serve(listener),
            instance::Claim::Primary(None) => {}
        }
    }

    // The window comes back as it was closed: size, maximized, and (where the platform has one) position.
    let saved = session::load().window.sane();
    let position = match saved.position() {
        Some((x, y)) => window::Position::Specific(iced::Point::new(x, y)),
        None => window::Position::Centered,
    };

    let result = iced::application(move || app::App::boot(args.clone()), app::App::update, app::App::view)
        .title(app::App::title)
        .subscription(app::App::subscription)
        .theme(app::App::theme)
        .window(window::Settings {
            size: Size::new(saved.width, saved.height),
            min_size: Some(Size::new(520.0, 360.0)),
            position,
            maximized: saved.maximized,
            icon: window_icon(),
            platform_specific: window::settings::PlatformSpecific {
                application_id: "veloedit".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        })
        .exit_on_close_request(false) // we persist the session first (see Message::CloseRequested)
        .antialiasing(false)
        .run();
    instance::cleanup();
    result
}
