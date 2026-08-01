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
mod kodi_setup;
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
                  so two people can watch together in different languages.",
    // The stock layout with one addition: an opening blank line, so the text
    // does not start hard against the command that asked for it. It has to
    // come from `before_help`, since clap trims leading whitespace from both
    // `long_about` and the template itself.
    before_help = " ",
    before_long_help = " ",
    help_template = "{before-help}{about-with-newline}\n\
                     {usage-heading} {usage}\n\n\
                     {all-args}{after-help}"
)]
struct Args {
    /// The video to play: a path, or a URL such as http:// or smb://
    file: Option<String>,

    /// Audio for the primary output: a track number, a language code, `ad`
    /// for described audio, or `en:ad` for both. 0 for no audio there
    #[arg(long, value_name = "T")]
    primary: Option<String>,

    /// Audio for the secondary output: a track number, a language code, `ad`
    /// for described audio, or `en:ad` for both. 0 for no audio there
    #[arg(long, value_name = "T")]
    secondary: Option<String>,

    /// Subtitles to show: a track number, a language code, a subtitle file
    /// name beside the video, or a preference name. 0 for none
    #[arg(long, value_name = "S")]
    subtitle: Option<String>,

    /// Print the file's audio tracks and subtitles with their numbers
    #[arg(long)]
    list_tracks: bool,

    /// Start video from the beginning, ignoring any saved position
    #[arg(long)]
    restart: bool,

    /// Forget the saved positions and track choices. Pass a FILE to limit to
    /// a single video
    #[arg(long)]
    forget: bool,

    /// Start fullscreen
    #[arg(long)]
    fullscreen: bool,

    /// Start windowed, overriding a remembered fullscreen preference
    #[arg(long, conflicts_with = "fullscreen")]
    windowed: bool,

    /// Used for launching from another application. See docs/integrations.md
    // Something else chose the video and is waiting for this playback of it to
    // finish, so only that video is played: no file browser, no confirmation
    // on the way out, and it exits when the video ends. Implied by --kodi.
    // Kept as a comment rather than help text: the documentation covers it,
    // and two accounts of the same flag drift apart.
    #[arg(long)]
    external: bool,

    /// Launched by Kodi: sync the resume position with its library. Implies
    /// --external
    // Set by the entry Kodi's playercorefactory.xml adds. It is never
    // inferred, because being launched by Kodi and being on a television are
    // separate facts - which is a reason for the flag rather than anything a
    // user needs told, so it is not help text.
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

/// Drops what is remembered about a video, or about all of them.
///
/// Named for what a person means by it rather than for the file it edits: the
/// position, and the tracks and subtitle chosen last time. Saying how many
/// were forgotten matters more than it looks, since the alternative is a
/// command that prints nothing and leaves you wondering whether it ran.
fn forget(source: Option<&source::Source>) -> Result<String, String> {
    let Some(source) = source else {
        let count = config::remembered();
        config::clear_all_resume()?;
        return Ok(match count {
            0 => "Nothing was remembered.".to_string(),
            1 => "Forgot 1 video.".to_string(),
            _ => format!("Forgot {count} videos."),
        });
    };

    Ok(if config::forget(&source.key()) {
        format!("Forgot {}.", source.label())
    } else {
        format!("Nothing was remembered about {}.", source.label())
    })
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

/// Points GStreamer and glib at the copies shipped beside the executable.
///
/// A packaged build carries its own GStreamer, GTK and plugins so that it
/// runs on a machine with none of them installed. Those libraries look for
/// their parts where they were *built*, which on someone else's machine is a
/// path that does not exist - or worse, one that does, belonging to a
/// different installation.
///
/// Silently does nothing for a build from source, which is meant to use what
/// the machine already has.
#[cfg(target_os = "windows")]
fn use_bundled_resources() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(root) = executable.parent() else {
        return;
    };
    let plugins = root.join("lib/gstreamer-1.0");
    if !plugins.is_dir() {
        return;
    }

    // SAFETY: called before any thread is started, and before GStreamer or
    // GTK read any of these. Setting an environment variable is only unsound
    // alongside a concurrent read of it.
    unsafe {
        // The suffixed names *replace* the directory GStreamer was compiled
        // to look in, rather than adding to it. Without them it also scans
        // whatever GStreamer installation the machine has, which loads a
        // second copy of glib into the process and undoes the point of
        // bundling - and quietly reinstates the plugins deliberately left
        // out of the package.
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugins);
        std::env::set_var("GST_PLUGIN_PATH_1_0", &plugins);
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH", &plugins);
        std::env::set_var("GST_PLUGIN_PATH", &plugins);

        // Scanning happens in a helper process, which GStreamer looks for
        // where it was built rather than beside itself.
        let scanner = root.join("libexec/gst-plugin-scanner.exe");
        if scanner.is_file() {
            std::env::set_var("GST_PLUGIN_SCANNER", scanner);
        }

        // The registry is a cache keyed by plugin paths. One left over from
        // another installation names plugins this package does not have.
        if let Some(cache) = dirs_cache() {
            std::env::set_var("GST_REGISTRY", cache.join("registry.bin"));
        }

        // glib otherwise loads these from where it was built, putting a
        // second copy of itself in the process. They also carry TLS, so
        // pointing at nothing would quietly break https.
        let gio = root.join("lib/gio/modules");
        if gio.is_dir() {
            std::env::set_var("GIO_MODULE_DIR", gio);
        }
        std::env::set_var("GSETTINGS_SCHEMA_DIR", root.join("share/glib-2.0/schemas"));
        std::env::set_var("XDG_DATA_DIRS", root.join("share"));
    }
}

