#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod diagnostics;
#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod motion;
#[cfg(windows)]
mod preferences;
#[cfg(windows)]
mod renderer_guard;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod ui_profile;
#[cfg(windows)]
mod windows_compat;

fn main() {
    #[cfg(windows)]
    diagnostics::install();
    #[cfg(windows)]
    startup::begin();
    let helper =
        std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--finish-heypixel"));
    #[cfg(windows)]
    {
        if !helper {
            match renderer_guard::launch_if_needed() {
                Ok(Some(code)) => std::process::exit(code),
                Ok(None) => {}
                Err(error) => {
                    gui::show_error(&format!("{error:#}"));
                    std::process::exit(1);
                }
            }
        }
        windows_compat::initialize();
    }
    #[cfg(windows)]
    if !helper {
        std::thread::spawn(|| {
            let _ = iced::advanced::graphics::text::font_system();
            startup::record("fonts_ready");
        });
    }
    let pending = if !helper && !fast_pack::client_audio::ENABLED {
        std::thread::spawn(|| {
            let _ = fast_pack::client_audio::finish_pending();
        });
        Ok(false)
    } else {
        fast_pack::client_audio::finish_pending()
    };
    match pending {
        Ok(true) => return,
        Ok(false) => {}
        Err(error) => {
            eprintln!("Fast Pack: {error:#}");
            std::process::exit(1);
        }
    }
    #[cfg(windows)]
    {
        renderer_guard::entered();
        let runtime = renderer_guard::runtime_lease();
        if let Err(error) = gui::run() {
            if renderer_guard::pending() {
                eprintln!("{error:#}");
            } else {
                gui::show_error(&format!("{error:#}"));
            }
            std::process::exit(1);
        }
        if let Some(runtime) = runtime {
            runtime.finish();
        }
    }
    #[cfg(not(windows))]
    if let Err(error) = fast_pack::cli::run() {
        eprintln!("Fast Pack: {error:#}");
        std::process::exit(1);
    }
}
