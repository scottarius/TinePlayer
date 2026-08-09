// Release builds are GUI applications, so double-clicking the executable
// doesn't open a console window behind it. Debug builds keep the console
// so development output stays visible. When a release build *is* launched
// from a terminal, `attach_parent_console` below reconnects stdout/stderr
// to it so the command-line flags still report anything useful.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod align;
mod app;
mod appearance;
mod awake;
mod browser;
mod config;
mod controls;
mod devices;
mod display;
mod gamepad;
mod kodi;
mod kodi_setup;
mod languages;
mod media_keys;
mod nfo;
mod pipeline;
mod player;
mod probe;
mod sound;
mod source;
mod subtitles;
mod updates;

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

    /// Start playing straight away, without the menu. Needs a FILE and a
    /// primary output device already set
    // Deliberately its own flag rather than inferred from the track options
    // being present: "did the user want the menu" is a question those options
    // cannot answer, and guessing it meant --primary alone silently skipped
    // the menu while --fullscreen alone did not.
    #[arg(long, requires = "file")]
    play: bool,

    /// Print the file's audio tracks and subtitles with their numbers
    #[arg(long)]
    list_tracks: bool,

    /// Print the names of this machine's audio output devices, as
    /// primary_sink and secondary_sink want them
    #[arg(long)]
    list_devices: bool,

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

/// Prints the audio outputs this machine has, one per line.
///
/// Exactly the strings the settings menu shows and `primary_sink` matches
/// against, with nothing added: a name with a space in it is common, so
/// decorating the list would mean explaining how to undecorate it.
fn list_devices() -> Result<(), String> {
    let names = devices::output_device_names()?;
    if names.is_empty() {
        return Err("No audio output devices found.".to_string());
    }
    for name in names {
        println!("{name}");
    }
    Ok(())
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
    let cache = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(config::DIR_NAME);
    std::fs::create_dir_all(&cache).ok()?;
    Some(cache)
}

