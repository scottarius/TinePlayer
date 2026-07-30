// Release builds are GUI applications, so double-clicking the executable
// doesn't open a console window behind it. Debug builds keep the console
// so development output stays visible. When a release build *is* launched
// from a terminal, `attach_parent_console` below reconnects stdout/stderr
// to it so the command-line flags still report anything useful.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod appearance;
mod browser;
mod config;
mod controls;
mod devices;
mod display;
mod gamepad;
mod kodi;
mod languages;
mod pipeline;
mod player;
mod probe;
mod sound;
mod source;
mod subtitles;

use clap::Parser;
use gtk::prelude::*;

use app::App;
use config::Config;

#[derive(Parser)]
#[command(
    // Takes the version straight from Cargo.toml, so a built binary can always
    // be asked what it is. Worth having before packaged downloads exist:
    // a copy someone has had sitting around is otherwise unidentifiable.
    version,
    // Otherwise clap uses the lowercase crate name, so both --version and the
    // usage line would disagree with the executable people actually run.
    name = "TinePlayer",
    about = "Play a video with two audio tracks routed to two output devices.",
    long_about = "Play a video with two audio tracks routed to two output devices, \
                  so two people can watch together in different languages.\n\n\
                  Run with no arguments to pick everything in the window."
)]
struct Args {
    /// Video to play: a file, or a URL that GStreamer can open (`http://`,
    /// `smb://`). Omit it to choose one in the window.
    file: Option<String>,

    /// Audio track for the Primary output, numbered as `--list-tracks`
    /// shows them. 0 means no audio on this output.
    #[arg(long, value_name = "N")]
    primary: Option<u32>,

    /// Audio track for the Secondary output, numbered as `--list-tracks`
    /// shows them. 0 means no audio on this output.
    #[arg(long, value_name = "N")]
    secondary: Option<u32>,

    /// Subtitles to show, numbered as `--list-tracks` shows them. 0 means
    /// none.
    #[arg(long, value_name = "N")]
    subtitle: Option<u32>,

    /// Print the file's audio tracks and subtitles with their numbers, then
    /// exit
    #[arg(long)]
    list_tracks: bool,

    /// Start from the beginning, ignoring any saved resume position
    #[arg(long)]
    restart: bool,

    /// Start fullscreen
    #[arg(long)]
    fullscreen: bool,

    /// Start windowed, overriding a remembered fullscreen preference
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,

    /// Launched by Kodi: take the resume position from its library and hand
    /// it back, and leave choosing the video to Kodi
    ///
    /// Set by the entry Kodi's playercorefactory.xml adds. It is never
    /// inferred, because being launched by Kodi and being on a television are
    /// separate facts.
    #[arg(long)]
    kodi: bool,
}

/// `dtsdec` (wraps libdca) produces silent output on this platform's
/// GStreamer build - confirmed by testing the same real DTS track through
/// `avdec_dca` (libav's DTS decoder) directly, which decodes it correctly.
/// Both decode without error or warning; `dtsdec` just doesn't produce
/// audible output. Lowering its rank makes decodebin's auto-selection skip
/// it in favor of `avdec_dca` instead. Linux-validated as working (the
/// original Pi testing used `dtsdec` successfully), so this is Windows-only
/// - not changing behavior on a platform we know is fine.
#[cfg(target_os = "windows")]
fn disable_broken_dtsdec() {
    if let Some(factory) = gstreamer::ElementFactory::find("dtsdec") {
        use gstreamer::prelude::PluginFeatureExtManual;
        factory.set_rank(gstreamer::Rank::NONE);
    }
}

/// A GUI-subsystem binary has no console of its own, so output vanishes
/// even when the user ran it from a terminal. Reattaching to the parent's
/// console restores it for that case, and fails harmlessly when there
/// isn't one (i.e. the executable was double-clicked).
#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn attach_parent_console() {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

#[cfg(not(all(target_os = "windows", not(debug_assertions))))]
fn attach_parent_console() {}

/// `gst-plugin-gtk4`'s non-GL frame path emits
/// `g_object_unref: assertion 'G_IS_OBJECT (object)' failed` once per video
/// frame, which floods the console (dozens of lines per second) and buries
/// anything worth reading. It shows up on Windows, where the official
/// GStreamer build ships no EGL support so the GL path is unavailable.
///
/// Measured before suppressing it: memory and handle counts stay flat over
/// minutes of playback, so the failed unref is releasing nothing - it's
/// noise from upstream rather than a leak.
///
/// The filter is deliberately narrow: only this exact message, only from
/// GLib-GObject at CRITICAL. Everything else, including other criticals
/// from the same domain, is printed as usual.
fn silence_upstream_unref_spam() {
    glib::log_set_handler(
        Some("GLib-GObject"),
        glib::LogLevels::LEVEL_CRITICAL,
        false,
        false,
        |domain, level, message| {
            if message.contains("g_object_unref") && message.contains("assertion") {
                return;
            }
            eprintln!(
                "{}-{}: {message}",
                domain.unwrap_or("GLib"),
                match level {
                    glib::LogLevel::Error => "ERROR",
                    glib::LogLevel::Critical => "CRITICAL",
                    glib::LogLevel::Warning => "WARNING",
                    glib::LogLevel::Message => "MESSAGE",
                    glib::LogLevel::Info => "INFO",
                    glib::LogLevel::Debug => "DEBUG",
                }
            );
        },
    );
}