/// Somewhere writable to keep GStreamer's plugin registry, so a packaged
/// build does not fight over the one belonging to an installed GStreamer.
#[cfg(target_os = "windows")]
fn dirs_cache() -> Option<std::path::PathBuf> {
    let cache = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tineplayer");
    std::fs::create_dir_all(&cache).ok()?;
    Some(cache)
}

/// The same for a macOS bundle, where the parts live under Contents.
#[cfg(target_os = "macos")]
fn use_bundled_resources() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    // .../TinePlayer.app/Contents/MacOS/TinePlayer
    let Some(contents) = executable.parent().and_then(|macos| macos.parent()) else {
        return;
    };
    if contents.file_name().is_none_or(|name| name != "Contents") {
        return;
    }

    let resources = contents.join("Resources");
    let plugins = resources.join("gstreamer-1.0");
    if !plugins.is_dir() {
        return;
    }

    // SAFETY: as above - before any thread exists, and before GStreamer or
    // GTK read any of these.
    unsafe {
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH_1_0", &plugins);
        std::env::set_var("GST_PLUGIN_PATH_1_0", &plugins);
        std::env::set_var("GST_PLUGIN_SYSTEM_PATH", &plugins);
        std::env::set_var("GST_PLUGIN_PATH", &plugins);

        let scanner = resources.join("libexec/gst-plugin-scanner");
        if scanner.is_file() {
            std::env::set_var("GST_PLUGIN_SCANNER", scanner);
        }
        let gio = resources.join("gio-modules");
        if gio.is_dir() {
            std::env::set_var("GIO_MODULE_DIR", gio);
        }
        // Its own registry, for the same reason as on Windows: one built from
        // another GStreamer names plugins this bundle does not have.
        if let Some(home) = std::env::var_os("HOME") {
            let cache = std::path::PathBuf::from(home).join("Library/Caches/tineplayer");
            if std::fs::create_dir_all(&cache).is_ok() {
                std::env::set_var("GST_REGISTRY", cache.join("registry.bin"));
            }
        }
        std::env::set_var(
            "GSETTINGS_SCHEMA_DIR",
            resources.join("share/glib-2.0/schemas"),
        );
        std::env::set_var("XDG_DATA_DIRS", resources.join("share"));
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn use_bundled_resources() {}

/// Switches on GTK's accessibility backend, which Windows otherwise leaves
/// off.
///
/// Without this a screen reader sees the window and nothing inside it: no
/// buttons, no rows, no names, however carefully those names are set. GTK
/// speaks to Windows through AccessKit, which ships beside it but is not
/// selected unless asked for.
///
/// Left alone if the environment already names a backend, so anyone
/// debugging accessibility can still choose one.
#[cfg(target_os = "windows")]
fn enable_accessibility() {
    if std::env::var_os("GTK_A11Y").is_none() {
        // Safe here and nowhere later: this runs before GTK starts and
        // before any thread that might read the environment exists.
        unsafe { std::env::set_var("GTK_A11Y", "accesskit") };
    }
}

#[cfg(not(target_os = "windows"))]
fn enable_accessibility() {}

fn main() -> std::process::ExitCode {
    attach_parent_console();
    // Before anything reads the environment, and before GStreamer starts.
    use_bundled_resources();
    enable_accessibility();

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

    // Before the checks below, so forgetting a video works whether or not it
    // is still there to play.
    if args.forget {
        return match forget(source.as_ref()) {
            Ok(said) => {
                println!("{said}");
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::ExitCode::FAILURE
            }
        };
    }

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

    // Nothing to fall back on: the whole point of the mode is that something
    // else chose the video, and with no browser there is no way to pick one.
    if (args.external || args.kodi) && source.is_none() {
        eprintln!("--external needs a video to play");
        return std::process::ExitCode::FAILURE;
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

    // Never refuses to launch: an unconfigured install just opens the menu
    // with its outputs unset, and one whose settings could not be read opens
    // on defaults and says so. Output devices are a menu row, so there is no
    // separate setup step to reach, and refusing would put the only place to
    // fix it out of reach.
    let (config, config_problem) = Config::load();
    if let Some(problem) = config_problem.as_deref() {
        eprintln!("{problem}");
    }

    // Set before GTK initializes: it reads these to find the compositor,
    // and a process launched over SSH or spawned by another application
    // doesn't inherit them.
    display::apply_display_env(&display::resolve_display(&config));

    // Any of them being given means "start playing", so the menu is skipped.
    let preset = (args.primary.is_some() || args.secondary.is_some() || args.subtitle.is_some())
        .then_some(app::Preset {
            primary: args.primary.clone(),
            secondary: args.secondary.clone(),
            subtitle: args.subtitle.clone(),
        });

    // One instance at a time, except under a launcher. GTK gives uniqueness
    // for free once an application has an id: a second launch hands its
    // activation to the one already running and exits.
    //
    // Not under `--external`, though. Whatever started us is waiting for this
    // process to end before it decides the film is over, and a launch that
    // returned immediately would tell it the film had finished before it
    // began.
    let flags = if args.external || args.kodi {
        gtk::gio::ApplicationFlags::NON_UNIQUE
    } else {
        gtk::gio::ApplicationFlags::empty()
    };
    let gtk_app = gtk::Application::builder()
        .application_id("app.tineplayer.TinePlayer")
        .flags(flags)
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
        // Kodi is one launcher among others, so it turns the general mode on
        // and adds only the parts that are about Kodi itself.
        let external = args.external || args.kodi;
        let restart = args.restart;
        let fullscreen = (args.fullscreen || config.fullscreen) && !args.windowed;
        // Asked for fullscreen by something that is also waiting for this
        // playback to end: it put the window where it wants it, and letting a
        // viewer shrink it out from under the launcher helps nobody. A
        // remembered preference does not count - only being told so here.
        let locked_fullscreen = args.fullscreen && external;
        let kodi = args.kodi;
        gtk_app.connect_activate(move |gtk_app| {
            // A second launch arrives here, in the process already running.
            // Raising what is open is the whole of the answer: no error, no
            // second window, and whatever is playing carries on.
            if let Some(window) = gtk_app.windows().first() {
                window.present();
                return;
            }
            App::build(
                gtk_app,
                config.clone(),
                file.clone(),
                preset.clone(),
                app::Launch {
                    restart,
                    fullscreen,
                    locked_fullscreen,
                    external,
                    kodi,
                },
                config_problem.clone(),
            );
        });
    }

    // Empty args: clap has already parsed our command line, and GTK would
    // otherwise try to interpret flags like --restart itself.
    gtk_app.run_with_args::<&str>(&[]);
    std::process::ExitCode::SUCCESS
}