/// The same for a macOS bundle, where the parts live under Contents.
#[cfg(target_os = "macos")]
fn use_bundled_resources() {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    // .../TinePlayer.app/Contents/MacOS/tineplayer
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
///
/// Must run before `display::apply_display_env`, which sets `GTK_A11Y=none`
/// when nothing has claimed it, to quiet a warning on machines with no session
/// bus. Whichever runs first wins.
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

/// Draws text through fontconfig rather than DirectWrite.
///
/// GTK picks DirectWrite on Windows and Core Text on macOS, and neither one
/// can see the fonts this application ships, because both ask the operating
/// system for fonts rather than reading a fontconfig directory. Linux already
/// draws through fontconfig, which is why the Pi was the one platform where
/// the bundled fonts worked before this existed.
///
/// It is also what the Windows bug was on its own: DirectWrite's fallback
/// never reached the Indic fonts Windows itself ships, so Bengali, Hindi,
/// Malayalam, Punjabi, Tamil and Telugu drew as boxes on a machine with
/// Nirmala UI in `C:\Windows\Fonts` the whole time, while Cyrillic, Greek,
/// Hebrew, Georgian, Chinese, Japanese and Korean were fine beside them.
///
/// Left alone when it is already set, so anyone who needs DirectWrite - for a
/// rendering difference, or to rule this out while chasing something else -
/// can still ask for it.
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn use_fontconfig() {
    if std::env::var_os("PANGOCAIRO_BACKEND").is_none() {
        set_env("PANGOCAIRO_BACKEND", std::ffi::OsStr::new("fc"));
    }
}

/// Linux already draws through fontconfig, so there is nothing to choose.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn use_fontconfig() {}

/// Sets an environment variable somewhere the C libraries will actually see
/// it.
///
/// On Windows there are two environments. `std::env::set_var` writes the Win32
/// block through `SetEnvironmentVariableW`, while the C runtime keeps its own
/// copy taken at startup and only updated through `_putenv`. `getenv` reads
/// the second one.
///
/// GStreamer never noticed, because GLib's `g_getenv` asks Win32. fontconfig
/// uses plain `getenv`, so everything set for it was silently ignored:
/// measured 2026-08-03, the same configuration produced no cache files when
/// the application set the variables and three when the shell did.
fn set_env(name: &str, value: &std::ffi::OsStr) {
    // SAFETY: called before GTK starts and before any thread exists, which is
    // the condition that makes setting an environment variable sound.
    unsafe { std::env::set_var(name, value) };

    #[cfg(target_os = "windows")]
    {
        unsafe extern "C" {
            fn _putenv_s(name: *const std::ffi::c_char, value: *const std::ffi::c_char) -> i32;
        }
        let Some(text) = value.to_str() else { return };
        let (Ok(name), Ok(value)) = (std::ffi::CString::new(name), std::ffi::CString::new(text))
        else {
            return;
        };
        // SAFETY: two C strings that outlive the call, into a C runtime
        // function that copies them.
        unsafe { _putenv_s(name.as_ptr(), value.as_ptr()) };
    }
}

/// Where the fonts TinePlayer ships live, relative to the running executable.
///
/// Beside it in a package, and up in the source tree when built from one, so
/// a developer's build draws the same text as a released one rather than
/// falling back to whatever the machine has.
fn bundled_fonts() -> Option<std::path::PathBuf> {
    let executable = std::env::current_exe().ok()?;
    let root = executable.parent()?;
    let candidates = [
        // Packaged: beside the executable, or under Resources in a bundle.
        root.join("fonts"),
        root.join("../Resources/fonts"),
        // Built from source: target/release/tineplayer, so the tree is two up.
        root.join("../../data/fonts"),
    ];
    candidates.into_iter().find(|path| path.is_dir())
}

/// Points fontconfig at the fonts TinePlayer ships, alongside the machine's
/// own.
///
/// The language menu names every language in its own script, and no desktop
/// has all of them. Measured 2026-08-03: Windows draws nothing for Bengali,
/// Hindi, Malayalam, Punjabi, Tamil or Telugu; Raspberry Pi OS has no Korean,
/// Chinese or Telugu font at all; macOS is missing most of the same set. The
/// fonts are a few hundred kilobytes because they carry only the characters
/// this application draws - see packaging/fonts/build-fonts.py.
///
/// The system directories are kept, and kept second. Everything that is not
/// ours - file names, device names, track titles - is drawn from whatever the
/// machine has, and this only adds to that rather than replacing it.
///
/// A Linux package installs its fonts where fontconfig already looks, so this
/// finds nothing there and does nothing, which is the intended outcome.
fn use_bundled_fonts() {
    // Somebody who has pointed fontconfig somewhere themselves meant it, and
    // is better placed to add these fonts to their own configuration than to
    // discover this overwrote it.
    if std::env::var_os("FONTCONFIG_PATH").is_some() {
        return;
    }
    let Some(fonts) = bundled_fonts() else {
        return;
    };
    let Ok(fonts) = fonts.canonicalize() else {
        return;
    };
    // Written beside the settings rather than into the installation, which may
    // be read-only, and rewritten every run so a moved installation cannot
    // leave a config naming somewhere it no longer is.
    let Some(dir) = config::app_dir_for_fontconfig() else {
        return;
    };
    let cache = dir.join("cache");
    let conf = dir.join("fonts.conf");
    // Verbatim, minus the extended-length prefix Windows canonicalization
    // adds, which fontconfig does not understand.
    let clean = |path: &std::path::Path| {
        path.display()
            .to_string()
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
    };
    let system = if cfg!(target_os = "windows") {
        "<dir>WINDOWSFONTDIR</dir>\n  <dir>WINDOWSUSERFONTDIR</dir>"
    } else if cfg!(target_os = "macos") {
        "<dir>/System/Library/Fonts</dir>\n  <dir>/Library/Fonts</dir>\n  <dir>~/Library/Fonts</dir>"
    } else {
        "<dir>/usr/share/fonts</dir>\n  <dir>/usr/local/share/fonts</dir>\n  <dir>~/.local/share/fonts</dir>"
    };
    // The machine's own configuration first, so this adds to it rather than
    // standing in for it.
    //
    // Replacing it looked fine and was not: the rules that say what
    // `sans-serif` and `serif` actually mean live in fontconfig's conf.d, and
    // without them a generic family resolves to whatever sorts first. On the
    // Pi that was FreeMono, so anything falling back to a generic name came
    // out in a light serif face - which is what "they do not look identical
    // on each platform" turned out to be.
    //
    // Every path is optional. Whichever exists is used, and on a system with
    // none of them the directories below still stand on their own.
    let includes = [
        "/etc/fonts/fonts.conf",
        "/opt/homebrew/etc/fonts/fonts.conf",
        "/usr/local/etc/fonts/fonts.conf",
    ]
    .iter()
    .map(|path| format!("  <include ignore_missing=\"yes\">{path}</include>\n"))
    .collect::<String>();
    let document = format!(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n\
         <fontconfig>\n\
         {includes}  \
         <dir>{}</dir>\n  {system}\n  \
         <cachedir>{}</cachedir>\n\
         </fontconfig>\n",
        clean(&fonts),
        clean(&cache)
    );
    if std::fs::create_dir_all(&cache).is_err() || std::fs::write(&conf, document).is_err() {
        return;
    }
    set_env("FONTCONFIG_PATH", dir.as_os_str());
}

fn main() -> std::process::ExitCode {
    attach_parent_console();
    // Before anything reads the environment, and before GStreamer starts.
    use_bundled_resources();
    enable_accessibility();
    use_fontconfig();
    use_bundled_fonts();

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

    // Before anything that needs a file: this asks about the machine rather
    // than about a video, and wanting the device names is a common reason to
    // run TinePlayer from a terminal at all - they are what primary_sink and
    // secondary_sink are matched against, and they cannot be guessed.
    if args.list_devices {
        return match list_devices() {
            Ok(()) => std::process::ExitCode::SUCCESS,
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
    let (mut config, config_problem) = Config::load();
    if let Some(problem) = config_problem.as_deref() {
        eprintln!("{problem}");
    }

    // First run: somewhere to play, rather than nowhere.
    //
    // An unset primary output meant a menu that could be navigated but not
    // played from, with the reason two screens away under Settings. Choosing
    // the system default is what somebody would pick anyway, and it is a
    // starting point rather than a decision - the row is still there and
    // still says what it is set to.
    //
    // Saved rather than only defaulted in memory, so the settings row and
    // config.yaml agree about what is in force. Only ever when nothing is
    // set: a device chosen and later unplugged keeps its name, which is what
    // makes the "may have been unplugged" message possible.
    if config.primary_sink.is_none()
        && let Some(device) = devices::default_output_device_name()
    {
        config.primary_sink = Some(device);
        if let Err(e) = config.save() {
            eprintln!("Could not save the default audio output: {e}");
        }
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
        // Whether the video was asked for, which decides how hard a failure to
        // open it should land. See `Launch::remembered`.
        let asked_for = source.is_some();
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
        let remembered = !asked_for && file.is_some();
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
        let play = args.play;
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
                    play,
                    remembered,
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