fn list_tracks(source: &source::Source) -> Result<(), String> {
    let media = probe::probe_media(source)?;

    println!("Audio tracks in {}:", source.label());
    println!("  0  None");
    for (position, track) in media.audio.iter().enumerate() {
        let mut line = format!(
            "  {}  {} — {} {}ch",
            position + 1,
            track.language,
            track.codec,
            track.channels
        );
        if !track.title.is_empty() {
            line.push_str(&format!(" — {}", track.title));
        }
        println!("{line}");
    }

    // The same list the menu offers, in the same order, so the numbers here
    // are the ones `--subtitle` takes. Includes subtitle files sitting beside
    // the video, not just what is inside it.
    let subtitles = subtitles::options(source.local(), &media.subtitles);
    println!();
    println!("Subtitles:");
    println!("  0  None");
    for (position, option) in subtitles.iter().enumerate() {
        println!("  {}  {}", position + 1, option.label());
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    attach_parent_console();

    let args = Args::parse();

    gstreamer::init().expect("Failed to initialize GStreamer");
    #[cfg(target_os = "windows")]
    disable_broken_dtsdec();
    silence_upstream_unref_spam();

    // gtk4paintablesink is statically linked into this binary rather than
    // installed as a shared GStreamer plugin (it isn't packaged for Debian
    // or shipped with the GStreamer Windows installer), so it has to be
    // registered explicitly - GStreamer's normal plugin scan won't find it.
    gstgtk4::plugin_register_static().expect("Failed to register gtk4paintablesink");

    let source = args.file.as_deref().map(source::Source::parse);

    if args.list_tracks {
        let Some(source) = source.as_ref() else {
            eprintln!("--list-tracks needs a file");
            return std::process::ExitCode::FAILURE;
        };
        return match list_tracks(source) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                std::process::ExitCode::FAILURE
            }
        };
    }

    // Deliberately not fatal. A missing file used to end the process here,
    // which is invisible when something else launched the player: the window
    // never appears and there is no terminal to read the reason from. The
    // window opens and says so instead. Still printed, for anyone who did run
    // it from a terminal.
    if let Some(source) = source.as_ref()
        && !source.is_available()
    {
        eprintln!("File not found: {}", source.label());
    }

    // Loading is allowed to fail: an unconfigured install just opens the
    // menu with its outputs unset, rather than refusing to launch. Output
    // devices are a menu row, so there's no separate setup step to reach.
    let config = Config::load().unwrap_or_default();

    // Set before GTK initializes: it reads these to find the compositor,
    // and a process launched over SSH or spawned by another application
    // doesn't inherit them.
    display::apply_display_env(&display::resolve_display(&config));

    // Any of them being given means "start playing", so the menu is skipped.
    let preset = (args.primary.is_some() || args.secondary.is_some() || args.subtitle.is_some())
        .then_some(app::Preset {
            primary: args.primary,
            secondary: args.secondary,
            subtitle: args.subtitle,
        });

    let gtk_app = gtk::Application::builder()
        .application_id("dev.tineplayer.TinePlayer")
        .build();

    {
        // Falls back to whatever was open last time, so relaunching lands on
        // the film you were watching. Skipped if it has since been moved or
        // deleted, which would otherwise open onto an error.
        // Kodi always passes the file it wants played, so falling back to the
        // last one would be wrong there as well as pointless.
        let file = source.or_else(|| {
            (!args.kodi)
                .then(|| {
                    config
                        .last_video
                        .clone()
                        .filter(|path| path.exists())
                        .map(source::Source::File)
                })
                .flatten()
        });
        let restart = args.restart;
        let fullscreen = (args.fullscreen || config.fullscreen) && !args.windowed;
        let kodi = args.kodi;
        gtk_app.connect_activate(move |gtk_app| {
            App::build(
                gtk_app,
                config.clone(),
                file.clone(),
                preset,
                restart,
                fullscreen,
                kodi,
            );
        });
    }

    // Empty args: clap has already parsed our command line, and GTK would
    // otherwise try to interpret flags like --restart itself.
    gtk_app.run_with_args::<&str>(&[]);
    std::process::ExitCode::SUCCESS
}
