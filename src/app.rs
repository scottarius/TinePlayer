use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use crate::source::Source;
use std::rc::Rc;

use gstreamer::prelude::DeviceExt;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::appearance;
use crate::config::Config;
use crate::controls::Controls;
use crate::devices::list_audio_output_devices;
use crate::pipeline::Playing;
use crate::player::Playback;
use crate::probe::AudioTrack;
use crate::sound::Sounds;
use crate::subtitles::{Subtitle, SubtitleChoice};

/// Marks the overlay a modal is stacked in, so that opening one over another
/// can tell it apart from a page that happens to be built out of an overlay
/// too - which the media page is.
const MODAL_STACK: &str = "tp-modal-stack";

/// How many languages either summary line names before it counts the rest.
///
/// The line has one line's worth of room and must not wrap: the rows below it
/// sit at a fixed height, so anything that grows here would push them down.
/// Six is past the point where a list is being read rather than scanned, and a
/// file with more than six subtitle languages is a disc rip whose exact
/// inventory is a chooser away.
///
/// Whatever is left over is said rather than dropped - "+5 more". Stopping at
/// six in silence reads as a complete list, which on a file carrying eleven is
/// not merely crowded but wrong.
const MOST_LANGUAGES: usize = 6;

/// What a track that never stated its language is called on the page.
///
/// "Unknown" rather than the container's own word for it: `und` is what the
/// file says and "Undetermined" is what the specification calls that, and
/// neither is what a viewer would say about a soundtrack they can plainly
/// hear. It says the same thing in the word already being used everywhere
/// else something is missing.
const UNKNOWN_LANGUAGE: &str = "Unknown";

/// One summary line's markup: the label, the languages that fit, and a count
/// of the ones that did not.
///
/// Both lines are built from this, which is the point of its being a function:
/// audio and subtitles differ only in what they are handed, and a rule applied
/// in one place cannot drift out of step with the other. In practice the audio
/// line rarely reaches the limit and the subtitle line often does, so the
/// truncation would otherwise go untested on the side that shows it least.
///
/// What was left off is counted and said outright. Stopping at the limit in
/// silence reads as a complete list, so a file with eleven subtitle languages
/// appeared to have six - worse than a long line, because it is wrong rather
/// than merely crowded. The count is dimmed like the label, so it reads as a
/// note about the list rather than as another language in it.
fn summary_markup(name: &str, languages: &[String]) -> String {
    let shown = match languages.is_empty() {
        true => NO_TRACKS.to_string(),
        false => languages
            .iter()
            .take(MOST_LANGUAGES)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
    };
    let more = match languages.len().saturating_sub(MOST_LANGUAGES) {
        0 => String::new(),
        extra => format!(", <span alpha='60%'>+{extra} more</span>"),
    };
    format!(
        "<span alpha='60%'>{name}:</span> {}{more}",
        glib::markup_escape_text(&shown),
    )
}

/// What Kodi handing a video over should do, in the order the chooser offers
/// them, indexed by whether `--play` is written into Kodi's arguments.
///
/// One list rather than two, because the row states what is in force and the
/// chooser offers the alternatives: written out twice they would eventually
/// disagree, and a row reading one thing while its own chooser marks another
/// as current is the kind of fault nobody reports.
///
/// The menu is first, and is what no flag means. Choosing the two audio tracks
/// is the reason this application exists, so landing there is the answer an
/// integration should have to be talked out of rather than into.
const HANDOVER: [&str; 2] = ["Show Track Selection Menu", "Play Video Immediately"];

/// What a summary line says when the file carries no such track at all.
///
/// Distinct from `Unknown`, and the difference is worth keeping: one means
/// there is a track and nobody said what language it is in, the other means
/// there is nothing there to choose.
const NO_TRACKS: &str = "None";

/// Which setting a chooser screen is editing. The menu drills into one of
/// these and returns once a choice is made.
#[derive(Clone, Copy, PartialEq)]
enum Setting {
    PrimaryDevice,
    PrimaryTrack,
    SecondaryDevice,
    SecondaryTrack,
    Subtitles,
    PrimaryLanguage,
    SecondaryLanguage,
    SubtitleLanguage,
    SubtitleFont,
    /// What one Kodi does with TinePlayer, and what happens when it hands a
    /// video over. Both carry that installation's place in the list the
    /// Kodi pane was built from, since there may be several.
    KodiType(usize),
    KodiHandover(usize),
}

/// What a slider on the settings screen is setting.
///
/// Most are percentages, which is what lets one set of arithmetic serve them.
/// The delay is the exception: it is milliseconds, so it carries its own step,
/// range and reading rather than borrowing the percentage ones.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    /// The level for one output, by role.
    Volume(&'static str),
    /// How far one output is held back, by role, in milliseconds.
    Offset(&'static str),
    /// How big the interface is, in steps either side of its normal size.
    Scale,
    /// Subtitle size, in points against the video's own resolution.
    SubtitleSize,
    ResumeThreshold,
    WatchedThreshold,
}

impl Slider {
    /// How far one press moves it. Levels move in fives, being a rough
    /// setting anyone can hear; the thresholds move by one, since the useful
    /// range of each is narrow enough that fives would be three choices.
    /// The delay moves in tens, which is about the smallest step that can be
    /// told apart against a picture and still crosses its range in a few
    /// seconds of holding.
    fn step(self) -> f64 {
        match self {
            Slider::Volume(_) => 5.0,
            Slider::Offset(_) => 10.0,
            // A tenth of a step, which is about a nine per cent change in
            // size - small enough to settle on a size, large enough to cross
            // the range in a few seconds of holding.
            Slider::Scale => 0.1,
            // A point at a time. The range is small enough that anything
            // coarser would be six choices in a row of buttons.
            Slider::SubtitleSize => 1.0,
            _ => 1.0,
        }
    }

    fn range(self) -> std::ops::RangeInclusive<f64> {
        match self {
            Slider::Volume(_) => 0.0..=100.0,
            // Both directions. Holding a sink back is unbounded; pulling one
            // forward is limited by how much audio the pipeline has already
            // buffered, which measured comfortably past half a second.
            Slider::Offset(_) => -crate::config::MAX_OFFSET_MS..=crate::config::MAX_OFFSET_MS,
            // Below one per cent is indistinguishable from starting over, and
            // past a quarter of a film nothing would ever be resumable.
            Slider::ResumeThreshold => 1.0..=25.0,
            // Anything under half is not watching it, and a hundred means
            // sitting through the credits to be counted.
            Slider::WatchedThreshold => 50.0..=100.0,
            // Steps rather than the multiplier itself, so the middle is the
            // normal size and the two halves are the same length. Three steps
            // either way, which is a third at one end and three times at the
            // other.
            Slider::Scale => -3.0..=3.0,
            // Against the video's own height rather than the screen's, so
            // these hold whatever it is played back on. Below eight is
            // unreadable at any size; past twenty-four covers the picture.
            Slider::SubtitleSize => 8.0..=24.0,
        }
    }
}

/// A size chosen by hand, held to what the slider could have produced.
///
/// The file is editable, so it can hold anything at all; the interface has to
/// stay usable enough to change it back from inside.
fn chosen_scale(config: &crate::config::Config) -> Option<f64> {
    config
        .ui_scale
        .map(|scale| scale.clamp(appearance::MIN_CHOSEN_SCALE, appearance::MAX_CHOSEN_SCALE))
}

/// A size in steps either side of normal, as the multiplier it means.
///
/// Geometric rather than added: a step down is the same change as a step up,
/// so three steps down is exactly a third where three up is exactly three
/// times. Adding a fixed amount instead would make the lower half of the
/// slider cover almost nothing and the upper half everything.
fn scale_from_steps(steps: f64) -> f64 {
    let scale = 3f64.powf(steps / 3.0);
    // To the hundredth, so the file holds a number somebody could have typed
    // and the reading beside the bar is what was stored.
    (scale * 100.0).round() / 100.0
}

/// The same, backwards, for putting the bar where a stored size says.
fn steps_from_scale(scale: f64) -> f64 {
    3.0 * scale.max(0.01).log(3.0)
}

/// How a size reads beside its bar: the multiplier, without trailing noughts.
fn scale_label(scale: f64) -> String {
    let text = format!("{scale:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    format!("{text}x")
}

/// How far an output is shifted, wherever that is shown.
///
/// One function for the settings screen and the panel during playback,
/// because two of them drifted into two different styles for the same number
/// and the same feature read as two.
///
/// Signed and short. It is watched while it moves against a picture, where
/// what matters is seeing it change; words for the direction read better at
/// rest but turn every step into something to be re-read.
pub fn offset_label(ms: f64) -> String {
    let ms = ms.round();
    if ms == 0.0 {
        // Round can give -0, which formats with a sign that says the output
        // is shifted when it is not.
        "0ms".to_string()
    } else {
        format!("{ms:+}ms")
    }
}

/// Rows of the settings screen, in the order they appear.
/// Longer than a keyboard leaves between repeats, and short enough not to read
/// as a delay on an ordinary press. Windows repeats at up to thirty a second,
/// which is a gap of about thirty-three milliseconds.
const REPEAT_GAP: std::time::Duration = std::time::Duration::from_millis(90);

/// Rows a page jump covers, roughly a screenful at the default size. What
/// makes a folder of a hundred films navigable without a hundred presses.
const PAGE_ROWS: i32 = 8;

/// Space kept for the reading beside a bar, in characters. Shared by the
/// settings sliders and the volume panel, so a row of one lines up with a row
/// of the other.
///
/// Sized to the longest any of them shows - "-1000ms" - because the width is a
/// floor and not a ceiling: a longer reading widens the label, which moves the
/// bar, which moves under the pointer that is dragging it. Anything added that
/// reads longer than this has to raise it.
pub const READING_CHARS: i32 = 7;

/// How wide the alignment panel is, measured in characters of its own body
/// text.
///
/// Both the floor and the ceiling, so the three steps are one panel changing
/// what it says rather than three differently sized windows. It has to sit on
/// the text rather than on the container, because GTK offers no maximum width
/// on a box - and the text and the track names are the only things in the
/// panel that could push it wider anyway. Around 74 characters is also about
/// as long a line as is comfortable to read.
const ALIGN_PANEL_CHARS: i32 = 74;

/// A floor in unscaled pixels as well, for the case the character measure
/// cannot cover: a narrow font would otherwise draw a panel too cramped to
/// read from across a room, which is the distance this is built for.
const ALIGN_PANEL_MIN: f64 = 520.0;

/// Font families offered in the menu. Generic names Pango always resolves
/// rather than an enumeration of everything installed, which would run to
/// hundreds of rows. `subtitle_font` in the config takes any description.
const SUBTITLE_FONTS: [&str; 5] = ["Sans Bold", "Sans", "Serif Bold", "Serif", "Monospace Bold"];

/// How long scrubbing must be still before the seek is actually performed.
/// Short enough to feel like it happens on release, long enough to bridge the
/// gap between auto-repeat steps and the release events X11 interleaves
/// between them.
/// Scrub redraw interval. The movement is driven from here rather than from
/// input repeats, so it stays smooth at every speed.
const SCRUB_TICK: std::time::Duration = std::time::Duration::from_millis(33);

/// Safety net: if a release is somehow missed, scrubbing still ends rather
/// than running away.
const SCRUB_ABANDON: std::time::Duration = std::time::Duration::from_millis(700);

/// Tracked so Escape can mean "go back one level" rather than one fixed
/// action: out of playback, out of a chooser, or out of the application.
#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Menu,
    Settings,
    Browser,
    PasteUri,
    VideoSource,
    Opening,
    /// The three steps of aligning an audio file, in the one panel that
    /// carries them: which track to measure against, the measuring, and what
    /// it found. Separate screens because backing out means different things
    /// at each - nothing has been decided at the first, a thread is running at
    /// the second, and the third is only a report.
    AlignChoose,
    AlignProgress,
    AlignResult,
    Confirm,
    Notices,
    /// The panels the Kodi pane can open over itself: the folder
    /// browser for naming a Kodi by hand, the confirmation asked before the
    /// first change to a file and before a removal, the sandbox instructions
    /// for a Flatpak, and a failure to write.
    ///
    /// These were nine, and were a wizard. The five that collected answers are
    /// rows on the pane now.
    KodiFolder,
    KodiConfirm,
    KodiPermission,
    KodiError,
    /// The Quick Connect code, while it waits to be approved. Its own screen
    /// rather than one of the panels below, because something is running
    /// behind it: the polling stops when this stops being what is on screen.
    JellyfinConnect,
    /// Everything else the Jellyfin pane opens over itself - the server
    /// address, the question asked before disconnecting, and anything that
    /// went wrong. One variant rather than three, since what they have in
    /// common is the whole of what is asked of them: Escape returns to the
    /// pane, exactly as pressing Cancel does.
    JellyfinPanel,
    ConfirmQuit,
    Error,
    Playing,
}

/// Which output a choice is about, where the two are otherwise handled alike.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    Primary,
    Secondary,
}

impl Role {
    /// How the config file names this output. Two spellings of one thing, and
    /// this is where they meet.
    fn key(self) -> &'static str {
        match self {
            Role::Primary => "primary",
            Role::Secondary => "secondary",
        }
    }
}

/// What choosing a row on the main menu does.
///
/// Carried beside each row rather than worked out from its position: the
/// alignment rows come and go with the audio files chosen, so a fixed index
/// would name a different row depending on what is set.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuAction {
    Device(Role),
    Track(Role),
    Align(Role),
    Subtitles,
}

/// One line of a chooser: what it says, and which choice it stands for.
/// `None` is the "None" entry, which most of these lists begin with.
type Choice = (String, Option<usize>);

/// Everything a chooser needs to draw itself.
struct Choices {
    entries: Vec<Choice>,
    /// The choice already in force, so the list opens on it rather than at the
    /// top. `None` when nothing is set, which lands on the "None" row every
    /// list that has one begins with.
    current: Option<usize>,
    /// Entries with a rule drawn above them, by index. Only the subtitle
    /// preference has any: it offers three unlike things in one list - nothing,
    /// four ways of following an output, and two hundred languages - and
    /// without the rules they read as one long undifferentiated run.
    dividers: Vec<usize>,
}

/// Puts a selector's rows in, and can be run again when what they should say
/// has changed - which for a device list is a moment after it opens.
type Fill = dyn Fn(&Rc<App>);

/// One row on the settings screen, named rather than numbered.
///
/// **These were twenty-three `const ROW_*: i32` values, and the numbering was
/// the bug.** Every list of switches and sliders was keyed by position, so
/// inserting a row moved everything below it and the widgets went on being
/// built against the old numbers - a comment in the old code records exactly
/// that happening, a switch landing on the wrong row and leaving another with
/// none. Categories would have made it worse, each pane starting its own count
/// from zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    InterfaceScale,
    Sounds,
    StartFullscreen,
    ReadMetadata,
    ShowBackdrop,
    ResumeThreshold,
    WatchedThreshold,
    Updates,
    UpdateStatus,
    ClearData,
    /// The five rows each output has, told apart by which output they are for
    /// rather than by five more names apiece.
    Device(Role),
    Language(Role),
    Description(Role),
    Volume(Role),
    Sync(Role),
    SubtitlePreference,
    SubtitleSize,
    SubtitleFont,
    /// The rows one Kodi installation has, by its place in the list the pane
    /// was built from. Unlike every other row here there may be none of these,
    /// or several sets of them - which is why the category is told what was
    /// found before it can say what it holds.
    ///
    /// Which of the three an installation gets depends on how it was
    /// installed: a Snap has only the first, and says on it why there is
    /// nothing else, and only a Flatpak has the last.
    KodiType(usize),
    KodiHandover(usize),
    KodiPermission(usize),
    /// Stands in for the groups when there are none, so the pane says why it
    /// is empty rather than only offering to add something.
    KodiNone,
    KodiAdd,
    /// The one row the Jellyfin pane has, in whichever of its two states it
    /// is in. Never both: a Connect that is really a Disconnect, or a
    /// Disconnect on a pane with nothing to disconnect from, would each be a
    /// row that means the opposite of what it says.
    JellyfinConnect,
    JellyfinDisconnect,
    Notices,
}

impl Item {
    /// The bar this row carries, if it carries one.
    fn slider(self) -> Option<Slider> {
        Some(match self {
            Item::InterfaceScale => Slider::Scale,
            Item::SubtitleSize => Slider::SubtitleSize,
            Item::Volume(role) => Slider::Volume(role.key()),
            Item::Sync(role) => Slider::Offset(role.key()),
            Item::ResumeThreshold => Slider::ResumeThreshold,
            Item::WatchedThreshold => Slider::WatchedThreshold,
            _ => return None,
        })
    }

    /// The chooser this row opens, if it opens one.
    fn setting(self) -> Option<Setting> {
        Some(match self {
            Item::Device(Role::Primary) => Setting::PrimaryDevice,
            Item::Device(Role::Secondary) => Setting::SecondaryDevice,
            Item::Language(Role::Primary) => Setting::PrimaryLanguage,
            Item::Language(Role::Secondary) => Setting::SecondaryLanguage,
            Item::SubtitlePreference => Setting::SubtitleLanguage,
            Item::SubtitleFont => Setting::SubtitleFont,
            Item::KodiType(index) => Setting::KodiType(index),
            Item::KodiHandover(index) => Setting::KodiHandover(index),
            _ => return None,
        })
    }

    /// Whether a switch sits on this row, which decides two things: that a
    /// click on the row itself must not work it, and that activating the row
    /// from the keyboard must.
    fn has_switch(self) -> bool {
        matches!(
            self,
            Item::InterfaceScale
                | Item::Sounds
                | Item::StartFullscreen
                | Item::ReadMetadata
                | Item::ShowBackdrop
                | Item::Description(_)
                | Item::Volume(_)
                | Item::Sync(_)
                | Item::Updates
        )
    }
}

/// What the Kodi pane needs to know about one Kodi to say which rows
/// it has and what heads them.
///
/// A descriptor rather than the `Setup` itself, so `Category::items` stays a
/// plain function of its inputs: a test can ask what the pane holds for three
/// imagined installations without a disk to find any of them on.
/// Where the connection flow was started from, and so where it goes back to.
///
/// It is reachable from two screens that have nothing to do with each other -
/// the settings pane, and the page shown when no video is loaded - and a flow
/// that always returned to Settings would strand somebody who never opened it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ConnectFrom {
    Settings,
    Menu,
}

/// How far the pairing with a Jellyfin server has got, which is all the pane
/// needs to know to say what is on it.
///
/// Two states rather than three. A server that has been named but never
/// approved reads exactly like one that has not been named at all - there is
/// an address to set and a code to ask for - and the difference between them
/// is whether Connect can be pressed, which is a fact about one row rather
/// than about the pane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum JellyfinPane {
    NotConnected,
    Connected,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct KodiPane {
    /// The group heading, which is the installation's name: "KODI 21.1
    /// (STANDARD)". Held rather than derived because working it out means
    /// asking the system what version it installed.
    heading: String,
    confinement: crate::kodi_setup::Confinement,
}

/// The left column of the settings screen, and what each of its entries holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    General,
    Outputs,
    Subtitles,
    /// The server TinePlayer can be cast from. Named for the one thing in it,
    /// on the rule the Kodi category set below.
    Jellyfin,
    /// Named for the one thing in it rather than for the kind of thing it is.
    /// It was "Integrations", which reads as a place to put the next one - and
    /// everything in here is a group per Kodi installation, so a second kind
    /// of integration would land among them with nothing to say where Kodi
    /// ends and it begins. Whatever comes next gets a category of its own,
    /// which is what Jellyfin above is.
    Kodi,
    About,
}

impl Category {
    /// The order the column shows them in, which is the only thing the order
    /// of these decides.
    const ALL: [Category; 6] = [
        Category::General,
        Category::Outputs,
        Category::Subtitles,
        Category::Jellyfin,
        Category::Kodi,
        Category::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Category::General => "General",
            Category::Outputs => "Outputs",
            Category::Subtitles => "Subtitles",
            Category::Jellyfin => "Jellyfin",
            Category::Kodi => "Kodi",
            Category::About => "About",
        }
    }

    /// What the right-hand pane shows, and the heading each group opens with.
    ///
    /// `kodis` is every Kodi installation found, and `jellyfin` how far the
    /// pairing with a server has got. Both are passed in rather than looked up
    /// here so this stays a plain function of its inputs, and so a test can ask
    /// what a category holds without an application to ask it of - one walks
    /// the disk and the other reads a credentials file.
    ///
    /// The headings are what make Outputs readable: it holds two rows called
    /// Volume and two called Audio Sync, and until now they were told apart
    /// only by which half of the list they were in. The Kodi category now works the
    /// same way, one heading per installation.
    fn items(
        self,
        kodis: &[KodiPane],
        jellyfin: JellyfinPane,
    ) -> Vec<(Option<Cow<'static, str>>, Item)> {
        match self {
            Category::General => vec![
                (Some("INTERFACE".into()), Item::InterfaceScale),
                (None, Item::Sounds),
                (None, Item::StartFullscreen),
                (Some("LIBRARY".into()), Item::ReadMetadata),
                (None, Item::ShowBackdrop),
                (None, Item::ResumeThreshold),
                (None, Item::WatchedThreshold),
                (Some("UPDATES".into()), Item::Updates),
                (None, Item::UpdateStatus),
                // Last, and alone under its own heading: it is the one thing
                // on this screen that destroys something.
                (Some("DATA".into()), Item::ClearData),
            ],
            Category::Outputs => vec![
                (Some("FIRST OUTPUT".into()), Item::Device(Role::Primary)),
                (None, Item::Language(Role::Primary)),
                (None, Item::Description(Role::Primary)),
                (None, Item::Volume(Role::Primary)),
                (None, Item::Sync(Role::Primary)),
                (Some("SECOND OUTPUT".into()), Item::Device(Role::Secondary)),
                (None, Item::Language(Role::Secondary)),
                (None, Item::Description(Role::Secondary)),
                (None, Item::Volume(Role::Secondary)),
                (None, Item::Sync(Role::Secondary)),
            ],
            Category::Subtitles => vec![
                (None, Item::SubtitlePreference),
                (None, Item::SubtitleSize),
                (None, Item::SubtitleFont),
            ],
            Category::Kodi => {
                // Nothing found: one heading rather than two, because the row
                // saying so and the row that does something about it are the
                // same subject. A pane offering only "Add a Kodi Folder" would
                // leave somebody wondering whether it had looked.
                if kodis.is_empty() {
                    return vec![(Some("KODI".into()), Item::KodiNone), (None, Item::KodiAdd)];
                }

                let mut rows: Vec<(Option<Cow<'static, str>>, Item)> = Vec::new();
                for (index, kodi) in kodis.iter().enumerate() {
                    rows.push((Some(kodi.heading.clone().into()), Item::KodiType(index)));
                    // A Snap gets the one row and no others. It cannot start
                    // an external player at all, so a handover question below
                    // it would be a setting for something that will not
                    // happen - and the row itself carries the reason.
                    if !kodi.confinement.supported() {
                        continue;
                    }
                    rows.push((None, Item::KodiHandover(index)));
                    if kodi.confinement == crate::kodi_setup::Confinement::Flatpak {
                        rows.push((None, Item::KodiPermission(index)));
                    }
                }
                // Under a heading of its own: it belongs to no installation,
                // and without one it reads as another row of the last group.
                rows.push((Some("OTHER".into()), Item::KodiAdd));
                rows
            }
            // One heading over the lot. Unlike Kodi there is only ever one
            // server, so a group per anything would be a group of one.
            // One row, which is the whole of what there is to do: connect,
            // or stop being connected. The server and the account are facts
            // rather than settings, so they are stated in the note under the
            // heading instead of taking a row each and inviting a press.
            Category::Jellyfin => match jellyfin {
                JellyfinPane::NotConnected => {
                    vec![(Some("JELLYFIN".into()), Item::JellyfinConnect)]
                }
                JellyfinPane::Connected => {
                    vec![(Some("JELLYFIN".into()), Item::JellyfinDisconnect)]
                }
            },
            // The text itself is not a row - see `about_body`, which the
            // pane draws above these.
            Category::About => vec![(None, Item::Notices)],
        }
    }
}

/// What the file browser was opened to find.
///
/// Held on the application rather than passed down, because stepping into a
/// folder re-enters the browser and would otherwise forget the errand.
#[derive(Clone, Copy, PartialEq, Default)]
enum Errand {
    #[default]
    Video,
    /// A separate soundtrack for one of the two outputs.
    Audio(Role),
    /// A subtitle file from somewhere other than beside the video.
    Subtitle,
}

/// A screen's navigation, held while a popover borrows the keyboard.
struct NavState {
    list: Option<gtk::ListBox>,
    header: Vec<gtk::Button>,
    footer: Vec<gtk::Button>,
    header_entry: Option<gtk::Button>,
    stops: Vec<gtk::Widget>,
    copy_root: Option<gtk::Widget>,
}

/// What the alignment thread has to say for itself, on its way back to the
/// main thread.
enum Step {
    /// How many of the three windows have finished.
    Window(usize),
    Done(crate::align::Verdict),
}

/// How far the Quick Connect thread has got, on its way back to the main
/// thread.
///
/// One channel for the whole pairing rather than one per stage: asking for a
/// code and waiting for it to be approved are two halves of one errand, and
/// the panel shows them in the same place.
enum QuickConnect {
    /// What the server calls itself, asked before anything else because it is
    /// unauthenticated and cheap, and because a panel that can say which
    /// server it is talking to should say so before asking for a code.
    Named(String),
    /// The six characters to show, once the server has issued them.
    Code(String),
    /// Approved, with the account it granted. Boxed because it is much the
    /// largest of the three, and every message would otherwise be its size.
    Done(Box<crate::jellyfin::Account>),
    /// Refused, expired, or a server that could not be reached. All of them
    /// end the same way: say so, and let another code be asked for.
    Failed(String),
}

/// Choices given on the command line, which skip the menu entirely.
#[derive(Clone)]
pub struct Preset {
    /// A track number as `--list-tracks` prints them, a language code, `ad`,
    /// or `en:ad`. See [`crate::probe::resolve_audio`].
    pub primary: Option<String>,
    pub secondary: Option<String>,
    /// A number, a language code, or a subtitle file name beside the video.
    pub subtitle: Option<String>,
}

/// How this run was started, as against what it should play.
///
/// Grouped because they arrive together from the command line and are read
/// together here, and because the list was going to keep growing.
#[derive(Clone, Copy)]
pub struct Launch {
    /// Ignore any saved position and start from the beginning.
    pub restart: bool,
    pub fullscreen: bool,
    /// Fullscreen is not the viewer's to change: a launcher asked for it and
    /// is waiting for this playback, so the controls for it are gone rather
    /// than present and refusing.
    pub locked_fullscreen: bool,
    /// Something else chose the video and is waiting for this playback.
    pub external: bool,
    /// That something else is Kodi, which can also be talked to.
    pub kodi: bool,
    /// Start playing rather than opening the menu.
    pub play: bool,
}

/// Everything the menu can act on. Devices persist to the config file;
/// the file and track choices last for the session.
pub struct App {
    window: gtk::ApplicationWindow,
    /// Holds the display awake while a film is playing. See [`crate::awake`].
    awake: crate::awake::KeepAwake,
    config: RefCell<Config>,
    /// What the version check has found, and what has been seen of it.
    /// Held here so the settings screen and the badge on the button that
    /// opens it read the same answer.
    updates: RefCell<crate::updates::State>,
    /// The buttons currently on screen that should carry the mark when
    /// a new version is waiting to be seen.
    update_badges: RefCell<Vec<gtk::Button>>,
    file: RefCell<Option<Source>>,
    /// What is known about the file on screen: its name, its artwork, and
    /// whatever a sidecar or the container itself had to say. Default when
    /// there is no file, and default is a perfectly good page - most files
    /// have no sidecar and the layout is designed up from that case.
    details: RefCell<crate::metadata::Details>,
    /// Whether a now-playing update is already queued behind the main loop.
    ///
    /// Set while one is pending so that a scrub, which commits a seek on every
    /// release, does not queue a copy of the poster for each of them.
    now_playing_queued: Cell<bool>,
    /// Artwork already decoded for the file on screen, keyed by nothing more
    /// than being the current file: it is dropped whenever one is loaded.
    ///
    /// Held so that returning from a chooser redraws the page instantly. The
    /// menu is rebuilt on every trip in and out of one, and re-reading a
    /// backdrop from a network share each time is both slow and visible.
    /// Whether the page being built already has artwork that has only just
    /// arrived, and so should fade rather than appear.
    ///
    /// A page opened with its pictures already in hand draws them; it does not
    /// perform.
    fade_art: Cell<bool>,
    /// The two places artwork is drawn, kept so that a picture arriving late
    /// can be put into the page that is already on screen.
    ///
    /// Rebuilding the page instead would be simpler and is what this used to
    /// do. It is wrong: artwork can take seconds - a backdrop over a network
    /// especially - and by then somebody may be part-way down the track lists
    /// choosing what to watch. Rebuilding under them moves their focus and
    /// undoes what they were doing, to deliver a picture they did not ask for
    /// yet.
    backdrop_widget: RefCell<Option<crate::artwork::Artwork>>,
    poster_frame: RefCell<Option<gtk::Box>>,
    poster_art: RefCell<Option<gdk::Texture>>,
    backdrop_art: RefCell<Option<gdk::Texture>>,
    /// Bumped whenever a different file is loaded, so artwork still arriving
    /// from a thread for the previous one is dropped rather than drawn.
    art_generation: Cell<u64>,
    /// The output devices as last enumerated, and whether an enumeration has
    /// ever finished.
    ///
    /// Held because finding them is not cheap: it starts a GStreamer device
    /// monitor, which probes every audio backend on the machine and takes long
    /// enough on the main thread to be seen as lag when a menu opens. The list
    /// changes only when hardware is plugged in or unplugged, so it is worth
    /// keeping between openings rather than asking again each time.
    device_names: RefCell<Vec<String>>,
    device_scan: Cell<bool>,
    /// The rebuild waiting for a drag-resize to stop, and the poster height
    /// the page on screen was built at. See [`App::rebuild_when_resize_ends`].
    resize_settle: RefCell<Option<glib::SourceId>>,
    built_poster: Cell<f64>,
    tracks: RefCell<Vec<AudioTrack>>,
    primary_track: RefCell<Option<u32>>,
    secondary_track: RefCell<Option<u32>>,
    /// A separate audio file feeding an output, in place of any track inside
    /// the video. Takes precedence over that output's track, which is left
    /// alone so it comes back if the file is cleared.
    primary_file: RefCell<Option<Source>>,
    secondary_file: RefCell<Option<Source>>,
    /// Which output the browser is picking a soundtrack for, or `None` when it
    /// is picking a video. Held here because stepping into a folder re-enters
    /// the browser and would otherwise lose the errand it was opened on.
    /// What the browser is open for. One value rather than a flag per errand:
    /// the browser, the system dialog and the row handler all ask this, and
    /// two flags could answer differently.
    errand: Cell<Errand>,
    /// What alignment worked out for each output, in milliseconds, ready to
    /// add to whatever the viewer has set. Already negated: alignment reports
    /// how late the audio runs, and a sink is held back by a negative offset.
    ///
    /// Zero when nothing has been measured, which is most of the time - so the
    /// arithmetic below is the same whether there is a baseline or not.
    primary_baseline: Cell<f64>,
    secondary_baseline: Cell<f64>,
    /// The video's running time in seconds, which alignment needs to place its
    /// three windows across it. Zero when the source could not say, which some
    /// live streams cannot.
    duration_s: Cell<f64>,
    /// Everything on offer for the current file: streams inside it, then
    /// subtitle files sitting beside it.
    subtitle_options: RefCell<Vec<Subtitle>>,
    subtitle: RefCell<Option<SubtitleChoice>>,
    playback: RefCell<Option<Rc<Playback>>>,
    screen: RefCell<Screen>,
    /// Restored when returning from a chooser, so the menu comes back with
    /// the row you left from still highlighted.
    menu_row: RefCell<i32>,
    settings_row: RefCell<i32>,
    sounds: RefCell<Sounds>,
    restart: bool,
    /// The list the current screen is built around, and the button below it.
    ///
    /// The keyboard reaches these through GTK's own focus handling, but the
    /// gamepad has no events to hand to GTK, so it needs to move the
    /// selection itself and therefore needs to know what it is moving.
    nav_list: RefCell<Option<gtk::ListBox>>,
    /// A second list beside the main one, waiting to be put into the tab
    /// order.
    ///
    /// Held rather than added directly because a screen builds its column
    /// before it wires its navigation, and `set_nav` rebuilds the order from
    /// scratch - so anything added ahead of it was thrown away again.
    nav_side_list: RefCell<Option<gtk::ListBox>>,
    /// What Tab moves between on this screen, in order: the header buttons,
    /// the lists, then the footer buttons.
    ///
    /// Kept because GTK will not do it. A GtkListBox implements focus
    /// traversal by moving between its rows, so once no row can take focus it
    /// reports that it cannot be focused at all and Tab steps straight over
    /// it - even though focusing the list directly works perfectly well.
    nav_stops: RefCell<Vec<gtk::Widget>>,
    /// The sliders on the settings screen, by the row each one sits in, so
    /// left and right can find the one that is selected. Emptied whenever a
    /// screen without them is built.
    settings_sliders: RefCell<Vec<(Item, Slider, gtk::Scale, gtk::Label)>>,
    /// Which category the settings screen is showing, kept so leaving and
    /// coming back lands where it was left rather than at the top.
    settings_category: Cell<Category>,
    /// Whether the keyboard is in the settings themselves rather than in the
    /// column of categories.
    ///
    /// The screen is entered a step at a time: the categories take the keys
    /// first, Enter hands them to the settings beside them, and Escape hands
    /// them back before it leaves the screen. Left and right cannot do that
    /// job here - they belong to the bars on half these rows, and a row
    /// without one would have moved the focus off the pane instead.
    in_settings_pane: Cell<bool>,
    /// What the right-hand pane is showing, by row. The one place a row's
    /// position is turned back into what it is.
    pane_items: RefCell<Vec<Item>>,
    /// The About page's scroll position, so up and down can move a page that
    /// has nothing on it to select.
    about_scroll: RefCell<Option<gtk::Adjustment>>,
    /// Where selectable text lives on the screen being shown, so Ctrl+C can
    /// find it. Set by the screens that have any, cleared by every other.
    copy_root: RefCell<Option<gtk::Widget>>,
    /// The switches on the settings screen, by row, so a toggle can move the
    /// one it belongs to instead of rebuilding the screen under the viewer.
    settings_switches: RefCell<Vec<(Item, gtk::Switch)>>,
    /// The settings list itself, so a row can be redrawn without rebuilding
    /// the screen around it.
    settings_list: RefCell<Option<gtk::ListBox>>,
    /// The column of categories beside it, so the keyboard can be handed back
    /// to it from outside the function that built it.
    settings_categories: RefCell<Option<gtk::ListBox>>,
    /// What a category says above its rows, where it says anything. Only About
    /// does: its text used to be a screen of its own, two steps away from the
    /// row that named it.
    settings_body: RefCell<Option<gtk::Box>>,
    /// The Kodi installations the Kodi pane was last built from, so a
    /// row can say what it is and act on it without scanning the disk again
    /// for every label it draws.
    kodi_setups: RefCell<Vec<crate::kodi_setup::Setup>>,
    /// Whether the settings row about to be activated was clicked rather than
    /// chosen with a key or a gamepad. A switch row responds to a press on
    /// the switch itself, not to a click anywhere along the row - but Enter
    /// on the selected row must still work it, and both arrive here as the
    /// same activation.
    clicked_row: Cell<bool>,
    /// Set while a switch is being moved to match what it already reports, so
    /// its own handler knows not to act on it.
    settling_switch: Cell<bool>,
    /// Whether the key that works the highlighted control is still down, so
    /// that holding it acts once rather than on every repeat the keyboard
    /// sends.
    key_held: Cell<bool>,
    /// Whether the press now in progress started the volume button's hold,
    /// and so still has the ordinary press to do when the key comes up.
    hold_started: Cell<bool>,
    /// Counts releases, so one waiting to be believed can be dropped when a
    /// repeat arrives behind it.
    releases: Cell<u64>,
    /// The size a drag has reached, kept until the bar is let go. Nothing
    /// while the size is not being dragged.
    wanted_scale: Cell<Option<f64>>,
    nav_footer: RefCell<Vec<gtk::Button>>,
    /// Buttons above the list: the browser's path trail, and the media page's
    /// play and settings row. Up from the first row reaches them, the way Down
    /// reaches the footer.
    nav_header: RefCell<Vec<gtk::Button>>,
    /// Which header button Up from the list should land on, where the last
    /// one is not the right answer.
    ///
    /// The default is the last, which is what a path trail wants: the crumb
    /// nearest the list is the folder you are in. A row of actions wants the
    /// opposite - the first is the one the page is for, and arriving on
    /// Settings when you meant Play is a button's width of travel every time.
    nav_header_entry: RefCell<Option<gtk::Button>>,
    controls: RefCell<Option<Rc<Controls>>>,
    /// Whether the window was already maximized when fullscreen was entered,
    /// so that leaving fullscreen can put back the state it found rather than
    /// the one fullscreen implies. See [`App::toggle_fullscreen`].
    maximized_before_fullscreen: Cell<bool>,
    /// The size the window last had while it was an ordinary window, kept so
    /// it can be written down on the way out.
    ///
    /// Tracked rather than read at the end, because by then it may not be one:
    /// a window closed while maximized or fullscreen reports the screen, and
    /// saving that would mean opening at screen size for ever after with
    /// nothing to go back to.
    windowed_size: Cell<(i32, i32)>,
    /// Kept so the interface can be re-scaled after the fact.
    styles: gtk::CssProvider,
    /// The scale in force, which the settings screen reports and the
    /// monitor check below may revise.
    scale: Cell<f64>,
    /// Bumped whenever a scrub ends, retiring the ticker that was driving it.
    scrub_generation: Cell<u64>,
    /// Last time a scrub key or button was seen held.
    scrub_seen: Cell<Option<std::time::Instant>>,
    /// Drives the controls readout while a file is playing.
    tick: RefCell<Option<glib::SourceId>>,
    /// Something else chose the video and is waiting for this playback of it:
    /// no browser, no drag and drop, no confirmation on the way out. Set by
    /// `--external`, and by `--kodi`, which implies it.
    external: bool,
    /// Whether fullscreen is fixed for this run. See [`Launch`].
    locked_fullscreen: bool,
    /// Whether the error on screen ended the session: a video named on the
    /// command line that could not be opened leaves nothing to go back to, so
    /// its button closes the player. Every other error returns to the menu.
    error_is_fatal: Cell<bool>,
    /// What Kodi says it is playing through us: its title, database id, resume
    /// point, and the path to report progress against. Fetched once at startup,
    /// because it cannot change while we are the player. `None` when Kodi was
    /// not involved or did not answer, which is not an error.
    kodi_item: RefCell<Option<crate::kodi::Item>>,
    /// The server this installation is paired with, once it has been reached.
    ///
    /// Absent when Jellyfin was never set up, when the pairing was revoked,
    /// and while the server is unreachable - all of which are ordinary, and
    /// none of which stop anything else working.
    jellyfin: RefCell<Option<crate::jellyfin::Client>>,
    /// What the pairing file says, as the settings pane last read it.
    ///
    /// Held rather than read per row, for the same reason the Kodi
    /// installations are: every label, value and enabled state on that pane
    /// comes out of this, and a file read apiece would be a dozen for one
    /// screen. Re-read whenever the pane is built, so a token revoked from
    /// elsewhere shows up on the next visit.
    jellyfin_pairing: RefCell<Option<crate::jellyfin::Pairing>>,
    /// Bumped whenever a Quick Connect is started or abandoned, so the polling
    /// left over from one attempt cannot outlive it and approve another.
    jellyfin_attempt: Cell<u64>,
    /// Which screen the connection flow was opened from, and so where
    /// finishing or cancelling it returns to.
    connect_from: Cell<ConnectFrom>,
    /// What Jellyfin knows about the video on screen, when it was cast from
    /// there. The counterpart to `kodi_item`, and read by the same three
    /// accessors: a launcher's library knows the title and where the viewer
    /// stopped better than the file does.
    jellyfin_item: RefCell<Option<crate::jellyfin::Item>>,
    /// The open connection. Dropping it closes the socket, which is how
    /// disconnecting works - and while it is closed TinePlayer is not on
    /// anybody's phone, so it is held for the life of the application.
    jellyfin_session: RefCell<Option<crate::jellyfin::Session>>,
    /// Ticks since Jellyfin was last told where playback had reached.
    jellyfin_reported: Cell<u32>,
    /// What Jellyfin calls this viewing. One string for the whole of it, from
    /// started to stopped, because that is how the server ties the reports
    /// together into a single session rather than three unrelated events.
    jellyfin_play_session: RefCell<String>,
    /// Where playback had reached when it was last left, and the video it
    /// belongs to. Offered as a resume point regardless of how far in it was,
    /// unlike a position read back from disk.
    ///
    /// The saved-position rules exist to answer "were you part way through
    /// this, days ago" - a minute into a long film is a false start rather
    /// than progress. Within one session that question is already answered:
    /// you were watching it a moment ago. Backing out to change a setting and
    /// losing your place is the exact annoyance those rules guard against.
    session_resume: RefCell<Option<(String, u64)>>,
    /// Whether subtitles were switched off during this video, so leaving
    /// playback and coming back does not turn them on again. Cleared when a
    /// different video is loaded, or when a different subtitle is chosen -
    /// picking one is asking to see it.
    subtitles_hidden: Cell<bool>,
    /// Whether a volume change is waiting to be written out. Dragging a slider
    /// produces a change per pixel, and each one would otherwise be a write to
    /// disk.
    volume_save_pending: Cell<bool>,
    /// Whether everything is silenced at once. Held here rather than in the
    /// configuration, and separately from each output's own mute, because it is
    /// a layer over them: the outputs go on being set to whatever they were set
    /// to underneath it, and a film that started silent because of a door
    /// knocked on last week would be a bug rather than a memory.
    hushed: Cell<bool>,
    /// Whether a report of what the sound is doing is already on its way to
    /// Jellyfin. Dragging a bar produces a change per pixel, and each one would
    /// otherwise be a request across the house.
    sound_report_pending: Cell<bool>,
    /// The state of a hold on the gamepad's left face button, which silences
    /// everything rather than changing the subtitles. The same button for
    /// both because they are the same question - whether you are being given
    /// the sound or the words. Kept here rather than in the controls: it is
    /// about what a button meant, not about the strip, and it works whether
    /// or not the strip is on screen.
    subtitles_hold: Cell<u64>,
    subtitles_holding: Cell<bool>,
    subtitles_held: Cell<bool>,
    /// The screen a modal was opened from, so backing out of one returns
    /// there. Reached by shortcut as well as by row, so it cannot be assumed
    /// to be the step that offers them.
    origin: Cell<Screen>,
}

impl App {
    pub fn build(
        gtk_app: &gtk::Application,
        config: Config,
        file: Option<Source>,
        preset: Option<Preset>,
        launch: Launch,
        config_problem: Option<String>,
    ) {
        let Launch {
            restart,
            fullscreen,
            locked_fullscreen,
            external,
            kodi,
            play,
        } = launch;
        appearance::force_dark();
        suppress_error_bell();

        // Sized from the tallest monitor to begin with, since no window exists
        // yet to ask which one it is on. Corrected below once there is.
        let styles = install_styles();
        let monitor = appearance::tallest_monitor();
        let scale = appearance::resolve_scale(config.ui_scale, monitor.as_ref());
        styles.load_from_data(&style_css(scale));
        if config.ui_scale.is_none()
            && scale != 1.0
            && let Some(monitor) = monitor.as_ref()
        {
            eprintln!(
                "Interface scaled {scale}x for a {}px-tall display. \
                 Set ui_scale in the config file to override.",
                monitor.geometry().height()
            );
        }

        let sounds = Sounds::new(config.sounds, config.primary_sink.clone());

        let (width, height) = default_window_size(
            scale,
            monitor.as_ref(),
            (config.window_width, config.window_height),
        );
        let window = gtk::ApplicationWindow::builder()
            .application(gtk_app)
            .title("TinePlayer")
            .default_width(width)
            .default_height(height)
            .build();

        // Which monitor the window landed on is only knowable once it has
        // been realized, and on a mixed setup (a television beside a desk
        // monitor) that is the difference between a readable menu and a tiny
        // one. Skipped entirely when the size was set by hand.

        let app = Rc::new(App {
            window: window.clone(),
            awake: crate::awake::KeepAwake::new(gtk_app),
            config: RefCell::new(config),
            updates: RefCell::new(crate::updates::load()),
            update_badges: RefCell::new(Vec::new()),
            file: RefCell::new(None),
            details: RefCell::new(Default::default()),
            now_playing_queued: Cell::new(false),
            fade_art: Cell::new(false),
            backdrop_widget: RefCell::new(None),
            poster_frame: RefCell::new(None),
            poster_art: RefCell::new(None),
            backdrop_art: RefCell::new(None),
            art_generation: Cell::new(0),
            device_names: RefCell::new(Vec::new()),
            device_scan: Cell::new(false),
            maximized_before_fullscreen: Cell::new(false),
            windowed_size: Cell::new((0, 0)),
            resize_settle: RefCell::new(None),
            built_poster: Cell::new(0.0),
            tracks: RefCell::new(Vec::new()),
            primary_file: RefCell::new(None),
            secondary_file: RefCell::new(None),
            errand: Cell::new(Errand::Video),
            primary_baseline: Cell::new(0.0),
            secondary_baseline: Cell::new(0.0),
            duration_s: Cell::new(0.0),
            primary_track: RefCell::new(None),
            secondary_track: RefCell::new(None),
            subtitle_options: RefCell::new(Vec::new()),
            subtitle: RefCell::new(None),
            playback: RefCell::new(None),
            screen: RefCell::new(Screen::Menu),
            menu_row: RefCell::new(0),
            settings_row: RefCell::new(0),
            sounds: RefCell::new(sounds),
            restart,
            nav_list: RefCell::new(None),
            nav_side_list: RefCell::new(None),
            nav_stops: RefCell::new(Vec::new()),
            settings_sliders: RefCell::new(Vec::new()),
            settings_category: Cell::new(Category::General),
            in_settings_pane: Cell::new(false),
            pane_items: RefCell::new(Vec::new()),
            about_scroll: RefCell::new(None),
            copy_root: RefCell::new(None),
            settings_switches: RefCell::new(Vec::new()),
            settings_list: RefCell::new(None),
            settings_categories: RefCell::new(None),
            settings_body: RefCell::new(None),
            kodi_setups: RefCell::new(Vec::new()),
            clicked_row: Cell::new(false),
            settling_switch: Cell::new(false),
            key_held: Cell::new(false),
            hold_started: Cell::new(false),
            releases: Cell::new(0),
            wanted_scale: Cell::new(None),
            nav_footer: RefCell::new(Vec::new()),
            nav_header: RefCell::new(Vec::new()),
            nav_header_entry: RefCell::new(None),
            controls: RefCell::new(None),
            styles: styles.clone(),
            scale: Cell::new(scale),
            scrub_generation: Cell::new(0),
            scrub_seen: Cell::new(None),
            tick: RefCell::new(None),
            external,
            locked_fullscreen,
            error_is_fatal: Cell::new(false),
            kodi_item: RefCell::new(None),
            jellyfin: RefCell::new(None),
            jellyfin_pairing: RefCell::new(crate::jellyfin::load()),
            jellyfin_attempt: Cell::new(0),
            connect_from: Cell::new(ConnectFrom::Settings),
            jellyfin_item: RefCell::new(None),
            jellyfin_session: RefCell::new(None),
            jellyfin_reported: Cell::new(0),
            jellyfin_play_session: RefCell::new(String::new()),
            session_resume: RefCell::new(None),
            subtitles_hidden: Cell::new(false),
            volume_save_pending: Cell::new(false),
            hushed: Cell::new(false),
            sound_report_pending: Cell::new(false),
            subtitles_hold: Cell::new(0),
            subtitles_holding: Cell::new(false),
            subtitles_held: Cell::new(false),
            origin: Cell::new(Screen::Menu),
        });

        // Weak, so the polling closure doesn't keep the application alive
        // after its window has gone.
        {
            let weak = Rc::downgrade(&app);
            crate::gamepad::install(move |action| {
                if let Some(app) = weak.upgrade() {
                    app.handle_action(action);
                }
            });
        }

        // Playback has to be torn down before the window goes away, so the
        // resume position is written and the audio devices are released.
        {
            let app = app.clone();
            window.connect_close_request(move |_| {
                app.stop_playback();
                glib::Propagation::Proceed
            });
        }

        // Which monitor the window landed on is only knowable once it has
        // been realized, and on a mixed setup (a television beside a desk
        // monitor) that is the difference between a readable menu and a tiny
        // one. Skipped entirely when the size was set by hand.
        // Watched whatever the size is set to now: a size set by hand can be
        // handed back to the screen while running, and nothing would be
        // listening if these were attached only when it started out
        // automatic. `follow_automatic_scale` decides whether to act.
        let weak = Rc::downgrade(&app);
        window.connect_realize(move |window| {
            let Some(app) = weak.upgrade() else { return };
            app.follow_automatic_scale(window);
            // The surface is what reports the window's size as it is dragged,
            // and it does not exist until here. Connected to rather than the
            // window's own properties because it survives every rebuild of the
            // page, so this handler is attached exactly once.
            if let Some(surface) = window.surface() {
                let weak = Rc::downgrade(&app);
                surface.connect_layout(move |_, _, _| {
                    let Some(app) = weak.upgrade() else { return };
                    app.note_windowed_size();
                    app.rebuild_when_resize_ends();
                });
            }
        });
        // And again whenever the window fills the screen or stops doing so,
        // since that is what the automatic size depends on.
        let weak = Rc::downgrade(&app);
        window.connect_fullscreened_notify(move |window| {
            let Some(app) = weak.upgrade() else { return };
            app.follow_automatic_scale(window);
        });

        // The media page draws its poster as a proportion of the page's
        // height, which is read when the page is built. Filling the screen
        // changes that height by a long way in one step, so the page is
        // rebuilt to match rather than left with a poster sized for a window
        // half the height.
        //
        // Connected here, once, rather than by the page that wants it: a
        // handler attached while building the menu would be attached again by
        // every rebuild, and each rebuild would then trigger the next.
        // Deferred to an idle so the rebuild does not tear down the widgets
        // whose own handlers are still running.
        for maximize in [true, false] {
            let weak = Rc::downgrade(&app);
            let watch = move |window: &gtk::ApplicationWindow| {
                let _ = window;
                let Some(app) = weak.upgrade() else { return };
                if *app.screen.borrow() != Screen::Menu {
                    return;
                }
                glib::idle_add_local_once(move || {
                    if *app.screen.borrow() == Screen::Menu {
                        app.show_menu();
                    }
                });
            };
            match maximize {
                true => window.connect_maximized_notify(watch),
                false => window.connect_fullscreened_notify(watch),
            };
        }

        {
            let weak = Rc::downgrade(&app);
            window.connect_close_request(move |_| {
                if let Some(app) = weak.upgrade() {
                    app.remember_window_size();
                }
                glib::Propagation::Proceed
            });
        }

        {
            let weak = Rc::downgrade(&app);
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, _, _| {
                if let Some(app) = weak.upgrade() {
                    app.show_pointer();
                }
            });
            window.add_controller(motion);
        }

        app.install_key_handling();
        app.install_accelerators(gtk_app);

        // Find the outputs now, in the background, so the first menu that
        // lists them opens with them already in it rather than with
        // "Searching for outputs..." and a pause. Startup is where there is
        // time to spare for this: nothing is waiting on the answer, and the
        // probe takes long enough to be seen if it is left until a menu wants
        // it. Every opening still looks again - see `show_selector` - so this
        // is a head start rather than the only look.
        app.scan_devices_soon(|_| {});

        // Applied to the window itself rather than at playback, so the
        // menus are fullscreen too.
        if fullscreen {
            window.fullscreen();
        }

        // Asked before the file is loaded, because the answer supplies the
        // title shown for it and the resume position it starts from. Kodi is
        // the only thing that knows either, and only it can say which library
        // item this playback is.
        if kodi {
            *app.kodi_item.borrow_mut() = crate::kodi::current_item();
        }

        let unopenable = match &file {
            Some(source) => app.set_file(source).err().map(|e| (source.clone(), e)),
            None => None,
        };

        // Track choices from the command line are applied whether or not
        // playback is starting. Without --play they simply arrive already
        // made, so the menu opens on them and they can be checked before
        // pressing Play.
        //
        // Each output is only touched when its own flag was given. Assigning
        // both meant `--primary` alone silenced the secondary, because an
        // absent flag resolved to no track - so naming one output threw away
        // what the language preference had already chosen for the other.
        if let Some(preset) = preset.as_ref()
            && app.file.borrow().is_some()
        {
            let resolve = |spec: &str| -> Option<u32> {
                match crate::probe::resolve_audio(spec, &app.tracks.borrow()) {
                    Ok(choice) => choice,
                    // Reported rather than obeyed silently, the same way a
                    // subtitle that cannot be resolved is: playing the
                    // wrong track is not what was asked for either.
                    Err(e) => {
                        eprintln!("{e}");
                        None
                    }
                }
            };
            // A spec naming a file that exists is an audio file to play on
            // that output, rather than anything to look for inside the video.
            // Checked before the track specs because none of them can be a
            // path: a number, a language code and `ad` are all short words,
            // and a file has to be there on disk to be taken for one.
            let as_file = |spec: &str| {
                let source = Source::parse(spec);
                source.is_available().then_some(source)
            };
            if let Some(spec) = preset.primary.as_deref() {
                match as_file(spec) {
                    Some(file) => *app.primary_file.borrow_mut() = Some(file),
                    None => *app.primary_track.borrow_mut() = resolve(spec),
                }
            }
            if let Some(spec) = preset.secondary.as_deref() {
                match as_file(spec) {
                    Some(file) => *app.secondary_file.borrow_mut() = Some(file),
                    None => *app.secondary_track.borrow_mut() = resolve(spec),
                }
            }

            // Only touched when asked for, so a video's remembered
            // subtitle survives being launched with audio flags alone.
            if let Some(spec) = preset.subtitle.as_deref() {
                // The languages actually going to the outputs, so a mode
                // like "primary_forced" means the same on the command line
                // as it does in the settings.
                let language_of = |index: Option<u32>| {
                    index.and_then(|index| {
                        app.tracks
                            .borrow()
                            .iter()
                            .find(|track| track.index == index)
                            .map(|track| track.language.clone())
                    })
                };
                let primary = language_of(*app.primary_track.borrow());
                let secondary = language_of(*app.secondary_track.borrow());
                match crate::subtitles::resolve(
                    spec,
                    &app.subtitle_options.borrow(),
                    primary.as_deref(),
                    secondary.as_deref(),
                ) {
                    Ok(choice) => *app.subtitle.borrow_mut() = choice,
                    // Reported rather than obeyed silently: playing with
                    // the wrong subtitles, or none, is not what was asked
                    // for either way.
                    Err(e) => eprintln!("{e}"),
                }
            }
            // An audio file named on the command line arrives after the media
            // was applied, so whatever alignment was measured for that pairing
            // has to be read again now that the pairing is known.
            app.load_baselines();
        }

        match (&unopenable, &config_problem) {
            // Nothing to choose from if the video could not be read, so the
            // reason is shown instead of an empty menu.
            //
            // The video comes first when both went wrong: it is what someone
            // asked for, and settings that failed to load can be seen for
            // themselves in the menu behind.
            (Some((source, error)), _) => app.show_source_error(source, error, true),
            // Not fatal: Back lands in the menu, which is where the settings
            // would be put right.
            (None, Some(problem)) => app.show_error(problem, false),
            // Asked for outright rather than inferred. Refused out loud when
            // there is nowhere to play to, since silently showing the menu
            // instead would leave a launcher waiting on a film that never
            // started, with nothing said about why.
            (None, None) if play => {
                if app.config.borrow().primary_sink.is_some() {
                    app.start_playback(app.restart);
                } else {
                    app.show_error(
                        "No audio output has been chosen yet, so there is nowhere to play.

                         Choose one under Settings, or run with --list-devices and set                          primary_sink in config.yaml.",
                        false,
                    );
                }
            }
            (None, None) => app.show_menu(),
        }

        // Never when something else is driving. A film handed over by Kodi is
        // not a session anyone chose to start, and a launcher waiting on
        // playback has no use for news about a release.
        if !external {
            app.check_for_updates(false);
        }

        window.present();

        // After the window is on screen, which is when it first has anything
        // for Windows to attach to. Windows sends the media keys as window
        // messages rather than as keys, so there they arrive through here
        // instead of the key handler; everywhere else this installs nothing,
        // because Linux reports them as ordinary keysyms. Both routes end at
        // `handle_media`, so the two can never disagree.
        {
            let weak = Rc::downgrade(&app);
            crate::media_keys::install(&window, move |command| {
                weak.upgrade().is_some_and(|app| app.handle_media(command))
            });
        }

        // Reaches the paired Jellyfin server, if there is one. Everything it
        // does is allowed to fail quietly: a server that is off is not a
        // reason for a video player to say anything on the way up.
        app.start_jellyfin();
    }

    /// The commands that belong to the application rather than to a screen,
    /// bound as actions with accelerators instead of keys matched by hand.
    ///
    /// `<Primary>` does not do what it is reputed to. It resolves to Control
    /// on macOS exactly as it does on Windows and Linux, so binding it alone
    /// left Command-Q dead - which is the bug this was meant to fix. Measured
    /// 2026-08-08 on GTK 4.22.4 by printing what `gtk::accelerator_parse`
    /// returns: `<Primary>q` came back `CONTROL_MASK`, and Command-Q did
    /// nothing on a Mac until Command was named outright.
    ///
    /// So macOS names Command outright, as `<Meta>`. That is measured too:
    /// a synthesised Command-G arrives at the key handler as `META_MASK` and
    /// a Control-G as `CONTROL_MASK`, on the same build seconds apart.
    /// Command raises no key event of its own, so there is nothing to learn
    /// from watching the modifier alone - the letter beside it is what
    /// carries the answer.
    ///
    /// Control stays bound on macOS as well: it is what the other two
    /// platforms use, someone who presses it means the same thing by it, and
    /// nothing else on a Mac wants Control-Q.
    ///
    /// Only commands with nothing focused behind them live here. An
    /// accelerator claims its key ahead of the widget that has focus, so copy
    /// and the rest stay in the key controller below, where a text field can
    /// still take the key first. See `primary_mask`.
    fn install_accelerators(self: &Rc<Self>, gtk_app: &gtk::Application) {
        // Command-Q on a Mac, Control-Q elsewhere, with W beside it: there is
        // one window, so closing it and quitting are the same act, and both
        // keys get reached for.
        //
        // Straight out, without the "Close the Player?" question Escape asks
        // from the top of the menu. That question guards against a keypress
        // nobody meant, which Escape can be; this is not a combination anyone
        // presses by accident. The resume position is still written, through
        // the window's close handler.
        let quit = gtk::gio::SimpleAction::new("quit", None);
        {
            let app = self.clone();
            quit.connect_activate(move |_, _| {
                // Waited on, unlike the window's own close handler: the
                // process is about to end, and the last progress report to
                // Kodi goes out on a detached thread that exiting would take
                // with it. The stop button under a launcher waits for the same
                // reason.
                app.finish_playback(true);
                app.window.close();
            });
        }
        gtk_app.add_action(&quit);
        bind_accels(gtk_app, "app.quit", &["q", "w"]);

        // Where every desktop platform keeps its preferences.
        //
        // Gated to the same screens as Ctrl+O and Ctrl+L, and for a sharper
        // reason: reaching Settings from playback means stopping the film,
        // which is what the control bar's settings button does deliberately
        // and what a shortcut must never do quietly. Leaving it off the wizard
        // screens keeps it from jumping out of a half-finished Kodi setup.
        let settings = gtk::gio::SimpleAction::new("settings", None);
        {
            let app = self.clone();
            settings.connect_activate(move |_, _| {
                // Copied out before `show_settings`, which takes the same cell
                // mutably.
                let screen = *app.screen.borrow();
                if matches!(screen, Screen::Menu | Screen::VideoSource) {
                    app.enter_settings();
                }
            });
        }
        gtk_app.add_action(&settings);
        bind_accels(gtk_app, "app.settings", &["comma"]);
    }

    fn install_key_handling(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let primary = primary_mask();
        let app = self.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            app.hide_pointer();
            let playing = app.playback.borrow().is_some();
            match key {
                // Only claimed during playback - the menus need Space for
                // activating whatever row has focus.
                //
                // The transport keys on a keyboard, a headset or a remote
                // arrive as ordinary key events, so they cost nothing but a
                // name here. Most hardware sends one key for play and pause
                // together, which is what Space already is.
                //
                // Windows delivers none of them. Measured 2026-08-08 by
                // synthesising the VK_MEDIA_* keys and tracing this handler:
                // the events arrive, four for four, with a keyval of
                // 0xffffff - `VoidSymbol`. GDK's Windows backend has no
                // mapping from Windows' media keys to the XF86Audio keysyms,
                // so there is nothing to match on and no way to match it from
                // here. Matched by name anyway for the platforms whose keysyms
                // are real, and worth knowing before anyone tries to debug the
                // Windows half of it.
                gdk::Key::space if playing => {
                    app.toggle_pause();
                    app.wake_controls();
                    glib::Propagation::Stop
                }
                gdk::Key::AudioPlay | gdk::Key::AudioPause if playing => {
                    app.handle_media(crate::media_keys::Command::PlayPause);
                    glib::Propagation::Stop
                }
                // Deliberately what Escape does rather than what the stop
                // button does, which under a launcher closes the application
                // instead of returning to the menu. Two meanings for "stop"
                // are enough without a third.
                gdk::Key::AudioStop if playing => {
                    app.handle_media(crate::media_keys::Command::Stop);
                    glib::Propagation::Stop
                }
                // The skip keys move by the same ten seconds the arrows and
                // the control bar's own buttons do, through the same path.
                //
                // Not "next track", which is what they mean on a music player:
                // there is no playlist here to step through, and a key marked
                // with a bar and a triangle is exactly the shape of the two
                // buttons sitting either side of pause on the control bar.
                // Rewind and fast-forward, which some keyboards have instead,
                // land on the same thing.
                gdk::Key::AudioNext
                | gdk::Key::AudioForward
                | gdk::Key::AudioPrev
                | gdk::Key::AudioRewind
                    if playing =>
                {
                    app.handle_media(
                        if matches!(key, gdk::Key::AudioNext | gdk::Key::AudioForward) {
                            crate::media_keys::Command::Next
                        } else {
                            crate::media_keys::Command::Previous
                        },
                    );
                    glib::Propagation::Stop
                }
                // Only during playback: elsewhere the arrows belong to the
                // menus, where left and right mean nothing.
                gdk::Key::Left if playing => {
                    app.controls_left_right(-1);
                    glib::Propagation::Stop
                }
                gdk::Key::Right if playing => {
                    app.controls_left_right(1);
                    glib::Propagation::Stop
                }
                // In the menus they belong to a slider if one is selected,
                // and to nothing otherwise.
                gdk::Key::Left if app.settings_slider(-1) => glib::Propagation::Stop,
                gdk::Key::Right if app.settings_slider(1) => glib::Propagation::Stop,
                // And nothing at all otherwise, anywhere on that screen.
                //
                // Left unhandled the key falls through to GTK's own
                // directional search, which finds whichever pane is to the
                // side and moves the focus into it - stepping between the two
                // by a route that Enter and Escape were meant to replace. A
                // row with no bar on it has nothing for these keys to do.
                gdk::Key::Left | gdk::Key::Right if app.on_settings() => glib::Propagation::Stop,
                // Always goes back one level, so it never quits by surprise
                // from somewhere the user was only browsing.
                // Only while the button row is held: elsewhere in playback
                // there is nothing highlighted to press, and Enter should not
                // quietly become a second play/pause.
                gdk::Key::Up if playing => {
                    app.enter_controls();
                    glib::Propagation::Stop
                }
                gdk::Key::Down if playing => {
                    app.leave_controls();
                    glib::Propagation::Stop
                }
                // Straight out, whatever the strip happens to be doing. A
                // keyboard already has Down for putting the strip away, so
                // spending Escape on it as well made leaving a film two
                // presses when it reads as one.
                //
                // With one exception, added because the menus made it one: an
                // open chooser is closed first, exactly as the gamepad's B
                // does. A list of soundtracks laid over the film is something
                // you are inside of, and Escape is the key for getting out of
                // what you are inside of - leaving the film outright from
                // there is a much bigger step than the press suggests.
                //
                // Only for an open chooser. With nothing open, Escape still
                // goes straight out rather than putting the strip away, which
                // is what Down is for on a keyboard.
                gdk::Key::Escape => {
                    app.close_chooser_or_go_back();
                    glib::Propagation::Stop
                }
                // Ours rather than GTK's, which cannot see the lists at all.
                // Shift+Tab arrives as ISO_Left_Tab on X11 and Wayland both,
                // so the modifier is not enough to tell them apart.
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                    let backwards = key == gdk::Key::ISO_Left_Tab
                        || state.contains(gdk::ModifierType::SHIFT_MASK);
                    if app.move_focus_stop(if backwards { -1 } else { 1 }) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                // Between the two panes of the browser, and along a slider
                // where the selected row carries one. Same order the gamepad
                // uses, so the two cannot disagree.
                gdk::Key::Left | gdk::Key::Right if !playing => {
                    let delta = if key == gdk::Key::Left { -1 } else { 1 };
                    if app.settings_slider(delta) || app.move_between_lists(delta) {
                        glib::Propagation::Stop
                    } else {
                        glib::Propagation::Proceed
                    }
                }
                // Rows cannot take focus, so GTK no longer activates one for
                // us: pressing a row is now this. A button keeps its own
                // behaviour, and a text field consumes the key before this
                // sees it.
                gdk::Key::Return | gdk::Key::KP_Enter => {
                    app.activate_focused();
                    glib::Propagation::Stop
                }
                // Available on every screen, not just during playback: on a
                // television the menus want the whole display too.
                gdk::Key::Page_Up => {
                    app.move_selection(-PAGE_ROWS);
                    glib::Propagation::Stop
                }
                gdk::Key::Page_Down => {
                    app.move_selection(PAGE_ROWS);
                    glib::Propagation::Stop
                }
                // Home and End only ever reach this on the About page.
                //
                // GtkListBox binds them itself and lands exactly where a
                // jump-to-first and jump-to-last should, so every screen with
                // rows was already served and nothing here is needed for one.
                // Measured 2026-08-08 rather than assumed: with a trace at the
                // top of this handler, pressing either key on a list printed
                // nothing at all, because the list consumes it in the focus
                // chain long before a bubble-phase controller on the window
                // sees it. The About page is the one screen with nothing to
                // select, and its text scrolls once the interface is scaled up.
                //
                // Left unclaimed during playback: seeking to the start is
                // plausible, seeking to the end is not, and the pair is worth
                // less there than the decisions it would need.
                gdk::Key::Home | gdk::Key::End
                    if !playing && app.scroll_about_edge(key == gdk::Key::End) =>
                {
                    glib::Propagation::Stop
                }
                // F11 alongside F: it is what a browser, a file manager and
                // every other video player use, and costs one name.
                gdk::Key::f | gdk::Key::F | gdk::Key::F11 => {
                    app.toggle_fullscreen();
                    glib::Propagation::Stop
                }
                // Only during playback: there is nothing to turn off from a
                // menu, and the choosers want the letter for type-ahead.
                gdk::Key::c | gdk::Key::C if playing => {
                    app.toggle_subtitles();
                    glib::Propagation::Stop
                }
                // The same silence the volume button is held for, without
                // having to reach the button first.
                gdk::Key::m | gdk::Key::M if playing => {
                    app.toggle_mute();
                    glib::Propagation::Stop
                }
                gdk::Key::t | gdk::Key::T if playing => {
                    app.toggle_time_readout();
                    glib::Propagation::Stop
                }
                // Steps each output through the file's audio tracks while it
                // plays. A shortcut ahead of the real thing, which is a chooser
                // per output on the control strip.
                gdk::Key::a | gdk::Key::A if playing => {
                    app.cycle_audio("primary");
                    glib::Propagation::Stop
                }
                gdk::Key::s | gdk::Key::S if playing => {
                    app.cycle_audio("secondary");
                    glib::Propagation::Stop
                }
                // The shortcut GTK's own file chooser and every web browser use
                // to reach an address bar, worth having from the menu which is
                // otherwise two steps away from the panel.
                //
                // Not from inside a modal, which already is one of the two
                // ways of choosing a video, and never when something else
                // chose the video: the menu's row for it is disabled then, and
                // a shortcut past that would let a keypress replace what a
                // launcher is waiting on.
                gdk::Key::l | gdk::Key::L
                    if state.intersects(primary)
                        && !app.external
                        && matches!(*app.screen.borrow(), Screen::Menu | Screen::VideoSource) =>
                {
                    app.show_paste_uri();
                    glib::Propagation::Stop
                }
                // The shortcut for copying, which GTK would otherwise only
                // deliver to whichever widget has focus - and the text on the
                // About page deliberately never takes it.
                //
                // Matched here rather than bound as an accelerator for exactly
                // that reason in reverse: an accelerator would claim the key
                // ahead of the focused widget, so a text field would lose its
                // own copy. `copy_selection` saying no is what hands the key
                // back.
                gdk::Key::c | gdk::Key::C if state.intersects(primary) && app.copy_selection() => {
                    glib::Propagation::Stop
                }
                // The other half of the pair, and the shortcut every desktop
                // application uses for opening a file.
                gdk::Key::o | gdk::Key::O
                    if state.intersects(primary)
                        && !app.external
                        && matches!(*app.screen.borrow(), Screen::Menu | Screen::VideoSource) =>
                {
                    app.browse_for_file();
                    glib::Propagation::Stop
                }
                // Last, so it can't shadow the keys above: anything else
                // during playback summons the timeline without claiming the
                // key.
                _ if playing => {
                    app.wake_controls();
                    glib::Propagation::Proceed
                }
                _ => glib::Propagation::Proceed,
            }
        });
        {
            let app = self.clone();
            controller.connect_key_released(move |_, key, _, _| match key {
                gdk::Key::Left | gdk::Key::Right => app.end_scrub(),
                _ => {}
            });
        }
        // Dropping a file on the window loads it, from any screen including
        // mid-playback. Quicker than any picker when the file is already in
        // front of you in a file manager.
        //
        // Left out for the same reason the browser is: something else chose
        // the video and is waiting for this playback of it to end.
        if !self.external {
            let app = self.clone();
            let drop = gtk::DropTarget::new(gtk::gio::File::static_type(), gdk::DragAction::COPY);
            drop.connect_drop(move |_, value, _, _| {
                let Ok(file) = value.get::<gtk::gio::File>() else {
                    return false;
                };
                // Only local files have a path; a remote URI has nothing for
                // filesrc to open.
                let Some(path) = file.path() else {
                    return false;
                };
                app.stop_playback();
                let source = Source::File(path);
                match app.set_file(&source) {
                    Ok(()) => app.show_menu(),
                    Err(e) => app.show_source_error(&source, &e, false),
                }
                true
            });
            self.window.add_controller(drop);
        }

        self.window.add_controller(controller);

        // Enter, taken before the focused widget can have it.
        //
        // A transport button is a real button, and GTK activates a focused
        // one on Enter - so the key never reached the handler above at all.
        // Holding it opened and shut the panel on every repeat while nothing
        // here saw a single press. Claimed in the capture phase, which runs
        // from the window down, and only while the strip has something
        // highlighted: everywhere else Enter still belongs to whatever has
        // the focus.
        let capture = gtk::EventControllerKey::new();
        capture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let app = self.clone();
        capture.connect_key_pressed(move |_, key, _, _| {
            if !matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) || !app.strip_takes_enter() {
                return glib::Propagation::Proceed;
            }
            app.press_activate();
            glib::Propagation::Stop
        });
        let app = self.clone();
        capture.connect_key_released(move |_, key, _, _| {
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && app.strip_takes_enter() {
                app.release_activate();
            }
        });
        self.window.add_controller(capture);
    }

    /// Pause or resume, keeping the display-awake hold in step with it.
    /// Everything that pauses goes through here.
    ///
    /// The button on the controls and the gamepad used to call the pipeline
    /// directly, each repeating the two lines below. That was harmless until
    /// there were three of them and one had something extra to do: pausing
    /// from the screen left the system's now-playing widget still showing a
    /// pause button, and pressing it did nothing, while the media key on the
    /// keyboard - which did come through here - worked.
    fn toggle_pause(self: &Rc<Self>) {
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.toggle_pause();
            self.awake.set(playback.is_playing());
        }
        self.publish_now_playing();
    }

    /// Tells the system what is playing, where the system cares.
    ///
    /// Called when the answer changes rather than on the tick: what is
    /// published is a position and a rate, and macOS extrapolates between
    /// them, so a film left playing stays correct without being told again.
    /// The four moments that do change it are playback starting, pausing,
    /// seeking, and stopping.
    ///
    /// Silent everywhere but macOS. See `media_keys::NowPlaying` for why it
    /// is not merely cosmetic there: it is what decides who receives a media
    /// key at all.
    fn publish_now_playing(self: &Rc<Self>) {
        // Queued behind the main loop rather than done here.
        //
        // Telling the system what is playing means writing the poster to disk
        // and several calls into another process, and `begin_playback` reaches
        // this by way of `stop_playback` - so all of that was running in the
        // middle of building the pipeline. On 2026-08-13 that was enough to
        // hang TinePlayer outright: the main thread ended up blocked inside
        // `gst_pad_push_event`, waiting on a lock a streaming thread held,
        // with the delay this introduced landing squarely in the window where
        // that race is possible.
        //
        // Nothing here needs to be immediate. The panel wants to know within a
        // moment, and the main loop is idle a moment later by definition.
        if self.now_playing_queued.replace(true) {
            return;
        }
        let app = self.clone();
        glib::idle_add_local_once(move || {
            app.now_playing_queued.set(false);
            app.send_now_playing();
        });
    }

    /// Gathers what is playing and hands it to the platform.
    fn send_now_playing(&self) {
        // Nothing chosen at all: the panel goes away rather than sitting
        // there empty. An empty one is worse than none, because the name it
        // shows when it has no title is the application's own identifier.
        if self.file.borrow().is_none() {
            crate::media_keys::set_now_playing(None);
            return;
        }
        let seconds = |time: Option<gstreamer::ClockTime>| {
            time.map(|time| time.nseconds() as f64 / 1e9).unwrap_or(0.0)
        };
        // A video chosen but not started is published too, stopped rather than
        // absent, so the panel names what is about to be watched and its play
        // button has something to do. Without it the media page showed a panel
        // with no title at all.
        let playback = self.playback.borrow();
        let (duration_s, elapsed_s, playing) = match playback.as_ref() {
            Some(playback) => (
                seconds(playback.duration()),
                seconds(playback.position()),
                playback.is_playing(),
            ),
            None => (self.details.borrow().duration_s, 0.0, false),
        };
        drop(playback);
        crate::media_keys::set_now_playing(Some(crate::media_keys::NowPlaying {
            // The same title as the titlebar and the media page, from the one
            // chain that resolves it.
            title: self.file_label().unwrap_or_default(),
            duration_s,
            elapsed_s,
            playing,
            // The poster the page found, cloned rather than borrowed: this
            // runs a handful of times per film, and the alternative is a
            // lifetime threaded through a platform boundary for nothing.
            artwork: self.details.borrow().poster.clone(),
        }));
    }

    // --- Jellyfin ------------------------------------------------------

    /// Puts artwork into the page that is already on screen.
    ///
    /// Only the two widgets it belongs in are touched, so focus, the row
    /// somebody is on, and anything they have open all stay exactly as they
    /// were. This is what a picture arriving three seconds after the page did
    /// should cost: a picture appearing, and nothing else moving.
    fn show_late_art(self: &Rc<Self>) {
        if let Some(backdrop) = self.backdrop_widget.borrow().as_ref()
            && let Some(texture) = self.backdrop_art.borrow().clone()
        {
            backdrop.set_texture(Some(texture));
            fade_in(backdrop);
        }

        // The poster is a picture where the placeholder was, so the frame's
        // child is replaced rather than a texture set: with no artwork the
        // frame holds a mark rather than an empty picture.
        let (Some(frame), Some(texture)) = (
            self.poster_frame.borrow().clone(),
            self.poster_art.borrow().clone(),
        ) else {
            return;
        };
        while let Some(child) = frame.first_child() {
            frame.remove(&child);
        }
        let picture = crate::artwork::Artwork::poster();
        picture.set_texture(Some(texture));
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        fade_in(&picture);
        frame.append(&picture);
    }

    /// Fills the page in from the library, for a video that came from one.
    ///
    /// Only the fields Jellyfin actually answered: an empty overview or a
    /// missing year leaves whatever the container had, on the grounds that
    /// something is better than nothing and the library is not always fuller
    /// than the file. The title is not among them - that already comes through
    /// `launcher_title` at the head of the same chain everything else uses.
    fn overlay_jellyfin_details(self: &Rc<Self>) {
        let Some(item) = self.jellyfin_item.borrow().clone() else {
            return;
        };
        {
            let mut details = self.details.borrow_mut();
            if !item.plot.is_empty() {
                details.plot = item.plot.clone();
            }
            if item.year.is_some() {
                details.year = item.year;
            }
            if !item.certificate.is_empty() {
                details.certificate = item.certificate.clone();
            }
            if item.rating.is_some() {
                details.rating = item.rating;
            }
            if !item.genres.is_empty() {
                details.genres = item.genres.clone();
            }
            if item.episode.is_some() {
                details.episode = item.episode;
            }
            if !item.aired.is_empty() {
                details.aired = item.aired.clone();
            }
            // The stream measures itself, so a runtime is only worth taking
            // where the container could not say.
            if details.duration_s <= 0.0
                && let Some(runtime) = item.runtime_ns
            {
                details.duration_s = runtime as f64 / 1e9;
            }
        }
        self.load_jellyfin_art(&item);
    }

    /// Fetches the poster and backdrop, and redraws when they land.
    ///
    /// Separately from the details, and after the page is already up, because
    /// these are the slow part - a backdrop is a picture from across the
    /// house. The page is perfectly good without them until they arrive, which
    /// is the same bargain artwork beside a file already makes.
    fn load_jellyfin_art(self: &Rc<Self>, item: &crate::jellyfin::Item) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        if item.poster_tag.is_none() && item.backdrop_tag.is_none() {
            return;
        }

        let id = item.id.clone();
        let poster_tag = item.poster_tag.clone();
        let backdrop_tag = item.backdrop_tag.clone();
        // The film these belong to, so a viewer who casts one and immediately
        // casts another does not get the first one's backdrop.
        let generation = self.art_generation.get();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // Asked for at the size they are drawn rather than whole: a
            // library backdrop can be several megabytes untouched.
            let poster = poster_tag
                .and_then(|tag| client.image(&id, "Primary", &tag, 600).ok())
                .map(crate::metadata::Art::Embedded);
            let backdrop = backdrop_tag
                .and_then(|tag| client.image(&id, "Backdrop/0", &tag, 1920).ok())
                .map(crate::metadata::Art::Embedded);
            let _ = sender.send((poster, backdrop));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let (poster, backdrop) = match receiver.try_recv() {
                Ok(pair) => pair,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Another video was opened while these were coming down.
            if app.art_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            {
                let mut details = app.details.borrow_mut();
                if poster.is_some() {
                    details.poster = poster;
                }
                if backdrop.is_some() {
                    details.backdrop = backdrop;
                }
            }
            app.start_art_load();
            glib::ControlFlow::Break
        });
    }

    /// Reaches the paired server, if there is one, and stays reachable.
    ///
    /// Everything here is allowed to fail quietly. A server that is off, a
    /// network that is out, a pairing that was revoked - none of them are
    /// reasons for a video player to complain on startup, and all of them are
    /// answered the same way: no cast target until it comes back.
    fn start_jellyfin(self: &Rc<Self>) {
        let Some(pairing) = crate::jellyfin::load() else {
            return;
        };
        // What the settings pane reads. Set here as well as when that pane is
        // built, so the rows are right the first time it is opened.
        *self.jellyfin_pairing.borrow_mut() = Some(pairing.clone());
        let Some(client) = crate::jellyfin::Client::new(&pairing) else {
            // Paired with a server but signed out of it, which is where a 401
            // leaves things. The settings screen offers a new code.
            return;
        };

        // Off the main thread: this talks to a server that may be asleep, and
        // the interface has a menu to draw.
        //
        // **Its answer is acted on, not merely printed.** This is the first
        // call made with a stored token, so it is the first thing to know that
        // a pairing has been revoked - and until 2026-08-15 it logged
        // "Jellyfin no longer accepts this connection" and carried on, leaving
        // the settings screen claiming to be connected to a server that had
        // deleted this device. Reported by Scott, who had done exactly that.
        let announcing = client.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(announcing.announce());
        });
        {
            let app = self.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                match receiver.try_recv() {
                    Ok(Ok(())) => {}
                    // The pairing is gone. Everything else about this server is
                    // now wrong, including the socket that is being opened
                    // below, which signing out puts down.
                    Ok(Err(crate::jellyfin::Error::Unauthorized)) => app.jellyfin_signed_out(),
                    // A server that is off or asleep, which is ordinary and
                    // not a reason to throw the pairing away.
                    Ok(Err(e)) => eprintln!("Jellyfin would not take our capabilities: {e}"),
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
                glib::ControlFlow::Break
            });
        }
        *self.jellyfin.borrow_mut() = Some(client);

        let app = self.clone();
        let session = crate::jellyfin::connect(&pairing, move |command| {
            app.handle_jellyfin(command);
        });
        *self.jellyfin_session.borrow_mut() = session;
    }

    /// What a phone asked for.
    ///
    /// Playstate commands are the ones TinePlayer already has actions for, so
    /// they go straight to the same places the remote and the media keys use -
    /// there is no second way to pause.
    fn handle_jellyfin(self: &Rc<Self>, command: crate::jellyfin::Command) {
        use crate::jellyfin::Command;
        match command {
            Command::Play {
                item_id,
                position_ns,
            } => self.play_jellyfin(&item_id, position_ns),
            Command::Pause => {
                if self.is_playing() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::Unpause => {
                if self.playback.borrow().is_some() && !self.is_playing() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::PlayPause => {
                if self.playback.borrow().is_some() {
                    self.toggle_pause();
                    self.wake_controls();
                }
            }
            Command::Stop => {
                if self.playback.borrow().is_some() {
                    self.go_back();
                }
            }
            Command::Seek(position_ns) => {
                if let Some(playback) = self.playback.borrow().as_ref() {
                    playback.aim_at(gstreamer::ClockTime::from_nseconds(position_ns));
                    playback.commit_seek();
                }
                self.publish_now_playing();
            }
            // Everything below drives the controls rather than the pipeline,
            // which is what keeps one answer to each of these questions: the
            // remote moves the same master and picks from the same lists the
            // person in the room does, and the strip is woken so what it did is
            // visible rather than mysterious.
            Command::SetVolume(level) => {
                if let Some(controls) = self.controls.borrow().clone() {
                    controls.master_to(level);
                }
                self.wake_controls();
            }
            Command::Mute | Command::Unmute | Command::ToggleMute => {
                if let Some(controls) = self.controls.borrow().clone() {
                    match command {
                        Command::Mute => controls.set_hushed(true),
                        Command::Unmute => controls.set_hushed(false),
                        _ => controls.toggle_hush(),
                    }
                }
                self.wake_controls();
            }
            Command::SetAudioStream(index) => {
                if let Some(row) = self.library_audio_row(index) {
                    self.choose_audio(Role::Primary.key(), row);
                    self.wake_controls();
                }
            }
            Command::SetSubtitleStream(index) => {
                if let Some(row) = self.library_subtitle_row(index) {
                    self.choose_subtitle(row);
                    self.wake_controls();
                }
            }
            // The pairing was revoked while we held it. Everything about this
            // server is now wrong, so it is put down rather than retried.
            Command::SignedOut => self.jellyfin_signed_out(),
        }
    }

    /// What a controller should show: the master, the blanket silence, and what
    /// the first output and the subtitles are playing, in Jellyfin's numbering.
    ///
    /// Worked out here rather than remembered as it changes, because every
    /// answer already lives somewhere - and a second copy kept in step by hand
    /// is how a remote comes to show something the player is not doing.
    fn reported_sound(&self) -> crate::jellyfin::Sound {
        use crate::subtitles::SubtitleChoice;
        let item = self.jellyfin_item.borrow();
        let streams = item.as_ref().map(|item| &item.streams);

        let audio = match (
            streams,
            self.playback
                .borrow()
                .as_ref()
                .and_then(|playback| playback.playing_on(Role::Primary.key())),
        ) {
            (Some(streams), Some(crate::pipeline::Playing::Track(position))) => {
                streams.audio_index(position)
            }
            // A separate audio file, which the library has no number for.
            _ => None,
        };

        let subtitle = match self.subtitle.borrow().as_ref() {
            // Off is an answer, and the one a controller most needs told: it is
            // what its selector falls back to showing when it is told nothing.
            None => Some(-1),
            // A file on the server, which already carries Jellyfin's own number.
            Some(SubtitleChoice::Library(index)) => Some(*index as i32),
            Some(SubtitleChoice::Embedded(position)) => streams
                .and_then(|streams| streams.subtitle_index(*position))
                .map(|index| index as i32),
            // A file on this machine, which a cast video does not have.
            Some(_) => None,
        };

        crate::jellyfin::Sound {
            level: self.config.borrow().master_volume(),
            muted: self.hushed.get(),
            audio,
            subtitle,
        }
    }

    /// Which row of the first output's soundtrack list one of Jellyfin's stream
    /// numbers is.
    ///
    /// That list is the film's own tracks in order and nothing else - the first
    /// output has no "None" row - so a position among the embedded tracks is
    /// the row. `None` for a stream that is external or is not audio, which is
    /// a remote asking for something this list cannot offer rather than an
    /// error worth reporting.
    fn library_audio_row(&self, index: u32) -> Option<usize> {
        let item = self.jellyfin_item.borrow();
        let position = item.as_ref()?.streams.audio_position(index)?;
        Some(position as usize)
    }

    /// The same for the subtitle chooser, whose first row is Off and whose rest
    /// follow `subtitle_options` in order.
    ///
    /// Matched against the options themselves rather than counted, because that
    /// list holds two kinds of thing at once: streams inside the container,
    /// which Jellyfin numbers among everything else, and files beside it on the
    /// server, which carry Jellyfin's own number already. Counting would put
    /// one kind out of step with the other.
    fn library_subtitle_row(&self, index: Option<u32>) -> Option<usize> {
        use crate::subtitles::Subtitle;
        // Off is a row like any other, and the one a remote can always reach.
        let Some(index) = index else { return Some(0) };
        let embedded = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .and_then(|item| item.streams.subtitle_position(index));
        let options = self.subtitle_options.borrow();
        let at = options.iter().position(|option| match option {
            Subtitle::Library { index: at, .. } => *at == index,
            Subtitle::Embedded { index: at, .. } => Some(*at) == embedded,
            _ => false,
        })?;
        Some(at + 1)
    }

    fn is_playing(&self) -> bool {
        self.playback
            .borrow()
            .as_ref()
            .is_some_and(|playback| playback.is_playing())
    }

    /// Resolves what was cast and opens it.
    ///
    /// The command carries an item id and nothing else - no address and,
    /// usually, no position - so the item is asked about before anything can
    /// be played. That happens on a worker thread, because it is a request to
    /// a server that may be across a house.
    fn play_jellyfin(self: &Rc<Self>, item_id: &str, position_ns: Option<u64>) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        let id = item_id.to_string();

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(client.item(&id));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            match result {
                Ok(mut item) => {
                    // A controller saying "play from here" outranks the
                    // library's own idea of where this viewer stopped.
                    if let Some(position) = position_ns {
                        item.resume_ns = Some(position).filter(|position| *position > 0);
                    }
                    app.open_jellyfin(item);
                }
                Err(crate::jellyfin::Error::Unauthorized) => app.jellyfin_signed_out(),
                Err(e) => eprintln!("Jellyfin would not describe that video: {e}"),
            }
            glib::ControlFlow::Break
        });
    }

    /// Takes a resolved item and plays it.
    fn open_jellyfin(self: &Rc<Self>, item: crate::jellyfin::Item) {
        let Some(client) = self.jellyfin.borrow().clone() else {
            return;
        };
        let source = Source::parse(&client.stream_url(&item));
        // Set before the source is opened, because everything that reads a
        // title or a resume position during opening looks here for it. Kodi's
        // is cleared for the same reason: two launchers claiming the same
        // video would be one of them wrong.
        *self.kodi_item.borrow_mut() = None;

        // What the tracks are, from the library rather than by reading the
        // file. The server has already analysed it, and asking again over HTTP
        // is redundant work that sometimes cannot finish: a QuickTime file
        // with its index at the end has to be read from the front to be
        // probed, which for a four-gigabyte film is minutes. Playback itself
        // is unaffected - it seeks straight to the index - so the probe was
        // the only thing that could not cope.
        //
        // Verified on 2026-08-14 that the library's stream order matches the
        // probe's exactly, which is what makes this safe: tracks are chosen by
        // position, and a different order would silently play the wrong one.
        let media = item.streams.as_media(item.runtime_ns.unwrap_or_default());
        *self.jellyfin_item.borrow_mut() = Some(item);

        // Straight in, with no spinner: there is nothing to wait for now that
        // the tracks are already known.
        match self.apply_media(&source, media) {
            Ok(()) => self.show_menu(),
            Err(e) => {
                eprintln!("Couldn't open {}: {e}", source.uri());
                self.show_source_error(&source, &e, false);
            }
        }
    }

    /// Puts down a pairing the server no longer honours.
    ///
    /// The token goes and the device identity stays, so connecting again
    /// replaces the existing device rather than leaving a trail of them. Said
    /// out loud, because a cast target that has quietly stopped being one is
    /// the failure nobody can diagnose from the sofa.
    fn jellyfin_signed_out(self: &Rc<Self>) {
        // Both halves of the connection find this out for themselves - the
        // capabilities call by its 401, the socket by its 403 - and either
        // alone has to be enough, since a server may refuse one and not the
        // other. So the second one to arrive says nothing and writes nothing
        // rather than repeating the message and the file write.
        if self.jellyfin.borrow().is_none() && self.jellyfin_session.borrow().is_none() {
            return;
        }
        *self.jellyfin.borrow_mut() = None;
        *self.jellyfin_session.borrow_mut() = None;
        if let Some(mut pairing) = crate::jellyfin::load() {
            pairing.sign_out();
            if let Err(e) = crate::jellyfin::save(&pairing) {
                eprintln!("Couldn't forget the Jellyfin token: {e}");
            }
            *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        }
        eprintln!("Jellyfin no longer accepts this connection. Connect to it again to cast.");
        // Redrawn only where it is being looked at. A pairing can be revoked
        // at any moment, and rebuilding a screen under somebody who is part
        // way through choosing a soundtrack would be a worse interruption than
        // the one being reported.
        if self.showing_jellyfin_pane() {
            self.show_settings();
        }
        // And the page shown when nothing is loaded, which offers a Connect
        // button only while there is nothing to disconnect from - so a token
        // revoked between that page being drawn and the server saying so left
        // it with no way to connect until something else redrew it. Safe to
        // rebuild only here: with no video there is nothing in hand to
        // interrupt, which is not true of the media page.
        if *self.screen.borrow() == Screen::Menu && self.file.borrow().is_none() {
            self.show_menu();
        }
    }

    /// Tells Jellyfin where playback has reached.
    ///
    /// Only for a video that came from there: a film opened from disk is
    /// nothing to do with the library, and reporting it would put a position
    /// against an item nobody watched.
    fn report_to_jellyfin(&self, moment: JellyfinMoment) {
        let (Some(client), Some(id)) = (
            self.jellyfin.borrow().clone(),
            self.jellyfin_item
                .borrow()
                .as_ref()
                .map(|item| item.id.clone()),
        ) else {
            return;
        };
        let position = self
            .playback
            .borrow()
            .as_ref()
            .and_then(|playback| playback.position())
            .map(|position| position.nseconds())
            .unwrap_or(0);
        let paused = !self.is_playing();
        let sound = self.reported_sound();

        // A new name for the viewing when it starts, and the same one after.
        if moment == JellyfinMoment::Started {
            *self.jellyfin_play_session.borrow_mut() = crate::jellyfin::Client::new_play_session();
        }
        let play_session = self.jellyfin_play_session.borrow().clone();
        if play_session.is_empty() {
            // Nothing was ever started, so there is no viewing to report on.
            return;
        }

        // On a thread, because the server may be slow and this happens while a
        // film is playing. Nothing waits on the answer.
        std::thread::spawn(move || {
            let result = match moment {
                JellyfinMoment::Started => client.started(&id, &play_session, position, sound),
                JellyfinMoment::Progress => {
                    client.progress(&id, &play_session, position, paused, sound)
                }
                JellyfinMoment::Stopped => client.stopped(&id, &play_session, position),
            };
            if let Err(e) = result {
                eprintln!("Jellyfin would not take the position: {e}");
            }
        });
    }

    /// What a media key means, wherever the platform reported it from: a
    /// keysym on Linux, a `WM_APPCOMMAND` on Windows. Says whether it was
    /// used, which Windows needs in order to decide whether to pass the key
    /// on to whatever else would have played.
    ///
    /// With a video chosen but not started, play begins it: the media page is
    /// where somebody arrives before pressing anything, and a play key that
    /// does nothing there reads as a broken key rather than as a deliberate
    /// silence. Everything else needs a film already running, and says so by
    /// declining the key so it can go to whatever else would have played.
    fn handle_media(self: &Rc<Self>, command: crate::media_keys::Command) -> bool {
        use crate::media_keys::Command;

        // Read and released before anything below can borrow it again.
        let playing = self
            .playback
            .borrow()
            .as_ref()
            .map(|playback| playback.is_playing());

        let Some(is_playing) = playing else {
            // Nothing is loaded to start, or there is nowhere to play it.
            let ready = self.file.borrow().is_some() && self.config.borrow().primary_sink.is_some();
            return match command {
                Command::Play | Command::PlayPause if ready => {
                    self.start_playback(false);
                    true
                }
                _ => false,
            };
        };

        let flip = || {
            self.toggle_pause();
            self.wake_controls();
        };

        match command {
            Command::PlayPause => flip(),
            // A keyboard with separate play and pause keys means them
            // literally, so neither flips what it asked for. Both are claimed
            // even when there is nothing to do: what was asked for is already
            // true, which is not the same as the key going unused.
            Command::Play if !is_playing => flip(),
            Command::Pause if is_playing => flip(),
            Command::Play | Command::Pause => {}
            Command::Stop => self.go_back(),
            Command::Next | Command::Previous => {
                self.scrub(if command == Command::Next {
                    crate::player::STEP_SECONDS
                } else {
                    -crate::player::STEP_SECONDS
                });
                self.end_scrub();
                self.wake_controls();
            }
        }
        true
    }

    /// One level up: out of playback, out of a chooser, or out of the
    /// application. Shared by Escape and the gamepad's back button so the two
    /// can never disagree about what "back" means.
    fn go_back(self: &Rc<Self>) {
        // Copied out first: the handlers below take the same cell mutably,
        // and holding the read borrow across them panics.
        let screen = *self.screen.borrow();
        match screen {
            Screen::Playing => self.leave_playback(),
            Screen::Confirm | Screen::Notices => self.show_settings(),
            // Everything Kodi opens is opened from the Kodi pane and
            // returns straight to it. Each is one panel over that pane rather
            // than a step in a sequence, so there is no part-answered state to
            // step back into: on a confirmation this is the same as pressing
            // Cancel, which is what Escape should mean on a panel whose other
            // button says Cancel.
            Screen::KodiConfirm
            | Screen::KodiFolder
            | Screen::KodiPermission
            | Screen::KodiError => self.return_to_kodi_settings(),
            // The same, for the pane beside it. Backing out of a waiting code
            // abandons the pairing rather than pausing it: the polling stops
            // because this screen is no longer showing, and the code the
            // server issued is left to expire on its own.
            Screen::JellyfinConnect | Screen::JellyfinPanel => self.leave_jellyfin_connect(),
            // Nothing to go back to when the video we were started for could
            // not be opened.
            Screen::Error if self.error_is_fatal.get() => self.window.close(),
            Screen::Opening => self.show_paste_uri(),
            // Leaving the middle step abandons the measurement rather than
            // stepping back into the track list: the thread cannot be stopped,
            // but its answer is dropped, and nothing has been written.
            Screen::PasteUri
            | Screen::Browser
            | Screen::AlignChoose
            | Screen::AlignProgress
            | Screen::AlignResult => self.return_to_origin(),
            // Out of the settings and back to the categories, and only then
            // out of the screen. Two steps because it is entered in two.
            Screen::Settings if self.in_settings_pane.get() => self.hold_settings_categories(),
            Screen::VideoSource | Screen::Settings | Screen::Error | Screen::ConfirmQuit => {
                self.show_menu()
            }
            Screen::Menu => self.show_confirm_quit(),
        }
    }

    /// Refreshes the controls readout ten times a second.
    ///
    /// Fast enough that the playhead slides rather than stepping: at twice a
    /// second the jumps were plainly visible against a timeline the width of
    /// the screen. It costs two pipeline queries a tick, which is nothing
    /// next to decoding video.
    fn start_tick(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        // Ticks since Kodi was last told where playback had reached. Counted
        // here rather than given a timer of its own so that it stops when
        // playback does, without anything extra to tear down.
        let mut since_report = 0u32;
        // What was last published to the system as the running time. Playback
        // can begin before GStreamer has worked the duration out, and a
        // now-playing entry claiming a film is zero seconds long stays wrong
        // until something else happens to republish it. Kept beside
        // `since_report` as closure state for the same reason: it belongs to
        // this timer and stops when it does.
        let mut published_duration = 0f64;
        let source = glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // Cloned out before touching the controls, which can rebuild the
            // screen if playback has ended underneath us.
            let playback = app.playback.borrow().clone();
            let controls = app.controls.borrow().clone();
            match (playback, controls) {
                (Some(playback), Some(controls)) => {
                    controls.update(&playback);

                    // Only when it changes, which is normally once a film.
                    // The position is deliberately not pushed on the tick:
                    // the system extrapolates it from the rate, so it stays
                    // right on its own between the moments that do change it.
                    let duration = playback
                        .duration()
                        .map(|time| time.nseconds() as f64 / 1e9)
                        .unwrap_or(0.0);
                    if duration != published_duration {
                        published_duration = duration;
                        app.publish_now_playing();
                    }

                    since_report += 1;
                    // Every 30 seconds, so that a player killed outright still
                    // leaves Kodi's library close to where you actually got to.
                    if since_report >= 300 {
                        since_report = 0;
                        playback.report_to_kodi();
                    }

                    // Jellyfin more often, because a phone watching this
                    // session shows the position as it moves rather than only
                    // after the fact. Ten seconds is what its own clients
                    // send, and it is one small request.
                    let told = app.jellyfin_reported.get() + 1;
                    app.jellyfin_reported.set(told);
                    if told >= 100 {
                        app.jellyfin_reported.set(0);
                        app.report_to_jellyfin(JellyfinMoment::Progress);
                    }
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        *self.tick.borrow_mut() = Some(source);
    }

    /// Shows where playback has reached, and nothing else. What a seek wants:
    /// the buttons appearing over the picture on every skip is more than was
    /// asked for.
    fn peek_controls(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            controls.update(&playback);
            controls.peek();
        }
    }

    /// Brings the controls up on any input during playback, so the timeline
    /// is there whenever someone reaches for a control.
    fn wake_controls(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            controls.update(&playback);
            controls.flash(!playback.is_playing());
        }
    }

    /// Up: reveal the strip and take hold of it, then climb from the buttons
    /// to the timeline.
    ///
    /// The first press lands on the buttons rather than the timeline, because
    /// the buttons are what cannot be reached any other way - left and right
    /// already seek without any of this.
    fn enter_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::None => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::Timeline),
            Row::Timeline => {}
            // The menu opens upward out of its button, so up climbs its rows
            // and stops at the top of them.
            Row::Volume => controls.move_output(-1),
            // A chooser opens the same way, so up climbs it likewise.
            Row::Audio => controls.move_audio(-1),
            Row::Subtitles => controls.move_subtitle(-1),
        }
    }

    /// Escape: shuts an open chooser, or leaves whatever is on screen.
    ///
    /// Back to the buttons rather than off the strip entirely, so the icon the
    /// chooser came out of is highlighted and can be seen to have changed -
    /// the same landing choosing a row gives, and the same the gamepad's B
    /// gives.
    fn close_chooser_or_go_back(self: &Rc<Self>) {
        use crate::controls::Row;
        // Cloned out rather than acted on through the borrow, the way every
        // other caller here does it: `set_row` runs the strip's own handlers,
        // and holding the cell open across them is how a re-entrant borrow
        // panic gets written.
        let controls = self.controls.borrow().clone();
        match controls {
            Some(controls)
                if matches!(controls.row(), Row::Volume | Row::Audio | Row::Subtitles) =>
            {
                controls.set_row(Row::Buttons);
            }
            _ => self.go_back(),
        }
    }

    /// Down: back to the buttons from the timeline, then let the strip go.
    fn leave_controls(self: &Rc<Self>) {
        use crate::controls::Row;
        let Some(controls) = self.controls.borrow().clone() else {
            return;
        };
        match controls.row() {
            Row::Timeline => controls.set_row(Row::Buttons),
            Row::Buttons => controls.set_row(Row::None),
            // Down the rows of the menu, and off the bottom of it back to the
            // speaker the menu came out of.
            Row::Volume => {
                if controls.at_last_output() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_output(1);
                }
            }
            // Down the soundtracks, and off the bottom back to the icon the
            // chooser came out of.
            Row::Audio => {
                if controls.at_last_audio() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_audio(1);
                }
            }
            // Down the list of subtitles, and off the bottom of it back to
            // the icon the chooser came out of.
            Row::Subtitles => {
                if controls.at_last_subtitle() {
                    controls.set_row(Row::Buttons);
                } else {
                    controls.move_subtitle(1);
                }
            }
            // Nothing is held, so there is nothing to put down - but the strip
            // may still be on screen from a seek or a moved mouse, and down
            // should be rid of that too.
            Row::None => {
                if controls.is_showing() {
                    controls.hide();
                }
            }
        }
    }

    /// A press on whatever the strip has highlighted. The volume button is
    /// held rather than pressed, so it waits for the release; everything else
    /// acts at once, as it always has.
    ///
    /// Cloned out of the cell before anything is pressed: stop and settings
    /// both tear playback down, which takes this same cell mutably, and doing
    /// that while a read borrow is alive panics.
    /// Whether Enter belongs to the control strip rather than to whatever
    /// happens to have the focus.
    fn strip_takes_enter(&self) -> bool {
        self.playback.borrow().is_some()
            && self
                .controls
                .borrow()
                .as_ref()
                .is_some_and(|controls| controls.takes_activation())
    }

    fn press_activate(self: &Rc<Self>) {
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        // Any release waiting to be believed is not one: the key is still
        // down. See `release_activate`.
        self.releases.set(self.releases.get() + 1);
        // Once per press, however long it is held. A key down sends presses
        // over and over, and acting on each one turned holding Enter into a
        // control worked dozens of times a second - a delay running away, or
        // an output muted and unmuted until the key came up.
        if self.key_held.replace(true) {
            return;
        }
        // Decided here rather than again on the way up: acting on a press can
        // move the strip somewhere else, and a release that asks a second
        // time gets an answer about wherever it has just moved to. Closing
        // the panel this way put the highlight back on the button, so the
        // release read as a fresh press on it and opened the panel again.
        let holds = controls.holds_press();
        self.hold_started.set(holds.is_some());
        match holds {
            Some(hold) => controls.press_hold(hold),
            None => controls.activate_focused(),
        }
    }

    /// Letting go of a held button. Does the ordinary thing unless the hold
    /// already did something else.
    fn release_activate(self: &Rc<Self>) {
        // Held back rather than acted on, because a key held down does not
        // simply repeat: it sends a release before each repeat, and taking
        // those at face value ended the hold before it could ever reach its
        // six hundred milliseconds - so holding Enter on the volume button
        // opened and shut the panel over and over instead of silencing
        // everything. A release followed closely by a press was never one.
        let mark = self.releases.get() + 1;
        self.releases.set(mark);
        let app = self.clone();
        glib::timeout_add_local_once(REPEAT_GAP, move || {
            if app.releases.get() != mark {
                return;
            }
            app.finish_release();
        });
    }

    /// A release that outlived the gap between repeats, and so is real.
    fn finish_release(self: &Rc<Self>) {
        self.key_held.set(false);
        let controls = self.controls.borrow().clone();
        let Some(controls) = controls else { return };
        // Only a press that started a hold has anything left to do here.
        // Everything else acted on the way down.
        if self.hold_started.replace(false) && controls.release_hold() {
            controls.activate_focused();
        }
    }

    /// Writes the configuration out a second after the last volume change,
    /// rather than on each one. The level itself takes effect immediately;
    /// this is only about remembering it.
    /// Tells a controller what the sound is doing now, rather than leaving it
    /// to the next scheduled report.
    ///
    /// Those go every ten seconds, which is right for a position that a phone
    /// can interpolate between and wrong for a level: moving the master in the
    /// room left the slider on somebody's phone showing the old value for most
    /// of a minute, which reads as a remote that has lost the player rather
    /// than one that is a moment behind. Reported by Scott, 2026-08-14.
    ///
    /// The same debounce the configuration write uses, and for the same reason,
    /// with a shorter wait because this one is about what somebody is watching
    /// happen on a second screen.
    fn report_sound_soon(self: &Rc<Self>) {
        if self.sound_report_pending.replace(true) {
            return;
        }
        let app = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            app.sound_report_pending.set(false);
            // The scheduled report starts its ten seconds again from here, so a
            // drag does not leave one following a moment behind it.
            app.jellyfin_reported.set(0);
            app.report_to_jellyfin(JellyfinMoment::Progress);
        });
    }

    fn save_volume_soon(self: &Rc<Self>) {
        if self.volume_save_pending.replace(true) {
            return;
        }
        let app = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_secs(1), move || {
            app.volume_save_pending.set(false);
            if let Err(e) = app.config.borrow().save() {
                eprintln!("Could not save volume: {e}");
            }
        });
    }

    /// Left and right: between the buttons while the button row is held,
    /// through the video everywhere else.
    fn controls_left_right(self: &Rc<Self>, direction: isize) {
        use crate::controls::Row;
        let row = self
            .controls
            .borrow()
            .as_ref()
            .map(|controls| controls.row())
            .unwrap_or(Row::None);
        // Swallowed rather than passed on: there is nowhere sideways to go in
        // a list, and seeking the film out from under an open chooser would be
        // worse than doing nothing.
        if matches!(row, Row::Subtitles | Row::Audio) {
            return;
        }
        if row == Row::Buttons || row == Row::Volume {
            if let Some(controls) = self.controls.borrow().as_ref() {
                if row == Row::Volume {
                    controls.adjust_level(direction);
                } else {
                    controls.move_focus(direction);
                }
            }
            return;
        }
        self.scrub(direction as f64 * crate::player::STEP_SECONDS);
    }

    /// Begins or continues a scrub. Nothing moves until the ticker decides
    /// this is a hold; a tap resolves to a single step when released.
    fn scrub(self: &Rc<Self>, seconds: f64) {
        let playback = self.playback.borrow().clone();
        let Some(playback) = playback else { return };

        let already = playback.is_scrubbing();
        playback.scrub_input(seconds);
        self.scrub_seen.set(Some(std::time::Instant::now()));
        self.peek_controls();
        if already {
            return;
        }

        let generation = self.scrub_generation.get();
        let weak = Rc::downgrade(self);
        let mut last = std::time::Instant::now();
        glib::timeout_add_local(SCRUB_TICK, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if app.scrub_generation.get() != generation {
                return glib::ControlFlow::Break;
            }
            let playback = app.playback.borrow().clone();
            let Some(playback) = playback else {
                return glib::ControlFlow::Break;
            };

            // Auto-repeat is what keeps this alive; long enough without it
            // and the release must have gone missing.
            let stale = app
                .scrub_seen
                .get()
                .is_none_or(|seen| seen.elapsed() > SCRUB_ABANDON);
            if stale {
                app.end_scrub();
                return glib::ControlFlow::Break;
            }

            let now = std::time::Instant::now();
            playback.scrub_tick(now - last);
            last = now;
            app.peek_controls();
            glib::ControlFlow::Continue
        });
    }

    /// The direction was let go: perform the one seek the gesture asked for.
    fn end_scrub(&self) {
        let playback = self.playback.borrow().clone();
        let Some(playback) = playback else { return };
        if !playback.is_scrubbing() {
            return;
        }
        self.scrub_generation
            .set(self.scrub_generation.get().wrapping_add(1));
        self.scrub_seen.set(None);
        playback.commit_scrub();
        // A peek, matching the press that began this. Waking the whole strip
        // here brought the buttons in on every release, so a tap of the arrow
        // keys made the bar duck and pop back.
        self.peek_controls();
    }

    fn toggle_fullscreen(&self) {
        if self.locked_fullscreen {
            return;
        }
        let wanted = !self.window.is_fullscreen();
        if wanted {
            // Read before the change, because a fullscreen window reports
            // itself maximized whether or not anybody maximized it.
            self.maximized_before_fullscreen
                .set(self.window.is_maximized());
            self.window.fullscreen();
        } else {
            self.window.unfullscreen();
            // Put back the state fullscreen was entered from, said outright
            // in both directions rather than left to GTK.
            //
            // Leaving fullscreen does not restore it: a window that was never
            // maximized comes back maximized, because fullscreen implies
            // maximized and that is the state handed back - and one launched
            // fullscreen comes back maximized having never been drawn at its
            // own size at all. Asking only for the un-maximize fixed those two
            // and broke the third: a window maximized on purpose came back
            // windowed, because GTK restores the size it had and not the fact
            // that it was maximized. So both halves are asked for.
            //
            // The flag is only ever set on the way in, so a window launched
            // fullscreen has never set it and takes the default - which is the
            // right answer for exactly that case.
            match self.maximized_before_fullscreen.get() {
                true => self.window.maximize(),
                false => self.window.unmaximize(),
            }
            // The pointer only hides in fullscreen, and leaving takes the
            // countdown that would have brought it back with it.
            if let Some(controls) = self.controls.borrow().as_ref() {
                controls.reveal_pointer();
            }
        }

        // Deliberately not written down. Whether to *open* fullscreen is a
        // setting somebody sets on purpose, and it used to be whatever the
        // window happened to be at the moment they quit - so pressing F11 once
        // on the way out changed how the application started for ever after.
    }

    /// Takes the pointer off the screen once something else is driving.
    ///
    /// Fullscreen only. A window sits on a desktop the pointer belongs to as
    /// much as to us - there is a title bar above it and other windows behind -
    /// and one that vanishes while crossing an application is one somebody
    /// then has to go hunting for. Fullscreen is the case where this is all
    /// there is, and a pointer left over the menu is just something on screen.
    ///
    /// The playback strip does the same for itself against the picture; this
    /// is the menus, which had no reason to think about the pointer until they
    /// filled a television.
    fn hide_pointer(&self) {
        if self.window.is_fullscreen() {
            self.window.set_cursor_from_name(Some("none"));
        }
    }

    /// And puts it back the moment it moves, which is the only signal that
    /// somebody has picked the mouse up again.
    fn show_pointer(&self) {
        self.window.set_cursor(None);
    }

    /// Records what the gamepad should be moving through. Screens built from
    /// buttons alone pass `None`, and fall back to GTK's directional focus.
    fn set_nav(&self, list: Option<&gtk::ListBox>, header: &[gtk::Button], footer: &[gtk::Button]) {
        // Every screen goes through here, which makes it the one place that
        // can be sure a screen with selectable text is no longer the one on
        // display. A screen that has some sets it again afterwards.
        *self.copy_root.borrow_mut() = None;
        *self.nav_list.borrow_mut() = list.cloned();
        *self.nav_header.borrow_mut() = header.to_vec();
        // Cleared here so it belongs to one screen only: a page that wants Up
        // to land somewhere particular says so after wiring its navigation.
        *self.nav_header_entry.borrow_mut() = None;
        *self.nav_footer.borrow_mut() = footer.to_vec();

        let mut stops: Vec<gtk::Widget> = header.iter().map(|b| b.clone().upcast()).collect();
        // A column beside the list comes first, being to its left.  Taken
        // rather than read, so it belongs to this screen only.
        if let Some(side) = self.nav_side_list.borrow_mut().take() {
            stops.push(side.upcast());
        }
        if let Some(list) = list {
            stops.push(list.clone().upcast());
        }
        stops.extend(footer.iter().map(|b| b.clone().upcast()));
        *self.nav_stops.borrow_mut() = stops;
    }

    /// The button Down from the list should land on.
    ///
    /// The first *usable* one rather than simply the first: with no video
    /// chosen the play button is insensitive, and stopping there would leave
    /// the gear beside it unreachable without a pointer.
    fn first_footer(footer: &[gtk::Button]) -> Option<&gtk::Button> {
        footer.iter().find(|button| button.is_sensitive())
    }

    /// The header button Up from the list should land on.
    fn last_header(header: &[gtk::Button]) -> Option<&gtk::Button> {
        header.iter().rev().find(|button| button.is_sensitive())
    }

    fn handle_action(self: &Rc<Self>, action: crate::gamepad::Action) {
        use crate::gamepad::Action;
        self.hide_pointer();
        match action {
            Action::Up if self.playback.borrow().is_some() => self.enter_controls(),
            Action::Down if self.playback.borrow().is_some() => self.leave_controls(),
            Action::Up => self.move_selection(-1),
            Action::Down => self.move_selection(1),
            Action::Left if self.playback.borrow().is_some() => self.controls_left_right(-1),
            Action::Right if self.playback.borrow().is_some() => self.controls_left_right(1),
            // The same three in the same order the arrow keys use: a slider on
            // the selected row, then the panes of the browser, then whatever
            // GTK can find. move_between_lists has to be in here explicitly -
            // child_focus cannot reach a list, because the rows are not
            // focusable and the list being a focus stop is our arrangement
            // rather than something GTK's directional search knows about.
            // The bars answer these where there is one, and nothing else on
            // that screen does - see the key handler for why silence matters.
            Action::Left if self.on_settings() => {
                self.settings_slider(-1);
            }
            Action::Right if self.on_settings() => {
                self.settings_slider(1);
            }
            Action::Left => {
                if !self.settings_slider(-1) && !self.move_between_lists(-1) {
                    self.window.child_focus(gtk::DirectionType::Left);
                }
            }
            Action::Right => {
                if !self.settings_slider(1) && !self.move_between_lists(1) {
                    self.window.child_focus(gtk::DirectionType::Right);
                }
            }
            // During playback the lower face button is the obvious place for
            // play/pause, and there is nothing else on screen to activate.
            // On the button row the lower face button presses whatever is
            // highlighted. Everywhere else in playback it is play/pause, which
            // is what it should be when nothing is being driven.
            Action::Activate | Action::PlayPause if self.playback.borrow().is_some() => {
                let on_buttons = self
                    .controls
                    .borrow()
                    .as_ref()
                    .is_some_and(|controls| controls.takes_activation());
                if on_buttons && action == Action::Activate {
                    self.press_activate();
                    return;
                }
                self.toggle_pause();
                self.wake_controls();
            }
            Action::Activate => self.activate_focused(),
            Action::PlayPause => {}
            Action::ActivateReleased if self.playback.borrow().is_some() => self.release_activate(),
            Action::ActivateReleased => {}
            Action::DirectionReleased => self.end_scrub(),
            Action::PageUp => self.move_selection(-PAGE_ROWS),
            Action::PageDown => self.move_selection(PAGE_ROWS),
            // Harmless during playback, where there are no stops to move
            // between and this does nothing.
            Action::FocusNext => {
                self.move_focus_stop(1);
            }
            Action::FocusPrevious => {
                self.move_focus_stop(-1);
            }
            // Whatever is on screen goes away first, whether it is being
            // driven or simply lingering: backing out of the film while the
            // strip is up would be a surprise either way.
            Action::Back => {
                let showing = self
                    .controls
                    .borrow()
                    .as_ref()
                    .is_some_and(|controls| controls.is_showing());
                if showing {
                    if let Some(controls) = self.controls.borrow().as_ref() {
                        controls.hide();
                    }
                } else {
                    self.go_back();
                }
            }
            Action::Fullscreen => self.toggle_fullscreen(),
            // Ignored outside playback, matching the keyboard: there is
            // nothing to turn off from a menu.
            // During playback this button is held for silence and tapped for
            // subtitles, so the tap waits for the release to know which it
            // was. Everywhere else there is nothing to silence and nothing to
            // subtitle, so it does neither.
            Action::Subtitles if self.playback.borrow().is_some() => self.press_subtitles(),
            Action::Subtitles => {}
            Action::SubtitlesReleased if self.playback.borrow().is_some() => {
                self.release_subtitles()
            }
            Action::SubtitlesReleased => {}
            Action::TimeReadout => self.toggle_time_readout(),
        }
    }

    /// Moves the selection one row, obeying the same boundary rules the
    /// keyboard does: the footer button sits below the last row, and the top
    /// of the list is a hard stop rather than wrapping.
    fn move_selection(self: &Rc<Self>, delta: i32) {
        if self.scroll_about(delta) {
            return;
        }
        // Cloned out before anything can rebuild the screen underneath us.
        let list = self.nav_list.borrow().clone();
        let footer = self.nav_footer.borrow().clone();
        let header = self.nav_header.borrow().clone();

        let Some(list) = list else {
            // A screen of buttons and no rows. Between the two rows by name,
            // since a directional search cannot reliably get from one to the
            // other when they are not above one another on the page.
            let focused = |buttons: &[gtk::Button]| buttons.iter().any(|button| button.has_focus());
            let landing = match delta {
                _ if delta > 0 && focused(&header) => footer.first(),
                _ if delta < 0 && focused(&footer) => header.first(),
                _ => None,
            };
            if let Some(button) = landing {
                self.sounds.borrow().click();
                button.grab_focus();
                return;
            }
            let direction = if delta < 0 {
                gtk::DirectionType::Up
            } else {
                gtk::DirectionType::Down
            };
            self.window.child_focus(direction);
            return;
        };

        let last = last_row_index(&list);
        let select = |index: i32| {
            if let Some(row) = list.row_at_index(index) {
                self.sounds.borrow().click();
                list.select_row(Some(&row));
                settle_on(&row);
            }
        };

        if header.iter().any(|button| button.has_focus()) {
            if delta > 0 {
                select(0);
            }
            return;
        }
        if footer.iter().any(|button| button.has_focus()) {
            if delta < 0 {
                select(last);
            }
            return;
        }

        let position = list.selected_row().map(|row| row.index()).unwrap_or(0);
        let next = position + delta;
        // A page that runs off the end stops at the end, rather than doing
        // nothing: only a single step from the very edge should be ignored.
        if next < 0 {
            if position > 0 {
                select(0);
            } else if let Some(button) = self
                .nav_header_entry
                .borrow()
                .clone()
                .or_else(|| App::last_header(&header).cloned())
            {
                self.sounds.borrow().click();
                button.grab_focus();
            }
            return;
        }
        if next > last && position < last {
            select(last);
            return;
        }
        if next > last {
            if let Some(button) = App::first_footer(&footer) {
                self.sounds.borrow().click();
                button.grab_focus();
            }
            return;
        }
        select(next);
    }

    /// Activates whatever holds focus. Rows go through the list's
    /// `row-activated` signal, which is what the screens connect to; anything
    /// else (the footer, the confirm screen's buttons) activates directly.
    fn activate_focused(self: &Rc<Self>) {
        // Disambiguated: GtkWindowExt and RootExt both define `focus`.
        let Some(widget) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return;
        };
        let list = self.nav_list.borrow().clone();

        // The focus is on a row again, so that a screen reader has something
        // to announce. Both shapes are still accepted: the row directly, and
        // the list for the moment after Tab has landed on one but before a
        // row has been settled on.
        let focused_list = widget
            .downcast_ref::<gtk::ListBoxRow>()
            .and_then(|row| row.parent())
            .and_downcast::<gtk::ListBox>()
            .or_else(|| widget.downcast_ref::<gtk::ListBox>().cloned())
            .or_else(|| list.filter(|list| list.has_focus()));
        if let Some(list) = focused_list
            && let Some(row) = list.selected_row()
        {
            self.sounds.borrow().click();
            list.emit_by_name::<()>("row-activated", &[&row]);
            return;
        }
        widget.activate();
    }

    /// Turns subtitles on or off for the playback in progress, and brings the
    /// strip up so the change is visible: the letters dim or light, which is
    /// the only confirmation when the moment has no subtitle to draw anyway.
    fn toggle_subtitles(&self) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        let showing = playback.toggle_subtitles();
        self.subtitles_hidden.set(!showing);
        self.push_subtitle_state();
        self.wake_controls();
    }

    /// Steps one output to the next audio track in the file, on `A` for the
    /// primary and `S` for the secondary.
    ///
    /// Ahead of the chooser rather than instead of it: switching live is
    /// proven, and this makes it reachable while the rest - a menu per output,
    /// and the branch regrouping that two outputs on one track needs - is
    /// built. The reason it says nothing on screen is that there is nowhere
    /// yet to say it; the chooser is where a track name belongs.
    fn cycle_audio(&self, role: &str) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        if let Err(reason) = playback.cycle_audio(role) {
            eprintln!("Cannot step the {role} audio: {reason}");
        }
        self.wake_controls();
    }

    /// The chooser's rows and which of them is in force: Off, then everything
    /// the video offers, in the order the media page lists them.
    ///
    /// Browsing for a file is deliberately not among them, though the media
    /// page's version of this list ends with it. That opens a screen of its
    /// own, and going looking on disk belongs to the page you choose from
    /// before pressing play rather than to a list laid over a running film.
    ///
    /// "Off" rather than the page's "None", because on the strip it is the
    /// same state the icon and the toggle already call off.
    fn subtitle_entries(&self) -> (Vec<String>, Option<usize>) {
        let mut entries = vec!["Off".to_string()];
        let chosen = self.subtitle.borrow().clone();
        // Off unless something matches, which is also the answer when a
        // remembered choice names a subtitle this video does not have.
        let mut current = Some(0);
        for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
            if chosen.as_ref() == Some(&option.choice()) {
                current = Some(position + 1);
            }
            entries.push(subtitle_label(option));
        }
        (entries, current)
    }

    /// One output's soundtrack list, and which row it is playing.
    ///
    /// The film's own tracks, with "None" first for the second output only:
    /// playing nothing on the second is a legitimate choice in a way it is not
    /// on the first, where it would mean a film with no sound at all.
    ///
    /// Browsing for a separate audio file is deliberately not here, though the
    /// media page's version of this list ends with it. That opens a screen of
    /// its own, and going looking on disk belongs to the page you choose from
    /// before pressing play - the same rule the subtitle chooser follows.
    /// Takes the playback rather than reading it off `self`, because playback
    /// starting has not put it into its cell yet - the same reason
    /// [`Self::show_subtitle_state`] takes one. Reading `self` here marked the
    /// first row of both lists on every film, since with no playback to ask,
    /// nothing matched what was playing.
    fn audio_entries(&self, playback: &Playback, role: Role) -> (Vec<String>, Option<usize>) {
        let offers_none = role == Role::Secondary;
        let mut entries = Vec::new();
        if offers_none {
            entries.push("None".to_string());
        }
        let playing = playback.playing_on(role.key());
        let offset = usize::from(offers_none);
        // None when the output is on a separate file, which this list has no
        // row for: nothing in it is in force, so nothing is marked.
        let mut current = match playing {
            Some(Playing::File(_)) => None,
            None if offers_none => Some(0),
            _ => None,
        };
        for (position, track) in self.tracks.borrow().iter().enumerate() {
            if playing == Some(Playing::Track(track.index)) {
                current = Some(position + offset);
            }
            entries.push(describe_audio_track(track));
        }
        (entries, current)
    }

    /// Fills both outputs' menus with what this video offers.
    fn push_audio_entries(&self, playback: &Playback, controls: &Rc<Controls>) {
        for (index, role) in [Role::Primary, Role::Secondary].into_iter().enumerate() {
            let (entries, current) = self.audio_entries(playback, role);
            controls.set_audio_entries(index, &entries, current);
        }
    }

    /// Puts one output onto the soundtrack at `at` in its own list, without
    /// stopping the film.
    fn choose_audio(self: &Rc<Self>, role: &str, at: usize) {
        let Some(playback) = self.playback.borrow().clone() else {
            return;
        };
        let offers_none = role == Role::Secondary.key();
        // The "None" row on the second output's list, which is a row rather
        // than a track and so cannot be looked up among them.
        let wanted = match (offers_none, at) {
            (true, 0) => None,
            _ => {
                let index = at - usize::from(offers_none);
                match self.tracks.borrow().get(index) {
                    Some(track) => Some(Playing::Track(track.index)),
                    None => return,
                }
            }
        };
        if let Err(reason) = playback.set_audio(role, wanted) {
            eprintln!("Cannot change the {role} soundtrack: {reason}");
        }
        if let Some(controls) = self.controls.borrow().clone() {
            self.push_audio_entries(&playback, &controls);
        }
    }

    /// Tells the strip what subtitles are doing and what there is to choose
    /// from, which is one answer in three places: whether the icon can be
    /// worked at all, whether it is lit, and what the chooser lists.
    ///
    /// Takes both rather than reading them off `self`, because playback
    /// starting has not put either into its cell yet.
    fn show_subtitle_state(&self, playback: &Playback, controls: &Rc<Controls>) {
        // What the video offers, not what is attached. The icon opens a list
        // that includes turning subtitles on, so a film started with them off
        // has to be able to reach it - which asking whether anything is
        // attached would refuse.
        let offers = !self.subtitle_options.borrow().is_empty() || playback.has_subtitles();
        // What has been chosen, not what the pipeline has got to yet.
        //
        // A switch takes a moment to arrive, and the overlay is deliberately
        // blank until it does - see `Playback::set_subtitle`. An icon that
        // read the pipeline therefore dimmed at the start of every switch and
        // stayed dim, because nothing comes back to ask again once the
        // subtitle lands. The choice is the honest answer to what the icon is
        // saying: subtitles are on, and one is on its way.
        let showing = self.subtitle.borrow().is_some() && !self.subtitles_hidden.get();
        controls.set_subtitles(offers, showing);
        let (entries, current) = self.subtitle_entries();
        controls.set_subtitle_entries(&entries, current);
    }

    /// The same, for everywhere that can simply ask what is playing.
    fn push_subtitle_state(&self) {
        let playback = self.playback.borrow().clone();
        let controls = self.controls.borrow().clone();
        if let (Some(playback), Some(controls)) = (playback, controls) {
            self.show_subtitle_state(&playback, &controls);
        }
    }

    /// Takes a row from the chooser and puts it into the film already running.
    ///
    /// Row zero is Off; the rest follow `subtitle_options` in order, which is
    /// the order [`Self::subtitle_entries`] built them in. The choice is
    /// remembered as well as applied, the same as choosing one from the media
    /// page: it is the same decision, made later.
    fn choose_subtitle(self: &Rc<Self>, entry: usize) {
        let playback = self.playback.borrow().clone();
        let file = self.file.borrow().clone();
        let (Some(playback), Some(file)) = (playback, file) else {
            return;
        };
        let picked = match entry.checked_sub(1) {
            None => None,
            Some(index) => match self.subtitle_options.borrow().get(index) {
                Some(option) => Some(option.choice()),
                // A list that changed under the press. Nothing to apply, and
                // the mark stays where it was.
                None => return,
            },
        };

        // Already what is playing, and already showing it. Nothing to do, and
        // doing it anyway would rebuild the subtitle chain to arrive back
        // where it started - a blank second in the middle of a film for no
        // reason. This is also what makes pressing straight through the
        // chooser a way of closing it, since it opens on this very row.
        //
        // The second half is not redundant: picking the subtitle that is
        // already chosen but switched off is how it is asked for again.
        //
        // Asked of what has been chosen rather than of the pipeline, for the
        // reason `show_subtitle_state` gives - mid-switch the pipeline is
        // deliberately showing nothing, and taking that at face value would
        // make every second press of the same row a needless switch.
        if picked == *self.subtitle.borrow() && self.subtitles_hidden.get() == (entry == 0) {
            return;
        }

        // Located here for the reason it is at the start of playback: finding
        // one can need the server address and access token, which are ours to
        // know and the pipeline's to be kept out of.
        let located = match self.locate_subtitle(&file, picked.as_ref()) {
            Ok(located) => located,
            // The same answer playback gives when a subtitle cannot be found
            // as a film opens: it gives up the subtitle and not the film.
            // Nothing is recorded either, so the mark stays on whatever is
            // still playing - which is what says the choice did not take.
            Err(e) => {
                eprintln!("{e}");
                return;
            }
        };
        if let Err(e) = playback.set_subtitle(located.as_ref()) {
            eprintln!("{e}");
            return;
        }

        *self.subtitle.borrow_mut() = picked;
        // Choosing one is asking to see it, whatever the toggle was doing for
        // the last. Off is the exception, being the toggle said deliberately.
        self.subtitles_hidden.set(entry == 0);
        self.remember_tracks();
        self.push_subtitle_state();
        self.wake_controls();
    }

    /// Starts a hold on the left face button. Nothing happens yet: what the
    /// press meant is only known when it is let go, or when it has been down
    /// long enough to have meant the other thing.
    fn press_subtitles(self: &Rc<Self>) {
        if self.subtitles_holding.replace(true) {
            return;
        }
        self.subtitles_held.set(false);
        let mark = self.subtitles_hold.get() + 1;
        self.subtitles_hold.set(mark);
        let app = self.clone();
        glib::timeout_add_local_once(crate::controls::HOLD, move || {
            if app.subtitles_hold.get() != mark {
                return;
            }
            app.subtitles_held.set(true);
            app.toggle_mute();
        });
    }

    /// Changes the subtitles, unless the hold already silenced everything.
    fn release_subtitles(self: &Rc<Self>) {
        self.subtitles_holding.set(false);
        self.subtitles_hold.set(self.subtitles_hold.get() + 1);
        if !self.subtitles_held.replace(false) {
            self.toggle_subtitles();
        }
    }

    /// Moves the level on the settings row that is selected, and says whether
    /// there was one. Left and right do nothing else on this screen, so they
    /// are free to mean this where a slider is sitting.
    fn settings_slider(self: &Rc<Self>, direction: isize) -> bool {
        // On that screen and no other. The sliders are held on the application
        // rather than on the page they belong to, and they outlive it: leaving
        // settings does not empty the list, so this went on matching by row
        // number against whatever screen came next. Backing out to the media
        // page and pressing Left moved the interface size, because the row
        // selected there had the same number as the row the size sits on.
        if *self.screen.borrow() != Screen::Settings || !self.in_settings_pane.get() {
            return false;
        }
        let Some(index) = self
            .nav_list
            .borrow()
            .as_ref()
            .and_then(|list| list.selected_row())
            .map(|row| row.index())
        else {
            return false;
        };
        let Some(item) = self.item_at(index) else {
            return false;
        };
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((kind, scale, value)) = found else {
            return false;
        };
        // Snapped to the step rather than added to: a value set finely with a
        // pointer, or from the panel during playback, otherwise carries its
        // odd remainder through every press that follows.
        let step = kind.step();
        let now = scale.value();
        // Nudged by a step from where it is, snapped onto the step grid. The
        // nudge is what the epsilon protects: a value already sitting exactly
        // on a step would otherwise floor to itself and go nowhere, which is
        // what stopped the interface size after one press - its steps are a
        // tenth, and rounding to a whole number made every press compute the
        // same target.
        let ratio = now / step;
        let moved = if direction > 0 {
            ((ratio + 1e-6).floor() + 1.0) * step
        } else {
            ((ratio - 1e-6).ceil() - 1.0) * step
        };
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        scale.set_value(moved);
        self.set_slider(kind, moved, &value);
        // Safe here: nothing is holding the bar, so redrawing cannot be read
        // as another movement.
        if kind == Slider::Scale {
            self.apply_scale(moved);
        }
        true
    }

    /// Silences the output the selected row belongs to, or lets it go. What
    /// activating a level row does, since there is nothing to open.
    fn toggle_settings_mute(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Volume(role), scale, value)) = found else {
            return;
        };
        let muted = !self.config.borrow().muted(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_volume(role, scale.value() / 100.0);
            config.set_muted(role, muted);
        }
        value.set_text(&volume_label(scale.value() / 100.0, muted));
        // On is unmuted, so the switch reads as the output being heard rather
        // than as the mute being applied. A silenced output's bar is dimmed
        // with it: the level it will come back to is worth still showing, and
        // moving it while nothing can be heard is not.
        scale.set_sensitive(!muted);
        value.set_sensitive(!muted);
        self.set_settings_switch(item, !muted);
        self.save_volume_soon();
    }

    /// Turns an output's delay on or off, keeping whatever it is set to.
    ///
    /// Off is how somebody checks whether a delay is helping: winding it to
    /// zero would answer the same question and lose the value they spent time
    /// finding.
    fn toggle_settings_offset(self: &Rc<Self>, item: Item) {
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == item)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        let Some((Slider::Offset(role), scale, value)) = found else {
            return;
        };
        let on = !self.config.borrow().offset_on(role);
        {
            let mut config = self.config.borrow_mut();
            config.set_offset_on(role, on);
            let _ = config.save();
        }
        // Heard straight away, like the delay itself: the point of the switch
        // is comparing with and without while something is playing.
        self.push_offset_live(role);
        scale.set_sensitive(on);
        value.set_text(&offset_label(self.config.borrow().applied_offset_ms(role)));
        value.set_sensitive(on);
        self.set_settings_switch(item, on);
    }

    /// Where a slider stands now, and how that reads beside it.
    fn slider_state(&self, kind: Slider) -> (f64, String) {
        let config = self.config.borrow();
        match kind {
            Slider::Volume(role) => {
                let level = config.volume(role);
                (level * 100.0, volume_label(level, config.muted(role)))
            }
            Slider::Offset(role) => {
                // The bar keeps the stored delay, so turning it back on shows
                // what it will be; the reading says what is actually being
                // applied, which while it is off is nothing.
                (
                    config.offset_ms(role),
                    offset_label(config.applied_offset_ms(role)),
                )
            }
            Slider::ResumeThreshold => {
                let percent = config.resume_min_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::WatchedThreshold => {
                let percent = config.watched_percent().round();
                (percent, format!("{percent}%"))
            }
            Slider::Scale => {
                // The bar sits at whatever size is in force either way, so
                // turning the switch off starts from what is on screen. The
                // reading says Auto rather than the number, since while the
                // switch is on that number is a consequence and not a
                // setting.
                let chosen = chosen_scale(&config);
                let scale = chosen.unwrap_or_else(|| self.scale.get());
                let reading = match chosen {
                    Some(scale) => scale_label(scale),
                    None => "Auto".to_string(),
                };
                (steps_from_scale(scale), reading)
            }
            Slider::SubtitleSize => {
                let size = config
                    .subtitle_size
                    .unwrap_or(crate::pipeline::DEFAULT_SUBTITLE_SIZE);
                (size as f64, size.to_string())
            }
        }
    }

    /// Writes a slider through to the configuration and puts the reading
    /// beside it in step. Turning an output up unmutes it, as the panel
    /// during playback does.
    fn set_slider(self: &Rc<Self>, kind: Slider, moved: f64, value: &gtk::Label) {
        let range = kind.range();
        let moved = moved.clamp(*range.start(), *range.end());
        {
            let mut config = self.config.borrow_mut();
            match kind {
                Slider::Volume(role) => {
                    config.set_volume(role, moved / 100.0);
                    config.set_muted(role, false);
                }
                Slider::Offset(role) => config.set_offset_ms(role, moved),
                Slider::ResumeThreshold => config.resume_min_percent = Some(moved),
                Slider::WatchedThreshold => config.watched_percent = Some(moved),
                Slider::Scale => config.ui_scale = Some(scale_from_steps(moved)),
                Slider::SubtitleSize => config.subtitle_size = Some(moved.round() as u32),
            }
        }
        // Nothing redrawn here. Restyling moves the bar under whatever is
        // moving it, which GTK reads as another movement, which restyles
        // again - a loop that ran the size to its limit as soon as it was
        // dragged. Who calls this decides when it is safe: a key press
        // applies at once, a drag waits to be let go.
        // Heard straight away when a film is playing, so a delay can be placed
        // against the picture rather than guessed at and checked later.
        // The configuration above already holds `moved`, so this reads the
        // same number back rather than adding the baseline to it by hand.
        if let Slider::Offset(role) = kind {
            self.push_offset_live(role);
        }
        value.set_text(&match kind {
            Slider::Volume(_) => volume_label(moved / 100.0, false),
            Slider::Offset(_) => offset_label(moved),
            Slider::Scale => scale_label(scale_from_steps(moved)),
            Slider::SubtitleSize => format!("{}", moved.round()),
            _ => format!("{}%", moved.round()),
        });
        self.save_volume_soon();
    }

    /// Swaps the right-hand readout between the length and what is left.
    fn toggle_time_readout(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_remaining();
        }
    }

    /// Silences every output at once, or puts back what each was doing. The
    /// same thing holding the volume button does, reached directly.
    fn toggle_mute(&self) {
        let controls = self.controls.borrow().clone();
        if let Some(controls) = controls {
            controls.toggle_hush();
        }
    }

    fn stop_playback(self: &Rc<Self>) {
        self.finish_playback(false);
    }

    /// Leaves playback for the menu, remembering where it had reached.
    ///
    /// What Escape, the stop button and the settings button all do, so that
    /// stepping out to change something and coming back is one motion however
    /// it was asked for.
    fn leave_playback(self: &Rc<Self>) {
        let position = self
            .playback
            .borrow()
            .as_ref()
            .and_then(|playback| playback.position())
            .map(|position| position.nseconds())
            .filter(|position| *position > 0);
        if let Some((key, position)) = self.storage_key().zip(position) {
            *self.session_resume.borrow_mut() = Some((key, position));
        }
        self.stop_playback();
        self.show_menu();
    }

    /// Tears playback down, saving or clearing the resume position as it goes.
    ///
    /// `wait_for_kodi` holds on until the last progress report has actually
    /// reached Kodi. That only matters when the process is about to end, since
    /// the report goes out on a detached thread and exiting would take it
    /// along; everywhere else it would be a stall for nothing.
    fn finish_playback(self: &Rc<Self>, wait_for_kodi: bool) {
        // Whatever else happens below, stop holding the display awake: this
        // is reached from the window closing as well as from playback ending.
        self.awake.set(false);
        if let Some(tick) = self.tick.borrow_mut().take() {
            tick.remove();
        }
        if let Some(controls) = self.controls.borrow_mut().take() {
            controls.cancel();
            // Playback ending with the pointer hidden would leave the menus
            // behind it without one.
            controls.reveal_pointer();
        }
        // Before the playback is taken, because the position is read from
        // it and a stopped report with nowhere to read from would file zero.
        self.report_to_jellyfin(JellyfinMoment::Stopped);
        if let Some(playback) = self.playback.borrow_mut().take() {
            playback.stop();
            if wait_for_kodi {
                playback.finish_reporting();
            }
        }
        self.window.set_title(Some("TinePlayer"));
        // After the playback is dropped above, so this reads "nothing".
        self.publish_now_playing();
    }

    /// Where playback should pick up, and the title to show for the file.
    ///
    /// Under Kodi its library is the authority, so playback starts from the
    /// position Kodi's own interface was just showing and the two never
    /// visibly disagree. Its answer stands even when it holds no resume point:
    /// a film Kodi considers unwatched starts at the beginning rather than
    /// Works out where a chosen subtitle actually comes from.
    ///
    /// The three kinds resolve against three different things - the video's
    /// own folder, the path as given, and the paired server - which is why
    /// this is here rather than in the pipeline: only the application knows
    /// all three. A library's subtitle resolves to a URL carrying the access
    /// token, and that URL is built here, used, and never stored.
    fn locate_subtitle(
        &self,
        source: &Source,
        choice: Option<&crate::subtitles::SubtitleChoice>,
    ) -> Result<Option<crate::subtitles::SubtitleSource>, String> {
        use crate::subtitles::{SubtitleChoice, SubtitleSource};

        let uri_for = |path: std::path::PathBuf| {
            glib::filename_to_uri(&path, None)
                .map(|uri| SubtitleSource::Uri(uri.to_string()))
                .map_err(|e| format!("Can't open {}: {e}", path.display()))
        };

        match choice {
            None => Ok(None),
            Some(SubtitleChoice::Embedded(index)) => Ok(Some(SubtitleSource::Embedded(*index))),
            // A name, which means the folder the video is in. A source with no
            // folder - anything opened by URL - has no subtitle files beside
            // it to have chosen in the first place.
            Some(SubtitleChoice::External(name)) => source
                .local()
                .and_then(|video| video.parent())
                .map(|folder| folder.join(name))
                .ok_or_else(|| format!("Can't find {name}: it sits beside a local video"))
                .and_then(uri_for)
                .map(Some),
            // A path, which means itself. Chosen by hand from somewhere else
            // on disk, or named on the command line, and so not tied to where
            // the video happens to live.
            Some(SubtitleChoice::File(path)) => uri_for(path.clone()).map(Some),
            // Only a video the library is playing has these, and both halves
            // are needed: the client holds the address and token, the item
            // holds which media source the index counts against.
            Some(SubtitleChoice::Library(index)) => {
                let client = self.jellyfin.borrow().clone();
                let item = self.jellyfin_item.borrow().clone();
                match (client, item) {
                    (Some(client), Some(item)) => Ok(Some(SubtitleSource::Uri(
                        client.subtitle_url(&item, *index),
                    ))),
                    _ => Err("Can't fetch that subtitle: it belongs to a library this video did not come from".to_string()),
                }
            }
        }
    }

    /// wherever our own file happens to remember. Only a Kodi that does not
    /// answer at all falls back to `positions.json`.
    ///
    /// The title comes from the same call, so it is refreshed here rather
    /// than costing a second round trip.
    fn resume_position(&self) -> Option<u64> {
        let key = self.storage_key()?;
        // Ahead of everything, including Kodi's library: this is where the
        // viewer actually was, seconds ago, and no stored answer is better
        // informed than that.
        if let Some((remembered, position)) = self.session_resume.borrow().as_ref()
            && *remembered == key
        {
            return Some(*position);
        }
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return item.resume_ns;
        }
        if let Some(item) = self.jellyfin_item.borrow().as_ref() {
            return item.resume_ns;
        }
        crate::config::load_resume(&key)
            .and_then(|resume| resume.resume_position(self.config.borrow().resume_min_percent()))
    }

    /// How this video's position and track choices are filed.
    ///
    /// Kodi's own id when it launched us, which survives an add-on stream URL
    /// changing and is the same whichever form of the path is in play.
    /// Otherwise the source names itself.
    fn storage_key(&self) -> Option<String> {
        if let Some(item) = self.kodi_item.borrow().as_ref() {
            return Some(item.key());
        }
        // The item id, never the stream address: that carries an access token
        // which changes when it is regenerated, and every position filed
        // against the old one would be orphaned.
        if let Some(item) = self.jellyfin_item.borrow().as_ref() {
            return Some(format!("jellyfin:{}", item.id));
        }
        self.file.borrow().as_ref().map(Source::key)
    }

    /// The same key for a video that is not the current one yet.
    ///
    /// `apply_media` needs this: it reads what was remembered about the file it
    /// is loading, and `self.file` does not become that file until the end of
    /// it. Asking `storage_key` there returns the *previous* video's key - or
    /// none at all on the first file of a session, which is why remembered
    /// choices were quietly ignored at startup.
    fn storage_key_for(&self, source: &Source) -> String {
        match self.kodi_item.borrow().as_ref() {
            Some(item) => item.key(),
            None => source.key(),
        }
    }

    /// The title whatever launched us gave for this video, or empty.
    ///
    /// Handed to `metadata::resolve`, which puts it at the head of the same
    /// chain everything else uses. Kept as one accessor so no caller has to
    /// know that Kodi is currently the only thing that supplies one.
    fn launcher_title(&self) -> String {
        if let Some(title) = self
            .kodi_item
            .borrow()
            .as_ref()
            .map(|item| item.title.clone())
            .filter(|title| !title.is_empty())
        {
            return title;
        }
        self.jellyfin_item
            .borrow()
            .as_ref()
            .map(|item| item.title.clone())
            .unwrap_or_default()
    }

    /// What to call the current video on screen.
    ///
    /// Read from `details` rather than worked out a second time, so the
    /// titlebar cannot disagree with the media page about what is playing.
    /// The order behind it lives in `metadata::resolve`: the launcher's title,
    /// then a sidecar's, then the container's own tag, then the file name with
    /// its extension and any trailing year taken off.
    fn file_label(&self) -> Option<String> {
        if self.file.borrow().is_none() {
            return None;
        }
        let title = self.details.borrow().title.clone();
        if !title.is_empty() {
            return Some(title);
        }
        // Only reachable if resolve found nothing at all to call it, which its
        // own file-name fallback makes unlikely.
        self.file.borrow().as_ref().map(Source::label)
    }

    // --- Menu ----------------------------------------------------------

    /// Builds the screen the application sits on, without installing it.
    ///
    /// Two shapes behind one entry point, because everything that shows the
    /// menu wants whichever is right rather than having to ask first. With no
    /// video there is nothing to configure and nothing to play, so the page is
    /// an invitation to choose one. With a video it is a page about that
    /// video, and the choices sit under what they are choices about.
    ///
    /// Split out so the browser can raise the same page behind itself as a
    /// backdrop, which is what makes it read as a window opening over the
    /// menu rather than as another screen replacing it.
    fn build_menu_page(self: &Rc<Self>) -> (gtk::Widget, Option<gtk::ListBox>) {
        // What a resize compares against to decide whether rebuilding this
        // page would change anything - recorded here, for every menu page,
        // rather than where the poster is built.
        //
        // Only the media page has a poster, so recording it there left the
        // empty page's figure at whatever it happened to be, which never
        // matched and so always answered "yes, rebuild". The page was then
        // rebuilt every quarter second for as long as it was on screen, and
        // the surface layout that followed each rebuild scheduled the next.
        //
        // It was close to invisible, because the page it kept rebuilding looks
        // the same each time - but the pointer's idea of what is under it does
        // not survive the widget being destroyed and made again, so hovering a
        // button only lit it while the mouse was moving, and a click only
        // landed if it happened to arrive between two rebuilds.
        self.built_poster.set(self.poster_height(self.scale.get()));
        if self.file.borrow().is_none() {
            return (self.build_empty_page().upcast(), None);
        }
        let (page, list) = self.build_media_page();
        (page.upcast(), Some(list))
    }

    /// The page about the video that is loaded: what it is, above how it is
    /// about to be played.
    fn build_media_page(self: &Rc<Self>) -> (gtk::Overlay, gtk::ListBox) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        // Everything sits in one column, held to 16:9 by `hold_safe_area` so
        // that a wide window widens the artwork behind rather than the text on
        // top. A plot line three thousand pixels across is not a page anyone
        // reads, and a row whose value drifts that far from its label stops
        // reading as one row.
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(16.0))
            .margin_top(px(30.0))
            // Matched to the sides rather than to the top. The panel now runs
            // to the bottom of the page, so this margin is a visible edge
            // along it, and at 26 it read as a thinner border than the 34 down
            // either side.
            .margin_bottom(px(34.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled, not centered. The centering is `Column`'s job, and a
            // box that also centers itself shrinks to its natural width
            // inside the column it was just given - which is what truncated
            // every row value on a file with a short plot.
            .css_classes(["tp-media"])
            .build();

        // The poster keeps to the left for the height of the page; everything
        // else runs down the column beside it.
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(32.0))
            .vexpand(true)
            .build();
        columns.append(&self.poster_column(scale));

        let main = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .hexpand(true)
            .build();
        columns.append(&main);
        content.append(&columns);

        let (scroller, list) = scrolling_list();
        name_it(&list, "Playback Options");

        // The film's details sit still. Only the rows scroll, so the poster,
        // the title and the buttons stay where they are however long the list
        // gets - and the list scrolls under them rather than the page moving
        // as a whole.
        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(6.0))
            .valign(gtk::Align::Start)
            .build();
        for widget in self.heading_block(scale) {
            info.append(&widget);
        }
        main.append(&info);

        let file = self.file.borrow().clone();
        let config = self.config.borrow();
        let tracks = self.tracks.borrow();

        // Asked before the rows are built, not after: this is what fetches
        // Kodi's title as well as its resume point, and a row built ahead of
        // it would show the file name until something rebuilt the screen.
        let resume_at = self.resume_position();

        let has_file = file.is_some();
        let has_secondary = config.secondary_sink.is_some();

        // The rows, and the group each one opens - `None` for a row that
        // continues the group above it.
        //
        // Kept as a second list rather than a fifth element on the tuple so
        // that `alignment_row` can go on returning a row and nothing else. The
        // two are pushed together every time, which is what keeps them in step.
        let mut rows: Vec<(String, String, bool, MenuAction)> = Vec::new();
        let mut groups: Vec<Option<&str>> = Vec::new();
        let mut push = |group: Option<&'static str>,
                        row: Option<(String, String, bool, MenuAction)>| {
            if let Some(row) = row {
                groups.push(group);
                rows.push(row);
            }
        };

        // Which output, said once at the top of the group, rather than on the
        // front of all three rows under it. "First Output" and "Second Output"
        // rather than primary and secondary: the ordinal is the whole of what
        // distinguishes them to anyone watching, and Primary/Secondary is the
        // vocabulary of the code and the config file.
        push(
            Some("FIRST OUTPUT"),
            Some((
                "Output Device".to_string(),
                config
                    .primary_sink
                    .clone()
                    .unwrap_or_else(|| "Not set".to_string()),
                true,
                MenuAction::Device(Role::Primary),
            )),
        );
        push(
            None,
            Some((
                "Audio Track".to_string(),
                if has_file {
                    self.describe_audio(Role::Primary)
                } else {
                    "—".to_string()
                },
                has_file,
                MenuAction::Track(Role::Primary),
            )),
        );
        push(None, self.alignment_row(Role::Primary));

        push(
            Some("SECOND OUTPUT"),
            Some((
                "Output Device".to_string(),
                config
                    .secondary_sink
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
                true,
                MenuAction::Device(Role::Secondary),
            )),
        );
        push(
            None,
            Some((
                "Audio Track".to_string(),
                if has_file && has_secondary {
                    self.describe_audio(Role::Secondary)
                } else {
                    "—".to_string()
                },
                has_file && has_secondary,
                MenuAction::Track(Role::Secondary),
            )),
        );
        if has_secondary {
            push(None, self.alignment_row(Role::Secondary));
        }

        // Its own group rather than sitting with the audio pair: the subtitle
        // language is an independent choice, and may be a third language again
        // or a repeat of either soundtrack.
        push(
            Some("SUBTITLES"),
            Some((
                "Language".to_string(),
                self.describe_subtitle(),
                has_file,
                MenuAction::Subtitles,
            )),
        );

        let can_play = has_file && config.primary_sink.is_some();
        drop(tracks);
        drop(config);

        // What each row is called to anyone who cannot see the list. The group
        // heading is read once at the top of a group and does not survive into
        // a row announced on its own, so the name carries it: "Audio Track" is
        // two rows on this page and "First output, Audio Track" is one.
        //
        // Worked out here, where both lists are still in hand, and in title
        // case rather than the heading's capitals - a screen reader given
        // "FIRST OUTPUT" may spell it.
        let mut heading = String::new();
        let names: Vec<String> = rows
            .iter()
            .zip(&groups)
            .map(|((label, value, _, _), group)| {
                if let Some(group) = group {
                    heading = title_case(group);
                }
                row_name(&format!("{heading}, {label}"), value)
            })
            .collect();

        for ((label, value, enabled, _), name) in rows.iter().zip(&names) {
            append_named(&list, &menu_row(label, value, *enabled), name);
        }

        // A heading above the row that opens a group, and nothing above the
        // rest. Headings are not rows: they sit outside the selection model
        // and outside the focus chain, so they are unselectable and skipped by
        // the arrow keys without anything having to arrange it.
        //
        // That is also why the indent under them is gone. It said "this
        // belongs to the output above"; the heading says it for all three rows
        // at once, and says which output.
        //
        // It has to be done through this function rather than by setting the
        // header on each row directly, which is the obvious way and does
        // nothing: `set_header` only stores the widget on the row, and the
        // list parents and draws it from inside its header function - which
        // returns immediately when none is set. The headings were built, held
        // and never mounted.
        list.set_header_func(move |row, _before| {
            let index = row.index();
            match groups.get(index as usize).copied().flatten() {
                Some(group) => row.set_header(Some(&group_heading(group, scale, index == 0))),
                None => row.set_header(None::<&gtk::Widget>),
            }
        });

        let resumable = resume_at.is_some();

        // Between the film and the choices rather than under both. Playing is
        // what the page is for, so it sits where the eye arrives after
        // reading what the film is - and the rows below become what they
        // actually are, the settings you may want to change first rather than
        // a list to get past. Generous room above and below, so it reads as a
        // division of the page rather than as another row.
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .margin_top(px(34.0))
            .margin_bottom(px(34.0))
            .build();
        // Everything in this row packs to the left, over the rows it acts on:
        // playing, starting over, and then the two marks. Nothing expands, so
        // there is no gap pushing the marks to the far end - they read as the
        // rest of one row of controls rather than as a separate corner.
        let plays = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Start)
            .build();
        let mut play_buttons: Vec<gtk::Button> = Vec::new();

        // Resuming is the common case for a part-watched film, so it takes
        // the first position and the focus. Starting over is deliberate
        // enough to be worth its own button rather than a hidden modifier -
        // but not enough to be worth a word beside it, so once there are two
        // the second keeps only its mark. It is the same button either way;
        // what changes is how much room it argues for.
        let play = gtk::Button::new();
        play.set_child(Some(&marked_face(
            play_image(scale),
            &match resume_at {
                Some(position) => format!(
                    "  Resume ({})",
                    crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
                ),
                None => "  Play".to_string(),
            },
        )));
        // The face is two labels, so the button has no text of its own for a
        // screen reader to read off. Named outright instead.
        name_it(
            &play,
            &match resume_at {
                Some(position) => format!(
                    "Resume at {}",
                    crate::controls::format_time(gstreamer::ClockTime::from_nseconds(position))
                ),
                None => "Play".to_string(),
            },
        );
        play.add_css_class("tp-button");
        play.add_css_class("tp-action");
        play.add_css_class("tp-tall");
        play.set_sensitive(can_play);
        plays.append(&play);
        play_buttons.push(play);

        if resume_at.is_some() {
            let restart = gtk::Button::new();
            restart.set_child(Some(&marked_face(restart_image(scale), "")));
            restart.add_css_class("tp-button");
            restart.add_css_class("tp-action");
            restart.add_css_class("tp-action-icon");
            restart.add_css_class("tp-tall");
            restart.set_sensitive(can_play);
            // The word is gone from the face, so it has to be somewhere: a
            // tooltip for a pointer, and a name for a screen reader, which
            // would otherwise announce the glyph or nothing at all.
            restart.set_tooltip_text(Some("Start from the beginning"));
            name_it(&restart, "Restart");
            plays.append(&restart);
            play_buttons.push(restart);
        }
        buttons.append(&plays);

        let (fullscreen, gear) = self.corner_buttons();
        let open = self.browse_button();
        // Square, and as tall as the play button beside them. The marks are
        // built the same way on the empty page, where there is no tall button
        // to match, so this is asked for here rather than where they are made.
        for mark in [Some(&open), Some(&gear), fullscreen.as_ref()]
            .into_iter()
            .flatten()
        {
            mark.add_css_class("tp-tall");
        }
        // A little clear air between the pair that plays the film and the
        // marks that do not, so the row reads as two groups rather than a run
        // of equal buttons.
        open.set_margin_start(px(16.0));
        // Left out under a launcher: something else chose the film and is
        // waiting for this playback of it, so there is nothing to choose here.
        if !self.external {
            buttons.append(&open);
        }
        buttons.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            buttons.append(fullscreen);
        }
        let close = self.close_button();
        close.add_css_class("tp-tall");
        buttons.append(&close);

        // The page in order: what the film is, what to do about it, and then
        // the choices - which are the only part that scrolls.
        main.append(&buttons);
        // The rows sit in a panel of their own rather than loose on the page.
        // It runs to the bottom because the scroller inside it expands, which
        // is also what turns the space left below the last row into part of
        // the panel instead of a band of nothing.
        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .css_classes(["tp-menu-panel"])
            .build();
        panel.append(&scroller);
        main.append(&panel);

        // A header now rather than a footer, because that is where they sit:
        // Up from the first row reaches them, and Down from them returns.
        // Ordered as they appear, so left and right walk along the row.
        let mut header = play_buttons.clone();
        if !self.external {
            header.push(open);
        }
        header.push(gear);
        header.extend(fullscreen);
        header.push(close);

        {
            let app = self.clone();
            let actions: Vec<MenuAction> = rows.iter().map(|(_, _, _, action)| *action).collect();
            list.connect_row_activated(move |_, row| {
                // A row drawn insensitive is stating something rather than
                // offering it - the video row under Kodi, or a track row with
                // no file yet. Only the row's contents carry that; the
                // ListBoxRow that GTK wraps them in stays sensitive, and would
                // otherwise still take a click or Enter.
                //
                // Left focusable deliberately: the gamepad moves the selection
                // by grabbing focus, which fails on an insensitive widget and
                // would strand it here.
                if row.child().is_some_and(|child| !child.is_sensitive()) {
                    return;
                }
                app.sounds.borrow().click();
                *app.menu_row.borrow_mut() = row.index();
                match actions.get(row.index() as usize) {
                    Some(MenuAction::Device(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Primary)) => {
                        app.show_selector(Setting::PrimaryTrack, row)
                    }
                    Some(MenuAction::Device(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryDevice, row)
                    }
                    Some(MenuAction::Track(Role::Secondary)) => {
                        app.show_selector(Setting::SecondaryTrack, row)
                    }
                    Some(MenuAction::Align(role)) => app.show_align(*role),
                    Some(MenuAction::Subtitles) => app.show_selector(Setting::Subtitles, row),
                    None => {}
                }
            });
        }
        for (index, button) in play_buttons.iter().enumerate() {
            // With two buttons the second one restarts; with one it plays
            // from wherever it left off, which for a fresh file is the start.
            let restart = resumable && index == 1;
            let app = self.clone();
            button.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_playback(restart);
            });
        }

        self.wire_navigation(&list, &header, &[]);
        // Up from the top row lands on Play rather than on the far end of the
        // row, which is Settings. Playing is what the page is for, and it is
        // also what someone arrowing upwards off the list is reaching for.
        *self.nav_header_entry.borrow_mut() = header.first().cloned();
        (self.behind_artwork(&content), list)
    }

    /// Puts a page in front of the backdrop, and holds it to its column.
    ///
    /// Both screens go through here, so a page with no artwork still gets the
    /// same ground and the same width as one with it - which is what keeps the
    /// two from being two designs.
    fn behind_artwork(self: &Rc<Self>, content: &gtk::Box) -> gtk::Overlay {
        let backdrop = crate::artwork::Artwork::backdrop();
        let texture = self.backdrop_art.borrow().clone();
        let arrived = texture.is_some() && self.fade_art.get();
        backdrop.set_texture(texture);
        if arrived {
            fade_in(&backdrop);
        }
        *self.backdrop_widget.borrow_mut() = Some(backdrop.clone());

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));
        // The backdrop fills the window; the page inside stops widening once
        // lines get too long to read. See src/column.rs for why that is a
        // widget rather than something set on this box.
        let most = (PAGE_MAX_UNITS * self.scale.get()).round() as i32;
        overlay.add_overlay(&crate::column::Column::around(content, most));
        overlay
    }

    fn show_menu(self: &Rc<Self>) {
        let (page, list) = self.build_menu_page();

        *self.screen.borrow_mut() = Screen::Menu;
        self.window.set_child(Some(&page));

        // The empty page has no rows to land on: its two buttons are the
        // whole of it, and `build_empty_page` has already focused one.
        let Some(list) = list else { return };
        // Selected as well as focused: focus alone doesn't mark a row
        // selected, which left the list opening with nothing highlighted
        // until the first arrow key.
        let remembered = (*self.menu_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    // --- The media page ------------------------------------------------

    /// How tall the page is, or is about to be.
    ///
    /// Before the window is mapped it has no size, but it does already know
    /// the size it is going to open at - and that is not simply the interface
    /// scale times a constant, because the opening size is capped to the
    /// monitor. Guessing it as `700 * scale` put a 1050px poster in a 1325px
    /// window at 3x, which pushed the rows and the whole footer off the bottom
    /// of the screen.
    fn page_height(&self, scale: f64) -> f64 {
        match (self.window.height(), self.window.default_height()) {
            (0, 0) => 700.0 * scale,
            (0, planned) => planned as f64,
            (height, _) => height as f64,
        }
    }

    /// How tall the poster should be for the window as it stands: a share of
    /// the page, within hard bounds at both ends.
    ///
    /// The ceiling matters for more than composition. This is a size
    /// *request*, which is a minimum its window must honor, so a poster sized
    /// from the window's own height is a loop: the taller the window, the more
    /// height its contents insist on. Capping it breaks that - past this size
    /// the poster stops following the window, and the window stays free to be
    /// made smaller again.
    ///
    /// The floor is absolute rather than scaled for the opposite reason:
    /// scaled, it grows with the interface exactly when there is least room
    /// for it.
    fn poster_height(&self, scale: f64) -> f64 {
        (self.page_height(scale) * POSTER_SHARE).clamp(120.0, 620.0 * scale)
    }

    /// Remembers the window's size while it is an ordinary window.
    ///
    /// Neither maximized nor fullscreen: both report the screen's dimensions,
    /// and a size taken then is not a size the window can be restored to.
    fn note_windowed_size(&self) {
        if self.window.is_maximized() || self.window.is_fullscreen() {
            return;
        }
        let (width, height) = (self.window.width(), self.window.height());
        if width > 0 && height > 0 {
            self.windowed_size.set((width, height));
        }
    }

    /// Writes down where the window was left, on the way out.
    ///
    /// Every way of leaving goes through `window.close()`, which is what makes
    /// this one handler enough - the close button, Ctrl+Q, the confirmation,
    /// and a fatal error all end here.
    fn remember_window_size(&self) {
        let (width, height) = self.windowed_size.get();
        if width <= 0 || height <= 0 {
            return;
        }
        let mut config = self.config.borrow_mut();
        if config.window_width == Some(width) && config.window_height == Some(height) {
            return;
        }
        config.window_width = Some(width);
        config.window_height = Some(height);
        if let Err(e) = config.save() {
            eprintln!("Could not save the window size: {e}");
        }
    }

    /// Rebuilds the media page once a drag-resize has stopped moving.
    ///
    /// GTK has no "the resize finished" signal - `layout` arrives on every
    /// frame of a drag, and rebuilding the page on each one would be both slow
    /// and unpleasant to watch, the poster jumping under the pointer. So the
    /// rebuild is put on a short timer that each new size cancels and restarts,
    /// and only the last one in a drag survives to fire.
    ///
    /// Without this the poster only resized on maximize and restore, which
    /// have their own handler and change the height in one step. Dragging a
    /// window smaller left the page built for the size it used to be, which is
    /// the sort of thing that looks like a bug rather than a decision.
    ///
    /// The guard is the poster's own height rather than the window's: past the
    /// ceiling in [`App::poster_height`] the window can grow as much as it
    /// likes without the page looking any different, and rebuilding then would
    /// throw away the viewer's place in the list for nothing.
    fn rebuild_when_resize_ends(self: &Rc<Self>) {
        /// Long enough to sit out a drag, short enough that letting go and
        /// seeing the page settle reads as one action rather than two.
        const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

        if *self.screen.borrow() != Screen::Menu {
            return;
        }
        if let Some(pending) = self.resize_settle.borrow_mut().take() {
            pending.remove();
        }
        let app = self.clone();
        let source = glib::timeout_add_local_once(SETTLE, move || {
            *app.resize_settle.borrow_mut() = None;
            if *app.screen.borrow() != Screen::Menu {
                return;
            }
            if app.poster_height(app.scale.get()) == app.built_poster.get() {
                return;
            }
            app.show_menu();
        });
        *self.resize_settle.borrow_mut() = Some(source);
    }

    /// The poster, and the facts about the file under it.
    ///
    /// The two belong together and to nothing else on the page: one is what
    /// the film looks like and the other is what this copy of it is, and
    /// neither is a choice anybody makes. Keeping them in their own column
    /// leaves the whole of the space beside it for the choices.
    fn poster_column(self: &Rc<Self>, scale: f64) -> gtk::Box {
        let px = |base: f64| (base * scale).round() as i32;

        // Half the page's height, which is the proportion the comps are drawn
        // to - 550px of 1080 - and the reason this is not simply a size in
        // interface units. On a maximized ultrawide the page is held to a
        // 16:9 column far taller than the default window, and a poster fixed
        // in scaled pixels sits in the corner of it looking like a thumbnail
        // of itself. Bounded at both ends so a very short window still gets
        // something poster-shaped and a very tall one does not get a
        // billboard.
        //
        // Read when the page is built rather than tracked, so a window
        // resized while the menu is up keeps the size it was built at until
        // something rebuilds the page - which every trip into a chooser does.
        // The alternative is another custom widget, and this is a proportion
        // rather than a constraint: being a little out until the next rebuild
        // costs nothing that anyone can see.
        let height = self.poster_height(scale);
        // Two by three, which every poster in every library is drawn to.
        let width = height * 2.0 / 3.0;

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(12.0))
            .valign(gtk::Align::Start)
            .build();
        // Exactly as wide as the poster and no wider. Without this the column
        // is as wide as its widest *fact*, so a long codec name pushed the
        // whole page to the right and left a gap beside the poster that
        // belonged to nothing.
        column.set_size_request(width.round() as i32, -1);
        // Explicitly not expanding, and this is load-bearing. GTK propagates
        // `hexpand` up from children, so the poster picture asking to fill its
        // own frame quietly made this whole column an expanding one - and a
        // box then splits the spare width between it and the page beside it.
        // Measured: a column asking for 291px was being handed 567, which is
        // the gap that appeared to sit between the poster and the rows.
        column.set_hexpand(false);

        let frame = gtk::Box::builder()
            .css_classes(["tp-poster"])
            .halign(gtk::Align::Start)
            // Clipped, so a poster that is not exactly two by three is
            // cropped by the frame rather than allowed to reshape it.
            .overflow(gtk::Overflow::Hidden)
            .build();
        frame.set_size_request(width.round() as i32, height.round() as i32);

        match self.poster_art.borrow().clone() {
            Some(texture) => {
                // Fills the frame and keeps its shape, which is the same rule
                // the backdrop follows and the reason both are cropped rather
                // than letterboxed: a poster with bars down its sides reads
                // as a mistake. Real posters are two by three and are not
                // cropped at all; what this rescues is an episode thumbnail
                // or a scan that is a few pixels out.
                // Expanding is how it fills the frame: the widget draws a
                // texture and measures as nothing, so without this the frame
                // allocates it no width at all and the poster disappears.
                // The request stops at the column, which sets its own
                // `hexpand` explicitly - see there.
                let picture = crate::artwork::Artwork::poster();
                picture.set_texture(Some(texture));
                picture.set_hexpand(true);
                picture.set_vexpand(true);
                if self.fade_art.get() {
                    fade_in(&picture);
                }
                frame.append(&picture);
            }
            // Nothing found, which is the common case: of the 123 film folders
            // in the library this was written against, 28 carry artwork. The
            // mark is sized from the frame rather than from the interface, so
            // it keeps its place inside it at every window size.
            None => frame.append(&video_file_image(width * 0.42)),
        }
        *self.poster_frame.borrow_mut() = Some(frame.clone());
        column.append(&frame);

        let facts = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            // A few pixels in from the poster's edges, and the same on both:
            // the readings are ranged right, so without this they sat hard
            // against the frame above on one side while the names were inset
            // on the other.
            .margin_start(px(4.0))
            .margin_end(px(4.0))
            .build();
        // Two columns: what it is on the left, what it says on the right,
        // ranged against the poster's own right edge. As one run of text the
        // readings started at a different place on every line and there was
        // nothing to read down; against an edge they line up as a table, which
        // is what a column of measurements wants to be.
        for (name, value) in self.file_facts() {
            let line = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(px(8.0))
                .build();

            // Ellipsizing decides how a label *shrinks*; it does nothing to
            // what one asks for in the first place, which stays the full width
            // of its text. So a long reading here would widen the column past
            // the poster and push the whole page right. Both halves are capped
            // in what they may ask for; the width they actually get comes from
            // the poster above, and anything longer is cut with an ellipsis.
            let key = gtk::Label::new(Some(&format!("{name}:")));
            key.add_css_class("tp-fact");
            key.add_css_class("tp-fact-name");
            key.set_xalign(0.0);
            key.set_ellipsize(gtk::pango::EllipsizeMode::End);
            // Enough for the longest of them, "Resolution:". Capped at six it
            // cut every name to "Resol...", which is a label that has stopped
            // labelling anything. The pair still comes to well under the
            // poster's width, which is what the cap is protecting.
            key.set_max_width_chars(12);
            line.append(&key);

            let reading = gtk::Label::new(Some(&value));
            reading.add_css_class("tp-fact");
            reading.set_xalign(1.0);
            reading.set_ellipsize(gtk::pango::EllipsizeMode::End);
            reading.set_max_width_chars(12);
            // Pushes itself to the far edge. Safe only because the column
            // sets `hexpand` false outright - otherwise this request would
            // travel up and widen the whole left column, which is the fault
            // the poster picture caused before it.
            reading.set_hexpand(true);
            line.append(&reading);

            facts.append(&line);
        }
        column.append(&facts);
        column
    }

    /// What this copy of the film is, as opposed to what the film is.
    ///
    /// Only what is actually known: a remote source can be measured for none
    /// of it, and a line reading "Unknown" is worse than no line, so anything
    /// unanswered is simply absent. The order runs from what a viewer checks
    /// first to what they check last.
    fn file_facts(&self) -> Vec<(String, String)> {
        let details = self.details.borrow();
        [
            // Two lines rather than "1080p (H.264)". Together they are the
            // longest reading in the column, and the column is only as wide
            // as the poster - so as one line they were the thing that decided
            // how much room the picture got.
            ("Resolution", details.resolution()),
            ("Codec", details.codec()),
            ("Framerate", details.framerate()),
            ("Bitrate", details.bitrate()),
            (
                "Container",
                Some(details.container.clone()).filter(|c| !c.is_empty()),
            ),
            // Last, under the readings that describe the picture. It is the
            // one line here that says nothing about how the film will look.
            ("File size", details.filesize()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.to_string(), value)))
        .collect()
    }

    /// The title, the facts line, the summary, and what languages the file
    /// holds - everything above the choices.
    ///
    /// Everything except the summary keeps its natural height; the summary is
    /// held to three lines whether it has them or not, which is what stops the
    /// rows underneath moving between one film and the next.
    fn heading_block(self: &Rc<Self>, scale: f64) -> Vec<gtk::Widget> {
        let px = |base: f64| (base * scale).round() as i32;
        let details = self.details.borrow();
        let mut block: Vec<gtk::Widget> = Vec::new();

        let title = gtk::Label::new(Some(&details.title));
        title.add_css_class("tp-film-title");
        title.set_xalign(0.0);
        // One line, cut with an ellipsis. A filename with a release tag on it
        // is long and would happily take two - but the rows below sit at a
        // fixed distance from the top, and a title that is sometimes one line
        // and sometimes two is exactly the thing that moves them.
        title.set_wrap(false);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        block.push(title.upcast());

        // Year, running time, certificate, score, genres - whichever of them
        // anything answered. Spaced rather than punctuated between, which is
        // what the comps do and what keeps a line of three facts from reading
        // as a sentence.
        let mut facts: Vec<String> = Vec::new();
        // An episode says when it went out, in place of the year a film shows:
        // a date is what anybody would recognise an episode by, where a year
        // barely distinguishes it from the twenty others made alongside it.
        // Only where the sidecar gave one - an episode without a date falls
        // back to the year like anything else.
        match (&details.aired, details.year) {
            (aired, _) if !aired.is_empty() => facts.push(aired.clone()),
            (_, Some(year)) => facts.push(year.to_string()),
            _ => {}
        }
        // Beside the date rather than near the title: which episode this is
        // belongs with the facts about it, and the title is the episode's own
        // name. Two digits each, which is how everything else writes it and
        // what makes a column of them line up.
        facts.extend(
            details
                .episode
                .map(|(season, episode)| format!("S{season:02}E{episode:02}")),
        );
        facts.extend(details.runtime());
        if !details.certificate.is_empty() {
            facts.push(details.certificate.clone());
        }
        // A star, so a bare number is not left to be guessed at. Out of ten is
        // what every writer of this format stores and what the star implies,
        // and the sidecar is the only place it comes from - nothing is ever
        // fetched to produce it.
        //
        // The star is in a font TinePlayer ships, which the other marks in the
        // interface are not: see `INTERFACE_SYMBOLS` in
        // packaging/fonts/build-fonts.py before using any new symbol here.
        //
        // One decimal: the scrapers store three, and "8.235" is a precision
        // nobody asked for about an opinion.
        facts.extend(details.rating.map(|score| format!("★ {score:.1}")));
        if !details.genres.is_empty() {
            // Three at most. A scraper will happily list six, and the line has
            // the width of one line.
            facts.push(
                details
                    .genres
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !facts.is_empty() {
            let line = gtk::Label::new(Some(&facts.join("     ")));
            line.add_css_class("tp-film-facts");
            line.set_xalign(0.0);
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.set_margin_top(px(4.0));
            block.push(line.upcast());
        }

        // The summary, in a space of its own that is the same height whether
        // there is one or not. This is the only thing on the page held to a
        // fixed height, and it is the only one that needs to be: a plot runs
        // from nothing to a paragraph, and everything else here is one line or
        // absent. Reserving three lines for it is what keeps the rows below
        // from walking up and down the page as you step through a folder.
        let plot = gtk::Label::new(Some(&details.plot));
        plot.add_css_class("tp-film-plot");
        plot.set_xalign(0.0);
        plot.set_yalign(0.0);
        plot.set_wrap(true);
        plot.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        plot.set_lines(3);
        plot.set_ellipsize(gtk::pango::EllipsizeMode::End);
        plot.set_margin_top(px(12.0));
        // Filling the width it is given rather than a fraction of it. A
        // wrapping label asks for its whole text on one line, so it used to be
        // capped at twenty characters to stop it stretching the page - which
        // capped where it *wrapped* too, and left it running down the middle
        // of the column at about half width. Nothing needs to cap it now that
        // the poster column no longer expands and `Column` decides the page's
        // width outright.
        // -1, which is the value that means "no cap". Zero is a cap of zero
        // characters, and left it wrapping down the middle of the column.
        plot.set_max_width_chars(-1);
        plot.set_size_request(-1, px(PLOT_UNITS));
        block.push(plot.upcast());
        drop(details);

        // What is in the file, in languages rather than in track numbers.
        // The rows below say which track is going where; this says what there
        // was to choose from, which is the question someone asks before they
        // start opening choosers.
        //
        // Both lines are always drawn, even when there is nothing to put on
        // them. They are the two facts this application exists to act on, and
        // a line that comes and goes with the file moves everything under it.
        let spoken = (self.audio_languages(), self.subtitle_languages());
        let summary = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(1.0))
            .margin_top(px(14.0))
            .build();
        for (name, languages) in [("Audio", spoken.0), ("Subtitles", spoken.1)] {
            let line = gtk::Label::new(None);
            line.add_css_class("tp-fact");
            line.set_xalign(0.0);
            // Cut rather than wrapped: a second line here would push the rows
            // down on exactly the files that carry the most languages.
            line.set_ellipsize(gtk::pango::EllipsizeMode::End);

            line.set_markup(&summary_markup(name, &languages));
            summary.append(&line);
        }
        block.push(summary.upcast());
        block
    }

    /// Every language the file offers sound in, in the order the tracks are
    /// listed, with description called out.
    ///
    /// Deduplicated, because a file with four English tracks is offering one
    /// language four ways and a line reading "English, English, English,
    /// English" says less than one reading "English". A described track is a
    /// separate entry rather than a duplicate: it is a genuinely different
    /// thing to listen to, and for this application the most important entry
    /// on the line.
    fn audio_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for track in self.tracks.borrow().iter() {
            // A track that never said what it is still counts. Plenty of files
            // tag nothing at all - an AVI usually does not - and a line that
            // quietly left those out would claim a file had no soundtrack.
            let name = crate::languages::name_of_tag(&track.language).unwrap_or(UNKNOWN_LANGUAGE);
            let entry = match crate::probe::is_audio_description(&track.title) {
                true => format!("{name} (Described)"),
                false => name.to_string(),
            };
            if !named.contains(&entry) {
                named.push(entry);
            }
        }
        named
    }

    /// The same for subtitles, over everything on offer - streams inside the
    /// file and files sitting beside it alike.
    ///
    /// Both are things the viewer can pick, so a line that counted only the
    /// embedded ones would understate a folder full of `.srt` files, which is
    /// exactly the shape most of this library is in.
    fn subtitle_languages(&self) -> Vec<String> {
        let mut named: Vec<String> = Vec::new();
        for option in self.subtitle_options.borrow().iter() {
            // Labels arrive as a tag and possibly a title after it - "eng",
            // "eng — Forced", "en.hi" - and the language is the first word of
            // whichever shape it is.
            let tag = option
                .label()
                .split(" — ")
                .next()
                .unwrap_or_default()
                .split('.')
                .next()
                .unwrap_or_default();
            let name = crate::languages::name_of_tag(tag).unwrap_or(UNKNOWN_LANGUAGE);
            if !named.iter().any(|held| held == name) {
                named.push(name.to_string());
            }
        }
        named
    }

    /// The panel that offers the two ways to choose a video: the prompt, and
    /// a button for each.
    ///
    /// Shared by the screen shown when nothing is loaded and by the panel the
    /// browse button opens over a film, because they say the same thing and
    /// should not drift apart. `cancel` adds a third button and is what tells
    /// them apart: the empty screen has nowhere to go back to, while the panel
    /// is floating over a film that is still loaded.
    ///
    /// Returns the panel and its buttons, since what each one does depends on
    /// which screen asked for it. The Jellyfin button is absent when there is
    /// already a pairing, and the Cancel button when `cancel` is false.
    fn choose_source_panel(
        self: &Rc<Self>,
        scale: f64,
        cancel: bool,
    ) -> (
        gtk::Box,
        gtk::Button,
        gtk::Button,
        Option<gtk::Button>,
        Option<gtk::Button>,
    ) {
        let px = |base: f64| (base * scale).round() as i32;

        let middle = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(24.0))
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .vexpand(true)
            .build();
        // The mark only where the screen is otherwise empty. Over a film it
        // would be the application introducing itself in the middle of being
        // used.
        if !cancel {
            middle.append(&logo_image(scale * 2.2));
        }

        let prompt = gtk::Label::new(Some(
            "Drop a video file here, browse for a local file, or enter a URL",
        ));
        prompt.add_css_class("tp-empty-prompt");
        prompt.set_wrap(true);
        prompt.set_justify(gtk::Justification::Center);
        middle.append(&prompt);

        const BROWSE_ICON: &[u8] = include_bytes!("../data/ui/browse.png");
        const LINK_ICON: &[u8] = include_bytes!("../data/ui/link.png");
        const CONNECT_ICON: &[u8] = include_bytes!("../data/ui/connect.png");
        // Green in the file rather than tinted here, because a GTK image
        // cannot be recoloured - the same reason the muted soundtrack mark
        // fades with opacity instead of changing colour.
        const CONNECTED_ICON: &[u8] = include_bytes!("../data/ui/connected.png");

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::Center)
            .build();
        // Straight to the thing itself rather than to a menu row that opens
        // it: with one file to choose and two ways to choose it, a step in
        // between is a step for nothing.
        //
        // Each carries the mark of what it opens, and Browse carries the same
        // one the media page's button does - so the button on the page and the
        // button in the panel it opens are visibly the same errand.
        let browse = gtk::Button::new();
        browse.set_child(Some(&marked_face(
            marked_image(BROWSE_ICON, PLAY_MARK_PX * scale),
            "  Browse...",
        )));
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-action");
        name_it(&browse, "Browse");

        let address = gtk::Button::new();
        address.set_child(Some(&marked_face(
            marked_image(LINK_ICON, PLAY_MARK_PX * scale),
            "  Enter URL",
        )));
        address.add_css_class("tp-button");
        address.add_css_class("tp-action");
        name_it(&address, "Enter URL");

        buttons.append(&browse);
        buttons.append(&address);
        middle.append(&buttons);

        // Beneath the pair rather than beside them, for the reason the Cancel
        // button below is: those two choose a video and this does not. It
        // makes the television reachable from a phone, and the video is chosen
        // there afterwards.
        //
        // A button only while there is something to do. Once TinePlayer is
        // paired it is already a cast target whenever it is running, so the
        // button would offer to do something that is done - but the space says
        // so rather than going quiet, because "is this television reachable
        // from my phone?" is exactly the question this screen is looked at to
        // answer, and an absence is not an answer.
        let connect = match self.jellyfin_connected() {
            false => {
                let connect = gtk::Button::new();
                connect.set_child(Some(&marked_face(
                    marked_image(CONNECT_ICON, PLAY_MARK_PX * scale),
                    "  Connect to Jellyfin",
                )));
                connect.add_css_class("tp-button");
                connect.set_halign(gtk::Align::Center);
                name_it(&connect, "Connect to Jellyfin");
                middle.append(&connect);
                Some(connect)
            }
            // Stated, not offered. It is not focusable and takes no part in
            // the navigation: there is nothing to press, and a stop that does
            // nothing is worse than no stop at all.
            true => {
                let words = match self.jellyfin_server_label() {
                    Some(server) => format!("  Connected to Jellyfin ({server})"),
                    None => "  Connected to Jellyfin".to_string(),
                };
                // The same mark-and-words shape the buttons above use, so the
                // line reads as belonging with them rather than as a caption
                // that wandered in - but as a plain box, since there is
                // nothing to press.
                let connected =
                    marked_face(marked_image(CONNECTED_ICON, PLAY_MARK_PX * scale), &words);
                connected.add_css_class("tp-connected");
                connected.set_halign(gtk::Align::Center);
                connected.set_can_focus(false);
                name_it(&connected, &words);
                middle.append(&connected);
                None
            }
        };

        // On a row of its own beneath them rather than beside them: it is not
        // a third way to choose a video, and standing in line with two that
        // are made it look like one.
        let back = cancel.then(|| {
            let back = gtk::Button::with_label("Cancel");
            back.add_css_class("tp-button");
            back.set_halign(gtk::Align::Center);
            middle.append(&back);
            back
        });
        (middle, browse, address, connect, back)
    }

    /// The screen with no video on it: an invitation, and the two ways to
    /// accept it.
    ///
    /// Deliberately not the menu with everything greyed out. There is nothing
    /// to choose until there is a film to choose it for, and a page of dashes
    /// asks to be read before it can be dismissed. The gear stays, because
    /// this is where somebody who has just installed the application arrives
    /// and every setting they might need is behind it.
    fn build_empty_page(self: &Rc<Self>) -> gtk::Overlay {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(px(30.0))
            .margin_bottom(px(26.0))
            .margin_start(px(34.0))
            .margin_end(px(34.0))
            // Filled for the reason the media page is: `Column` does the
            // centering, and a box that centers itself as well collapses to
            // its contents and takes the footer's corner with it.
            .build();

        let (middle, browse, address, connect, _) = self.choose_source_panel(scale, false);
        content.append(&middle);

        {
            let app = self.clone();
            browse.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.browse_for_file();
            });
        }
        {
            let app = self.clone();
            address.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_paste_uri();
            });
        }
        if let Some(connect) = connect.as_ref() {
            let app = self.clone();
            connect.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_jellyfin_connect(ConnectFrom::Menu);
            });
        }

        // The same pair as the media page carries, in the same corner, so
        // they do not appear to move when a film is chosen.
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .halign(gtk::Align::End)
            .build();
        let (fullscreen, gear) = self.corner_buttons();
        footer.append(&gear);
        if let Some(fullscreen) = fullscreen.as_ref() {
            footer.append(fullscreen);
        }
        content.append(&footer);

        // Two rows rather than one run of four: the pair that chooses a
        // video, and the pair in the corner. Up and down move between the
        // rows, which is what they look like they should do - as one list they
        // fell through to GTK's own directional search, and it will not find a
        // button in the bottom corner from one in the middle of the page.
        let mut header = vec![browse.clone(), address];
        header.extend(connect);
        let mut footer = vec![gear];
        footer.extend(fullscreen);
        self.set_nav(None, &header, &footer);
        // And the arrows have to be sent somewhere. `wire_navigation` does
        // this for every screen built around a list, and this one is not - so
        // without it the keys reached a focused button, which does nothing
        // with them, and stopped there.
        for button in header.iter().chain(footer.iter()) {
            self.wire_arrows(button.upcast_ref());
        }
        // Deferred until the page is actually in the window. This is built
        // before `show_menu` installs it, and focus cannot be taken by a
        // widget that is not on screen yet - the same reason `settle_on`
        // waits for the map on the first screen of a session.
        match browse.is_mapped() {
            true => browse.grab_focus(),
            false => {
                browse.connect_map(|browse| {
                    browse.grab_focus();
                });
                true
            }
        };

        self.behind_artwork(&content)
    }

    /// The mark that closes the player, at the far end of the row.
    ///
    /// Where a window's own close button would be, and worth having because
    /// on a television there is no window: TinePlayer opens fullscreen with no
    /// titlebar, and quitting otherwise means knowing that Escape asks. It
    /// asks the same question Escape does rather than quitting outright.
    fn close_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../data/ui/close.png");

        let close = gtk::Button::new();
        close.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        close.add_css_class("tp-gear");
        close.set_focus_on_click(false);
        close.set_tooltip_text(Some("Close the player"));
        name_it(&close, "Close the player");
        {
            let app = self.clone();
            close.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_confirm_quit();
            });
        }
        close
    }

    /// The mark that opens the panel for choosing a different video.
    ///
    /// Drawn and placed like the settings and fullscreen marks rather than
    /// like the play button, because it is the same kind of thing: something
    /// the page can do, rather than the thing the page is for.
    fn browse_button(self: &Rc<Self>) -> gtk::Button {
        const ICON: &[u8] = include_bytes!("../data/ui/browse.png");

        let open = gtk::Button::new();
        open.set_child(Some(&marked_image(ICON, CORNER_MARK_PX * self.scale.get())));
        open.add_css_class("tp-gear");
        open.set_focus_on_click(false);
        open.set_tooltip_text(Some("Choose a video"));
        name_it(&open, "Choose a video");
        {
            let app = self.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.choose_video();
            });
        }
        open
    }

    /// The fullscreen mark and the gear, which sit together at the end of
    /// every footer on these two screens.
    ///
    /// Built here rather than twice, because the pair has three details worth
    /// not getting differently right in two places: the mark follows the
    /// window's own state, the gear carries the update badge, and neither
    /// takes focus from a click.
    fn corner_buttons(self: &Rc<Self>) -> (Option<gtk::Button>, gtk::Button) {
        // Maximize and restore rather than the usual fullscreen pair, which
        // is absent from the icon theme on both platforms and would draw the
        // missing-image glyph.
        let fullscreen = gtk::Button::new();
        fullscreen.set_child(Some(&fullscreen_image(
            self.window.is_fullscreen(),
            self.scale.get(),
        )));
        fullscreen.add_css_class("tp-gear");
        fullscreen.set_focus_on_click(false);
        fullscreen.set_tooltip_text(Some("Toggle fullscreen"));
        name_it(&fullscreen, "Toggle fullscreen");
        {
            let app = self.clone();
            fullscreen.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.toggle_fullscreen();
            });
        }
        {
            let weak = fullscreen.downgrade();
            let scale = self.scale.get();
            self.window.connect_fullscreened_notify(move |window| {
                if let Some(button) = weak.upgrade() {
                    button.set_child(Some(&fullscreen_image(window.is_fullscreen(), scale)));
                }
            });
        }

        let gear = gtk::Button::new();
        gear.set_child(Some(&settings_image(CORNER_MARK_PX * self.scale.get())));
        gear.add_css_class("tp-gear");
        gear.set_focus_on_click(false);
        gear.set_tooltip_text(Some("Settings"));
        name_it(&gear, "Settings");
        {
            let app = self.clone();
            gear.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.enter_settings();
            });
        }
        *self.update_badges.borrow_mut() = vec![gear.clone()];
        self.draw_update_badge();

        // Left out entirely when fullscreen is not this viewer's to change: a
        // button that declines to do the one thing it offers is worse than no
        // button.
        match self.locked_fullscreen {
            true => (None, gear),
            false => (Some(fullscreen), gear),
        }
    }

    /// Reads the artwork for the file just loaded, and redraws the page when
    /// it arrives.
    ///
    /// On a thread, because this is the part with a megabyte in it. A backdrop
    /// over a network share is long enough to be felt, and the page has to be
    /// on screen before it - a film's details held back until its wallpaper
    /// loads is the wrong thing to wait for.
    fn start_art_load(self: &Rc<Self>) {
        let (poster, backdrop) = {
            let details = self.details.borrow();
            (details.poster.clone(), details.backdrop.clone())
        };
        if poster.is_none() && backdrop.is_none() {
            return;
        }

        // What the artwork being read belongs to. A viewer who opens one film
        // and immediately opens another gets the second one's backdrop, not
        // whichever thread happened to finish last.
        let generation = self.art_generation.get();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let read = |art: Option<crate::metadata::Art>| {
                art.as_ref().and_then(crate::metadata::load_image)
            };
            let _ = sender.send((read(poster), read(backdrop)));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
            let (poster, backdrop) = match receiver.try_recv() {
                Ok(art) => art,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            if app.art_generation.get() != generation {
                return glib::ControlFlow::Break;
            }

            // Decoding happens here rather than on the thread: a GdkTexture
            // belongs to the main thread, and this is the only place that can
            // make one.
            let decode = |bytes: Option<Vec<u8>>| {
                let bytes = bytes?;
                match gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) {
                    Ok(texture) => Some(texture),
                    // Said out loud, because a poster that silently fails to
                    // appear looks like one that was never found - and the two
                    // want completely different things done about them.
                    Err(e) => {
                        eprintln!("Couldn't decode artwork: {e}");
                        None
                    }
                }
            };
            *app.poster_art.borrow_mut() = decode(poster);
            *app.backdrop_art.borrow_mut() = decode(backdrop);

            // Put into the page rather than rebuilding it, so that somebody
            // already choosing their tracks is left where they were.
            if *app.screen.borrow() == Screen::Menu {
                app.show_late_art();
            }
            glib::ControlFlow::Break
        });
    }

    // --- Choosers ------------------------------------------------------

    /// Enumerates the output devices on a thread, and calls `then` on the main
    /// thread if the answer differs from what the cache already held.
    ///
    /// For the popover, which opens immediately against whatever the cache
    /// already has and fills itself in when this lands. The probe is the one
    /// slow thing either menu does, and it is slow because it starts a device
    /// monitor - which asks every audio backend on the machine what it has.
    ///
    /// Polled rather than pushed, in the manner of the other threads here:
    /// nothing in this application may be touched from another thread, so the
    /// answer comes back through a channel and is picked up on this one.
    fn scan_devices_soon(self: &Rc<Self>, then: impl Fn(&Rc<Self>) + 'static) {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let names: Vec<String> = list_audio_output_devices()
                .map(|devices| {
                    devices
                        .iter()
                        .map(|device| device.display_name().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let _ = sender.send(names);
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(40), move || {
            let names = match receiver.try_recv() {
                Ok(names) => names,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                // Gone without an answer, which leaves nothing to show and no
                // reason to keep looking.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            app.device_scan.set(true);
            // Only when the answer is different. A refill re-selects the entry
            // in force, so running it against an unchanged list would throw
            // away wherever the viewer had arrowed to, a moment after they
            // got there.
            if *app.device_names.borrow() == names {
                return glib::ControlFlow::Break;
            }
            *app.device_names.borrow_mut() = names;
            then(&app);
            glib::ControlFlow::Break
        });
    }

    /// What a chooser offers, and which of it is already in force.
    ///
    /// Split out from the screen that shows it so a popover and a full page
    /// can offer exactly the same list. They differ in how they are put on
    /// screen and in nothing else, and two copies of this match is the way
    /// that stops being true.
    fn chooser_entries(self: &Rc<Self>, setting: Setting) -> Choices {
        // Entries are (display text, choice). `None` means the "None"
        // option, which every list offers except the primary device - an
        // output has to exist for anything to play.
        let mut entries: Vec<Choice> = Vec::new();
        let mut current: Option<usize> = None;
        let mut dividers: Vec<usize> = Vec::new();
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                if setting == Setting::SecondaryDevice {
                    entries.push(("None".to_string(), None));
                    // A rule under it. "None" here means "play nothing on a
                    // second output", which is a different kind of answer to
                    // the hardware listed below it - and the only list where
                    // this one is offered at all.
                    dividers.push(1);
                }
                let configured = {
                    let config = self.config.borrow();
                    if setting == Setting::PrimaryDevice {
                        config.primary_sink.clone()
                    } else {
                        config.secondary_sink.clone()
                    }
                };
                let devices = self.device_names.borrow();
                // Nothing found and nothing looked for yet: the caller is
                // showing this while the probe runs, so say so rather than
                // offering an empty list, which reads as "no outputs".
                if devices.is_empty() && !self.device_scan.get() {
                    entries.push(("Searching for outputs...".to_string(), None));
                }
                for (position, name) in devices.iter().enumerate() {
                    if configured.as_deref() == Some(name.as_str()) {
                        current = Some(position);
                    }
                    entries.push((name.clone(), Some(position)));
                }
            }
            Setting::Subtitles => {
                entries.push(("None".to_string(), None));
                // Under "None", and again above the row that leaves the film
                // to go looking on disk. What sits between is what the file
                // itself offers, and the two either side of it are answers of
                // a different kind.
                dividers.push(1);
                let chosen = self.subtitle.borrow().clone();
                for (position, option) in self.subtitle_options.borrow().iter().enumerate() {
                    if chosen.as_ref() == Some(&option.choice()) {
                        current = Some(position);
                    }
                    entries.push((subtitle_label(option), Some(position)));
                }
                // Last, after everything the video came with, the same way the
                // track lists offer one: a subtitle file from somewhere else
                // is the answer when what is wanted is not beside the film.
                dividers.push(entries.len());
                entries.push((
                    "Browse...".to_string(),
                    Some(self.subtitle_options.borrow().len()),
                ));
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                entries.push(("None".to_string(), None));
                dividers.push(1);
                let role = if setting == Setting::PrimaryTrack {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let chosen = *self.track_for(role).borrow();
                let file = self.file_for(role).borrow().clone();
                for (position, track) in self.tracks.borrow().iter().enumerate() {
                    if file.is_none() && chosen == Some(track.index) {
                        current = Some(position);
                    }
                    entries.push((describe_audio_track(track), Some(position)));
                }
                // Last, after everything inside the video: a separate file is
                // the answer when what you want is not in there at all, which
                // is most films with one soundtrack and a description track
                // downloaded beside them.
                let audio_file = entries.len() - 1;
                dividers.push(entries.len());
                if let Some(file) = file.as_ref() {
                    current = Some(audio_file);
                    entries.push((format!("Audio File: {}", file.label()), Some(audio_file)));
                } else {
                    entries.push(("Browse...".to_string(), Some(audio_file)));
                }
            }
            Setting::PrimaryLanguage | Setting::SecondaryLanguage => {
                let configured = {
                    let config = self.config.borrow();
                    if setting == Setting::PrimaryLanguage {
                        config.primary_language.clone()
                    } else {
                        config.secondary_language.clone()
                    }
                };
                current = language_position(configured.as_deref());
                // A rule under it, before the languages. The entry above is
                // not a language at all - it is the absence of a preference,
                // which leaves the choice to whatever the file offers first -
                // and run flush against Afrikaans it reads as one.
                dividers.push(1);
                // Worded exactly as the settings row shows it when unset, so
                // the list and the value it came from agree.
                entries.push((
                    if setting == Setting::PrimaryLanguage {
                        "First track".to_string()
                    } else {
                        "Second track".to_string()
                    },
                    None,
                ));
                for (position, (code, name, native, _)) in
                    crate::languages::LANGUAGES.iter().enumerate()
                {
                    entries.push((
                        crate::languages::menu_name(code, name, native),
                        Some(position),
                    ));
                }
            }
            Setting::SubtitleLanguage => {
                // The automatic choices first, then the languages, in one
                // list: they answer the same question, and following an
                // output is the answer most people want.
                let modes = crate::subtitles::MODES.len();
                let setting = self
                    .config
                    .borrow()
                    .subtitle_language
                    .clone()
                    .unwrap_or_else(|| crate::subtitles::DEFAULT_MODE.to_string());
                current = crate::subtitles::MODES
                    .iter()
                    .position(|(value, _)| *value == setting)
                    .or_else(|| {
                        crate::languages::LANGUAGES
                            .iter()
                            .position(|(code, _, _, _)| *code == setting)
                            .map(|position| modes + position)
                    });
                // Below "None", and again above the languages. What sits
                // between is the part worth choosing: following an output
                // tracks whatever is actually being heard, file by file, where
                // naming a language is a guess that holds until it does not.
                dividers.push(1);
                dividers.push(modes);
                for (position, (_, label)) in crate::subtitles::MODES.iter().enumerate() {
                    entries.push((label.to_string(), Some(position)));
                }
                for (position, (code, name, native, _)) in
                    crate::languages::LANGUAGES.iter().enumerate()
                {
                    entries.push((
                        crate::languages::menu_name(code, name, native),
                        Some(modes + position),
                    ));
                }
            }
            Setting::SubtitleFont => {
                let chosen = self
                    .config
                    .borrow()
                    .subtitle_font
                    .clone()
                    .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string());
                current = SUBTITLE_FONTS.iter().position(|font| *font == chosen);
                for (position, font) in SUBTITLE_FONTS.iter().enumerate() {
                    entries.push((font.to_string(), Some(position)));
                }
            }
            Setting::KodiType(index) => {
                use crate::kodi_setup::Registration;
                let Some((state, configured)) =
                    self.with_kodi_setup(index, |setup| (setup.state, setup.is_configured()))
                else {
                    return Choices {
                        entries,
                        current,
                        dividers,
                    };
                };
                current = Registration::ALL.iter().position(|option| *option == state);
                for (position, option) in Registration::ALL.iter().enumerate() {
                    entries.push((option.choice(configured).to_string(), Some(position)));
                }
                // A rule above removal, and only when it is a removal. The
                // other two entries are states to be in and this one is a
                // thing to do, which is the same reason the secondary device
                // list rules off its "None".
                if configured {
                    dividers.push(Registration::ALL.len() - 1);
                }
            }
            Setting::KodiHandover(index) => {
                let plays = self.with_kodi(index, |setup| setup.play);
                current = Some(usize::from(plays));
                for (position, choice) in HANDOVER.iter().enumerate() {
                    entries.push((choice.to_string(), Some(position)));
                }
            }
        }

        Choices {
            entries,
            current,
            dividers,
        }
    }

    /// Puts the current screen's navigation aside, so something on top of it
    /// can have the keyboard for a while.
    ///
    /// The application keeps one navigation model for the screen on display -
    /// which list the arrows drive, which buttons sit above and below it. A
    /// popover is the first thing that is neither a screen nor part of one: it
    /// needs the arrows while it is open and has to give them back exactly as
    /// it found them, because the page underneath is still there and still
    /// where the viewer will be returned to.
    fn take_nav(&self) -> NavState {
        NavState {
            list: self.nav_list.borrow().clone(),
            header: self.nav_header.borrow().clone(),
            footer: self.nav_footer.borrow().clone(),
            header_entry: self.nav_header_entry.borrow().clone(),
            stops: self.nav_stops.borrow().clone(),
            copy_root: self.copy_root.borrow().clone(),
        }
    }

    /// Gives the screen underneath its navigation back.
    fn put_nav(&self, state: NavState) {
        *self.nav_list.borrow_mut() = state.list;
        *self.nav_header.borrow_mut() = state.header;
        *self.nav_footer.borrow_mut() = state.footer;
        *self.nav_header_entry.borrow_mut() = state.header_entry;
        *self.nav_stops.borrow_mut() = state.stops;
        *self.copy_root.borrow_mut() = state.copy_root;
    }

    /// A selector over the row that opened it, rather than a page that
    /// replaces everything.
    ///
    /// The same entries a full chooser would list, from `chooser_entries`, in
    /// a popover anchored to the row. The page stays visible behind it, which
    /// is the point: what you are choosing for is still on screen, and the
    /// same widget will work over a playing film when these are wanted during
    /// playback.
    fn show_selector(self: &Rc<Self>, setting: Setting, anchor: &gtk::ListBoxRow) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;
        // A device list is not ready when the popover opens - it is being
        // probed on a thread - so the entries are held rather than captured,
        // and the rows are filled by something that can be run twice.
        let entries: Rc<RefCell<Vec<Choice>>> = Rc::new(RefCell::new(Vec::new()));
        let (scroller, list) = scrolling_list();
        let fill: Rc<Fill> = {
            let entries = entries.clone();
            let list = list.clone();
            Rc::new(move |app: &Rc<Self>| {
                let Choices {
                    entries: fresh,
                    current,
                    dividers,
                } = app.chooser_entries(setting);
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                for (text, _) in &fresh {
                    let entry = chooser_row(text);
                    // Right-aligned, unlike the same row on a full chooser
                    // page. The popover opens against a row whose value sits
                    // on the right, and the choices are that value's
                    // alternatives - so they read as a column under it rather
                    // than as a list that starts somewhere else.
                    entry.set_xalign(1.0);
                    append_named(&list, &entry, text);
                }
                // Opened on whatever is already in force. Grabbing focus
                // scrolls it into view, which is what a long list needs.
                let opening = fresh
                    .iter()
                    .position(|(_, choice)| *choice == current)
                    .unwrap_or(0) as i32;
                *entries.borrow_mut() = fresh;
                // A rule above the entries that begin a group. A header rather
                // than a row of its own, for the reason the media page's group
                // headings give: headers sit outside the selection model and
                // the focus chain, so a rule cannot be landed on. Set on every
                // fill, since the rows it describes are rebuilt each time.
                list.set_header_func(move |row, _| {
                    match dividers.contains(&(row.index() as usize)) {
                        true => {
                            row.set_header(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)))
                        }
                        false => row.set_header(None::<&gtk::Widget>),
                    }
                });
                if let Some(row) = list.row_at_index(opening) {
                    row.add_css_class("tp-current");
                    list.select_row(Some(&row));
                    settle_on(&row);
                } else {
                    // Nothing to settle on, but the claim is still worth
                    // making: it supersedes any settling left pending by the
                    // row this popover opened over, which would otherwise come
                    // due and pull the focus back out to the page.
                    claim_settling();
                    list.grab_focus();
                }
            })
        };
        fill(self);
        // As wide as its longest entry, between a floor and a ceiling.
        //
        // `propagate_natural_width` is the part that does the work, and its
        // absence is what made the first attempt at this a narrow column of
        // "...": without it a scrolled window's natural width *is* its
        // `min-content-width`, so the popover opened at the floor no matter
        // what was in it. Ellipsizing entries make that failure look like a
        // sizing bug rather than a missing property, because ellipsizing is
        // what lets a label shrink that far in the first place - it lowers the
        // minimum width and leaves the natural width alone, which is exactly
        // the number wanted here.
        // Fixed for a device list, which opens holding a placeholder and is
        // filled in a moment later: sized to its contents it would open narrow
        // and jump wider under the pointer. The row's own width is a stable
        // number and a generous one, and device names are long.
        let devices = matches!(setting, Setting::PrimaryDevice | Setting::SecondaryDevice);
        // Two different questions. Every opening of a device list goes and
        // looks again, because hardware is plugged in and unplugged between
        // openings and a cache that is never refreshed is only a stale list.
        // Only the first opening has nothing to show while that happens.
        let waiting = devices && !self.device_scan.get();
        if waiting {
            scroller.set_size_request(anchor.width().max(px(SELECTOR_MIN_WIDTH)), -1);
        }
        scroller.set_propagate_natural_width(true);
        scroller.set_min_content_width(px(SELECTOR_MIN_WIDTH));
        // A ceiling as well, for the one entry that has no natural length: an
        // audio file is named by its path, and some of those are a page wide.
        scroller.set_max_content_width(px(SELECTOR_MAX_WIDTH));
        // Tall lists scroll rather than growing past the window - the language
        // list is two hundred entries. Short ones stay short.
        scroller.set_max_content_height(px(SELECTOR_HEIGHT));
        scroller.set_propagate_natural_height(true);

        let popover = gtk::Popover::builder()
            .child(&scroller)
            .position(gtk::PositionType::Bottom)
            // No arrow: this is a panel of choices, not a speech bubble, and
            // the anchor is already obvious from where it opens.
            .has_arrow(false)
            .build();
        popover.add_css_class("tp-selector");
        popover.set_parent(anchor);
        // What the popover will be: its contents, plus the padding
        // `.tp-selector > contents` puts around them. Measured on the child
        // for the reason `aim` gives - the popover itself measures zero.
        let (_, content_width, _, _) = scroller.measure(gtk::Orientation::Horizontal, -1);
        aim_right(&popover, anchor, content_width + px(SELECTOR_PAD) * 2);

        // The arrows belong to the popover while it is up, and to the page
        // again the moment it is not.
        let saved = self.take_nav();
        self.wire_navigation(&list, &[], &[]);
        {
            let app = self.clone();
            let saved = std::cell::RefCell::new(Some(saved));
            popover.connect_closed(move |popover| {
                if let Some(saved) = saved.borrow_mut().take() {
                    app.put_nav(saved);
                }
                // A popover parented by hand has to be unparented by hand, or
                // it outlives the row and GTK complains when that row goes.
                if popover.parent().is_some() {
                    popover.unparent();
                }
            });
        }

        {
            let app = self.clone();
            let entries = entries.clone();
            let popover = popover.clone();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let choice = match entries.borrow().get(row.index() as usize) {
                    Some((_, choice)) => *choice,
                    None => return,
                };
                popover.popdown();
                // After the popover has gone, not during. Applying a choice
                // rebuilds the page underneath, which destroys the row this is
                // anchored to - and doing that while it is still up is how a
                // widget ends up parented to something that no longer exists.
                //
                // Rebuilt rather than patched because a choice can change more
                // than the row it was made on: picking a second output fills
                // in the rows below it, and clearing one empties them.
                let app = app.clone();
                let over = *app.screen.borrow();
                glib::idle_add_local_once(move || {
                    if app.apply_choice(setting, choice) {
                        return;
                    }
                    match over {
                        Screen::Settings => app.show_settings(),
                        _ => app.show_menu(),
                    }
                });
            });
        }

        popover.popup();
        // Deliberately not re-aimed once it is open. Correcting against
        // `popover.width()` after the fact was tried and is wrong twice over:
        // an allocated popover measures wider than its contents, because the
        // allocation carries the margin the shadow is drawn into, so the
        // correction moved a popover that had opened in the right place about
        // fifty pixels to the left - and it did it a frame late, in full view.
        //
        // Selecting the current entry is `fill`'s job, and it is run again
        // here so that it happens with the list allocated: scrolling a row
        // into view needs a size, and inside a popover there is none until it
        // has been shown.
        fill(self);

        // The outputs, once something has gone and found them. The popover is
        // already up with "Searching for outputs..." in it, and fills in when
        // this lands - which is the whole point of doing it this way, since
        // the probe is slow enough on the main thread to read as the menu
        // being stuck.
        if devices {
            let fill = fill.clone();
            // Only if it is still open. Refilling a popover that has been
            // dismissed would be pointless, and worse than pointless: it ends
            // by focusing the entry in force, which would take focus off the
            // page the viewer went back to.
            let popover = popover.downgrade();
            self.scan_devices_soon(move |app| {
                if popover
                    .upgrade()
                    .is_some_and(|popover| popover.is_visible())
                {
                    fill(app);
                }
            });
        }
    }

    fn wire_navigation(
        self: &Rc<Self>,
        list: &gtk::ListBox,
        header: &[gtk::Button],
        footer: &[gtk::Button],
    ) {
        self.set_nav(Some(list), header, footer);
        announce_selection(list);

        // Every arrow key goes through move_selection, which already knows
        // where the focus is and what should happen at each boundary - it is
        // what the gamepad and the page keys have always used.
        //
        // It has to, now that rows are not focusable: GtkListBox moves the
        // cursor by moving focus between rows, and with nothing in the list
        // able to take focus that does nothing at all. Capture phase so this
        // runs before the list's own bindings rather than after they have
        // swallowed the key.
        self.wire_arrows(list.upcast_ref());
        for button in header.iter().chain(footer.iter()) {
            self.wire_arrows(button.upcast_ref());
        }

        // Tabbing into a list has to land somewhere. GTK selects nothing on
        // its own now that no row takes focus, which left the list holding
        // focus with nothing highlighted and the arrow keys apparently dead.
        {
            let list_weak = list.downgrade();
            let controller = gtk::EventControllerFocus::new();
            controller.connect_enter(move |_| {
                let Some(list) = list_weak.upgrade() else {
                    return;
                };
                if list.selected_row().is_some() {
                    return;
                }
                let first = (0..).find(|index| {
                    list.row_at_index(*index)
                        .is_none_or(|row| row.is_sensitive())
                });
                if let Some(row) = first.and_then(|index| list.row_at_index(index)) {
                    list.select_row(Some(&row));
                }
            });
            list.add_controller(controller);
        }
    }

    /// Writes the current track pair against the current file, so a choice
    /// survives even if the file is never played.
    fn remember_tracks(&self) {
        let Some(key) = self.storage_key() else {
            return;
        };
        crate::config::save_tracks(
            &key,
            *self.primary_track.borrow(),
            *self.secondary_track.borrow(),
            self.subtitle.borrow().clone(),
            self.saved_path(Role::Primary),
            self.saved_path(Role::Secondary),
        );
    }

    /// The audio file chosen for an output, as something worth writing down.
    ///
    /// Only a local path: a file reached by URL is not ours to promise will
    /// still be there, and rebuilding one from a saved string is a different
    /// question from finding a file again.
    fn saved_path(&self, role: Role) -> Option<std::path::PathBuf> {
        self.file_for(role)
            .borrow()
            .as_ref()
            .and_then(|file| file.local().map(|path| path.to_path_buf()))
    }

    /// What the menu shows against the Subtitles row.
    fn describe_subtitle(&self) -> String {
        let Some(chosen) = self.subtitle.borrow().clone() else {
            return "None".to_string();
        };
        self.subtitle_options
            .borrow()
            .iter()
            .find(|option| option.choice() == chosen)
            .map(subtitle_label)
            .unwrap_or_else(|| "None".to_string())
    }

    /// Returns whether it has already moved to another screen, in which case
    /// the caller must not navigate on top of it.
    fn apply_choice(self: &Rc<Self>, setting: Setting, choice: Option<usize>) -> bool {
        match setting {
            Setting::PrimaryDevice | Setting::SecondaryDevice => {
                // From the cache the list was built from, not a fresh probe.
                // This used to enumerate the hardware all over again just to
                // turn the row that was pressed back into a name, which put a
                // second pause between the press and anything happening.
                let picked = {
                    let names = self.device_names.borrow();
                    choice.and_then(|index| names.get(index).cloned())
                };

                let mut cleared_secondary = false;
                {
                    let mut config = self.config.borrow_mut();
                    if setting == Setting::PrimaryDevice {
                        // The primary output cannot be cleared: without one
                        // there is nothing to play through.
                        if picked.is_none() {
                            return false;
                        }
                        config.primary_sink = picked;
                    } else {
                        config.secondary_sink = picked;
                        // A secondary track without a device to play it on is
                        // meaningless, so clear it alongside - and a separate
                        // audio file the same way, which was missed. Left set,
                        // it is still a choice the menu shows and the pipeline
                        // tries to honor, against an output that no longer
                        // exists.
                        if config.secondary_sink.is_none() {
                            *self.secondary_track.borrow_mut() = None;
                            *self.secondary_file.borrow_mut() = None;
                            cleared_secondary = true;
                        }
                    }
                    config.capture_display_session();
                    if let Err(e) = config.save() {
                        eprintln!("Failed to save config: {e}");
                    }
                }

                // Interface sounds follow the primary output, so they play
                // where the user is listening. Rebuilt on change rather
                // than only at startup, which previously meant a restart
                // before a newly chosen device took effect.
                if cleared_secondary {
                    self.remember_tracks();
                    // The file went with the device, so its alignment goes too.
                    self.load_baselines();
                }

                if setting == Setting::PrimaryDevice {
                    let (enabled, device) = {
                        let config = self.config.borrow();
                        (config.sounds, config.primary_sink.clone())
                    };
                    *self.sounds.borrow_mut() = Sounds::new(enabled, device);
                }
            }
            Setting::PrimaryLanguage | Setting::SecondaryLanguage => {
                let picked = choice
                    .and_then(|index| crate::languages::LANGUAGES.get(index))
                    .map(|(code, _, _, _)| code.to_string());
                let mut config = self.config.borrow_mut();
                if setting == Setting::PrimaryLanguage {
                    config.primary_language = picked;
                } else {
                    config.secondary_language = picked;
                }
                let _ = config.save();
            }
            Setting::SubtitleLanguage => {
                let modes = crate::subtitles::MODES.len();
                let picked = choice.map(|index| match index.checked_sub(modes) {
                    Some(language) => crate::languages::LANGUAGES[language].0.to_string(),
                    None => crate::subtitles::MODES[index].0.to_string(),
                });
                let mut config = self.config.borrow_mut();
                config.subtitle_language = picked;
                let _ = config.save();
            }
            Setting::SubtitleFont => {
                let mut config = self.config.borrow_mut();
                config.subtitle_font = choice
                    .and_then(|index| SUBTITLE_FONTS.get(index))
                    .map(|font| font.to_string());
                let _ = config.save();
            }
            Setting::KodiType(index) => return self.choose_kodi_type(index, choice),
            Setting::KodiHandover(index) => {
                let (Some(chosen), Some(setup)) = (choice, self.kodi_at(index)) else {
                    return false;
                };
                // Everything else about the entry stays as it is: this rewrites
                // our own element with one argument different. No confirmation
                // and no backup - by the time this row can be worked at all,
                // the entry being edited is one we wrote.
                let state = setup.state;
                if self.write_kodi(&setup, state, None, chosen == 1) {
                    self.return_to_kodi_settings();
                }
                return true;
            }
            Setting::Subtitles => {
                let options = self.subtitle_options.borrow();
                // The row after the last option is the browse one, which opens
                // a screen instead of settling anything here.
                if choice == Some(options.len()) {
                    drop(options);
                    self.browse_for_subtitle();
                    return true;
                }
                let picked = choice
                    .and_then(|index| options.get(index))
                    .map(|o| o.choice());
                drop(options);
                *self.subtitle.borrow_mut() = picked;
                // Choosing a subtitle is asking to see it, whatever the
                // toggle was doing for the last one.
                self.subtitles_hidden.set(false);
                self.remember_tracks();
            }
            Setting::PrimaryTrack | Setting::SecondaryTrack => {
                let role = if setting == Setting::PrimaryTrack {
                    Role::Primary
                } else {
                    Role::Secondary
                };
                let count = self.tracks.borrow().len();
                // The row after the last track is the audio file one, which
                // opens the browser instead of settling anything here.
                if choice == Some(count) {
                    self.browse_for_audio(role);
                    return true;
                }

                let tracks = self.tracks.borrow();
                let picked = choice.and_then(|index| tracks.get(index)).map(|t| t.index);
                drop(tracks);
                *self.track_for(role).borrow_mut() = picked;
                // Choosing anything inside the video, including None, is
                // choosing not to use a separate file on that output.
                *self.file_for(role).borrow_mut() = None;
                self.remember_tracks();
                // The pairing is gone, so the alignment measured for it has to
                // go with it. A baseline left behind is applied to a track
                // inside the video, which shares the video's timeline and needs
                // no correction - and a large one silences that output
                // outright. Measured on the Pi 2026-08-10: -830ms against an
                // embedded track produced no audio at all, while -300ms and
                // +830ms both played, so it is pulling the audio further
                // forward than the pipeline can deliver.
                self.load_baselines();
            }
        }
        false
    }

    // --- File selection ------------------------------------------------

    fn open_file_chooser(self: &Rc<Self>, start: &std::path::Path) {
        // FileChooserNative rather than FileDialog: the latter needs GTK
        // 4.10, above this project's 4.6 baseline. It also gives the real
        // system file dialog on each platform.
        // Which errand this is on, decided the same way the built-in browser
        // decides it, so the two always agree about what is being chosen.
        let errand = self.errand.get();
        let chooser = gtk::FileChooserNative::new(
            Some(match errand {
                Errand::Audio(_) => "Choose an audio file",
                Errand::Subtitle => "Choose a subtitle file",
                _ => "Choose a video",
            }),
            Some(&self.window),
            gtk::FileChooserAction::Open,
            Some("Open"),
            Some("Cancel"),
        );

        // The pipeline typefinds rather than assuming a container, so this
        // list is about not cluttering the dialog with non-video files, not
        // about what will actually play. Anything GStreamer can demux works,
        // which is why "All files" stays available below.
        let filter = gtk::FileFilter::new();
        let (name, extensions) = if errand == Errand::Subtitle {
            ("Subtitle files", &crate::subtitles::EXTENSIONS[..])
        } else if matches!(errand, Errand::Audio(_)) {
            ("Audio files", crate::browser::AUDIO_EXTENSIONS)
        } else {
            ("Video files", &crate::browser::VIDEO_EXTENSIONS[..])
        };
        filter.set_name(Some(name));
        for extension in extensions {
            // Case-insensitive by hand: GTK's pattern matching is not, and
            // ".MKV" off a camera or an old disc is common enough to matter.
            filter.add_pattern(&format!("*.{extension}"));
            filter.add_pattern(&format!("*.{}", extension.to_uppercase()));
        }
        chooser.add_filter(&filter);
        open_at(&chooser, start);

        let all = gtk::FileFilter::new();
        all.set_name(Some("All files"));
        all.add_pattern("*");
        chooser.add_filter(&all);

        let app = self.clone();
        // Where this was opened from, so canceling returns there rather than
        // dropping to the menu. Reached from the browser, canceling should
        // leave you in the folder you were looking at.
        let from_browser = *self.screen.borrow() == Screen::Browser;
        let folder = self.config.borrow().last_folder.clone();

        // Held by the closure so the dialog outlives this function; a
        // dropped FileChooserNative closes before the user can answer.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|f| f.path()))
                .flatten();
            held.borrow_mut().take();

            match chosen {
                // A subtitle or a soundtrack for the video already loaded,
                // rather than a video to load.
                Some(path) if errand == Errand::Subtitle => {
                    app.set_subtitle_file(&path);
                    app.show_menu();
                }
                Some(path) if matches!(errand, Errand::Audio(_)) => {
                    app.set_audio_file(&path);
                    app.show_menu();
                }
                // A file was picked, so the menu is where to go next either
                // way.
                Some(path) => {
                    let source = Source::File(path);
                    match app.set_file(&source) {
                        Ok(()) => app.show_menu(),
                        Err(e) => app.show_source_error(&source, &e, false),
                    }
                }
                None => match folder.as_deref().filter(|_| from_browser) {
                    Some(folder) => app.show_browser(folder, None),
                    None => app.show_menu(),
                },
            }
        });
        chooser.show();
    }

    /// Probes the file and chooses tracks for it.
    ///
    /// A file played before comes back with the tracks it was played with;
    /// otherwise the first track goes to the primary output and a different
    /// one to the secondary, which is the whole point of the application.
    fn set_file(self: &Rc<Self>, source: &Source) -> Result<(), String> {
        match crate::probe::probe_media(source) {
            Ok(media) => self.apply_media(source, media),
            Err(e) => {
                eprintln!("Couldn't read {}: {e}", source.uri());
                self.forget_file();
                Err(e)
            }
        }
    }

    /// Drops everything that described the file that was loaded.
    fn forget_file(&self) {
        *self.details.borrow_mut() = Default::default();
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        // Anything still being read for the file being forgotten is now for
        // the wrong one, and this is what tells it so.
        self.art_generation.set(self.art_generation.get() + 1);
        *self.tracks.borrow_mut() = Vec::new();
        *self.subtitle_options.borrow_mut() = Vec::new();
        *self.primary_track.borrow_mut() = None;
        *self.secondary_track.borrow_mut() = None;
        *self.subtitle.borrow_mut() = None;
        *self.file.borrow_mut() = None;
        self.duration_s.set(0.0);
    }

    /// Takes up a probed source: which tracks to start on, which subtitle,
    /// and what to show in the menu.
    ///
    /// Separate from the probing so that a caller which probed on a thread,
    /// rather than making the interface wait for it, has somewhere to hand
    /// the result back on the main thread.
    fn apply_media(
        self: &Rc<Self>,
        source: &Source,
        media: crate::probe::Media,
    ) -> Result<(), String> {
        // A different video starts with its subtitles showing, whatever the
        // last one was left doing.
        self.subtitles_hidden.set(false);
        // Kodi's one video player slot is necessarily this playback while it
        // waits for us, but a session started by hand with --kodi could attach
        // to a *different* external player's item. Lengths agreeing is a cheap
        // guard against that, and against writing progress onto the wrong film.
        if let Some(runtime) = self
            .kodi_item
            .borrow()
            .as_ref()
            .map(|item| item.runtime_s)
            .filter(|runtime| *runtime > 0)
            && media.duration_ns > 0
        {
            let ours = media.duration_ns / 1_000_000_000;
            if ours.abs_diff(runtime) > 5 {
                eprintln!(
                    "Kodi reports a {runtime}s item but this source is {ours}s;                      ignoring what it said and keeping local positions."
                );
                *self.kodi_item.borrow_mut() = None;
            }
        }

        // A video that did not come from Jellyfin must not wear the details of
        // one that did. Cleared here rather than when playback ends, because
        // `begin_playback` stops the previous playback on its way in - which
        // wiped the item a moment before anything could be reported about it.
        let cast = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .is_some_and(|item| source.uri().contains(&item.id));
        if !cast {
            *self.jellyfin_item.borrow_mut() = None;
        }

        // What the page shows about the file, from the sidecar beside it and
        // the container's own tags. Cheap - a small file and a few `is_file`
        // calls - and the artwork behind whatever it found is read separately,
        // on a thread, because that is the part with a megabyte in it.
        //
        // Taken here rather than further down because the lists below are
        // moved out of `media`, and this reads the whole of it.
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        *self.details.borrow_mut() =
            crate::metadata::resolve(source, &media, beside, &self.launcher_title());

        let duration_ns = media.duration_ns;
        let tracks = media.audio;
        // What the library holds beside the video, which only a cast video has.
        // These are files on the server rather than streams in the container,
        // so they are offered alongside the embedded ones rather than counted
        // among them.
        let library = self
            .jellyfin_item
            .borrow()
            .as_ref()
            .map(|item| item.streams.subtitle_options())
            .unwrap_or_default();
        let mut options = crate::subtitles::options(source.local(), &media.subtitles, &library);

        let (primary_language, secondary_language, subtitle_language, described) = {
            let config = self.config.borrow();
            (
                config.primary_language.clone(),
                config.secondary_language.clone(),
                config.subtitle_language.clone(),
                (
                    config.primary_audio_description,
                    config.secondary_audio_description,
                ),
            )
        };
        let describes =
            |track: &crate::probe::AudioTrack| crate::probe::is_audio_description(&track.title);

        // What ordinary selection is allowed to pick from: everything except
        // the described tracks, which are only ever chosen by asking for them.
        // Without this, a file whose first English track happens to be the
        // described one would hand narration to someone who never wanted it.
        //
        // Unless description is all there is. A file with nothing else would
        // otherwise start silent, which reads as the player being broken
        // rather than as a preference being honored.
        let pool: Vec<&crate::probe::AudioTrack> = {
            let plain: Vec<_> = tracks.iter().filter(|track| !describes(track)).collect();
            if plain.is_empty() {
                tracks.iter().collect()
            } else {
                plain
            }
        };

        // First track in the preferred language, if one was named.
        let by_language = |preferred: &Option<String>| -> Option<u32> {
            let code = preferred.as_deref()?;
            pool.iter()
                .find(|track| crate::languages::matches(&track.language, code))
                .map(|track| track.index)
        };
        // A described track for an output that asked for one. Not finding one
        // is not a failure - most files have none - so it falls back to the
        // ordinary choice rather than leaving the output silent.
        //
        // A named language is a hard requirement, not a preference to relax:
        // description narrated in a language you do not speak is worse than no
        // description at all, so the fallback is the right language undescribed
        // rather than the wrong language described.
        let described_track = |want: bool, preferred: &Option<String>| -> Option<u32> {
            if !want {
                return None;
            }
            let Some(code) = preferred.as_deref() else {
                return tracks
                    .iter()
                    .find(|track| describes(track))
                    .map(|track| track.index);
            };
            tracks
                .iter()
                .find(|track| describes(track) && crate::languages::matches(&track.language, code))
                // Then one whose language is not stated. Unknown is not the
                // same as wrong: a track tagged for another language is
                // rejected, but plenty of description carries no tag at all -
                // the tool most people use to add one sets a title and no
                // language - and refusing those would mean finding nothing in
                // the commonest case of all.
                .or_else(|| {
                    tracks
                        .iter()
                        .find(|track| describes(track) && !crate::languages::known(&track.language))
                })
                .map(|track| track.index)
        };

        // Keyed on the video being loaded rather than the one still current,
        // which is not this one until the end of this function.
        let saved = crate::config::load_resume(&self.storage_key_for(source))
            .and_then(|resume| resume.tracks);
        let (primary, secondary) = match saved.clone() {
            // A saved None is a real choice ("no audio on that output"), so a
            // saved pair is taken as it stands rather than filled in.
            Some(choice) => (choice.primary, choice.secondary),
            // Otherwise the preferred languages decide, falling back to the
            // old behavior of the first track and a different one.
            None => (
                described_track(described.0, &primary_language)
                    .or_else(|| by_language(&primary_language))
                    .or_else(|| pool.first().map(|t| t.index)),
                described_track(described.1, &secondary_language)
                    .or_else(|| by_language(&secondary_language))
                    .or_else(|| pool.get(1).map(|t| t.index)),
            ),
        };
        // The file may have been re-encoded since it was last played.
        let known = |choice: Option<u32>| choice.filter(|i| tracks.iter().any(|t| t.index == *i));

        *self.primary_track.borrow_mut() = known(primary);
        *self.secondary_track.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            known(secondary)
        } else {
            // Without a device to play it on, holding a secondary track only
            // produces a pipeline that fails to build.
            None
        };
        // A separate audio file, kept only if it is still where it was. One
        // that has been deleted, renamed, or is on a drive not mounted today
        // falls back to the track underneath it rather than failing when play
        // is pressed - the same rule the subtitle below follows.
        let still_there = |path: Option<&std::path::PathBuf>| {
            path.filter(|path| path.exists())
                .map(|path| Source::File(path.clone()))
        };
        *self.primary_file.borrow_mut() = still_there(
            saved
                .as_ref()
                .and_then(|choice| choice.primary_file.as_ref()),
        );
        *self.secondary_file.borrow_mut() = if self.config.borrow().secondary_sink.is_some() {
            still_there(
                saved
                    .as_ref()
                    .and_then(|choice| choice.secondary_file.as_ref()),
            )
        } else {
            None
        };

        // Only kept if it still resolves: an embedded stream the file no
        // longer has, or a subtitle file since deleted, quietly reverts to
        // none rather than failing when play is pressed.
        let subtitle = match saved {
            Some(choice) => choice.subtitle,
            // Follows whichever audio is actually going to each output, not
            // the language preference: the preference may have found nothing,
            // and what is being heard is what subtitles have to match.
            None => {
                let language_of = |index: Option<u32>| {
                    index.and_then(|index| {
                        tracks
                            .iter()
                            .find(|track| track.index == index)
                            .map(|track| track.language.as_str())
                    })
                };
                crate::subtitles::automatic(
                    &crate::subtitles::Auto::parse(
                        subtitle_language
                            .as_deref()
                            .unwrap_or(crate::subtitles::DEFAULT_MODE),
                    ),
                    &options,
                    language_of(known(primary)),
                    language_of(known(secondary)),
                )
            }
        };
        // A file chosen by hand is not beside the video, so nothing above
        // found it. Put it back before the check below, or the choice would be
        // dropped as unrecognised every time the file was loaded again - and
        // only if it is still on disk, since a remembered path can outlive the
        // file it names.
        if let Some(crate::subtitles::SubtitleChoice::File(path)) = subtitle.as_ref()
            && path.is_file()
        {
            options.push(crate::subtitles::chosen_file(path));
        }
        *self.subtitle.borrow_mut() =
            subtitle.filter(|choice| options.iter().any(|option| option.choice() == *choice));
        *self.subtitle_options.borrow_mut() = options;
        *self.tracks.borrow_mut() = tracks;
        *self.file.borrow_mut() = Some(source.clone());
        self.duration_s.set(duration_ns as f64 / 1e9);
        // Now that the video and its audio files are both settled, whatever
        // was measured about that pairing applies again.
        self.load_baselines();
        // What the library says, over what the stream could be asked. A cast
        // video has no sidecar beside it and its container tags are thin, so
        // without this it arrives with a title and an empty page.
        self.overlay_jellyfin_details();
        // The page can be drawn without artwork and filled in when it lands,
        // so this is started rather than waited for.
        self.start_art_load();

        // Only a local file is worth reopening: a remote URL can carry an
        // access token that expires, and whatever launched us will hand it over
        // again anyway.
        if let Some(path) = source.local() {
            let mut config = self.config.borrow_mut();
            config.last_video = Some(path.to_path_buf());
            let _ = config.save();
        }

        // The video is loaded and its page is about to be shown, so the system
        // is told what it is now rather than at the first play. Otherwise the
        // panel in the task bar sits there enabled with no title, and Windows
        // fills the gap with the application's own identifier - which is how
        // "Scottarius.TinePlayer" ended up where a film's name belongs, until
        // something had been played once.
        self.publish_now_playing();
        Ok(())
    }

    /// Says, on screen, why a video could not be opened.
    ///
    /// Worth a screen rather than a line on stderr: when something else
    /// launched the player there is no terminal to read, and the window
    /// closing again immediately is all anyone sees. That is exactly the case
    /// most likely to fail, because a media center can hand over a path or a
    /// URL that means nothing on this machine.
    ///
    /// The message GStreamer gave is shown as it stands. It is more specific
    /// than anything that could be inferred from the kind of source - an
    /// unmounted share, a refused connection and a missing file all arrive
    /// here, and guessing between them would sometimes be wrong.
    fn show_source_error(self: &Rc<Self>, source: &Source, error: &str, fatal: bool) {
        // Percent escaping is how a URI carries a space; it is not how anyone
        // wants to read a path. Decoded for display only - what gets opened is
        // still the escaped form. Anything that is not valid escaping is left
        // alone rather than mangled.
        let readable = |text: &str| {
            glib::Uri::unescape_string(text, None)
                .map(|decoded| decoded.to_string())
                .unwrap_or_else(|| text.to_string())
        };

        let mut message = format!(
            "Couldn't open:\n{}\n\n{}",
            readable(&source.uri()),
            readable(error)
        );
        // Whatever launched us handed over a path or URL this machine could
        // not open, so what helps is knowing which paths and URLs work, rather
        // than anything about the launcher itself.
        if self.external {
            message.push_str("\n\nSee docs/usage.md for the paths and URLs that can be played.");
        }
        self.show_error(&message, fatal);
    }

    // --- Browsing ------------------------------------------------------

    /// Notes the screen a modal is about to cover.
    ///
    /// Only the screens that are not themselves modals, so that one modal
    /// replacing another leaves the pair's origin alone. A modal recorded as
    /// its own origin is a trap: backing out of it returns to itself, and
    /// nothing closes it.
    fn remember_origin(&self) {
        let screen = *self.screen.borrow();
        if matches!(screen, Screen::Menu | Screen::VideoSource) {
            self.origin.set(screen);
        }
    }

    /// Back to whatever the modal was opened over.
    fn return_to_origin(self: &Rc<Self>) {
        match self.origin.get() {
            Screen::VideoSource => self.choose_video(),
            _ => self.show_menu(),
        }
    }

    /// Floats a page over the main menu, dimmed and unresponsive behind it.
    ///
    /// The menu is rebuilt rather than kept aside, because every screen here
    /// replaces the window's child outright and there is no earlier page still
    /// around to reuse. Building a second one is cheap next to what it buys:
    /// the browser reads as something opened over the menu instead of as
    /// another step deeper into it.
    /// A dialog over the screen behind it, held to one width.
    ///
    /// Every panel that states something and asks a question goes through
    /// here, so they are all the same measure however long their words are.
    /// Without a ceiling each one is as wide as its own longest sentence
    /// wants to be, which on a 3440px monitor is a single line across the
    /// whole screen - and two dialogs in a row are then visibly two different
    /// shapes for no reason a viewer could name.
    ///
    /// The cap is a `Column` rather than a size request, for the reason
    /// `src/column.rs` sets out at length: a size request is a minimum, so a
    /// panel whose natural width exceeds it widens anyway. The panel keeps the
    /// modal styling and the `Column` around it stays invisible, or the
    /// background would be drawn across the full width instead of behind the
    /// words.
    fn dialog(self: &Rc<Self>, page: &gtk::Box) -> gtk::Overlay {
        page.add_css_class("tp-modal");
        self.modal_around(&self.dialog_column(page))
    }

    /// The width ceiling on its own, for the two panels that fill the window
    /// rather than floating over a screen: closing the player is asked before
    /// there is anything to float over, and a fatal error has nothing left
    /// behind it worth showing.
    fn dialog_column(&self, page: &gtk::Box) -> crate::column::Column {
        let most = (DIALOG_MAX_UNITS * self.scale.get()).round() as i32;
        crate::column::Column::around(page, most)
    }

    fn modal(self: &Rc<Self>, page: &gtk::Box) -> gtk::Overlay {
        page.add_css_class("tp-modal");
        self.modal_around(page)
    }

    /// The scrim and the screen behind it, around whatever is being floated.
    fn modal_around(self: &Rc<Self>, content: &impl IsA<gtk::Widget>) -> gtk::Overlay {
        // Whatever is on screen right now, so the modal opens over the screen
        // it was actually opened from rather than always over the main menu.
        //
        // One modal replacing another hands back the page *behind* it instead
        // of the modal itself, or the dimming would stack up a layer deeper
        // every time.
        //
        // Nothing behind it is drawn as nothing. A menu built to stand in for
        // the screen behind was what this did before there was a real one to
        // use, and a rebuilt menu is not the screen it claims to be: it shows
        // the main menu behind a dialog opened from somewhere else entirely.
        // The window has a child from the first screen onwards, so what is
        // left here is the moment before that.
        // Only a *modal's* overlay is unwrapped, which is what the marker
        // class is for. The media page is an overlay too - artwork behind,
        // page in front - and taking its child handed back the bare backdrop
        // and threw the page away, so the browser opened over a film's
        // wallpaper with nothing on it.
        let modal_stack = |child: &gtk::Widget| {
            child
                .downcast_ref::<gtk::Overlay>()
                .is_some_and(|overlay| overlay.has_css_class(MODAL_STACK))
        };
        let backdrop: gtk::Widget = match self.window.child() {
            Some(child) if modal_stack(&child) => {
                let overlay = child.downcast::<gtk::Overlay>().expect("checked above");
                let under = overlay.child();
                overlay.set_child(None::<&gtk::Widget>);
                under.unwrap_or_else(|| empty_backdrop().upcast())
            }
            Some(child) => {
                self.window.set_child(None::<&gtk::Widget>);
                child
            }
            None => empty_backdrop().upcast(),
        };
        // Not just visually behind: an insensitive page cannot take focus, so
        // neither tab nor the gamepad can reach what is underneath.
        backdrop.set_sensitive(false);

        let scrim = gtk::Box::builder().css_classes(["tp-scrim"]).build();

        let overlay = gtk::Overlay::new();
        overlay.add_css_class(MODAL_STACK);
        overlay.set_child(Some(&backdrop));
        overlay.add_overlay(&scrim);
        overlay.add_overlay(content);
        overlay
    }

    /// A panel for the one thing browsing folders cannot reach: an address.
    ///
    /// Its own screen rather than a field in the browser, because a text field
    /// among the folders is a trap for a controller, which can neither type
    /// into one nor easily get out of it. Behind a row, it is only ever
    /// entered on purpose, and there is room to say what may be pasted.
    fn show_paste_uri(self: &Rc<Self>) {
        // Built by hand rather than from the list page every other screen
        // uses: that one leads with a header and a list, and here both would
        // be empty space above the only thing on the panel.
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        let heading = heading_label("Open a URL");
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let blurb = gtk::Label::builder()
            .label(
                "Enter an address to a video file, such as a link from a media server, a local file path, or a network path.",
            )
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&blurb);

        let field = gtk::Entry::new();
        field.add_css_class("tp-path");
        field.set_placeholder_text(Some("http://…"));
        gtk::prelude::EditableExt::set_alignment(&field, 0.5);
        field.set_hexpand(true);
        page.append(&field);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let open = gtk::Button::with_label("Open");
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing to open until there is something in the field, and an empty
        // one would only fail slowly against a source that does not exist.
        open.set_sensitive(false);
        {
            let open = open.clone();
            field.connect_changed(move |field| {
                open.set_sensitive(!field.text().trim().is_empty());
            });
        }
        buttons.append(&cancel);
        buttons.append(&open);
        page.append(&buttons);

        {
            let app = self.clone();
            let field = field.clone();
            open.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.open_typed_path(&field.text());
            });
        }
        {
            let app = self.clone();
            field.connect_activate(move |field| {
                if !field.text().trim().is_empty() {
                    app.open_typed_path(&field.text());
                }
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        // Its own tab order: the field, then the two buttons. Without stops
        // of its own there is nothing for Tab to move between, and the Open
        // button cannot be reached without a pointer.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&field);
        self.add_nav_stop(&cancel);
        self.add_nav_stop(&open);
        *self.screen.borrow_mut() = Screen::PasteUri;
        self.window.set_child(Some(&self.modal(&page)));
        // The field wants the caret from the moment it opens: this screen
        // exists to be typed into.
        field.grab_focus();

        // Filled in for you when the clipboard already holds something this
        // panel could open, and selected so typing replaces it. Better than a
        // Paste button: a controller cannot reach one, and a button says
        // nothing about whether pressing it would help.
        {
            let field = field.clone();
            gtk::prelude::WidgetExt::display(&self.window)
                .clipboard()
                .read_text_async(gtk::gio::Cancellable::NONE, move |text| {
                    let Ok(Some(text)) = text else { return };
                    let text = text.trim();
                    if looks_openable(text) {
                        field.set_text(text);
                        field.select_region(0, -1);
                    }
                });
        }
    }

    /// Opens whatever was typed into the paste panel.
    ///
    /// A folder browses to it, so typing a path is another way to navigate.
    /// Anything else is handed to [`Source`], which is what decides whether a
    /// string is a file or a URL, so this cannot disagree with what the
    /// command line accepts.
    fn open_typed_path(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        let as_path = std::path::Path::new(text);
        if as_path.is_dir() {
            self.show_browser(as_path, None);
            return;
        }

        self.show_opening(Source::parse(text));
    }

    /// Waits for a source to answer, with something on screen that says so.
    ///
    /// Reading a remote source is not quick and can fail slowly: an address
    /// nothing answers at takes the discoverer's full ten seconds. Doing that
    /// on the main thread froze the whole window, which reads as a crash
    /// rather than as waiting, so the probe runs on a thread of its own.
    fn show_opening(self: &Rc<Self>, source: Source) {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(28)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .margin_top(48)
            .margin_bottom(48)
            .margin_start(56)
            .margin_end(56)
            .build();

        // A floor rather than a fixed size: with only a spinner and a short
        // address on it, the panel would otherwise shrink to something much
        // narrower than the one it replaces, and the swap would read as the
        // window jumping about.
        page.set_size_request((560.0 * self.scale.get()).round() as i32, -1);

        let spinner = gtk::Spinner::new();
        spinner.set_size_request(
            (48.0 * self.scale.get()).round() as i32,
            (48.0 * self.scale.get()).round() as i32,
        );
        spinner.start();
        page.append(&spinner);
        page.append(&heading_label("Opening"));

        // The launcher's title where there is one, and the file name
        // otherwise. Nothing beside the file has been read yet at this point -
        // that is what the spinner is waiting for - so this is as much as can
        // be known, and for an add-on stream it is the difference between a
        // name and an opaque id.
        let opening = match self.launcher_title() {
            title if !title.is_empty() => title,
            _ => source.label(),
        };
        let what = gtk::Label::builder()
            .label(&opening)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&what);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.show_paste_uri());
        }

        *self.screen.borrow_mut() = Screen::Opening;
        self.window.set_child(Some(&self.modal(&page)));
        self.set_nav(None, &[], &[]);
        cancel.grab_focus();

        // A plain channel polled from the main loop, rather than anything
        // asynchronous: the probe returns once, and the result has to be
        // applied on this thread because everything it touches is `Rc`.
        let (sender, receiver) = std::sync::mpsc::channel();
        let probing = source.clone();
        std::thread::spawn(move || {
            let _ = sender.send(crate::probe::probe_media(&probing));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(80), move || {
            let result = match receiver.try_recv() {
                Ok(result) => result,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                // The thread is gone without an answer, which leaves nothing
                // to report and no reason to keep looking.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::Opening {
                return glib::ControlFlow::Break;
            }

            match result.and_then(|media| app.apply_media(&source, media)) {
                Ok(()) => app.show_menu(),
                Err(e) => {
                    eprintln!("Couldn't read {}: {e}", source.uri());
                    app.forget_file();
                    app.show_source_error(&source, &e, false);
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// The built-in browser: another list screen, so it navigates exactly
    /// like the menus and needs no pointer.
    ///
    /// `select` names the folder just stepped out of, which is then the row
    /// focus lands on. Going up otherwise dumps you at the top of a long
    /// list with no sense of where you were.
    /// The screen for choosing a video: folders, and the videos in them.
    fn show_browser(
        self: &Rc<Self>,
        directory: &std::path::Path,
        select: Option<&std::path::Path>,
    ) {
        // The same screen chooses a video and a separate soundtrack for one,
        // differing only in what it lists and what activating a row does.
        // Which of the two is in hand is held on the application rather than
        // passed down, because stepping into a folder re-enters here and would
        // otherwise forget what was being looked for.
        let mode = match self.errand.get() {
            Errand::Audio(_) => Browse::Audio,
            Errand::Subtitle => Browse::Subtitles,
            Errand::Video => Browse::Videos,
        };
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, mode);
        let entries = browser_entries(&directory, mode);

        // The two things done with a selection, together in the middle, in
        // the order every other pair in the application uses: the way out
        // first, then the action. Opening the system browser stays off to one
        // side, being a way out of this screen rather than a use of it.
        let choices = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing((24.0 * self.scale.get()).round() as i32)
            .build();
        choices.append(&page.cancel);
        choices.append(&page.open);

        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&choices));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries, self.scale.get());

        {
            let app = self.clone();
            let entries = entries.clone();
            let here = directory.clone();
            page.list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some(entry) = entries.get(row.index() as usize) else {
                    return;
                };
                match &entry.path {
                    Some(path) if path.is_dir() => app.show_browser(path, None),
                    // A soundtrack for the video already chosen, rather than a
                    // video: it replaces whatever track that output was on and
                    // hands straight back to the menu, where the row now names
                    // the file.
                    Some(path) if app.errand.get() == Errand::Subtitle => {
                        app.set_subtitle_file(path);
                        app.show_menu();
                    }
                    Some(path) if matches!(app.errand.get(), Errand::Audio(_)) => {
                        app.set_audio_file(path);
                        app.show_menu();
                    }
                    // Through the same screen a URL opens through, rather
                    // than reading the file here and moving when it is done.
                    // Probing is not instant - it starts a GStreamer
                    // discoverer, and a file on a network share can take a
                    // second or two - and doing it on this thread left the
                    // browser standing there with the row lit, looking like
                    // the press had been missed. This puts the spinner up
                    // first and reads the file behind it.
                    Some(path) => app.show_opening(Source::File(path.to_path_buf())),
                    // Up. Only offered when there is somewhere above to go:
                    // at the top of the tree the column to the left is how
                    // you reach anywhere else.
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_browser(parent, Some(&here));
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| app.go_back());
        }
        // The button does what a double click does, by asking the list to
        // activate the row rather than repeating what activation means. One
        // description of what opening a row is, in the handler above.
        {
            let list = page.list.clone();
            page.open.connect_clicked(move |_| {
                if let Some(row) = list.selected_row() {
                    list.emit_by_name::<()>("row-activated", &[&row]);
                }
            });
        }
        // Off unless a file is selected. Not a folder, which a double click
        // or Enter still steps into - the button is for choosing the thing
        // this screen exists to choose, and a folder is not it. Not the way
        // up, and not the notice a folder with nothing in it shows, which is
        // a row like any other to GTK.
        {
            let open = page.open.clone();
            let openable: Vec<bool> = entries.iter().map(|entry| entry.openable).collect();
            page.list.connect_row_selected(move |_, row| {
                let selected = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| openable.get(index).copied())
                    .unwrap_or(false);
                open.set_sensitive(selected);
            });
        }

        {
            let mut config = self.config.borrow_mut();
            config.last_folder = Some(directory.to_path_buf());
            let _ = config.save();
        }

        // The trail alone now that the arrow has gone: left from the current
        // folder simply walks back up it.
        // Typing a letter jumps to the first name that begins with it, which
        // is how a folder of two hundred films is reached without holding an
        // arrow key. Attached here rather than to every list: the browser is
        // the one screen whose rows are named by something other than us, and
        // so the one where a name cannot be predicted.
        {
            let labels: Vec<String> = entries
                .iter()
                .map(|entry| entry.label.trim().to_lowercase())
                .collect();
            let list = page.list.clone();
            let app = self.clone();
            // What was typed last, so a repeat of it can be told from a new
            // letter. Held by the controller rather than the application: it
            // belongs to this listing and is meaningless once it is gone.
            let last: RefCell<Option<String>> = RefCell::new(None);
            let controller = gtk::EventControllerKey::new();
            controller.connect_key_pressed(move |_, key, _, state| {
                // Nothing with a modifier on it: those are shortcuts, and
                // Ctrl+C on a browser row should stay Ctrl+C. Shift is let
                // through, being how a capital arrives.
                if state.intersects(
                    gdk::ModifierType::CONTROL_MASK
                        | gdk::ModifierType::ALT_MASK
                        | gdk::ModifierType::META_MASK,
                ) {
                    return glib::Propagation::Proceed;
                }
                let Some(typed) = key.to_unicode().filter(|c| c.is_alphanumeric()) else {
                    return glib::Propagation::Proceed;
                };
                let typed = typed.to_lowercase().to_string();
                // The same letter again walks on to the next name that starts
                // with it, wrapping at the end; a different letter starts from
                // the top. Without that, a folder holding a dozen films
                // beginning with "The" would answer every press with the same
                // row and look as though the key had done nothing.
                let again = last.borrow().as_deref() == Some(typed.as_str());
                *last.borrow_mut() = Some(typed.clone());
                let from = match again {
                    true => list
                        .selected_row()
                        .map_or(0, |row| row.index() as usize + 1),
                    false => 0,
                };
                let matching = |offset: usize| {
                    let index = (from + offset) % labels.len().max(1);
                    labels
                        .get(index)
                        .filter(|label| label.starts_with(&typed))
                        .map(|_| index)
                };
                let Some(index) = (0..labels.len()).find_map(matching) else {
                    // Nothing starts with it. Swallowed all the same, so a
                    // stray letter cannot fall through to whatever else on the
                    // screen might answer it.
                    return glib::Propagation::Stop;
                };
                if let Some(row) = list.row_at_index(index as i32) {
                    app.sounds.borrow().click();
                    list.select_row(Some(&row));
                    settle_on(&row);
                }
                glib::Propagation::Stop
            });
            page.list.add_controller(controller);
        }

        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), page.open.clone()],
        );
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::Browser;
        self.window.set_child(Some(&self.modal(&page.page)));

        let opening = select
            .and_then(|wanted| {
                entries
                    .iter()
                    .position(|entry| entry.path.as_deref() == Some(wanted))
            })
            // Otherwise the first real entry, skipping the rows that only
            // lead somewhere else: up, and the empty-folder notice.
            .or_else(|| entries.iter().position(|entry| entry.path.is_some()))
            // Nothing to open: the way up, rather than the line saying so.
            .unwrap_or(0) as i32;
        if let Some(row) = page.list.row_at_index(opening) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The scaffolding every browsing screen is built on.
    ///
    /// One page for two jobs - choosing a video, and choosing a folder to set
    /// Kodi up in - because they are the same screen with different rows in
    /// it. Built separately they drifted: the same trail, places column and
    /// system-browser button written twice, so a change to how browsing looks
    /// had to be made in both and was once made in only one.
    ///
    /// What differs is left to the caller: what the footer holds, what a row
    /// does when it is chosen, and where the cursor starts.
    fn browser_page(self: &Rc<Self>, directory: &std::path::Path, mode: Browse) -> BrowserPage {
        let (crumbs, crumb_buttons) = self.breadcrumbs(directory, mode.folders_only());

        let (page, list, _back, slot) = list_page_with(&crumbs, false);
        // The arrow's slot holds a fixed width for every screen to line up
        // against. With no arrow in it, that is just a gap before the trail.
        slot.set_visible(false);
        self.add_places_column(&page, directory, mode.folders_only(), &crumb_buttons);
        self.follow_focus(&list);

        // Along the foot with the way out, rather than tucked into the header:
        // both are things done with the browser rather than places inside it.
        // Still not focusable, and last: it exists for a pointer, and the
        // dialog it opens cannot be driven by a controller anyway.
        let browse_face = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        // Larger than the lettering beside it: at this size the icon is what
        // the eye finds first, and the words only confirm it.
        // The same folder the rows are drawn with, so the button that opens
        // another browser is marked with what it opens - smaller than in a
        // row, where it stands alone against a name; here it sits beside a
        // line of text on a button and should not outweigh it.
        let browse_icon = RowIcon::Folder.image_at(BUTTON_FOLDER_PX, self.scale.get());
        browse_face.append(&browse_icon);
        browse_face.append(&gtk::Label::new(Some("Open System Browser")));
        let browse = gtk::Button::builder().child(&browse_face).build();
        browse.add_css_class("tp-button");
        browse.add_css_class("tp-secondary");
        browse.set_can_focus(false);
        browse.set_valign(gtk::Align::Start);
        {
            let app = self.clone();
            // Wherever the listing behind it has reached. Handing over at the
            // top of the tree, or wherever the system dialog last was, means
            // walking back down a path already walked.
            let here = directory.to_path_buf();
            browse.connect_clicked(move |_| match mode {
                Browse::Videos => app.open_file_chooser(&here),
                Browse::Folders => app.choose_kodi_folder_natively(&here),
                // The same dialog, filtered to whatever is being looked for:
                // it reads which errand it is on for itself.
                Browse::Audio | Browse::Subtitles => app.open_file_chooser(&here),
            });
        }

        // What a click used to do on its own. A single click selects now, so
        // there has to be something a pointer can press to act on what it
        // selected - a double click is the shortcut, not the only way.
        let open = gtk::Button::with_label("Open");
        open.add_css_class("tp-button");
        open.add_css_class("tp-action");
        // Nothing is selected until the list is filled, and a row that opens
        // nothing leaves it off again. See `follow_open`.
        open.set_sensitive(false);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");

        // A click selects; it takes a second one to open. Set here rather than
        // on each screen, so the file browser and the folder chooser cannot
        // come to disagree about what a click does.
        //
        // The keyboard is untouched by it. GtkListBox emits `row-activated` on
        // a double click and on Enter either way, and Enter here goes through
        // `activate_focused`, which emits it by hand - so every handler is
        // reached exactly as it was.
        list.set_activate_on_single_click(false);

        BrowserPage {
            page,
            list,
            crumbs: crumb_buttons,
            browse,
            open,
            cancel,
        }
    }

    /// The current directory as a row of buttons, one per level, so any
    /// ancestor is a single press away rather than several trips through Up.
    ///
    /// Capped at the last few levels: a deep path would otherwise run off the
    /// side, and the leading button stands in for everything trimmed away.
    /// `folders` decides which browser a crumb reopens. Without it, stepping
    /// up the trail from the folder browser lands in the video browser, which
    /// is the same shape of screen doing an entirely different job.
    fn breadcrumbs(
        self: &Rc<Self>,
        directory: &std::path::Path,
        folders: bool,
    ) -> (gtk::Box, Vec<gtk::Button>) {
        use std::path::{Component, PathBuf};

        // Each level paired with the path that reaches it.
        let mut levels: Vec<(String, PathBuf)> = Vec::new();
        let mut walked = PathBuf::new();
        for component in directory.components() {
            match component {
                Component::Prefix(prefix) => {
                    walked.push(prefix.as_os_str());
                    // Rooted right here, because `H:` on its own does not mean
                    // the top of that drive: it means wherever that drive was
                    // last left, which is a relative path. Browsing to one
                    // works, since reading it still finds the right folder,
                    // but every entry under it is relative too and no URI can
                    // be made from those.
                    walked.push(std::path::MAIN_SEPARATOR_STR);
                    levels.push((
                        prefix.as_os_str().to_string_lossy().to_string(),
                        walked.clone(),
                    ));
                }
                Component::RootDir => {
                    if levels.is_empty() {
                        walked.push(std::path::MAIN_SEPARATOR_STR);
                        levels.push(("/".to_string(), walked.clone()));
                    }
                }
                Component::Normal(name) => {
                    walked.push(name);
                    levels.push((name.to_string_lossy().to_string(), walked.clone()));
                }
                _ => {}
            }
        }

        const SHOWN: usize = 4;
        let mut trimmed = Vec::new();
        if levels.len() > SHOWN {
            let hidden = levels.len() - SHOWN;
            // Leads to the level just above the first one still shown.
            trimmed.push(("…".to_string(), levels[hidden - 1].1.clone()));
            trimmed.extend_from_slice(&levels[hidden..]);
        } else {
            trimmed = levels;
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .hexpand(true)
            .build();
        let mut buttons = Vec::new();

        for (position, (label, target)) in trimmed.iter().enumerate() {
            if position > 0 {
                let separator = gtk::Label::new(Some("›"));
                separator.add_css_class("tp-crumb-separator");
                row.append(&separator);
            }

            let button = gtk::Button::with_label(label);
            button.add_css_class("tp-crumb");
            {
                let app = self.clone();
                let target = target.clone();
                let here = directory.to_path_buf();
                button.connect_clicked(move |_| {
                    app.sounds.borrow().click();
                    if folders {
                        app.show_kodi_folder(&target);
                        return;
                    }
                    // Selecting the folder you are already in should settle
                    // focus back on the listing rather than rebuild nothing.
                    let select = (target != here).then(|| here.clone());
                    app.show_browser(&target, select.as_deref());
                });
            }
            row.append(&button);
            buttons.push(button);
        }

        (row, buttons)
    }

    /// Where a video comes from: a folder on this machine, or an address.
    ///
    /// A step of its own rather than opening the browser straight away,
    /// because the two are not the same kind of thing. Walking folders finds
    /// what is here; an address reaches what is not, and no amount of
    /// browsing would ever lead to it.
    fn choose_video(self: &Rc<Self>) {
        let scale = self.scale.get();
        let (panel, browse, address, connect, cancel) = self.choose_source_panel(scale, true);
        let cancel = cancel.expect("asked for with a cancel button");

        // A floor rather than a fixed size, the way the Opening panel has one:
        // three buttons and a line of text would otherwise make a panel much
        // narrower than the page behind it, and the swap would read as the
        // window jumping about.
        panel.set_size_request((560.0 * scale).round() as i32, -1);

        {
            let app = self.clone();
            browse.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.browse_for_file();
            });
        }
        {
            let app = self.clone();
            address.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_paste_uri();
            });
        }
        if let Some(connect) = connect.as_ref() {
            let app = self.clone();
            connect.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.start_jellyfin_connect(ConnectFrom::Menu);
            });
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_menu();
            });
        }

        // The same words the empty screen shows, floated over the film rather
        // than replacing it: what is loaded is still loaded, and backing out
        // returns to it.
        self.remember_origin();
        let mut stops = vec![cancel.clone(), browse.clone(), address];
        stops.extend(connect);
        self.set_nav(None, &[], &stops);
        *self.screen.borrow_mut() = Screen::VideoSource;
        self.window.set_child(Some(&self.modal(&panel)));
        browse.grab_focus();
    }

    /// Opens the file browser where browsing last stopped.
    ///
    /// Always the built-in browser. Guessing from the last input was
    /// unpredictable: the same button opened different things depending on
    /// what you had touched. The system dialog is still reachable, from a
    /// pointer-only button in the footer.
    fn browse_for_file(self: &Rc<Self>) {
        // Whatever errand the browser was last on, this one is a video.
        self.errand.set(Errand::Video);
        self.open_browser();
    }

    /// The same browser, looking for a soundtrack to put on one output.
    ///
    /// Starts where the video is rather than where browsing left off: a
    /// separate audio track is usually downloaded to sit beside the film, and
    /// when it is not, the film's folder is still a better place to start from
    /// than wherever a video was last chosen.
    fn browse_for_audio(self: &Rc<Self>, role: Role) {
        self.errand.set(Errand::Audio(role));
        let beside = self
            .file
            .borrow()
            .as_ref()
            .and_then(|file| file.local().and_then(|path| path.parent()))
            .map(|folder| folder.to_path_buf());
        match beside {
            Some(folder) => self.show_browser(&folder, None),
            None => self.open_browser(),
        }
    }

    fn open_browser(self: &Rc<Self>) {
        let (remembered, last_video) = {
            let config = self.config.borrow();
            (config.last_folder.clone(), config.last_video.clone())
        };
        let start = crate::browser::start_location(remembered.as_deref(), last_video.as_deref());
        self.show_browser(&start, None);
    }

    /// What an output is playing, for its row on the menu: the name of a
    /// separate audio file when one is chosen, and otherwise the track.
    fn describe_audio(&self, role: Role) -> String {
        if let Some(file) = self.file_for(role).borrow().as_ref() {
            return file.label();
        }
        let chosen = *self.track_for(role).borrow();
        let tracks = self.tracks.borrow();
        match chosen {
            Some(index) => tracks
                .iter()
                .find(|track| track.index == index)
                .map(describe_audio_track)
                .unwrap_or_else(|| "None".to_string()),
            None => "None".to_string(),
        }
    }

    /// The alignment row for one output, when there is anything to align.
    ///
    /// Only offered against a separate audio file: a track inside the video
    /// shares the video's timeline and cannot be out of step with it. The rest
    /// are the things measuring needs and cannot do without - a track inside
    /// the video to line the file up against, a running time to place the
    /// three windows across, and a path on disk to file the answer under.
    fn alignment_row(&self, role: Role) -> Option<(String, String, bool, MenuAction)> {
        let file = self.file_for(role).borrow();
        let path = file.as_ref()?.local()?;
        if self.tracks.borrow().is_empty() || self.duration_s.get() <= 0.0 {
            return None;
        }
        let stored = self
            .storage_key()
            .and_then(|key| crate::config::load_alignment(&key, path));
        Some((
            // One name whether or not there is a stored answer. It used to say
            // "Auto-align" or "Re-align" to name what pressing it would do,
            // which the value beside it now says better: "Unsynced" against a
            // measured offset is the same distinction, in the column that
            // exists to carry state.
            "Sync".to_string(),
            match stored {
                Some(millis) => describe_lateness(millis),
                None => "Unsynced".to_string(),
            },
            true,
            MenuAction::Align(role),
        ))
    }

    /// Reads back what alignment worked out for whatever each output is
    /// playing, so the baseline is in force before the pipeline is built.
    ///
    /// Zero for a track inside the video: alignment is about a pairing of two
    /// files and there is nothing to pair a track with.
    fn load_baselines(&self) {
        let key = self.storage_key();
        for role in [Role::Primary, Role::Secondary] {
            let stored = key.as_deref().and_then(|key| {
                let file = self.file_for(role).borrow();
                let path = file.as_ref()?.local()?;
                crate::config::load_alignment(key, path)
            });
            // Negated on the way in: alignment says how late the audio runs,
            // and a sink is held back by a negative offset.
            let cell = match role {
                Role::Primary => &self.primary_baseline,
                Role::Secondary => &self.secondary_baseline,
            };
            cell.set(-stored.unwrap_or(0.0));
        }
    }

    /// The alignment baseline for one output.
    fn baseline_ms(&self, role: &str) -> f64 {
        match role {
            "primary" => self.primary_baseline.get(),
            _ => self.secondary_baseline.get(),
        }
    }

    /// What the sink should actually be held back by: what the viewer asked
    /// for, plus what alignment worked out. The two are separate quantities -
    /// one describes the headphones, the other describes the pair of files -
    /// and only the first is ever shown on the slider.
    fn offset_for(&self, role: &str) -> f64 {
        self.config.borrow().applied_offset_ms(role) + self.baseline_ms(role)
    }

    /// Sends an output's whole delay to the pipeline: what the viewer asked
    /// for, plus what alignment worked out for the file being played.
    ///
    /// The one road to a sink, deliberately. The sum used to be rebuilt by
    /// hand at each of the four places that change either half, and the one
    /// behind the sync control during playback rebuilt it wrong - it sent the
    /// slider's own value, so touching sync threw the alignment away and left
    /// the audio seconds out. A half-applied offset is worse than none, and
    /// the way to stop that recurring is to leave nowhere else to apply one.
    fn push_offset(&self, playback: &Playback, role: &str) {
        playback.set_offset_ms(role, self.offset_for(role));
    }

    /// Sends an output's level to the pipeline: what that output is set to,
    /// times the master over both of them.
    ///
    /// The one road to a sink's level, for the reason `push_offset` is the one
    /// road to its delay. Two outputs and a master mean two numbers behind
    /// every level, and every place that rebuilt the sum by hand would be a
    /// place free to leave the master out - which sounds exactly like a level
    /// that ignores the control somebody just moved.
    fn push_volume(&self, role: &str) {
        let level = self.config.borrow().volume(role);
        self.push_volume_at(role, level);
    }

    /// The same, for a level that is not in the configuration - which is any
    /// level that is not being kept, such as everything silenced for a knock at
    /// the door.
    fn push_volume_at(&self, role: &str, level: f64) {
        let level = self.effective(level);
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_volume(role, level);
        }
    }

    /// What a level actually plays at once the master over both outputs is
    /// taken into account. The only place the two are multiplied together.
    fn effective(&self, level: f64) -> f64 {
        level * self.config.borrow().master_volume()
    }

    /// Sends whether an output is actually silent: whether it is muted in its
    /// own right, or everything is.
    ///
    /// The two are kept apart all the way down to here, which is what lets the
    /// menu go on showing each output's own state while everything is quiet.
    /// `muted` is passed in rather than read back, because a silence nobody is
    /// keeping never reaches the configuration to be read from.
    fn push_mute(&self, role: &str, muted: bool) {
        if let Some(playback) = self.playback.borrow().as_ref() {
            playback.set_muted(role, muted || self.hushed.get());
        }
    }

    /// The same, for whatever is playing now, if anything is. Cloned out of
    /// the cell rather than borrowed across the call, since what it reaches
    /// takes the same borrows.
    fn push_offset_live(&self, role: &str) {
        if let Some(playback) = self.playback.borrow().clone() {
            self.push_offset(&playback, role);
        }
    }

    /// The track chosen for one output, and the file chosen for it, where the
    /// two outputs are otherwise handled by the same code.
    fn track_for(&self, role: Role) -> &RefCell<Option<u32>> {
        match role {
            Role::Primary => &self.primary_track,
            Role::Secondary => &self.secondary_track,
        }
    }

    fn file_for(&self, role: Role) -> &RefCell<Option<Source>> {
        match role {
            Role::Primary => &self.primary_file,
            Role::Secondary => &self.secondary_file,
        }
    }

    /// Puts a chosen audio file on the output the browser was opened for.
    /// Opens the browser to find a subtitle file, starting where the video is.
    fn browse_for_subtitle(self: &Rc<Self>) {
        self.errand.set(Errand::Subtitle);
        let beside = self
            .file
            .borrow()
            .as_ref()
            .and_then(|file| file.local().and_then(|path| path.parent()))
            .map(|folder| folder.to_path_buf());
        match beside {
            Some(folder) => self.show_browser(&folder, None),
            None => self.open_browser(),
        }
    }

    /// Takes a subtitle file chosen by hand.
    ///
    /// Added to the options as well as chosen, so the menu can show it and the
    /// chooser can show it selected. Everything else in that list was found by
    /// looking beside the video, and this one never would be.
    fn set_subtitle_file(self: &Rc<Self>, path: &std::path::Path) {
        let option = crate::subtitles::chosen_file(path);
        let choice = option.choice();
        {
            let mut options = self.subtitle_options.borrow_mut();
            if !options.iter().any(|other| other.choice() == choice) {
                options.push(option);
            }
        }
        *self.subtitle.borrow_mut() = Some(choice);
        // Choosing a subtitle is asking to see it, whatever the toggle was
        // doing for the last one.
        self.subtitles_hidden.set(false);
        self.errand.set(Errand::Video);
        self.remember_tracks();
    }

    fn set_audio_file(self: &Rc<Self>, path: &std::path::Path) {
        let Errand::Audio(role) = self.errand.get() else {
            return;
        };
        let source = Source::File(path.to_path_buf());
        match role {
            Role::Primary => *self.primary_file.borrow_mut() = Some(source),
            Role::Secondary => *self.secondary_file.borrow_mut() = Some(source),
        }
        self.errand.set(Errand::Video);
        // Written down here, not left to playback to save: choosing a
        // soundtrack and then quitting without pressing play is choosing it,
        // and every other chooser on this screen remembers itself the same way.
        self.remember_tracks();
        // A pairing measured before comes back already lined up.
        self.load_baselines();
    }

    // --- Alignment -----------------------------------------------------

    /// The frame the three alignment steps share.
    ///
    /// One panel carrying all three in turn, rather than three screens: it is
    /// one errand, and the film it belongs to should stay visible behind it
    /// throughout. An overlay rather than a real modal window, for the reason
    /// the browser is one - a `transient_for` window takes the pointer but not
    /// the keyboard or the gamepad, both of which are driven from the main
    /// window and would carry on working the menu hidden behind it.
    fn align_page(&self, hint: &str) -> gtk::Box {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            // Centered and no taller than its contents, so the panel is the
            // size of the question rather than the size of the window.
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(32)
            .margin_bottom(32)
            .margin_start(44)
            .margin_end(44)
            .build();
        // The floor. Without it the panel shrinks around whatever the shortest
        // step has on it, and the three read as three differently sized
        // windows rather than one panel changing what it says.
        page.set_size_request((ALIGN_PANEL_MIN * self.scale.get()).round() as i32, -1);

        let heading = heading_label("Auto-Align");
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let hint = gtk::Label::builder()
            .label(hint)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            // The ceiling, and with the two set alike the floor as well. A
            // GtkBox has no maximum width, so the cap has to sit on the text
            // that would otherwise push it wide - and asking for the same
            // measure as a minimum is what makes all three steps come out the
            // same width instead of each shrinking to fit its own sentence.
            // In characters rather than pixels because that is what wraps, and
            // it holds at every interface scale without being multiplied.
            .width_chars(ALIGN_PANEL_CHARS)
            .max_width_chars(ALIGN_PANEL_CHARS)
            .css_classes(["tp-hint"])
            .build();
        page.append(&hint);
        page
    }

    /// Step one: which track inside the video to measure the audio file
    /// against.
    ///
    /// Asked rather than inferred, so the viewer can point it at the original
    /// soundtrack when the automatic pick would have taken a dub. It arrives
    /// with a sensible one already selected, so the common answer is a single
    /// press of Next.
    fn show_align(self: &Rc<Self>, role: Role) {
        // Nothing to align without both halves of the pairing.
        let tracks = self.tracks.borrow().clone();
        if self.file_for(role).borrow().is_none() || tracks.is_empty() {
            return;
        }

        let page = self.align_page(
            "Choose a reference audio track to align the external audio file with. \
             Usually the original language, or a language that matches the audio \
             description.",
        );

        let (scroller, list) = scrolling_list();
        name_it(&list, "Reference track");
        // Only as tall as the tracks need, up to a few rows. A list left to
        // expand makes the panel the height of the window whether it holds one
        // track or twelve, which is the opposite of what a short question wants.
        scroller.set_vexpand(false);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height((240.0 * self.scale.get()).round() as i32);
        page.append(&scroller);
        for track in &tracks {
            let text = describe_audio_track(track);
            let row = chooser_row(&text);
            row.set_xalign(0.5);
            // Held to the same measure as the body text. A track carrying a
            // long title would otherwise widen the whole panel, and it already
            // ellipsizes rather than wrapping.
            row.set_max_width_chars(ALIGN_PANEL_CHARS);
            append_named(&list, &row, &text);
        }

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let next = gtk::Button::with_label("Next");
        next.add_css_class("tp-button");
        next.add_css_class("tp-action");
        buttons.append(&cancel);
        buttons.append(&next);
        page.append(&buttons);

        // What the list is pointing at when Next is pressed, and what
        // activating a row means, are the same thing: the row is the choice.
        let start = {
            let app = self.clone();
            let list = list.clone();
            let tracks = tracks.clone();
            move || {
                let index = list.selected_row().map(|row| row.index()).unwrap_or(0);
                let Some(track) = tracks.get(index.max(0) as usize) else {
                    return;
                };
                app.sounds.borrow().click();
                app.show_align_progress(role, track.index);
            }
        };
        {
            let start = start.clone();
            list.connect_row_activated(move |_, _| start());
        }
        next.connect_clicked(move |_| start());
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.wire_navigation(&list, &[], &[cancel.clone(), next.clone()]);
        self.remember_origin();
        *self.screen.borrow_mut() = Screen::AlignChoose;
        self.window.set_child(Some(&self.modal(&page)));

        // The first track that is not a description, because a description is
        // the thing being lined up rather than the thing to line it up with -
        // it correlates against itself perfectly and says nothing. Falls back
        // to the first track when description is all the file has.
        let opening = tracks
            .iter()
            .position(|track| !crate::probe::is_audio_description(&track.title))
            .unwrap_or(0);
        if let Some(row) = list.row_at_index(opening as i32) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Step two: the measuring, which happens on a thread.
    ///
    /// Three sixty-second windows out of each of two files is around twelve
    /// seconds on a desktop and several times that on a Pi, so it cannot run
    /// on the main loop: the window would stop redrawing and the interface
    /// would read as having crashed. The thread reports through a channel this
    /// polls, which is how the rest of the application already waits on work -
    /// everything the answer touches is `Rc` and has to be applied here.
    fn show_align_progress(self: &Rc<Self>, role: Role, reference: u32) {
        let (video, audio) = {
            let file = self.file.borrow().clone();
            let audio = self.file_for(role).borrow().clone();
            match (file, audio) {
                (Some(video), Some(audio)) => (video, audio),
                _ => return,
            }
        };

        let page =
            self.align_page("Analyzing audio to align the tracks. This may take a few moments.");

        let bar = gtk::ProgressBar::new();
        bar.add_css_class("tp-align-bar");
        page.append(&bar);

        let status = gtk::Label::builder()
            .label("0%")
            .halign(gtk::Align::Center)
            .css_classes(["tp-hint"])
            .build();
        page.append(&status);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| app.go_back());
        }

        self.remember_origin();
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&cancel);
        *self.screen.borrow_mut() = Screen::AlignProgress;
        self.window.set_child(Some(&self.modal(&page)));
        cancel.grab_focus();

        let (sender, receiver) = std::sync::mpsc::channel();
        let duration = self.duration_s.get();
        let (video_uri, audio_uri) = (video.uri(), audio.uri());
        std::thread::spawn(move || {
            let progress = sender.clone();
            let verdict = crate::align::align(
                &video_uri,
                &audio_uri,
                duration,
                reference,
                // A failed send means nobody is listening any more, which is
                // what cancelling looks like from here. There is no way to
                // stop a decode part-way, so the thread runs to the end and
                // its answer is dropped.
                move |done| {
                    let _ = progress.send(Step::Window(done));
                },
            );
            let _ = sender.send(Step::Done(verdict));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            // Cancelled, or moved on some other way, while it was working.
            if *app.screen.borrow() != Screen::AlignProgress {
                return glib::ControlFlow::Break;
            }
            loop {
                match receiver.try_recv() {
                    Ok(Step::Window(done)) => {
                        // Three steps rather than a smooth climb: a window is
                        // one decode and cannot report its own progress, so
                        // anything finer would be invented.
                        let fraction = done as f64 / crate::align::WINDOWS as f64;
                        bar.set_fraction(fraction);
                        status.set_label(&format!("{:.0}%", fraction * 100.0));
                    }
                    Ok(Step::Done(verdict)) => {
                        app.show_align_result(role, verdict);
                        return glib::ControlFlow::Break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    // The thread is gone without an answer, which leaves
                    // nothing to report and no reason to keep looking.
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return glib::ControlFlow::Break;
                    }
                }
            }
        });
    }

    /// Step three: what it found, and applying it when there is anything to
    /// apply.
    ///
    /// A hidden baseline must never hide a wrong answer, so every outcome is
    /// said out loud. The two that change nothing say so plainly and point at
    /// the sync slider, which is what someone is left with when measuring
    /// cannot help.
    fn show_align_result(self: &Rc<Self>, role: Role, verdict: crate::align::Verdict) {
        use crate::align::Verdict;

        // Never named by output, because the answer is not one: it belongs to
        // this video and this audio file, and applies wherever that file is
        // played.
        let (hint, retry) = match verdict {
            Verdict::Offset { millis, .. } => {
                self.apply_alignment(role, millis);
                let rounded = millis.round();
                let shift = if rounded > 0.0 {
                    format!(
                        "The audio file runs {rounded:.0}ms late, and has been adjusted to \
                         sync with the video."
                    )
                } else if rounded < 0.0 {
                    format!(
                        "The audio file runs {:.0}ms early, and has been adjusted to sync \
                         with the video.",
                        -rounded
                    )
                } else {
                    "The audio file is already in sync with the video, no adjustment needed."
                        .to_string()
                };
                (shift, false)
            }
            // A rate difference is a slope rather than a shift, so no single
            // offset fixes it and averaging one would be a guess that drifts.
            Verdict::RateMismatch { .. } => (
                "The audio file runs at a different speed than the video and cannot be \
                 automatically adjusted."
                    .to_string(),
                true,
            ),
            Verdict::Unsure => (
                "The audio file could not be matched with the video.".to_string(),
                true,
            ),
        };

        let page = self.align_page(&hint);
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Offered only where it could help. Trying another reference track is
        // the answer when the one measured against was a dub and the separate
        // recording was made from the original.
        let again = gtk::Button::with_label("Try another reference track");
        again.add_css_class("tp-button");
        again.add_css_class("tp-action");

        // What the second button means depends on what happened. Where the
        // measurement worked there is nothing to do but accept it; where it
        // did not, the useful thing is to measure again against a different
        // track, and this button becomes the way out beside it.
        let done = gtk::Button::with_label(match retry {
            true => "Cancel",
            false => "Finish",
        });
        done.add_css_class("tp-button");
        if !retry {
            done.add_css_class("tp-action");
        }
        // Cancel first, then the action, which is the order every other pair
        // in the application sits in.
        buttons.append(&done);
        if retry {
            buttons.append(&again);
        }
        page.append(&buttons);

        {
            let app = self.clone();
            again.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_align(role);
            });
        }
        {
            let app = self.clone();
            done.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.go_back();
            });
        }

        self.remember_origin();
        // In the order they now sit, so Tab walks the row left to right.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&done);
        if retry {
            self.add_nav_stop(&again);
        }
        *self.screen.borrow_mut() = Screen::AlignResult;
        self.window.set_child(Some(&self.modal(&page)));
        // Whichever button is the action here: measuring again where that is
        // still worth doing, and accepting the answer where it is not.
        match retry {
            true => again.grab_focus(),
            false => done.grab_focus(),
        };
    }

    /// Writes an alignment down and puts it into force.
    ///
    /// Stored against the two paths together, so the same pairing never pays
    /// for the measuring twice, and read straight back rather than set here -
    /// `load_baselines` owns the sign convention, and two places deciding it
    /// would eventually disagree.
    fn apply_alignment(&self, role: Role, millis: f64) {
        let stored = {
            let file = self.file_for(role).borrow();
            file.as_ref()
                .and_then(Source::local)
                .map(|path| path.to_path_buf())
        };
        if let Some((key, path)) = self.storage_key().zip(stored) {
            crate::config::save_alignment(&key, &path, Some(millis));
        }
        self.load_baselines();
    }

    // --- Settings ------------------------------------------------------

    /// Everything that applies to the application rather than to the video
    /// currently loaded. Reached from the gear in the footer.
    /// What a settings row is called.
    fn item_label(&self, item: Item) -> String {
        match item {
            Item::InterfaceScale => "Interface Size".to_string(),
            Item::Sounds => "Navigation Sounds".to_string(),
            Item::StartFullscreen => "Always Start Fullscreen".to_string(),
            Item::ReadMetadata => "Read Metadata Beside Files".to_string(),
            Item::ShowBackdrop => "Show Backdrop Artwork".to_string(),
            Item::ResumeThreshold => "Resume Threshold".to_string(),
            Item::WatchedThreshold => "Watched Threshold".to_string(),
            Item::Updates => "Check for updates".to_string(),
            Item::UpdateStatus => self.version_label(),
            Item::ClearData => "Clear Saved Playback Data".to_string(),
            Item::Device(_) => "Output Device".to_string(),
            Item::Language(_) => "Preferred Language".to_string(),
            Item::Description(_) => "Prefer Audio Description".to_string(),
            Item::Volume(_) => "Volume".to_string(),
            Item::Sync(_) => "Audio Sync".to_string(),
            Item::SubtitlePreference => "Subtitle Preference".to_string(),
            Item::SubtitleSize => "Subtitle Size".to_string(),
            Item::SubtitleFont => "Subtitle Font".to_string(),
            Item::KodiType(_) => "Configure As".to_string(),
            Item::KodiHandover(_) => "When Kodi Opens TinePlayer".to_string(),
            Item::KodiPermission(_) => "Sandbox Permission".to_string(),
            Item::KodiNone => "No Kodi installations were found on this system".to_string(),
            // Named for what it actually wants. "Add a Kodi Folder" asked for
            // the wrong thing: Kodi's own folder is not where
            // playercorefactory.xml goes, and choosing it lands one level
            // above the folder that is.
            Item::KodiAdd => "Add User Data Folder".to_string(),
            Item::JellyfinConnect => "Connect to Jellyfin".to_string(),
            Item::JellyfinDisconnect => match self.jellyfin_server_label() {
                Some(server) => format!("Disconnect from {server}"),
                None => "Disconnect".to_string(),
            },
            Item::Notices => "Third-Party Notices".to_string(),
        }
    }

    /// What it reads against the label. Empty for the rows that carry a
    /// switch or a bar, which show their state in the control itself, and for
    /// the ones that only open something.
    fn item_value(&self, item: Item) -> String {
        let config = self.config.borrow();
        match item {
            Item::Device(role) => {
                let sink = match role {
                    Role::Primary => config.primary_sink.clone(),
                    Role::Secondary => config.secondary_sink.clone(),
                };
                sink.unwrap_or_else(|| match role {
                    Role::Primary => "Not set".to_string(),
                    Role::Secondary => "None".to_string(),
                })
            }
            Item::Language(role) => {
                let (code, unset) = match role {
                    Role::Primary => (&config.primary_language, "First track"),
                    Role::Secondary => (&config.secondary_language, "Second track"),
                };
                match code {
                    Some(code) => crate::languages::name_for(code),
                    None => unset.to_string(),
                }
            }
            Item::SubtitlePreference => {
                crate::subtitles::describe(config.subtitle_language.as_deref())
            }
            Item::SubtitleFont => config
                .subtitle_font
                .clone()
                .unwrap_or_else(|| crate::pipeline::DEFAULT_SUBTITLE_FONT.to_string()),
            Item::KodiType(index) => {
                drop(config);
                self.with_kodi(index, |setup| {
                    // What it cannot do outranks what it is set to. A Snap is
                    // never set to anything, and saying "Not configured"
                    // invites somebody to try.
                    match setup.confinement.supported() {
                        false => "Not supported".to_string(),
                        true => setup.state.describe().to_string(),
                    }
                })
            }
            Item::KodiHandover(index) => {
                drop(config);
                self.with_kodi(index, |setup| HANDOVER[usize::from(setup.play)].to_string())
            }
            // Not a claim that it has been granted, which nothing here checks.
            // The row opens the instructions, and this says there are some.
            Item::KodiPermission(_) => "Action needed".to_string(),
            Item::UpdateStatus => {
                drop(config);
                self.version_status()
            }
            _ => String::new(),
        }
    }

    /// Whether the switch on this row is on, for the rows that have one.
    fn item_switch(&self, item: Item) -> Option<bool> {
        let config = self.config.borrow();
        Some(match item {
            // On means the size is worked out from the screen, which is the
            // one switch here that turns the bar beside it off rather than on.
            Item::InterfaceScale => config.ui_scale.is_none(),
            Item::Sounds => config.sounds,
            Item::StartFullscreen => config.fullscreen,
            Item::ReadMetadata => config.read_metadata,
            Item::ShowBackdrop => config.show_backdrop,
            Item::Description(Role::Primary) => config.primary_audio_description,
            Item::Description(Role::Secondary) => config.secondary_audio_description,
            Item::Volume(role) => !config.muted(role.key()),
            Item::Sync(role) => config.offset_on(role.key()),
            Item::Updates => config.check_for_updates,
            _ => return None,
        })
    }

    /// A line under the row explaining what it does, for the settings whose
    /// names do not say it.
    ///
    /// Most do not have one, and that is the point: a note under every row is
    /// a wall of text nobody reads, and the ones that matter stop standing
    /// out. These are the settings whose effect is invisible until it happens,
    /// or whose name is a term of art.
    fn item_description(&self, item: Item) -> Option<&'static str> {
        Some(match item {
            Item::ReadMetadata => {
                "Find and read metadata beside video files like .nfo and images often provided by media libraries."
            }
            Item::ShowBackdrop => {
                "If backdrop artwork is found, display it behind the video details."
            }
            Item::ResumeThreshold => {
                "How much of a video should be viewed before offering the choice to resume a previously watched video."
            }
            Item::WatchedThreshold => {
                "How much of a video should be viewed to consider it as watched."
            }
            Item::Language(_) => "Attempt to auto-select a language track for the output.",
            Item::Description(_) => {
                "Attempt to auto-select an Audio Description track for the output."
            }
            Item::Sync(_) => {
                "Adjust the audio sync for the output. Useful for countering latency with bluetooth speakers and headphones."
            }
            Item::SubtitlePreference => "Attempt to auto-select subtitles when available.",
            Item::ClearData => {
                "Delete remembered video preferences, track choices, and resume positions."
            }
            Item::KodiPermission(_) => {
                "This Kodi runs in a sandbox and needs permission before it can start TinePlayer."
            }
            // Says which folder, because the obvious guess is the wrong one:
            // Kodi's user data lives apart from Kodi itself, and it does not
            // exist until Kodi has been run once.
            Item::KodiAdd => {
                "For a Kodi that was not found, such as a portable install. Its user data folder is the one holding guisettings.xml, not the folder Kodi itself is installed in."
            }
            // Says what will happen, because the answer is unusual enough to
            // be worth knowing before pressing it: no password is ever typed
            // into TinePlayer, which is the whole reason this is a code and
            // not a login form.
            Item::JellyfinConnect => {
                "Finds your server and shows a code to enter in a Jellyfin app you are already signed in to. No password is typed here."
            }
            Item::JellyfinDisconnect => {
                "Removes the access token stored on this machine and signs this device out of the server."
            }
            _ => return None,
        })
    }

    /// The note drawn under a row: its explanation, and for one row a link
    /// beside it.
    fn item_note(self: &Rc<Self>, item: Item, scale: f64) -> Option<gtk::Widget> {
        let text = row_note(self.item_description(item)?, scale);

        // Where the data this clears actually lives, openable rather than
        // printed. A path read off a television is a path nobody is going to
        // type, and the folder is the thing wanted anyway - to take a copy of
        // it before pressing the row above, or to see that it is really gone
        // afterwards.
        //
        // The data folder rather than the config one: they are not the same
        // place, and this row does not touch settings.
        //
        // A Kodi's own folder is offered the same way, but under its group
        // heading rather than on a row - see `GroupNote`.
        if item != Item::ClearData {
            return Some(text.upcast());
        }
        let Some(folder) = crate::config::positions_path()
            .parent()
            .map(|folder| folder.to_path_buf())
        else {
            return Some(text.upcast());
        };
        let sentence = text.text().to_string();
        // On the same line as the sentence it belongs to, rather than under
        // it: two lines of small print under one row reads as a paragraph.
        text.set_markup(&format!(
            "{}  <a href=\"{}\">Open user data folder</a>",
            glib::markup_escape_text(&sentence),
            glib::markup_escape_text(&gtk::gio::File::for_path(&folder).uri()),
        ));
        // Reported rather than swallowed: a link that does nothing looks like
        // a link that was pressed wrongly.
        {
            let folder = folder.clone();
            text.connect_activate_link(move |_, _| {
                show_folder(&folder);
                glib::Propagation::Stop
            });
        }

        Some(text.upcast())
    }

    /// Whether the row can be worked at all.
    ///
    /// The rule in every case here is the same: a control over something that
    /// does not exist yet is worse than no control, because it invites a
    /// choice and then does nothing with it. With nothing read from beside the
    /// file there is no artwork to draw. With TinePlayer not registered in a
    /// Kodi there is no entry for a handover setting to be part of, and no
    /// reason to grant that Kodi permission to start us. And an installation
    /// that cannot start an external player at all can be set to nothing.
    fn item_enabled(&self, item: Item) -> bool {
        match item {
            Item::ShowBackdrop => self.config.borrow().read_metadata,
            Item::KodiType(index) => self.with_kodi(index, |setup| setup.confinement.supported()),
            Item::KodiHandover(index) | Item::KodiPermission(index) => self
                .with_kodi(index, |setup| {
                    setup.confinement.supported() && setup.is_configured()
                }),
            // There to be read. Landing on it would be landing on a sentence.
            Item::KodiNone => false,
            _ => true,
        }
    }

    /// What to call the paired server on screen: its own name where it gave
    /// one, and its address otherwise. `None` when there is no pairing at all.
    fn jellyfin_server_label(&self) -> Option<String> {
        self.jellyfin_pairing
            .borrow()
            .as_ref()
            .map(crate::jellyfin::Pairing::label)
    }

    /// Whether there is a token to cast with, as the pane last read it.
    fn jellyfin_connected(&self) -> bool {
        self.jellyfin_pairing
            .borrow()
            .as_ref()
            .is_some_and(crate::jellyfin::Pairing::is_connected)
    }

    /// Whether the Jellyfin pane is what is on screen right now.
    ///
    /// Asked by the two things that can finish long after they were started -
    /// a token going stale, and a server being told about a disconnection - so
    /// that neither redraws a screen nobody is looking at or throws a panel
    /// over a film. The screen is copied out rather than tested in place,
    /// which is the rule `go_back` records: a caller acting on the answer takes
    /// the same cell mutably.
    fn showing_jellyfin_pane(&self) -> bool {
        let screen = *self.screen.borrow();
        screen == Screen::Settings && self.settings_category.get() == Category::Jellyfin
    }

    /// Which of the two shapes the Jellyfin pane takes.
    fn jellyfin_pane(&self) -> JellyfinPane {
        match self.jellyfin_connected() {
            true => JellyfinPane::Connected,
            false => JellyfinPane::NotConnected,
        }
    }

    /// What the Jellyfin heading says under itself.
    ///
    /// What the feature is, since a pane nobody has set up says nothing else
    /// about why it is there - and what pairing leaves behind, which is the
    /// one thing about this worth stating outright. Obfuscating a credential
    /// TinePlayer can read unattended would be theatre; saying where it is is
    /// not.
    fn jellyfin_group_note(&self) -> GroupNote {
        // Which server, and as whom - the two facts the rows used to spend
        // themselves on. Stated rather than offered, since neither is a thing
        // to press: the way to change either is to disconnect and connect
        // again, which is the row underneath.
        let connected = {
            let pairing = self.jellyfin_pairing.borrow();
            pairing.as_ref().map(|pairing| {
                let who = pairing
                    .account
                    .as_ref()
                    .map(|account| account.user_name.clone())
                    .filter(|name| !name.is_empty());
                match who {
                    Some(who) => format!("Connected to {} as {who}.", pairing.label()),
                    None => format!("Connected to {}.", pairing.label()),
                }
            })
        };
        GroupNote {
            sentence: match self.jellyfin_pane() {
                JellyfinPane::NotConnected => {
                    "Connect a Jellyfin server to cast videos to TinePlayer from the Jellyfin app on a phone or tablet. Connecting stores an access token on this machine that can read and stream that library.".to_string()
                }
                JellyfinPane::Connected => format!(
                    "{} Videos can be cast to TinePlayer from the Jellyfin app on a phone or tablet. The access token stored on this machine can read and stream this library, and disconnecting removes it.",
                    connected.unwrap_or_default(),
                ),
            },
            // Named rather than opened. The folder holds the token, and a
            // settings screen that offers to show somebody their own
            // credential in a file manager is offering the wrong thing.
            folder: None,
        }
    }

    /// What one installation's group heading says under itself: which file it
    /// is, and either why it cannot be used or the thing true of every Kodi
    /// and invisible until it bites - it reads that file once, at startup, so
    /// a change made here does nothing until it restarts.
    fn kodi_group_note(&self, index: usize) -> Option<GroupNote> {
        self.with_kodi_setup(index, |setup| GroupNote {
            // An installation that cannot be used says why instead. Nothing
            // will be modified there, so promising that it will would be the
            // one sentence on the screen that is not true.
            sentence: setup
                .confinement
                .unsupported_reason()
                .unwrap_or(
                    "This installation's playercorefactory.xml will be modified. Restart Kodi for changes to take effect.",
                )
                .to_string(),
            folder: Some(setup.userdata().to_path_buf()),
        })
    }

    /// One installation out of the list the pane was built from, by its place
    /// in it, with a default for the moment the list has moved on from under a
    /// row that was built against it.
    fn with_kodi<T: Default>(
        &self,
        index: usize,
        read: impl FnOnce(&crate::kodi_setup::Setup) -> T,
    ) -> T {
        self.with_kodi_setup(index, read).unwrap_or_default()
    }

    /// The same, for callers that need to tell "no such installation" apart
    /// from whatever the answer would have been.
    fn with_kodi_setup<T>(
        &self,
        index: usize,
        read: impl FnOnce(&crate::kodi_setup::Setup) -> T,
    ) -> Option<T> {
        self.kodi_setups.borrow().get(index).map(read)
    }

    /// Opens the settings screen from outside it, at the categories.
    ///
    /// Coming back from a chooser or from About calls `show_settings` directly
    /// and keeps whichever half of the screen the keyboard was in; arriving
    /// from the menu starts where the screen starts.
    fn enter_settings(self: &Rc<Self>) {
        self.in_settings_pane.set(false);
        self.show_settings();
    }

    /// Settings, as a column of categories and the rows of whichever one is
    /// chosen.
    ///
    /// One flat list of twenty-three rows before this, which is how it came to
    /// hold two rows called Volume and two called Audio Sync with nothing but
    /// their position to tell them apart.
    fn show_settings(self: &Rc<Self>) {
        let scale = self.scale.get();
        let px = |base: f64| (base * scale).round() as i32;
        let (page, list, back, _header) = list_page("Settings", true);

        // A fifth of what the window has, so a bar is a consistent share of
        // the screen whether that is a laptop or a television. The monitor
        // stands in before the window has been given a size.
        let slider_width = match self.window.width() {
            0 => appearance::monitor_for_window(&self.window)
                .map(|monitor| monitor.geometry().width())
                .unwrap_or(1920),
            width => width,
        } / 5;

        // The right-hand pane, rebuilt in place when the category changes
        // rather than by rebuilding the screen: the cursor is in the column on
        // the left at that moment, and rebuilding around it would take it away.
        // The list comes out of its scroller so a block of text can sit above
        // it inside the same one, which is what makes the two scroll together.
        // Taken out first: `gtk_box_append` refuses a widget that still has a
        // parent, and says so only in a log nobody is reading.
        let scroller = list
            .parent()
            .and_then(|viewport| viewport.parent())
            .and_downcast::<gtk::ScrolledWindow>();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        if let Some(scroller) = scroller.as_ref() {
            scroller.set_child(None::<&gtk::Widget>);
            let column = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .build();
            column.append(&body);
            column.append(&list);
            scroller.set_child(Some(&column));
            // What the arrows move when there is text rather than rows to move
            // through - see `reading_about`, which decides when that is.
            *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        }

        let fill: Rc<Fill> = {
            let list = list.clone();
            let body = body.clone();
            Rc::new(move |app: &Rc<Self>| {
                // What this category says for itself, before its rows.
                while let Some(child) = body.first_child() {
                    body.remove(&child);
                }
                *app.settings_body.borrow_mut() = match app.settings_category.get() {
                    Category::About => {
                        let text = app.about_body();
                        body.append(&text);
                        Some(text)
                    }
                    _ => None,
                };
                // Where Ctrl+A and Ctrl+C look for text. Set here as well as
                // in `settings_stage`, because choosing a category refills the
                // pane without going through it - so About selected from the
                // column had a body on screen and nothing pointing at it.
                *app.copy_root.borrow_mut() =
                    app.settings_body.borrow().clone().map(|body| body.upcast());
                while let Some(row) = list.row_at_index(0) {
                    list.remove(&row);
                }
                app.settings_switches.borrow_mut().clear();
                app.settings_sliders.borrow_mut().clear();

                // Found once per build of the pane, not once per row: it walks
                // the disk looking for Kodi, and every label, value and note on
                // those rows is read back out of this. Every installation now,
                // not only the configured ones - discovering them was the
                // wizard's first screen, and a pane that lists them needs no
                // such screen.
                if app.settings_category.get() == Category::Kodi {
                    *app.kodi_setups.borrow_mut() = app.known_kodis();
                }
                // Re-read for the same reason, and it is the more important of
                // the two: the token in that file can be revoked from a
                // dashboard on the other side of the house, and this pane is
                // where somebody comes to find out that it was.
                if app.settings_category.get() == Category::Jellyfin {
                    *app.jellyfin_pairing.borrow_mut() = crate::jellyfin::load();
                }
                let panes: Vec<KodiPane> = app
                    .kodi_setups
                    .borrow()
                    .iter()
                    .map(|setup| KodiPane {
                        heading: setup.label().to_uppercase(),
                        confinement: setup.confinement,
                    })
                    .collect();
                let entries = app
                    .settings_category
                    .get()
                    .items(&panes, app.jellyfin_pane());
                *app.pane_items.borrow_mut() = entries.iter().map(|(_, item)| *item).collect();

                for (index, (_, item)) in entries.iter().enumerate() {
                    let item = *item;
                    let label = app.item_label(item);
                    let enabled = app.item_enabled(item);

                    // Three kinds of row, and which one it is belongs to the
                    // item rather than to where it sits.
                    let widget = match (item.slider(), app.item_switch(item)) {
                        (Some(kind), on) => {
                            let (now, reading) = app.slider_state(kind);
                            let (widget, bar, value, switch) =
                                slider_row(&label, slider_width, kind.range(), now, &reading, on);
                            if kind == Slider::Scale {
                                let by_hand = app.config.borrow().ui_scale.is_some();
                                bar.set_sensitive(by_hand);
                                value.set_sensitive(by_hand);
                            }
                            app.wire_slider(kind, &bar, &value);
                            if let Some(switch) = switch {
                                app.settings_switches.borrow_mut().push((item, switch));
                            }
                            app.settings_sliders
                                .borrow_mut()
                                .push((item, kind, bar, value));
                            widget
                        }
                        (None, Some(on)) => {
                            let (widget, switch) = switch_row(&label, on);
                            switch.set_sensitive(enabled);
                            app.settings_switches.borrow_mut().push((item, switch));
                            widget
                        }
                        (None, None) => menu_row(&label, &app.item_value(item), enabled),
                    };

                    // The note goes inside the row rather than under it as a
                    // row of its own, which is what keeps it out of the way of
                    // everything: it cannot be selected, cannot be arrowed on
                    // to, and does not shift the numbering the pane is read by.
                    let widget = match app.item_note(item, scale) {
                        Some(note) => {
                            let stack = gtk::Box::builder()
                                .orientation(gtk::Orientation::Vertical)
                                .build();
                            stack.append(&widget);
                            stack.append(&note);
                            stack.upcast::<gtk::Widget>()
                        }
                        None => widget.upcast::<gtk::Widget>(),
                    };

                    let name = row_name(&label, &app.item_value(item));
                    append_named(&list, &widget, &name);
                    let Some(row) = list.row_at_index(index as i32) else {
                        continue;
                    };
                    row.set_sensitive(enabled);
                    if item == Item::UpdateStatus {
                        app.watch_update_row(&row);
                    }
                }

                // Each switch reports its own presses, now that it takes them
                // rather than letting them fall through to the row. Guarded
                // against the moves made from here when the same setting is
                // worked another way.
                for (item, switch) in app.settings_switches.borrow().iter() {
                    let app = app.clone();
                    let item = *item;
                    switch.connect_state_set(move |_, _| {
                        if !app.settling_switch.get() {
                            app.sounds.borrow().click();
                            app.apply_switch_item(item);
                        }
                        glib::Propagation::Proceed
                    });
                }

                // A heading above the row that opens a group, by the same
                // mechanism the media page uses: headers are not rows, so they
                // cannot be landed on.
                let headings: Vec<Option<String>> = entries
                    .iter()
                    .map(|(heading, _)| heading.as_ref().map(|text| text.to_string()))
                    .collect();
                // What each heading says under itself, by the row it sits
                // above. Only a Kodi group has one, and it belongs to the
                // installation rather than to the row that opens it - which is
                // why it is here and not a note on Player Type, where it read
                // as an explanation of that one setting.
                let notes: Vec<Option<GroupNote>> = entries
                    .iter()
                    .map(|(heading, item)| match (heading, item) {
                        (Some(_), Item::KodiType(index)) => app.kodi_group_note(*index),
                        (Some(_), Item::JellyfinConnect | Item::JellyfinDisconnect) => {
                            Some(app.jellyfin_group_note())
                        }
                        _ => None,
                    })
                    .collect();
                list.set_header_func(move |row, _| {
                    let index = row.index();
                    match headings.get(index as usize).and_then(Option::as_deref) {
                        Some(heading) => row.set_header(Some(&group_header(
                            heading,
                            notes.get(index as usize).and_then(Option::as_ref),
                            scale,
                            index == 0,
                        ))),
                        None => row.set_header(None::<&gtk::Widget>),
                    }
                });
                app.refresh_version_row();
            })
        };
        fill(self);

        // The categories, down the left.
        let (categories_scroller, categories) = scrolling_list();
        categories_scroller.set_size_request(px(CATEGORY_WIDTH), -1);
        for category in Category::ALL {
            append_named(
                &categories,
                &menu_row(category.title(), "", true),
                category.title(),
            );
        }
        if let Some(row) = Category::ALL
            .iter()
            .position(|category| *category == self.settings_category.get())
            .and_then(|index| categories.row_at_index(index as i32))
        {
            categories.select_row(Some(&row));
        }
        // Immediately, on the selection moving, rather than on the row being
        // activated: this is a column of what is being looked at, not a list of
        // things to do, and having to press a category to see it is a step that
        // says nothing.
        {
            let app = self.clone();
            let fill = fill.clone();
            categories.connect_row_selected(move |_, row| {
                let Some(category) = row
                    .map(|row| row.index() as usize)
                    .and_then(|index| Category::ALL.get(index).copied())
                else {
                    return;
                };
                if category == app.settings_category.get() {
                    return;
                }
                app.settings_category.set(category);
                // The remembered row belongs to the category it was in.
                *app.settings_row.borrow_mut() = 0;
                fill(&app);
            });
        }

        // Both panes on grounds of their own, the way the media page's rows
        // are: two lists side by side on a bare page have nothing to say where
        // either one ends.
        let Some(listing) = page.last_child() else {
            return;
        };
        page.remove(&listing);
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(16.0))
            .vexpand(true)
            .build();
        for (pane, expand, ground) in [
            (
                categories_scroller.clone().upcast::<gtk::Widget>(),
                false,
                "tp-bare",
            ),
            (listing.clone(), true, "tp-menu-panel"),
        ] {
            let panel = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .hexpand(expand)
                .css_classes([ground])
                .build();
            panel.append(&pane);
            columns.append(&panel);
        }
        page.append(&columns);

        // Watched in the capture phase, so a press is known about before
        // anything else handles it. Cleared on the way out rather than on
        // release, because the row is activated in between - and a press that
        // never activates a row must not leave the next key press looking like
        // a click.
        {
            let app = self.clone();
            let click = gtk::GestureClick::new();
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(move |_, _, _, _| app.clicked_row.set(true));
            let app = self.clone();
            click.connect_released(move |_, _, _, _| {
                let app = app.clone();
                glib::idle_add_local_once(move || app.clicked_row.set(false));
            });
            list.add_controller(click);
        }

        *self.settings_list.borrow_mut() = Some(list.clone());

        {
            let app = self.clone();
            list.connect_row_activated(move |_, row| {
                let Some(item) = app.item_at(row.index()) else {
                    return;
                };
                // A switch is worked by pressing the switch, not by clicking
                // the row it sits on: the row is a wide target, and hitting it
                // on the way past should not change a setting. Enter on the
                // selected row still does, which arrives here with nothing
                // having been clicked.
                if app.clicked_row.replace(false) && item.has_switch() {
                    return;
                }
                // A switch row is answered by the switch, which plays its own
                // click when it moves. Playing one here too would double it.
                if !item.has_switch() {
                    app.sounds.borrow().click();
                }
                // Remembered so returning from a chooser lands back on the row
                // it was opened from, as the main menu does.
                *app.settings_row.borrow_mut() = row.index();
                app.activate_item(item, row);
            });
        }
        {
            let app = self.clone();
            back.connect_clicked(move |_| app.show_menu());
        }

        // Enter hands the keyboard to the settings beside the category.
        {
            let app = self.clone();
            categories.connect_row_activated(move |_, _| {
                app.sounds.borrow().click();
                app.hold_settings_pane();
            });
        }

        // Both lists are wired for the arrows, and which of them the arrows
        // are actually driving is settled below by `set_nav`. Deliberately not
        // `nav_side_list`, which is how the browser puts its drives column in
        // the order beside its listing: that is what makes left and right step
        // between two lists, and left and right are spoken for here.
        self.wire_navigation(&list, std::slice::from_ref(&back), &[]);
        self.wire_arrows(categories.upcast_ref());
        announce_selection(&categories);
        *self.settings_categories.borrow_mut() = Some(categories.clone());

        // Tab moves the focus without going through either handler above, so
        // each pane says so for itself when the focus arrives. Without this the
        // arrows carried on driving the pane that was left behind.
        for (widget, pane) in [(categories.clone(), false), (list.clone(), true)] {
            let app = self.clone();
            let controller = gtk::EventControllerFocus::new();
            controller.connect_enter(move |_| {
                if *app.screen.borrow() != Screen::Settings {
                    return;
                }
                if app.in_settings_pane.get() != pane {
                    app.settings_stage(pane);
                }
                if pane {
                    app.select_focused_row();
                }
            });
            widget.add_controller(controller);
        }

        *self.screen.borrow_mut() = Screen::Settings;
        self.window.set_child(Some(&page));
        // Back where it was left. Coming out of a chooser returns to the row
        // that opened it, which is in the pane; arriving fresh starts in the
        // categories.
        match self.in_settings_pane.get() {
            true => self.hold_settings_pane(),
            false => self.hold_settings_categories(),
        }
    }

    /// Whether the settings screen is the one on display.
    fn on_settings(&self) -> bool {
        *self.screen.borrow() == Screen::Settings
    }

    /// Says which of the two panes the arrows are driving, without moving the
    /// focus itself.
    ///
    /// Split from the two below because the focus can arrive on its own: Tab
    /// steps between the panes, and the pane it lands on has to start taking
    /// the arrow keys without being asked to grab a focus it already has.
    ///
    /// Both lists stay in the tab order either way, which is what Tab moves
    /// through. That is also why left and right are kept away from
    /// `move_between_lists`, which walks the very same list of stops: it is the
    /// tab order and the left-right order at once everywhere else, and here
    /// those two need different answers.
    fn settings_stage(&self, pane: bool) {
        let (Some(list), Some(categories)) = (
            self.settings_list.borrow().clone(),
            self.settings_categories.borrow().clone(),
        ) else {
            return;
        };
        let Some(back) = self.nav_header.borrow().first().cloned() else {
            return;
        };
        self.in_settings_pane.set(pane);
        match pane {
            true => self.set_nav(Some(&list), std::slice::from_ref(&back), &[]),
            false => self.set_nav(Some(&categories), std::slice::from_ref(&back), &[]),
        }
        // After `set_nav`, which clears it: that is how a screen without
        // selectable text is sure of not leaving the last one's behind.
        *self.copy_root.borrow_mut() = self
            .settings_body
            .borrow()
            .clone()
            .map(|body| body.upcast());
        // Rewritten after `set_nav`, which builds the order from the one list
        // it was given. Tab should reach both, in the order they are read.
        *self.nav_stops.borrow_mut() = vec![back.upcast(), categories.upcast(), list.upcast()];
    }

    /// Gives the keyboard to the settings themselves.
    fn hold_settings_pane(self: &Rc<Self>) {
        let Some(list) = self.settings_list.borrow().clone() else {
            return;
        };
        // Nothing to step into: a category with no rows would take the keys
        // and answer nothing, and Escape would be the only way out.
        if list.row_at_index(0).is_none() {
            return;
        }
        self.settings_stage(true);
        let remembered = (*self.settings_row.borrow()).min(last_row_index(&list));
        if let Some(row) = list.row_at_index(remembered) {
            list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Gives it back to the column of categories.
    fn hold_settings_categories(self: &Rc<Self>) {
        let Some(categories) = self.settings_categories.borrow().clone() else {
            return;
        };
        self.settings_stage(false);
        if let Some(row) = Category::ALL
            .iter()
            .position(|category| *category == self.settings_category.get())
            .and_then(|index| categories.row_at_index(index as i32))
        {
            categories.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// Selects the row the focus has just landed in.
    ///
    /// A switch or a bar takes the focus when it is clicked, which carries it
    /// into the pane without going through the arrow keys - and the list's own
    /// arrival handler answers a list with nothing selected by selecting its
    /// first row. Clicking a switch two thirds of the way down therefore lit
    /// the row at the top. The row under the pointer is the one meant.
    fn select_focused_row(&self) {
        let Some(list) = self.settings_list.borrow().clone() else {
            return;
        };
        let Some(mut widget) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return;
        };
        // Up from whatever took the focus to the row holding it, which may be
        // a switch inside a box inside the row.
        loop {
            if let Some(row) = widget.downcast_ref::<gtk::ListBoxRow>()
                && row.parent().as_ref() == Some(list.upcast_ref::<gtk::Widget>())
            {
                list.select_row(Some(row));
                *self.settings_row.borrow_mut() = row.index();
                return;
            }
            match widget.parent() {
                Some(parent) => widget = parent,
                None => return,
            }
        }
    }

    /// Which setting a row in the right-hand pane is.
    fn item_at(&self, index: i32) -> Option<Item> {
        self.pane_items.borrow().get(index as usize).copied()
    }

    /// Takes the mark off the settings button once the version row is reached.
    ///
    /// Arriving on it is the moment somebody has been told, and pressing it
    /// should not be required to stop being nagged about something already
    /// seen. Attached whether or not there is anything new, since a check
    /// finishing while this screen is open can make there be.
    fn watch_update_row(self: &Rc<Self>, row: &gtk::ListBoxRow) {
        let app = self.clone();
        let controller = gtk::EventControllerFocus::new();
        controller.connect_enter(move |_| {
            let mut state = app.updates.borrow_mut();
            crate::updates::acknowledge(&mut state);
            drop(state);
            app.draw_update_badge();
        });
        row.add_controller(controller);
    }

    /// What a row does when it is chosen.
    fn activate_item(self: &Rc<Self>, item: Item, row: &gtk::ListBoxRow) {
        if let Some(setting) = item.setting() {
            self.show_selector(setting, row);
            return;
        }
        if item.has_switch() {
            self.work_switch_item(item);
            return;
        }
        match item {
            Item::ClearData => self.confirm_clear_data(),
            // Home, every time. Kodi's userdata lives under it on every
            // platform, and where the video browser was last says nothing
            // about where Kodi keeps its settings.
            Item::KodiAdd => self.show_kodi_folder(&crate::browser::home()),
            Item::KodiPermission(index) => self.show_kodi_permission(index),
            Item::JellyfinConnect => self.start_jellyfin_connect(ConnectFrom::Settings),
            Item::JellyfinDisconnect => self.confirm_jellyfin_disconnect(),
            Item::Notices => self.show_notices(),
            Item::UpdateStatus => self.open_release_page(),
            _ => {}
        }
    }

    /// Wires a bar to the setting it moves.
    fn wire_slider(self: &Rc<Self>, kind: Slider, bar: &gtk::Scale, value: &gtk::Label) {
        {
            let app = self.clone();
            let value = value.clone();
            bar.connect_change_value(move |_, scroll, moved| {
                app.set_slider(kind, moved, &value);
                if kind == Slider::Scale {
                    // A drag reports Jump, over and over, while the pointer
                    // holds the bar. Anything else - a step, a page, a scroll
                    // wheel - is finished by the time it arrives and can be
                    // drawn straight away.
                    if scroll == gtk::ScrollType::Jump {
                        app.wanted_scale.set(Some(moved));
                    } else {
                        app.apply_scale(moved);
                    }
                }
                glib::Propagation::Proceed
            });
        }
        // Let go of, and only then redrawn. Watched rather than handled, so the
        // bar keeps its own grip on the pointer while it is being dragged.
        if kind == Slider::Scale {
            let app = self.clone();
            let watcher = gtk::EventControllerLegacy::new();
            watcher.set_propagation_phase(gtk::PropagationPhase::Bubble);
            watcher.connect_event(move |_, event| {
                let done = matches!(
                    event.event_type(),
                    gdk::EventType::ButtonRelease | gdk::EventType::TouchEnd
                );
                if done && let Some(steps) = app.wanted_scale.take() {
                    app.apply_scale(steps);
                }
                glib::Propagation::Proceed
            });
            bar.add_controller(watcher);
        }
    }

    /// Turns the described-audio preference on or off for one output.
    ///
    /// A toggle rather than a chooser: there are two answers, and a screen to
    /// pick between them would be a screen with two rows on it.
    fn toggle_audio_description(self: &Rc<Self>, primary: bool) {
        {
            let mut config = self.config.borrow_mut();
            if primary {
                config.primary_audio_description = !config.primary_audio_description;
            } else {
                config.secondary_audio_description = !config.secondary_audio_description;
            }
            let _ = config.save();
        }
        // In place rather than rebuilding the screen: a rebuild reselects the
        // row but loses where the list was scrolled to, which threw the row
        // being pressed off the screen.
        let on = if primary {
            self.config.borrow().primary_audio_description
        } else {
            self.config.borrow().secondary_audio_description
        };
        self.set_settings_switch(
            if primary {
                Item::Description(Role::Primary)
            } else {
                Item::Description(Role::Secondary)
            },
            on,
        );
    }

    /// Moves the switch on a settings row to match what it now reports.
    fn set_settings_switch(&self, item: Item, on: bool) {
        self.settling_switch.set(true);
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
        {
            switch.set_active(on);
        }
        self.settling_switch.set(false);
    }

    /// Works the switch on a row the way a click on it would.
    ///
    /// Through the switch rather than straight to the setting, because GTK
    /// only runs the sliding animation from the switch's own gesture and
    /// activation. Setting its state moves it there in one frame, which is
    /// what made a key press look different from a click.
    fn work_switch_item(self: &Rc<Self>, item: Item) {
        let switch = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(row, _)| *row == item)
            .map(|(_, switch)| switch.clone());
        match switch {
            // Its own handler carries on from here, as it does for a click.
            Some(switch) => {
                switch.activate();
            }
            None => self.apply_switch_item(item),
        }
    }

    /// What a switch row actually changes, once something has asked for it.
    fn apply_switch_item(self: &Rc<Self>, item: Item) {
        match item {
            Item::InterfaceScale => self.toggle_automatic_scale(),
            Item::Sounds => self.toggle_sounds(),
            Item::StartFullscreen => self.toggle_start_fullscreen(),
            Item::ReadMetadata => self.toggle_read_metadata(),
            Item::ShowBackdrop => self.toggle_show_backdrop(),
            Item::Description(role) => self.toggle_audio_description(role == Role::Primary),
            Item::Volume(_) => self.toggle_settings_mute(item),
            Item::Sync(_) => self.toggle_settings_offset(item),
            Item::Updates => self.toggle_update_checks(),
            _ => {}
        }
    }

    /// Turns "open fullscreen" on or off.
    ///
    /// Only this changes it. Pressing F11 or the fullscreen mark is about the
    /// session in hand and leaves this alone - see [`App::toggle_fullscreen`].
    fn toggle_start_fullscreen(self: &Rc<Self>) {
        let mut config = self.config.borrow_mut();
        config.fullscreen = !config.fullscreen;
        let _ = config.save();
    }

    /// Turns the reading of sidecars and artwork beside a video on or off.
    ///
    /// The page is rebuilt afterwards, since what it can show has changed -
    /// and the backdrop row with it, which is only workable while this is on.
    fn toggle_read_metadata(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.read_metadata = !config.read_metadata;
            let _ = config.save();
        }
        self.reread_details();
        // The one row this governs, redrawn where it stands.
        //
        // Rebuilding the whole screen was what this did, and it moved the
        // cursor every time: a switch is worked without activating its row, so
        // the remembered row is whatever was last activated, and coming back
        // in lands on that instead of on the switch just pressed.
        self.refresh_backdrop_row();
    }

    /// Turns the backdrop row on or off to match whether there is anything
    /// to draw, without disturbing the screen around it.
    fn refresh_backdrop_row(&self) {
        let enabled = self.config.borrow().read_metadata;
        if let Some((_, switch)) = self
            .settings_switches
            .borrow()
            .iter()
            .find(|(item, _)| *item == Item::ShowBackdrop)
        {
            switch.set_sensitive(enabled);
        }
        let Some(index) = self
            .pane_items
            .borrow()
            .iter()
            .position(|item| *item == Item::ShowBackdrop)
        else {
            return;
        };
        let list = self.settings_list.borrow().clone();
        if let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) {
            row.set_sensitive(enabled);
        }
    }

    /// Turns the film's fanart behind the media page on or off.
    fn toggle_show_backdrop(self: &Rc<Self>) {
        {
            let mut config = self.config.borrow_mut();
            config.show_backdrop = !config.show_backdrop;
            let _ = config.save();
        }
        self.reread_details();
    }

    /// Reads what is beside the file again, after a setting changed what may
    /// be read at all.
    ///
    /// Nothing to do without a file: the answer is about a video, and the
    /// next one loaded will be read under whatever the setting now says.
    fn reread_details(self: &Rc<Self>) {
        let Some(source) = self.file.borrow().clone() else {
            return;
        };
        let beside = {
            let config = self.config.borrow();
            crate::metadata::Beside {
                metadata: config.read_metadata,
                backdrop: config.show_backdrop,
            }
        };
        let media = crate::probe::Media {
            audio: Vec::new(),
            subtitles: Vec::new(),
            duration_ns: 0,
            video: self.details.borrow().video.clone(),
            tags: Default::default(),
        };
        let mut details = crate::metadata::resolve(&source, &media, beside, &self.launcher_title());
        // The parts that came from the container rather than from beside the
        // file are already known and are not re-probed for a toggle.
        let held = self.details.borrow();
        details.duration_s = held.duration_s;
        details.container = held.container.clone();
        drop(held);
        *self.details.borrow_mut() = details;
        *self.poster_art.borrow_mut() = None;
        *self.backdrop_art.borrow_mut() = None;
        self.art_generation.set(self.art_generation.get() + 1);
        self.start_art_load();
    }

    /// Turns the version check on or off.
    ///
    /// Rebuilds the screen rather than only moving the switch, because the row
    /// underneath comes and goes with it. Turning it on asks straight away:
    /// somebody who has just switched it on is asking the question now, and
    /// waiting until tomorrow to answer would look like it does not work.
    fn toggle_update_checks(self: &Rc<Self>) {
        let on = {
            let mut config = self.config.borrow_mut();
            config.check_for_updates = !config.check_for_updates;
            let _ = config.save();
            config.check_for_updates
        };
        if on {
            self.check_for_updates(true);
        }
        self.set_settings_switch(Item::Updates, on);
        self.refresh_version_row();
    }

    /// The version this is, on the left of its row.
    fn version_label(&self) -> String {
        format!("Current Version: v{}", env!("CARGO_PKG_VERSION"))
    }

    /// What the check made of it, on the right, or nothing while checking is
    /// off. "Up to date" rather than "Latest", which beside an arrow read as
    /// an instruction to go and get the latest rather than as a statement
    /// that this is it.
    fn version_status(&self) -> String {
        if !self.config.borrow().check_for_updates {
            return String::new();
        }
        match crate::updates::newer(&self.updates.borrow()) {
            Some((version, _)) => {
                format!(
                    "Update available: v{}",
                    version.trim_start_matches(['v', 'V'])
                )
            }
            None => "Up to date".to_string(),
        }
    }

    /// Redraws the row naming the version, in place.
    ///
    /// In place rather than by rebuilding the screen: turning the check on or
    /// off changes two words, and rebuilding for it threw the whole page away
    /// and drew it again - which flickers and moves every row under whatever
    /// was pointing at one.
    fn refresh_version_row(&self) {
        // Found by asking which row is the version one, rather than by a fixed
        // number: it is only in the pane at all when General is the category
        // being shown.
        let Some(index) = self
            .pane_items
            .borrow()
            .iter()
            .position(|item| *item == Item::UpdateStatus)
        else {
            return;
        };
        let list = self.settings_list.borrow().clone();
        let Some(row) = list.and_then(|list| list.row_at_index(index as i32)) else {
            return;
        };
        let (label, value) = (self.version_label(), self.version_status());
        let widget = menu_row(&label, &value, true);
        // The arrow means "this opens something", so it belongs only when
        // there is a release to go and look at.
        let newer = crate::updates::newer(&self.updates.borrow()).is_some();
        if let Some(chevron) = widget.last_child() {
            chevron.set_visible(newer);
        }
        row.set_child(Some(&widget));
        name_it(&row, &row_name(&label, &value));
        if newer {
            row.add_css_class("tp-badge-row");
        } else {
            row.remove_css_class("tp-badge-row");
        }
    }

    /// Opens the release page in whatever the machine uses for links.
    fn open_release_page(self: &Rc<Self>) {
        let url = {
            let state = self.updates.borrow();
            crate::updates::newer(&state).map(|(_, url)| url.to_string())
        };
        if let Some(url) = url {
            gtk::gio::AppInfo::launch_default_for_uri(&url, None::<&gtk::gio::AppLaunchContext>)
                .unwrap_or_else(|e| eprintln!("Could not open {url}: {e}"));
        }
    }

    /// Looks for a newer release, unless it is too soon to ask again.
    ///
    /// Off the main thread and reported back through a polled channel, the
    /// same way reading a video is: everything it touches afterwards is `Rc`
    /// and belongs to this thread.
    fn check_for_updates(self: &Rc<Self>, now: bool) {
        if !self.config.borrow().check_for_updates {
            return;
        }
        let previous = self.updates.borrow().clone();
        if !now && !crate::updates::due(&previous) {
            return;
        }

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::updates::check(&previous));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
            let state = match receiver.try_recv() {
                Ok(state) => state,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            crate::updates::save(&state);
            *app.updates.borrow_mut() = state;
            app.draw_update_badge();
            // Only if Settings is open behind it, so the answer appears
            // rather than waiting to be opened again. The one row, not the
            // screen: rebuilding it under somebody reading it is a flicker
            // and a jump for the sake of two words.
            if *app.screen.borrow() == Screen::Settings {
                app.refresh_version_row();
            }
            glib::ControlFlow::Break
        });
    }

    /// Marks or unmarks the button that opens Settings.
    ///
    /// The mark says there is something in there worth seeing, which is true
    /// exactly until somebody has seen it - so reaching the row that names the
    /// version clears this, while the row keeps its own mark for as long as
    /// the version is there to be had.
    fn draw_update_badge(&self) {
        let wanted = crate::updates::unseen(&self.updates.borrow());
        for button in self.update_badges.borrow().iter() {
            if wanted {
                button.add_css_class("tp-badge");
            } else {
                button.remove_css_class("tp-badge");
            }
        }
    }

    fn toggle_sounds(self: &Rc<Self>) {
        let (enabled, device) = {
            let mut config = self.config.borrow_mut();
            config.sounds = !config.sounds;
            let _ = config.save();
            (config.sounds, config.primary_sink.clone())
        };
        *self.sounds.borrow_mut() = Sounds::new(enabled, device);
        self.set_settings_switch(Item::Sounds, enabled);
    }

    /// Hands the size back to the screen, or takes it over by hand.
    ///
    /// Taking it over keeps whatever is on screen now, so the switch changes
    /// who decides the size rather than the size itself.
    fn toggle_automatic_scale(self: &Rc<Self>) {
        let now_automatic = self.config.borrow().ui_scale.is_some();
        {
            let mut config = self.config.borrow_mut();
            // Taking it over keeps what is on screen, so the switch changes
            // who decides the size rather than the size itself.
            config.ui_scale = if now_automatic {
                None
            } else {
                Some(self.scale.get())
            };
            let _ = config.save();
        }
        if now_automatic {
            self.follow_automatic_scale(&self.window.clone());
        }
        let found = self
            .settings_sliders
            .borrow()
            .iter()
            .find(|(row, ..)| *row == Item::InterfaceScale)
            .map(|(_, kind, scale, value)| (*kind, scale.clone(), value.clone()));
        if let Some((kind, scale, value)) = found {
            let (now, reading) = self.slider_state(kind);
            scale.set_value(now);
            value.set_text(&reading);
            scale.set_sensitive(!now_automatic);
            value.set_sensitive(!now_automatic);
        }
        self.set_settings_switch(Item::InterfaceScale, now_automatic);
    }

    /// Redraws the interface at the size the bar is now at.
    fn apply_scale(self: &Rc<Self>, steps: f64) {
        let scale = scale_from_steps(steps);
        if scale != self.scale.get() {
            self.restyle(scale);
        }
        let _ = self.config.borrow().save();
    }

    /// Re-renders at whatever the automatic size should be now.
    ///
    /// The screen's own scale while the window fills it, and 1x while it does
    /// not. The automatic size exists for a television read from a sofa, and
    /// a window on the same 4K monitor is read from arm's length - scaling
    /// that up only leaves less room in a window somebody chose the size of.
    ///
    /// A size set by hand is that size in both, which is what asking for one
    /// means.
    fn follow_automatic_scale(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        if self.config.borrow().ui_scale.is_some() {
            return;
        }
        let wanted = if window.is_fullscreen() {
            appearance::monitor_for_window(window)
                .map(|monitor| appearance::scale_for(&monitor))
                .unwrap_or(1.0)
        } else {
            1.0
        };
        if wanted != self.scale.get() {
            self.restyle(wanted);
        }
    }

    /// Re-renders every size in the interface at a new scale.
    fn restyle(self: &Rc<Self>, scale: f64) {
        self.scale.set(scale);
        self.styles.load_from_data(&style_css(scale));

        // The stylesheet is only half of a size. Everything drawn rather than
        // styled takes its size in Rust at the moment the page is built - the
        // poster, the marks on the buttons, every margin, the width the page
        // is held to - and none of that moves when the stylesheet is
        // reloaded. Restyling alone therefore left the two halves disagreeing:
        // type at the new size inside a page laid out for the old one.
        //
        // It shows worst where the change is largest. A 4K television picks
        // 2x, so a page built at 1x and restyled kept a half-size poster and
        // half-size margins under full-size text, and the whole composition
        // sat in the top of the screen with the bottom third empty.
        //
        // Rebuilding is cheap here and this happens on a monitor change or a
        // fullscreen toggle, not on a drag.
        if *self.screen.borrow() == Screen::Menu {
            let app = self.clone();
            glib::idle_add_local_once(move || {
                if *app.screen.borrow() == Screen::Menu {
                    app.show_menu();
                }
            });
        }
    }

    /// What is running, who wrote what it is built on, and under what terms.
    ///
    /// Prose rather than the two version rows this replaced: the versions were
    /// only ever there to be read out when something went wrong, and the
    /// licenses of the work TinePlayer is built on ask to be acknowledged
    /// somewhere a person can find them. A packaged application with no About
    /// page has nowhere to put either.
    /// What About says, as a block of widgets rather than a screen.
    ///
    /// It was a screen reached from a row, which put the version, the license
    /// and where the settings file lives two steps and a page transition away
    /// from a viewer looking for exactly those things. The About category
    /// shows this directly, with the notices below it as the one row.
    fn about_body(self: &Rc<Self>) -> gtk::Box {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(12.0))
            // Room of its own inside the panel. Prose against the edge of a
            // box reads as something that overflowed into it, where the rows
            // below have their own padding and look placed.
            .margin_top(px(ABOUT_INSET))
            .margin_bottom(px(ABOUT_INSET))
            .margin_start(px(ABOUT_INSET))
            .margin_end(px(ABOUT_INSET))
            .build();

        // The mark beside the name, which is the one place in the application
        // that says which player this is in so many words.
        let title = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(px(14.0))
            .build();
        // Larger than the one in the corner of a header, which shares a fixed
        // slot with the back arrow and is sized to it. Here it stands beside
        // the application's name and is the only picture on the page.
        let mark = logo_image(self.scale.get());
        mark.set_pixel_size(px(ABOUT_LOGO));
        // Both centered against each other, or the mark hangs above a line of
        // text half its height.
        mark.set_valign(gtk::Align::Center);
        let name = about_heading(&format!("TinePlayer {}", env!("CARGO_PKG_VERSION")));
        name.add_css_class("tp-about-title");
        name.set_valign(gtk::Align::Center);
        title.append(&mark);
        title.append(&name);
        body.append(&title);

        // What it is, before what it is made of. Everything else on this page
        // assumes you already know, which is no use to somebody who has
        // inherited the machine it is installed on.
        body.append(&about_heading("Watch together, in different languages."));
        body.append(&about_text(
            "A player that allows people to watch videos together while hearing separate soundtracks.",
        ));

        body.append(&about_text(
            "Free software under the MIT License, Copyright (c) 2026 Scott Bounds. You may use, change and pass it on, provided the copyright notice travels with it. It comes with no warranty of any kind.",
        ));
        // The domain rather than the repository, and followed rather than
        // only shown. A released binary cannot be edited: if the repository
        // is ever renamed or moved, a link baked into it breaks for good,
        // where a domain we own can simply be pointed somewhere else. It is
        // also shorter to read from across a room and possible to type from
        // memory, which a full GitHub path is not.
        body.append(&about_link(
            "Report issues or check for updates at",
            "https://tineplayer.app",
            "tineplayer.app",
        ));

        // The attribution without the numbers, which are worth stating exactly
        // and are stated below where they can be read off rather than picked
        // out of a sentence.
        body.append(&about_heading("Built with"));
        body.append(&about_text(
            "GStreamer and GTK, both free software under the GNU Lesser General Public License.",
        ));
        // Pointed at the copy in hand rather than at the one on the web. The
        // notices are compiled into the binary and sit one row below this, and
        // the machines this player is built for are televisions where opening
        // a browser is not something a D-pad does well.
        body.append(&about_text(
            "Also the work of a good many people writing Rust libraries, all attributed under Third-Party Notices below.",
        ));

        // What a bug report needs, in one place and readable off the screen.
        //
        // The renderer earns its line here. GTK picks one for the machine, and
        // the same drawing can come out differently on two of them - a blend
        // node this application used to draw its backdrop with looked right on
        // Windows and was all but invisible on a Raspberry Pi, which is a
        // difference nobody can report without being told what to look at.
        body.append(&about_heading("App Details"));
        // One label rather than a line each, so a single drag takes the lot.
        // Every paragraph on this page holds its own selection - GTK gives a
        // label one, and labels do not share - so five lines could be copied
        // only one at a time, which is the opposite of what somebody gathering
        // them for a bug report needs.
        //
        // GStreamer is asked for its numbers rather than its version string,
        // which begins with its own name and read as "GStreamer: GStreamer
        // 1.28.5".
        let (major, minor, micro, _) = gstreamer::version();
        body.append(&about_text(&format!(
            "TinePlayer: {}\nSystem: {} ({})\nGTK: {}.{}.{}\nGStreamer: {major}.{minor}.{micro}\nRenderer: {}",
            env!("CARGO_PKG_VERSION"),
            os_name(),
            std::env::consts::ARCH,
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version(),
            self.renderer_name(),
        )));
        body
    }

    /// Which of GTK's renderers is drawing this window.
    ///
    /// Read from the window rather than from `GSK_RENDERER`, which names only
    /// what was asked for: unset is the ordinary case, and a request GTK could
    /// not honour falls back to another without saying so.
    fn renderer_name(&self) -> String {
        self.window
            .renderer()
            .map(|renderer| renderer.type_().name().to_string())
            .unwrap_or_else(|| "not yet drawn".to_string())
    }

    /// The notices for everything TinePlayer is built from, in the
    /// application rather than only on a web page.
    ///
    /// Every package already carries THIRD-PARTY.md as a file, which is what
    /// the licenses actually ask for. This is about being able to read it: the
    /// machines this player is built for are televisions and HTPCs driven by a
    /// gamepad, where there may be no browser at all and opening one is not
    /// something a D-pad does well. The link on the About page stays for
    /// anyone who would rather read it on the web.
    ///
    /// Built into the binary rather than read from beside it, so it is there
    /// whichever way TinePlayer was installed, and cannot be separated from
    /// the thing it describes.
    fn show_notices(self: &Rc<Self>) {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(20.0))
            .margin_top(px(28.0))
            .margin_bottom(px(28.0))
            .margin_start(px(32.0))
            .margin_end(px(32.0))
            .build();
        page.append(&heading_label("Third-Party Notices"));

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(10.0))
            .build();
        let mut blocks = notices_blocks(include_str!("../THIRD-PARTY.md"));
        // The file's own title, which the dialog says above this already. Read
        // as a file it belongs there; read here it is the same three words
        // twice, an inch apart.
        if matches!(blocks.first(), Some(Notice::Heading(_))) {
            blocks.remove(0);
        }
        let last = blocks.len().saturating_sub(1);
        for (index, block) in blocks.into_iter().enumerate() {
            let widget = match block {
                Notice::Heading(text) => about_heading(&text),
                Notice::Text(text) => about_text(&text),
            };
            // The closing line is a remark about the list rather than part of
            // it, and sitting one row's gap under two hundred crates it read
            // as another entry. A heading would be too much for one sentence;
            // the space is enough to separate it.
            if index == last {
                widget.set_margin_top(px(24.0));
            }
            body.append(&widget);
        }

        // Two hundred crates will not fit on any screen, so the dialog keeps
        // to a share of the window and the list scrolls inside it.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&body)
            .build();
        scroller.set_focusable(false);
        let height = (self.window.height() as f64 * NOTICES_SHARE).round() as i32;
        scroller.set_max_content_height(height.max(px(320.0)));
        scroller.set_propagate_natural_height(true);
        // And a width, which the height alone does not give: the text wraps,
        // so its natural width is whatever the longest unwrapped line happens
        // to be, and left to that the dialog spans the window. A line of prose
        // is read at a comfortable length or not at all.
        scroller.set_propagate_natural_width(true);
        scroller.set_max_content_width(px(NOTICES_WIDTH));
        page.set_halign(gtk::Align::Center);
        page.append(&scroller);

        let close = gtk::Button::with_label("Close");
        close.add_css_class("tp-button");
        close.set_halign(gtk::Align::Center);
        page.append(&close);
        {
            let app = self.clone();
            close.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }

        // Over the settings rather than in place of them: the notices are
        // something looked up and dismissed, and the screen they were reached
        // from is still where the viewer was.
        //
        // Nothing to select, so up and down scroll instead - the arrangement
        // the About text uses beside it.
        self.set_nav(None, std::slice::from_ref(&close), &[]);
        *self.about_scroll.borrow_mut() = Some(scroller.vadjustment());
        *self.copy_root.borrow_mut() = Some(body.upcast());
        *self.screen.borrow_mut() = Screen::Notices;
        self.window.set_child(Some(&self.modal(&page)));
        close.grab_focus();
    }

    /// Copies whatever is selected on the screen being shown, and says
    /// whether there was anything. Each paragraph is its own label and holds
    /// its own selection, so the first one holding any is the one that was
    /// dragged across.
    ///
    /// Done by hand because GTK delivers Ctrl+C to whichever widget has
    /// focus, and selectable text here deliberately never takes focus: it
    /// would put a caret in the middle of a screen driven by arrow keys.
    fn copy_selection(&self) -> bool {
        let root = self.copy_root.borrow().clone();
        let Some(root) = root else { return false };
        self.copy_from(&root)
    }

    fn copy_from(&self, widget: &gtk::Widget) -> bool {
        if let Some(label) = widget.downcast_ref::<gtk::Label>()
            && let Some((from, to)) = label.selection_bounds()
        {
            let selected: String = label
                .text()
                .chars()
                .skip(from as usize)
                .take((to - from) as usize)
                .collect();
            self.window.clipboard().set_text(&selected);
            return true;
        }
        let mut next = widget.first_child();
        while let Some(child) = next {
            if self.copy_from(&child) {
                return true;
            }
            next = child.next_sibling();
        }
        false
    }

    /// Moves the About page when there is nothing to select on it. Says
    /// whether it did, so ordinary navigation can carry on elsewhere.
    /// Whether what is on screen is a page of text with no rows to move
    /// through, so the arrows should scroll it instead.
    ///
    /// The About text no longer has a screen of its own - it is a block above
    /// the notices row in the settings pane - so this asks where the keyboard
    /// is as well as which screen it is. In the column of categories the
    /// arrows are moving between categories and must not scroll anything.
    fn reading_about(&self) -> bool {
        *self.screen.borrow() == Screen::Notices
            || (self.on_settings()
                && self.in_settings_pane.get()
                && self.settings_category.get() == Category::About)
    }

    fn scroll_about(&self, delta: i32) -> bool {
        if !self.reading_about() {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        // A third of a screenful a press: enough to make progress, little
        // enough to keep your place on the page.
        let step = adjustment.page_size() / 3.0;
        let moved = adjustment.value() + delta as f64 * step;
        adjustment.set_value(moved.clamp(adjustment.lower(), about_bottom(&adjustment)));
        true
    }

    /// The same for Home and End, on the pages with no rows to give them to.
    fn scroll_about_edge(&self, end: bool) -> bool {
        if !self.reading_about() {
            return false;
        }
        let Some(adjustment) = self.about_scroll.borrow().clone() else {
            return false;
        };
        adjustment.set_value(if end {
            about_bottom(&adjustment)
        } else {
            adjustment.lower()
        });
        true
    }

    /// Puts the settings screen back with the Kodi category showing.
    ///
    /// Where a confirmation, an error, or the folder browser comes back to.
    /// Rebuilding is what re-reads every Kodi from disk, so the rows state
    /// what is in the files rather than what was asked for.
    fn return_to_kodi_settings(self: &Rc<Self>) {
        self.settings_category.set(Category::Kodi);
        self.in_settings_pane.set(true);
        self.show_settings();
    }

    /// A fresh reading of one installation, taken at the moment it is about to
    /// be written to rather than out of the list the pane was built from -
    /// which may be minutes old and describes a file that anything else on the
    /// machine is free to have changed since.
    fn kodi_at(&self, index: usize) -> Option<crate::kodi_setup::Setup> {
        let userdata = self.with_kodi_setup(index, |setup| setup.userdata().to_path_buf())?;
        Some(crate::kodi_setup::setup_at(userdata))
    }

    /// Setting the Player Type row, which is the only row that registers
    /// TinePlayer with Kodi or takes it back out.
    ///
    /// Two of the three answers are asked about first, and for opposite
    /// reasons. **Removal** is asked about because it undoes something. **The
    /// first setting of any file** is asked about because until it happens the
    /// file is entirely somebody else's: they may have players and comments in
    /// there that we are about to edit around, and being told which file is
    /// being changed and what is being kept is the least this can do. After
    /// that, changing Optional to Default is editing our own entry, and asking
    /// again would be asking permission to change a setting on the settings
    /// screen.
    ///
    /// This is what the five-screen wizard came down to. Everything it
    /// collected - which Kodi, what type, what handover, whether to back up -
    /// is a row on this pane or a rule with an obvious answer.
    fn choose_kodi_type(self: &Rc<Self>, index: usize, choice: Option<usize>) -> bool {
        use crate::kodi_setup::Registration;

        let (Some(chosen), Some(setup)) = (choice, self.kodi_at(index)) else {
            return false;
        };
        let Some(want) = Registration::ALL.get(chosen).copied() else {
            return false;
        };
        // Nothing asked for, so nothing done, and above all nothing asked.
        if want == setup.state {
            return false;
        }

        // Kept whatever is being set, so that changing type does not silently
        // change what Kodi does when it hands a video over.
        let play = setup.play;
        let label = setup.label();

        if want == Registration::Absent {
            let app = self.clone();
            self.confirm_kodi(
                "Remove Configuration?",
                &[&format!(
                    "TinePlayer will be removed as an external player from {label}."
                )],
                Confirm {
                    label: "Remove",
                    destructive: true,
                },
                move || {
                    let Some(setup) = app.kodi_at(index) else {
                        return app.return_to_kodi_settings();
                    };
                    let userdata = setup.userdata().to_path_buf();
                    if app.write_kodi(&setup, Registration::Absent, None, play) {
                        // Before the pane is drawn again, or it would be drawn
                        // from a list this is about to shorten. A folder named
                        // by hand is only worth remembering while something is
                        // set up in it.
                        app.forget_kodi_path(&userdata);
                        app.return_to_kodi_settings();
                    }
                },
            );
            return true;
        }

        if setup.is_configured() {
            // Our own entry, rewritten. Nothing here is anybody else's.
            if self.write_kodi(&setup, want, None, play) {
                self.return_to_kodi_settings();
            }
            // Answered either way: the pane has been put back, or an error
            // panel is up over it and must not be drawn over.
            return true;
        }

        // The first time TinePlayer touches this file. The backup is settled
        // here rather than at write time so the name cannot drift: computing
        // it twice would give two names a second apart.
        let backup = setup
            .backup_by_default()
            .then(|| crate::kodi_setup::backup_path(&setup.file));

        let app = self.clone();
        self.confirm_kodi(
            &format!("Configure {label}?"),
            &["Are you sure you want to edit this installation's playercorefactory.xml file?"],
            Confirm {
                label: "Configure",
                destructive: false,
            },
            move || {
                let Some(setup) = app.kodi_at(index) else {
                    return app.return_to_kodi_settings();
                };
                if app.write_kodi(&setup, want, backup.as_deref(), play) {
                    app.return_to_kodi_settings();
                }
            },
        );
        true
    }

    /// The one place anything is written to a Kodi, and the one place a
    /// failure to is reported. Answers whether it was written.
    ///
    /// Deliberately does not put the pane back itself. Two callers have
    /// something to do between the write and the rebuild - removal has a
    /// remembered folder to forget - and a rebuild in here would draw the pane
    /// from state that was about to change. On a failure the error panel is up
    /// and the answer is false, which is what stops a caller drawing the pane
    /// over the top of it.
    #[must_use]
    fn write_kodi(
        self: &Rc<Self>,
        setup: &crate::kodi_setup::Setup,
        want: crate::kodi_setup::Registration,
        backup: Option<&std::path::Path>,
        play: bool,
    ) -> bool {
        match crate::kodi_setup::apply(setup, want, backup, play) {
            Ok(()) => true,
            Err(e) => {
                self.show_kodi_error(&e, {
                    let app = self.clone();
                    move || app.return_to_kodi_settings()
                });
                false
            }
        }
    }

    /// Takes a folder somebody browsed to, once it has been checked.
    ///
    /// A folder that does not look like Kodi's user data is refused rather
    /// than taken, because writing to the wrong one fails silently: the rows
    /// would read as configured, Kodi would carry on playing videos itself,
    /// and nothing anywhere would say why.
    ///
    /// Dismissing goes back to the browser at the folder that was refused,
    /// which is what "choose another folder" needs to be able to mean.
    fn take_kodi_folder(self: &Rc<Self>, chosen: std::path::PathBuf) {
        let userdata = crate::kodi_setup::userdata_from(chosen);
        if crate::kodi_setup::looks_like_userdata(&userdata) {
            return self.remember_kodi_path(userdata);
        }

        let app = self.clone();
        let refused = userdata.clone();
        self.kodi_notice(
            "This does not look like Kodi's user data folder",
            &[
                &userdata.display().to_string(),
                "A user data folder usually holds guisettings.xml and a Database folder.",
                "Please choose another folder.",
            ],
            move || app.show_kodi_folder(&refused),
        );
    }

    /// Keeps track of a folder somebody named by hand, so it heads a group of
    /// its own on the pane like anything found by itself.
    ///
    /// Written down as soon as it is named rather than once something has been
    /// set up in it, which is when the wizard used to do it. The pane is built
    /// from the installations known, so a folder that is not written down is a
    /// folder that vanishes on the way back from the browser - and there would
    /// be nothing to set up in.
    ///
    /// One TinePlayer already finds by itself is not written down, since it
    /// would be found twice and listed once anyway.
    fn remember_kodi_path(self: &Rc<Self>, userdata: std::path::PathBuf) {
        let found = crate::kodi_setup::find_all(&[])
            .iter()
            .any(|setup| setup.userdata() == userdata);
        if !found {
            let mut config = self.config.borrow_mut();
            if !config.kodi_paths.contains(&userdata) {
                config.kodi_paths.push(userdata);
                let _ = config.save();
            }
        }
        self.return_to_kodi_settings();
    }

    /// Stops keeping track of a folder somebody named by hand, once nothing is
    /// set up in it. One that TinePlayer finds by itself is not forgotten,
    /// because it was never remembered: it will be found again next time.
    fn forget_kodi_path(self: &Rc<Self>, userdata: &std::path::Path) {
        let mut config = self.config.borrow_mut();
        if config.kodi_paths.iter().any(|path| path == userdata) {
            config.kodi_paths.retain(|path| path != userdata);
            let _ = config.save();
        }
    }

    /// Every Kodi on this machine, including any folder named by hand that we
    /// are still keeping track of.
    fn known_kodis(&self) -> Vec<crate::kodi_setup::Setup> {
        let extra = self.config.borrow().kodi_paths.clone();
        crate::kodi_setup::find_all(&extra)
    }

    /// The places column that sits to the left of a browser's listing.
    ///
    /// Home, the drives or filesystem, and whatever is mounted - all at once
    /// rather than on a separate screen reached by stepping off the top of
    /// the tree. Moving between the two lists is left and right, which the
    /// keyboard and the gamepad both do by ordinary directional focus.
    ///
    /// `folders` says which browser a drive reopens, the same way the
    /// breadcrumbs do.
    fn places_column(
        self: &Rc<Self>,
        current: &std::path::Path,
        folders: bool,
    ) -> Option<(gtk::ScrolledWindow, gtk::ListBox)> {
        let roots = crate::browser::places();
        if roots.is_empty() {
            return None;
        }

        let list = gtk::ListBox::new();
        list.add_css_class("tp-menu");
        list.set_selection_mode(gtk::SelectionMode::Browse);
        list.set_activate_on_single_click(true);

        // Which place the listing is inside, so the column says where you are
        // as well as where you could go. The longest match wins: a volume
        // under /mnt is a better answer than the filesystem root that also
        // contains it.
        let here = crate::browser::rooted(current);
        let mut selected: Option<(i32, usize)> = None;
        for (index, entry) in roots.iter().enumerate() {
            append_named(&list, &chooser_row(&entry.label), &entry.label);
            if here.starts_with(&entry.path) {
                let depth = entry.path.components().count();
                if selected.is_none_or(|(_, best)| depth > best) {
                    selected = Some((index as i32, depth));
                }
            }
        }
        let selected = selected.map(|(index, _)| index);
        if let Some(row) = selected.and_then(|index| list.row_at_index(index)) {
            // Marked as the one in force, and the cursor starts there - but
            // the two part company as soon as the viewer moves, which is the
            // whole point of marking it separately.
            row.add_css_class("tp-current");
            list.select_row(Some(&row));
        }

        {
            let app = self.clone();
            let paths: Vec<std::path::PathBuf> = roots.iter().map(|e| e.path.clone()).collect();
            list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                if let Some(path) = paths.get(row.index() as usize) {
                    if folders {
                        app.show_kodi_folder(path);
                    } else {
                        app.show_browser(path, None);
                    }
                }
            });
        }
        self.follow_focus(&list);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .width_request((220.0 * self.scale.get()).round() as i32)
            .child(&list)
            .build();
        scroller.set_focusable(false);
        list.set_focusable(true);
        Some((scroller, list))
    }

    /// Sends this widget's up and down keys through `move_selection`, which
    /// knows where the focus is and what each boundary should do.
    ///
    /// Needed on anything that can hold focus beside a list, now that rows
    /// cannot: GtkListBox moves its cursor by moving focus between rows, and
    /// with nothing able to take it that does nothing at all. Capture phase,
    /// so this runs before the list's own bindings swallow the key.
    fn wire_arrows(self: &Rc<Self>, widget: &gtk::Widget) {
        let app = self.clone();
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_key_pressed(move |_, key, _, _| match key {
            gdk::Key::Up => {
                app.move_selection(-1);
                glib::Propagation::Stop
            }
            gdk::Key::Down => {
                app.move_selection(1);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        widget.add_controller(controller);
    }

    /// Puts a widget into the tab order at the end.
    fn add_nav_stop(&self, widget: &impl IsA<gtk::Widget>) {
        self.nav_stops.borrow_mut().push(widget.clone().upcast());
    }

    /// Moves to the next or previous thing on this screen worth stopping on.
    ///
    /// Returns whether it did, so a screen with no stops of its own - a text
    /// panel, say - falls back to GTK's own handling rather than trapping the
    /// key.
    fn move_focus_stop(self: &Rc<Self>, delta: isize) -> bool {
        let stops = self.nav_stops.borrow().clone();
        if stops.is_empty() {
            return false;
        }
        let focused = gtk::prelude::GtkWindowExt::focus(&self.window);
        // Which stop the focus is in, rather than which stop it is: focus on
        // a button inside a stop still counts as being there.
        let at = focused.and_then(|widget| {
            stops.iter().position(|stop| {
                *stop == widget || stop.is_ancestor(&widget) || widget.is_ancestor(stop)
            })
        });
        let next = match at {
            Some(at) => (at as isize + delta).rem_euclid(stops.len() as isize) as usize,
            // Nowhere in particular yet: forwards starts at the beginning,
            // backwards at the end.
            None if delta > 0 => 0,
            None => stops.len() - 1,
        };
        if let Some(stop) = stops.get(next) {
            self.sounds.borrow().click();
            stop.grab_focus();
        }
        true
    }

    /// Moves between two lists sitting side by side, and does nothing
    /// anywhere else: left and right are for the panes of the browser, not a
    /// second way to reach the buttons.
    fn move_between_lists(self: &Rc<Self>, delta: isize) -> bool {
        // Not on the settings screen, whose two lists are in the tab order
        // together and are stepped between with Enter and Escape. Left and
        // right there belong to the bars on the rows.
        if *self.screen.borrow() == Screen::Settings {
            return false;
        }
        let stops = self.nav_stops.borrow().clone();
        let Some(focused) = gtk::prelude::GtkWindowExt::focus(&self.window) else {
            return false;
        };
        let Some(at) = stops.iter().position(|stop| {
            *stop == focused || stop.is_ancestor(&focused) || focused.is_ancestor(stop)
        }) else {
            return false;
        };
        if !stops[at].is::<gtk::ListBox>() {
            return false;
        }
        let next = at as isize + delta;
        if next < 0 || next as usize >= stops.len() {
            return false;
        }
        let next = &stops[next as usize];
        if !next.is::<gtk::ListBox>() {
            return false;
        }
        self.sounds.borrow().click();
        next.grab_focus();
        true
    }

    /// Makes a list the one the gamepad drives whenever it holds the focus.
    ///
    /// The navigation machinery knows about a single list at a time, which is
    /// all any other screen needs. With two side by side, which one is "the"
    /// list has to follow the focus, or the gamepad keeps driving whichever
    /// was wired last however far the viewer has moved away from it.
    fn follow_focus(self: &Rc<Self>, list: &gtk::ListBox) {
        let app = self.clone();
        let controller = gtk::EventControllerFocus::new();
        {
            let list = list.clone();
            controller.connect_enter(move |_| {
                *app.nav_list.borrow_mut() = Some(list.clone());
            });
        }
        list.add_controller(controller);
    }

    /// Puts a browser's listing beside its drive column.
    ///
    /// `list_page_with` has already put the listing in the page; this takes
    /// it back out and rebuilds that row with the drives to its left.
    fn add_places_column(
        self: &Rc<Self>,
        page: &gtk::Box,
        current: &std::path::Path,
        folders: bool,
        header: &[gtk::Button],
    ) {
        let Some(listing) = page.last_child() else {
            return;
        };
        let Some((places, list)) = self.places_column(current, folders) else {
            return;
        };
        page.remove(&listing);

        // The column takes the width it asked for and the listing takes the
        // rest. Without this the listing is given its minimum, which for a
        // list of names is very little, and the folders end up in a ribbon
        // down one side of the screen.
        places.set_hexpand(false);
        listing.set_hexpand(true);

        // Handed to set_nav, which puts it in the order ahead of the listing
        // it sits left of, and driven by the same keys once it has focus.
        *self.nav_side_list.borrow_mut() = Some(list.clone());
        self.wire_arrows(list.upcast_ref());
        // Its own, since wire_navigation only ever sees a screen's main list.
        announce_selection(&list);

        // Up from the top of the column reaches the trail above it, the same
        // way it does from the listing.
        {
            let app = self.clone();
            let header: Vec<glib::WeakRef<gtk::Button>> =
                header.iter().map(|button| button.downgrade()).collect();
            let controller = gtk::EventControllerKey::new();
            // Weak, since the controller is added to the very list it watches
            // and holding a strong reference would keep the pair alive.
            let watched = list.downgrade();
            controller.connect_key_pressed(move |_, key, _, _| {
                let Some(list) = watched.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                if key != gdk::Key::Up || list.selected_row().map(|row| row.index()) != Some(0) {
                    return glib::Propagation::Proceed;
                }
                let buttons: Vec<gtk::Button> = header
                    .iter()
                    .filter_map(|button| button.upgrade())
                    .collect();
                if let Some(button) = App::last_header(&buttons) {
                    app.sounds.borrow().click();
                    button.grab_focus();
                }
                glib::Propagation::Stop
            });
            list.add_controller(controller);
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .vexpand(true)
            .build();
        row.append(&places);
        row.append(&listing);
        page.append(&row);
    }

    /// Browsing for Kodi's userdata folder, in TinePlayer's own browser.
    ///
    /// The system's folder chooser would do the job, but not from a sofa: it
    /// is a desktop dialog that a gamepad cannot drive and that draws itself
    /// at desktop sizes on a television. This is the same browser used for
    /// finding a video, showing only folders, with choosing the current one
    /// on a button beside the way out.
    ///
    /// Deliberately a sibling of `show_browser` rather than a mode of it.
    /// That one carries a paste row, video entries, a remembered location and
    /// an origin to return to, none of which belong here, and threading a
    /// purpose through all of it would put the video browser at risk for the
    /// sake of a screen that shares only its shape.
    /// The screen for choosing the folder Kodi keeps its settings in.
    fn show_kodi_folder(self: &Rc<Self>, directory: &std::path::Path) {
        let directory = crate::browser::rooted(directory);
        let page = self.browser_page(&directory, Browse::Folders);
        let entries = browser_entries(&directory, Browse::Folders);

        // What this browser is for, said on it. It is reached by a trail of
        // folder names and nothing else, so without this the only statement of
        // what to look for was on the row that opened it, a screen ago - and
        // the wrong answer here is one that fails silently later.
        let prompt = row_note(
            "Choose Kodi's user data folder - the one holding guisettings.xml.",
            self.scale.get(),
        );
        prompt.set_halign(gtk::Align::Center);
        page.page.append(&prompt);

        let choose = gtk::Button::with_label("Choose This Folder");
        choose.add_css_class("tp-button");
        choose.add_css_class("tp-action");
        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        buttons.append(&page.cancel);
        buttons.append(&choose);
        let footer = gtk::CenterBox::new();
        footer.set_start_widget(Some(&page.browse));
        footer.set_center_widget(Some(&buttons));
        page.page.append(&footer);

        fill_browser_list(&page.list, &entries, self.scale.get());

        {
            let app = self.clone();
            let entries = entries.clone();
            let here = directory.clone();
            page.list.connect_row_activated(move |_, row| {
                app.sounds.borrow().click();
                let Some(entry) = entries.get(row.index() as usize) else {
                    return;
                };
                match &entry.path {
                    Some(path) => app.show_kodi_folder(path),
                    None => {
                        if let Some(parent) = here.parent() {
                            app.show_kodi_folder(parent);
                        }
                    }
                }
            });
        }
        {
            let app = self.clone();
            let directory = directory.clone();
            choose.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.take_kodi_folder(directory.clone());
            });
        }
        {
            let app = self.clone();
            page.cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }

        // Same order they are laid out in, or moving between them runs
        // backwards against what is on screen.
        self.wire_navigation(
            &page.list,
            &page.crumbs,
            &[page.cancel.clone(), choose.clone()],
        );
        *self.screen.borrow_mut() = Screen::KodiFolder;
        self.window.set_child(Some(&self.modal(&page.page)));
        if let Some(row) = page.list.row_at_index(0) {
            page.list.select_row(Some(&row));
            settle_on(&row);
        }
    }

    /// The system's own folder chooser, for anyone who would rather use it.
    fn choose_kodi_folder_natively(self: &Rc<Self>, start: &std::path::Path) {
        let chooser = gtk::FileChooserNative::new(
            Some("Choose Kodi's user data folder"),
            Some(&self.window),
            gtk::FileChooserAction::SelectFolder,
            Some("Choose"),
            Some("Cancel"),
        );
        open_at(&chooser, start);
        let app = self.clone();
        // Held by the closure so the dialog outlives this function; a dropped
        // FileChooserNative closes before the user can answer. Same handling
        // as the video chooser.
        let held = RefCell::new(Some(chooser.clone()));
        chooser.connect_response(move |chooser, response| {
            let chosen = (response == gtk::ResponseType::Accept)
                .then(|| chooser.file().and_then(|file| file.path()))
                .flatten();
            held.borrow_mut().take();
            if let Some(folder) = chosen {
                app.take_kodi_folder(folder);
            }
        });
        chooser.show();
    }

    /// Something went wrong, said plainly, with a way back to where it was
    /// worth trying from.
    fn show_kodi_error(self: &Rc<Self>, message: &str, back: impl Fn() + 'static) {
        self.kodi_notice("Configuration Error", &[message], back);
    }

    /// A panel that states something and offers only to be dismissed.
    ///
    /// Distinct from [`confirm_kodi`], which asks a question and therefore has
    /// two answers. Nothing here is being decided: the one button is a way on
    /// from something already settled, so it says OK rather than naming an
    /// action nobody is taking.
    ///
    /// [`confirm_kodi`]: Self::confirm_kodi
    fn kodi_notice(self: &Rc<Self>, title: &str, lines: &[&str], back: impl Fn() + 'static) {
        let page = wizard_page(title);
        for line in lines {
            page.append(&wizard_text(line, false));
        }

        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("tp-button");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                back();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiError;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }

    /// A panel that states something and asks whether to go ahead.
    ///
    /// Cancel always returns to the Kodi pane, because that is the one
    /// place any of this is opened from now. It used to take a destination:
    /// backing out of the wizard's summary went to the screen the answer had
    /// been given on, so it could be changed rather than the whole sequence
    /// restarted. With the answers on rows there is nothing to restart, and
    /// the row is on the pane behind this panel.
    fn confirm_kodi(
        self: &Rc<Self>,
        title: &str,
        lines: &[&str],
        confirm: Confirm<'_>,
        action: impl Fn() + 'static,
    ) {
        let page = wizard_page(title);
        for line in lines {
            // A command is the one thing somebody has to reproduce exactly,
            // so it is set apart and wraps by character rather than by word.
            let command = line.starts_with("flatpak ");
            page.append(&wizard_text(line, command));
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        // Backing out is never the hazard, so Cancel is never the red one.
        // It used to be: red was put on whichever button was left over, so a
        // confirmation of something harmless painted the way out as the
        // dangerous choice.
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let destructive = confirm.destructive;
        let confirm = gtk::Button::with_label(confirm.label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class(match destructive {
            true => "tp-danger",
            false => "tp-action",
        });
        row.append(&cancel);
        row.append(&confirm);
        page.append(&row);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[cancel.clone(), confirm.clone()], &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiConfirm;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel, so a reflexive second press changes nothing.
        cancel.grab_focus();
    }

    /// The permission a Flatpak Kodi needs before it can start TinePlayer at
    /// all, as something to read and run rather than something done quietly on
    /// somebody's behalf.
    ///
    /// This was a step in the wizard, which meant everyone who had a Flatpak
    /// Kodi met it exactly once, at the moment they were busy setting the
    /// thing up, and never again. It is a row now: still there the next day,
    /// when the film did not play and the question is why.
    ///
    /// Granting it lets Kodi run *any* command on the machine, which is a real
    /// widening of what an installed application can do, so the panel says so
    /// and TinePlayer never runs it.
    fn show_kodi_permission(self: &Rc<Self>, index: usize) {
        let Some(manual) = self
            .with_kodi_setup(index, |setup| setup.confinement)
            .and_then(crate::kodi_setup::manual_step)
        else {
            return;
        };

        let page = wizard_page(manual.what);
        page.append(&wizard_text(manual.why, false));
        if let Some(command) = manual.command {
            page.append(&wizard_text("Run this once, in a terminal:", false));
            page.append(&wizard_text(command, true));
        }
        page.append(&wizard_text(manual.cost, false));
        if let Some(undo) = manual.undo {
            page.append(&wizard_text("To undo it:", false));
            page.append(&wizard_text(undo, true));
        }

        let ok = gtk::Button::with_label("Done");
        ok.add_css_class("tp-button");
        ok.add_css_class("tp-action");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_kodi_settings();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        // Ctrl+C reaches the command, which is the whole point of the panel.
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::KodiPermission;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }

    // --- Jellyfin pairing ----------------------------------------------

    /// Puts the settings screen back with the Jellyfin category showing.
    ///
    /// The counterpart to [`return_to_kodi_settings`], and it does the same
    /// job: rebuilding is what re-reads the pairing file, so the rows state
    /// what is stored rather than what was asked for.
    ///
    /// [`return_to_kodi_settings`]: Self::return_to_kodi_settings
    fn return_to_jellyfin_settings(self: &Rc<Self>) {
        // Any code still waiting is abandoned by leaving, so nothing arriving
        // late can pair a server the viewer has walked away from.
        self.jellyfin_attempt.set(self.jellyfin_attempt.get() + 1);
        self.settings_category.set(Category::Jellyfin);
        self.in_settings_pane.set(true);
        self.show_settings();
    }

    /// Opens the connection flow, remembering where it was opened from.
    ///
    /// One dialog for the whole of it rather than a row per question. Pairing
    /// is a single errand somebody does once, and splitting it across a
    /// settings pane made three rows out of two facts - which server, and the
    /// code that proves it is yours.
    fn start_jellyfin_connect(self: &Rc<Self>, from: ConnectFrom) {
        self.connect_from.set(from);
        self.show_jellyfin_address();
    }

    /// Leaves the flow, by finishing it or backing out of it.
    ///
    /// Back to whichever screen opened it. Always returning to Settings would
    /// strand somebody who started from the empty page and never went there.
    fn leave_jellyfin_connect(self: &Rc<Self>) {
        // Anything still polling belongs to an attempt that is now over.
        self.jellyfin_attempt.set(self.jellyfin_attempt.get() + 1);
        match self.connect_from.get() {
            ConnectFrom::Settings => self.return_to_jellyfin_settings(),
            ConnectFrom::Menu => self.show_menu(),
        }
    }

    /// The first half of the dialog: which server.
    ///
    /// It looks for one while the field sits there, and fills it in with
    /// whatever answers - so on the machine this is built for, a box wired to a
    /// television and driven by a remote, the address need never be typed at
    /// all. The field stays editable because a server reached across a VPN or
    /// on another subnet will never answer a broadcast, and its owner knows the
    /// address perfectly well.
    ///
    /// **What is found fills the field rather than becoming a list to choose
    /// from.** A list would be a second question on a panel that has one, and
    /// on a home network the answer is one server.
    fn show_jellyfin_address(self: &Rc<Self>) {
        /// Long enough for a server to answer twice over, short enough to sit
        /// through. Jellyfin replies in milliseconds; the wait is for one that
        /// is busy or asleep, not for the network.
        const LOOK_FOR: std::time::Duration = std::time::Duration::from_secs(2);

        let page = wizard_page("Connect to Jellyfin");
        let hint = wizard_text("Looking for a server on this network...", false);
        page.append(&hint);

        let field = gtk::Entry::new();
        field.add_css_class("tp-path");
        field.set_placeholder_text(Some("http://jellyfin.local:8096"));
        gtk::prelude::EditableExt::set_alignment(&field, 0.5);
        field.set_hexpand(true);
        // Whatever was here before, which is the address of a server this
        // installation has been paired with and may be pairing with again.
        let known = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .map(|pairing| pairing.server.clone())
            .unwrap_or_default();
        field.set_text(&known);
        page.append(&field);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let connect = gtk::Button::with_label("Connect");
        connect.add_css_class("tp-button");
        connect.add_css_class("tp-action");
        connect.set_sensitive(!field.text().trim().is_empty());
        {
            let connect = connect.clone();
            field.connect_changed(move |field| {
                connect.set_sensitive(!field.text().trim().is_empty());
            });
        }
        buttons.append(&cancel);
        buttons.append(&connect);
        page.append(&buttons);

        let start = {
            let app = self.clone();
            let field = field.clone();
            move || {
                let typed = field.text();
                if typed.trim().is_empty() {
                    return;
                }
                app.sounds.borrow().click();
                app.begin_quick_connect(&typed);
            }
        };
        {
            let start = start.clone();
            connect.connect_clicked(move |_| start());
        }
        {
            let start = start.clone();
            field.connect_activate(move |_| start());
        }
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.leave_jellyfin_connect();
            });
        }

        // Its own tab order: the field, then the two buttons. Without stops
        // there is nothing for Tab to move between and Connect cannot be
        // reached from the keyboard at all.
        self.set_nav(None, &[], &[]);
        self.add_nav_stop(&field);
        self.add_nav_stop(&cancel);
        self.add_nav_stop(&connect);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        // `dialog` rather than `modal`, so this is the same measure as every
        // other panel that states something and asks a question. Left
        // uncapped, a panel is as wide as its own longest sentence wants to
        // be - which on a wide monitor is most of the screen, and makes the
        // two halves of this one flow visibly different shapes.
        self.window.set_child(Some(&self.dialog(&page)));
        field.grab_focus();
        // Selected, so typing replaces it: an address being offered is usually
        // taken or replaced whole rather than edited.
        field.select_region(0, -1);

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(crate::jellyfin::discover(LOOK_FOR));
        });

        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            let found = match receiver.try_recv() {
                Ok(found) => found,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            // Gone: cancelled, or already past this step. A panel taken out of
            // the window has no root, which is cheaper to ask than remembering
            // which screen replaced it.
            if page.root().is_none() {
                return glib::ControlFlow::Break;
            }
            match found.first() {
                Some(server) => {
                    hint.set_text(&format!("Found {} on this network.", server.name));
                    // Only if nobody has typed since. Overwriting an address
                    // somebody is part way through entering would be the worst
                    // thing this could do with its answer.
                    if field.text() == known {
                        field.set_text(&server.address);
                        field.select_region(0, -1);
                    }
                }
                None => hint.set_text(
                    "No server answered on this network. Enter its address, which is also what a server on another network or behind a VPN needs.",
                ),
            }
            glib::ControlFlow::Break
        });
    }

    /// Writes down the address, then asks the server to start a pairing.
    ///
    /// A scheme is added when there is none, because "hoth:8096" is what
    /// somebody types and every request made with it would fail with nothing on
    /// screen to say why. Plain HTTP is what a Jellyfin server on a home
    /// network answers to; anybody reaching one over the internet types the
    /// https themselves.
    fn begin_quick_connect(self: &Rc<Self>, typed: &str) {
        let typed = typed.trim();
        let address = match typed.contains("://") {
            true => typed.to_string(),
            false => format!("http://{typed}"),
        };

        let pairing = match self.jellyfin_pairing.borrow().clone() {
            Some(mut pairing) => {
                pairing.set_server(&address);
                pairing
            }
            None => crate::jellyfin::Pairing::new(&address),
        };
        if let Err(e) = crate::jellyfin::save(&pairing) {
            return self.jellyfin_notice("Could Not Save", &[&e]);
        }
        *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        self.show_jellyfin_code();
    }

    /// The second half of the dialog: the code, and waiting for it.
    ///
    /// A code rather than a login form, and that is not a matter of taste: this
    /// runs on a television, where typing a password with a remote is
    /// miserable, and it means no password is ever typed into TinePlayer at
    /// all. The viewer approves it in a Jellyfin app they are already signed
    /// in to.
    ///
    /// One thread does the whole of it - asking for the code, then polling
    /// until somebody approves it - and reports each step back to the main loop
    /// through a channel, which is the same shape everything else here uses to
    /// talk to a server without the interface stopping.
    fn show_jellyfin_code(self: &Rc<Self>) {
        /// How often to ask whether the code has been approved. Often enough
        /// that pressing approve on a phone and looking up at the television
        /// shows it done, rarely enough not to be a request a second for the
        /// several minutes somebody may take to find their phone.
        const ASK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
        /// Five minutes of asking. Jellyfin expires a code of its own accord
        /// around then, so waiting longer only produces a code that cannot
        /// work and a screen that does not say so.
        const TRIES: usize = 150;

        let Some(pairing) = self.jellyfin_pairing.borrow().clone() else {
            return;
        };
        if pairing.server.is_empty() {
            return;
        }

        let attempt = self.jellyfin_attempt.get() + 1;
        self.jellyfin_attempt.set(attempt);

        let page = wizard_page("Connect to Jellyfin");
        // Filled in once the server answers. Empty rather than absent, so the
        // panel does not change shape under the eye when the code arrives.
        let code = gtk::Label::new(None);
        code.add_css_class("tp-code");
        code.set_selectable(true);
        code.set_can_focus(false);
        page.append(&code);
        let status = wizard_text("Asking the server for a code...", false);
        page.append(&status);

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        cancel.set_halign(gtk::Align::Center);
        page.append(&cancel);
        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.leave_jellyfin_connect();
            });
        }

        self.set_nav(None, std::slice::from_ref(&cancel), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinConnect;
        // The same cap as the step before it, so pressing Connect changes what
        // the panel says rather than how big it is.
        self.window.set_child(Some(&self.dialog(&page)));
        cancel.grab_focus();

        let server = pairing.server.clone();
        let device_id = pairing.device_id.clone();
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let asking = alive.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Some(name) = crate::jellyfin::server_name(&server)
                && sender.send(QuickConnect::Named(name)).is_err()
            {
                return;
            }
            let pending = match crate::jellyfin::quick_connect_start(&server, &device_id) {
                Ok(pending) => pending,
                Err(e) => {
                    let _ = sender.send(QuickConnect::Failed(e.to_string()));
                    return;
                }
            };
            if sender
                .send(QuickConnect::Code(pending.code.clone()))
                .is_err()
            {
                return;
            }
            for _ in 0..TRIES {
                // Checked before the request rather than after it, so
                // cancelling stops the asking rather than stopping one round
                // later.
                if !asking.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                match crate::jellyfin::quick_connect_poll(&server, &device_id, &pending) {
                    Ok(Some(account)) => {
                        let _ = sender.send(QuickConnect::Done(Box::new(account)));
                        return;
                    }
                    // Nobody has approved it yet, which is the ordinary answer
                    // while somebody finds their phone.
                    Ok(None) => {}
                    Err(e) => {
                        let _ = sender.send(QuickConnect::Failed(e.to_string()));
                        return;
                    }
                }
                std::thread::sleep(ASK_EVERY);
            }
            let _ = sender.send(QuickConnect::Failed(
                "Nobody approved the code in time. Ask for another.".to_string(),
            ));
        });

        let app = self.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            // Left behind by a panel that has been closed, or by a second
            // attempt started over the top of this one. Either way this one is
            // over, and the thread is told so it stops asking.
            if app.jellyfin_attempt.get() != attempt {
                alive.store(false, std::sync::atomic::Ordering::Relaxed);
                return glib::ControlFlow::Break;
            }
            let step = match receiver.try_recv() {
                Ok(step) => step,
                Err(std::sync::mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            };
            match step {
                // Held rather than shown: this panel is about the code. It is
                // written down when the pairing is saved, and it is what every
                // screen afterwards calls this server.
                QuickConnect::Named(name) => {
                    if let Some(pairing) = app.jellyfin_pairing.borrow_mut().as_mut() {
                        pairing.name = Some(name);
                    }
                    glib::ControlFlow::Continue
                }
                QuickConnect::Code(shown) => {
                    code.set_text(&shown);
                    status.set_text(
                        "In a Jellyfin app you are signed in to, open Quick Connect from the user menu and enter this code.",
                    );
                    glib::ControlFlow::Continue
                }
                QuickConnect::Done(account) => {
                    app.jellyfin_paired(*account);
                    glib::ControlFlow::Break
                }
                QuickConnect::Failed(why) => {
                    app.jellyfin_notice("Could Not Connect", &[&why]);
                    glib::ControlFlow::Break
                }
            }
        });
    }

    /// Somebody approved the code. Writes the token down and goes on the air.
    ///
    /// Connecting straight away rather than at the next start: this is the
    /// moment the viewer is watching to see whether it worked, and a cast
    /// target that appears on their phone only after a restart looks like one
    /// that did not.
    fn jellyfin_paired(self: &Rc<Self>, account: crate::jellyfin::Account) {
        let Some(mut pairing) = self.jellyfin_pairing.borrow().clone() else {
            return;
        };
        pairing.account = Some(account);
        if let Err(e) = crate::jellyfin::save(&pairing) {
            return self.jellyfin_notice("Could Not Save", &[&e]);
        }
        *self.jellyfin_pairing.borrow_mut() = Some(pairing);
        self.start_jellyfin();
        self.leave_jellyfin_connect();
    }

    /// Asked before disconnecting, because it throws a pairing away.
    fn confirm_jellyfin_disconnect(self: &Rc<Self>) {
        let server = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .map(crate::jellyfin::Pairing::label)
            .unwrap_or_default();

        let app = self.clone();
        self.confirm_jellyfin(
            "Disconnect from Jellyfin?",
            &[
                &format!("TinePlayer will no longer appear as a player in {server}."),
                "The access token stored on this machine will be removed. Connecting again takes a new code.",
            ],
            Confirm {
                label: "Disconnect",
                destructive: true,
            },
            move || app.disconnect_jellyfin(),
        );
    }

    /// Ends the pairing here and, as far as it can, at the server too.
    ///
    /// The server is told on a worker thread while the local file goes at
    /// once. Waiting on it would mean a settings screen that hangs for as long
    /// as a switched-off server takes to time out, for a message the viewer has
    /// already decided the answer to - and what they asked for is to stop being
    /// paired, which is true the moment the token is gone from this machine.
    fn disconnect_jellyfin(self: &Rc<Self>) {
        let client = self
            .jellyfin_pairing
            .borrow()
            .as_ref()
            .and_then(crate::jellyfin::Client::new);
        if let Some(client) = client {
            let app = self.clone();
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(client.disconnect());
            });
            glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                match receiver.try_recv() {
                    Ok(Ok(())) => {}
                    // Said rather than swallowed, and said where it can be
                    // acted on. This is now only reached when the server could
                    // not be reached at all - a single logout either revokes
                    // the token and removes the device or does neither - so the
                    // pairing really is still live over there, and the viewer
                    // is the only one who can end it. Only over the pane it was
                    // asked from: a panel arriving over a film minutes later
                    // would be a worse fault than the one it reports.
                    Ok(Err(e)) => {
                        eprintln!("Jellyfin was not told about the disconnection: {e}");
                        if app.showing_jellyfin_pane() {
                            app.jellyfin_notice(
                                "Disconnected Here Only",
                                &[
                                    "The access token stored on this machine has been removed.",
                                    "The server could not be reached, so it still lists this device and the token still works. Remove TinePlayer under Devices in the Jellyfin dashboard to end it there.",
                                    &e.to_string(),
                                ],
                            );
                        }
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        return glib::ControlFlow::Continue;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                }
                glib::ControlFlow::Break
            });
        }

        *self.jellyfin.borrow_mut() = None;
        // Dropping it closes the socket, which is what takes TinePlayer off
        // everybody's phone.
        *self.jellyfin_session.borrow_mut() = None;
        if let Err(e) = crate::jellyfin::remove() {
            eprintln!("Couldn't remove the Jellyfin pairing: {e}");
        }
        *self.jellyfin_pairing.borrow_mut() = None;
        self.return_to_jellyfin_settings();
    }

    /// A panel stating something the Jellyfin pane has to say, with the one
    /// way on from it.
    fn jellyfin_notice(self: &Rc<Self>, title: &str, lines: &[&str]) {
        let page = wizard_page(title);
        for line in lines {
            page.append(&wizard_text(line, false));
        }

        let ok = gtk::Button::with_label("OK");
        ok.add_css_class("tp-button");
        ok.set_halign(gtk::Align::Center);
        page.append(&ok);
        {
            let app = self.clone();
            ok.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_jellyfin_settings();
            });
        }

        self.set_nav(None, std::slice::from_ref(&ok), &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        self.window.set_child(Some(&self.dialog(&page)));
        ok.grab_focus();
    }

    /// The same question shape the Kodi pane asks, returning to this pane
    /// instead.
    fn confirm_jellyfin(
        self: &Rc<Self>,
        title: &str,
        lines: &[&str],
        confirm: Confirm<'_>,
        action: impl Fn() + 'static,
    ) {
        let page = wizard_page(title);
        for line in lines {
            page.append(&wizard_text(line, false));
        }

        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let destructive = confirm.destructive;
        let confirm = gtk::Button::with_label(confirm.label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class(match destructive {
            true => "tp-danger",
            false => "tp-action",
        });
        row.append(&cancel);
        row.append(&confirm);
        page.append(&row);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.return_to_jellyfin_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[cancel.clone(), confirm.clone()], &[]);
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::JellyfinPanel;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel, so a reflexive second press changes nothing.
        cancel.grab_focus();
    }

    fn confirm_clear_data(self: &Rc<Self>) {
        let app = self.clone();
        self.show_confirm("Clear all saved playback data?", "Clear", move || {
            if let Err(e) = crate::config::clear_all_resume() {
                eprintln!("{e}");
            }
            // The loaded file keeps its choices for this session; only
            // what was written down is gone.
            app.show_settings();
        });
    }

    /// A yes-or-no panel over the screen that asked the question.
    ///
    /// Over it rather than in place of it, which is what it used to be: a
    /// question about something on the screen behind should leave that screen
    /// where it is, and answering it should put nothing back together.
    ///
    /// The confirming button is destructive, because this panel is. It exists
    /// for one question - whether to throw away what has been remembered - and
    /// a red button on a question that only ever destroys something is the
    /// application's own rule rather than a decision taken here.
    fn show_confirm(
        self: &Rc<Self>,
        message: &str,
        confirm_label: &str,
        action: impl Fn() + 'static,
    ) {
        let px = |base: f64| (base * self.scale.get()).round() as i32;
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(px(28.0))
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(px(36.0))
            .margin_bottom(px(36.0))
            .margin_start(px(44.0))
            .margin_end(px(44.0))
            .build();
        let heading = heading_label(message);
        heading.set_halign(gtk::Align::Center);
        page.append(&heading);

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let confirm = gtk::Button::with_label(confirm_label);
        confirm.add_css_class("tp-button");
        confirm.add_css_class("tp-danger");
        buttons.append(&cancel);
        buttons.append(&confirm);
        page.append(&buttons);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_settings();
            });
        }
        {
            let app = self.clone();
            confirm.connect_clicked(move |_| {
                app.sounds.borrow().click();
                action();
            });
        }

        self.set_nav(None, &[], &[]);
        *self.screen.borrow_mut() = Screen::Confirm;
        self.window.set_child(Some(&self.dialog(&page)));
        // Cancel takes focus, so a reflexive second press doesn't destroy
        // anything.
        cancel.grab_focus();
    }

    // --- Playback ------------------------------------------------------

    /// Shows the black video surface, then starts playback a frame later.
    ///
    /// Building the pipeline and seeking to a resume position both happen on
    /// this thread, so nothing repaints until they finish. Swapping the window
    /// first and letting one frame through means the menu disappears the
    /// instant Play is pressed, and the wait happens against black - which is
    /// what a video starting looks like anyway. Accurate seeking made this
    /// worth doing: it decodes forward to the exact position, and on a long
    /// film that is visible.
    fn start_playback(self: &Rc<Self>, restart: bool) {
        if self.file.borrow().is_none() {
            return;
        }

        let waiting = gtk::Box::builder()
            .css_classes([crate::player::VIDEO_CSS_CLASS])
            .hexpand(true)
            .vexpand(true)
            .build();
        self.window.set_child(Some(&waiting));

        // Playback begins on a frame that has actually been drawn.
        //
        // This used to wait 16 milliseconds and hope - one frame at 60Hz - and
        // that held from a button press, where GTK is already mid-way through
        // dispatching an event and will draw shortly after. It did not hold
        // when the play came from a media key, which arrives on an idle
        // callback: the pipeline was built against a surface that had never
        // been presented, and on an AV1 video being resumed the D3D12 decoder
        // deadlocked in `gst_video_decoder_finish_frame` while holding the pad
        // lock the seek's flush needed. TinePlayer stopped drawing entirely,
        // and only from that entry point, and only on the first play of a
        // session. Found 2026-08-13 with a debugger on the hung process.
        //
        // The tick callback is GTK's own answer to "after the next frame", so
        // there is nothing to tune and nothing to be unlucky with.
        let started = Rc::new(Cell::new(false));
        {
            let app = self.clone();
            let started = started.clone();
            waiting.add_tick_callback(move |_, _| {
                if !started.replace(true) {
                    // Queued rather than run here. A tick callback fires in
                    // the frame clock's update phase, which is before the
                    // frame is painted - building the pipeline in it left the
                    // surface still unpresented and deadlocked in the same
                    // place, with the menu visibly still on screen. An idle
                    // queued from here runs once that whole frame, paint
                    // included, has finished.
                    let app = app.clone();
                    glib::idle_add_local_once(move || app.begin_playback(restart));
                }
                glib::ControlFlow::Break
            });
        }
        // And a way out for a window that is never drawn at all - minimized,
        // or hidden behind something full screen - where no frame arrives and
        // the tick callback would never run. Waiting forever there would be a
        // worse bug than the one above.
        {
            let app = self.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if !started.replace(true) {
                    app.begin_playback(restart);
                }
            });
        }
    }

    /// Swaps the black surface for the video once a frame from the resume
    /// point has actually been drawn, or after a moment if none ever is.
    ///
    /// Waiting on the reported position is not enough. A flushing seek updates
    /// the pipeline's segment before the sink has rendered anything, so
    /// position says "arrived" while the picture on screen is still the
    /// opening frame from the preroll - which is exactly the flash this exists
    /// to prevent. The paintable tells us when a frame is genuinely there.
    ///
    /// Not driven by the pipeline's asynchronous-done message either: that
    /// fires for the preroll as well, so acting on it would reveal the picture
    /// at precisely the wrong moment.
    fn reveal_when_resumed(
        self: &Rc<Self>,
        widget: gtk::Overlay,
        paintable: Option<gdk::Paintable>,
        target: u64,
    ) {
        // Well inside a keyframe interval, and far enough from the opening
        // frame that the two cannot be mistaken for each other.
        const CLOSE_ENOUGH: u64 = 500_000_000;

        let reveal = {
            let app = self.clone();
            let widget = widget.clone();
            move || {
                // Playback may have been left while waiting, in which case
                // whatever replaced it should stay.
                if *app.screen.borrow() == Screen::Playing {
                    app.window.set_child(Some(&widget));
                }
            }
        };

        let Some(paintable) = paintable else {
            reveal();
            return;
        };

        let done = Rc::new(Cell::new(false));
        let handler = Rc::new(RefCell::new(None));
        {
            let app = self.clone();
            let reveal = reveal.clone();
            let done = done.clone();
            // Its own handle, so the outer one survives to be stored below.
            let registered = handler.clone();
            let id = paintable.connect_invalidate_contents(move |paintable| {
                if done.get() {
                    return;
                }
                let arrived = app
                    .playback
                    .borrow()
                    .as_ref()
                    .and_then(|playback| playback.position())
                    .is_some_and(|position| position.nseconds() + CLOSE_ENOUGH >= target);
                if !arrived {
                    return;
                }
                done.set(true);
                if let Some(id) = registered.borrow_mut().take() {
                    paintable.disconnect(id);
                }
                reveal();
            });
            *handler.borrow_mut() = Some(id);
        }

        // A seek that fails, or a source that never produces another frame,
        // would otherwise leave a black window and nothing to explain it.
        glib::timeout_add_local_once(std::time::Duration::from_secs(4), move || {
            if done.replace(true) {
                return;
            }
            if let Some(id) = handler.borrow_mut().take() {
                paintable.disconnect(id);
            }
            reveal();
        });
    }

    fn begin_playback(self: &Rc<Self>, restart: bool) {
        let Some(path) = self.file.borrow().clone() else {
            return;
        };
        self.stop_playback();

        // Belt and braces against the pipeline being asked for an output that
        // was never configured, whatever left the choice set.
        //
        // The file as well as the track, which it did not used to cover: an
        // audio file on the secondary output with no secondary device asked
        // for a sink that cannot be built, and the whole pipeline failed - so
        // a film with a perfectly good primary output would not play at all.
        let has_secondary_device = self.config.borrow().secondary_sink.is_some();
        let primary = *self.primary_track.borrow();
        let secondary = if has_secondary_device {
            *self.secondary_track.borrow()
        } else {
            None
        };
        let secondary_file = if has_secondary_device {
            self.secondary_file.borrow().clone()
        } else {
            None
        };
        // A separate audio file wins for that output. The track it displaces
        // is still remembered below, so clearing the file falls back to it.
        let audio_for = |file: Option<Source>, track: Option<u32>| match file {
            Some(file) => Some(crate::pipeline::AudioSource::File(file)),
            None => track.map(crate::pipeline::AudioSource::Track),
        };
        let primary_audio = audio_for(self.primary_file.borrow().clone(), primary);
        let secondary_audio = audio_for(secondary_file, secondary);

        let subtitle = self.subtitle.borrow().clone();
        if let Some(key) = self.storage_key() {
            crate::config::save_tracks(
                &key,
                primary,
                secondary,
                subtitle.clone(),
                self.saved_path(Role::Primary),
                self.saved_path(Role::Secondary),
            );
        }

        let app = self.clone();
        let on_ended = move |ended| {
            // Something else picked this video and is waiting for the playback
            // to finish, so reaching the end of it means there is nothing left
            // to do and the menu would only be in the way. An error is not the
            // same: quitting would take the reason off the screen with it.
            if app.external && ended == crate::player::Ended::Finished {
                app.finish_playback(true);
                app.window.close();
                return;
            }
            app.stop_playback();
            app.show_menu();
        };

        // "Restart" means start from the beginning whoever is asking, so it
        // beats both our saved position and Kodi's. Bound rather than passed
        // inline because the reveal below waits for playback to reach it.
        let resume = (!restart).then(|| self.resume_position()).flatten();

        // Worked out here rather than in the pipeline, because locating one
        // can need the server address and access token, which are ours to
        // know. A subtitle that cannot be found gives up the subtitle and not
        // the film: it is the least of what somebody pressed play for.
        let located = match self.locate_subtitle(&path, subtitle.as_ref()) {
            Ok(located) => located,
            Err(e) => {
                eprintln!("{e}");
                None
            }
        };
        // Either there is something to switch to, or something already chosen.
        // The second half is not redundant: `--play` goes straight past the
        // page that fills the list in, so a subtitle named on the command line
        // would otherwise resolve correctly and then have no overlay to be
        // drawn by.
        let offers_subtitles = !self.subtitle_options.borrow().is_empty() || located.is_some();

        let result = Playback::start(
            &path,
            primary_audio.as_ref(),
            secondary_audio.as_ref(),
            located.as_ref(),
            offers_subtitles,
            &self.config.borrow(),
            resume,
            self.storage_key().unwrap_or_default(),
            // Kodi's own path for the item, which is what it accepts progress
            // against. Empty when Kodi is not involved, which turns reporting
            // off rather than needing a flag of its own.
            self.kodi_item
                .borrow()
                .as_ref()
                .map(|item| item.file.clone())
                .unwrap_or_default(),
            on_ended,
        );

        match result {
            Ok(playback) => {
                // The pipeline set each sink from the configuration alone,
                // which is all it knows about. Any alignment baseline is added
                // here, once, before a frame has played.
                for role in ["primary", "secondary"] {
                    if self.baseline_ms(role) != 0.0 {
                        self.push_offset(&playback, role);
                    }
                }
                // Named by device rather than by role: "primary" and
                // "secondary" mean something to the configuration and nothing
                // to somebody trying to turn the headphones down.
                let outputs: Vec<(&'static str, String)> = {
                    let config = self.config.borrow();
                    [
                        ("primary", config.primary_sink.clone()),
                        ("secondary", config.secondary_sink.clone()),
                    ]
                    .into_iter()
                    .filter_map(|(role, name)| {
                        name.filter(|_| playback.has_output(role))
                            .map(|name| (role, name))
                    })
                    .collect()
                };
                let levels: Vec<(&str, f64, bool)> = outputs
                    .iter()
                    .map(|(role, _)| {
                        (
                            *role,
                            playback.volume(role).unwrap_or(1.0),
                            playback.muted(role),
                        )
                    })
                    .collect();

                let controls = Controls::new(
                    playback.widget(),
                    self.scale.get(),
                    self.window.is_fullscreen(),
                    self.locked_fullscreen,
                    &outputs,
                );
                controls.set_levels(&levels);
                // The pipeline built each output's level from that output's own
                // setting, which is all the configuration told it. The master is
                // applied here, once, before a frame has played - the same shape
                // as the alignment baseline below it.
                controls.set_master_level(self.config.borrow().master_volume());
                for (role, _) in &outputs {
                    playback.set_volume(role, self.effective(self.config.borrow().volume(role)));
                }
                // What the configuration holds for each output, so the panel
                // opens showing the shift already in force rather than zero.
                let syncs: Vec<(&str, f64, bool)> = {
                    let config = self.config.borrow();
                    outputs
                        .iter()
                        .map(|(role, _)| (*role, config.offset_ms(role), config.offset_on(role)))
                        .collect()
                };
                controls.set_syncs(&syncs);
                {
                    // Kept in the configuration, so a level set once holds for
                    // the next film: two outputs are rarely matched in
                    // loudness, and correcting that every time would be a
                    // chore rather than a control.
                    let app = self.clone();
                    controls.connect_volume(move |role, level, muted, persist| {
                        // Through the one function that knows about the master,
                        // rather than sent straight to the sink from here. What
                        // an output plays at is its own level times the master,
                        // and a second place doing that arithmetic is how the
                        // two come to disagree - the same lesson `push_offset`
                        // is already the answer to.
                        //
                        // Given the level rather than reading it back out of the
                        // configuration, because a level that is not being kept
                        // never reaches the configuration at all.
                        app.push_volume_at(role, level);
                        app.push_mute(role, muted);
                        if !persist {
                            return;
                        }
                        {
                            let mut config = app.config.borrow_mut();
                            config.set_volume(role, level);
                            config.set_muted(role, muted);
                        }
                        app.save_volume_soon();
                    });

                    // The master moves both outputs, so both are pushed again
                    // rather than one being singled out. Kept like a level and
                    // unlike a hush: somebody chose it, and a film that started
                    // at half volume because of last week is a setting, where a
                    // film that started silent would be a bug.
                    // Silencing everything is a layer over the outputs rather
                    // than a change to them, so what comes back is only whether
                    // the layer is on. Each output is then pushed at its own
                    // state underneath it, which is what it goes on showing.
                    let app = self.clone();
                    controls.connect_hush(move |hushed| {
                        app.hushed.set(hushed);
                        for role in ["primary", "secondary"] {
                            let muted = app.config.borrow().muted(role);
                            app.push_mute(role, muted);
                        }
                        app.report_sound_soon();
                    });

                    let app = self.clone();
                    controls.connect_master(move |level| {
                        app.config.borrow_mut().set_master_volume(level);
                        for role in ["primary", "secondary"] {
                            app.push_volume(role);
                        }
                        app.save_volume_soon();
                        app.report_sound_soon();
                    });

                    // Always kept, unlike a level silenced for a knock at the
                    // door: how far an output runs behind describes the
                    // equipment, not the moment.
                    let app = self.clone();
                    controls.connect_sync(move |role, ms, on| {
                        {
                            let mut config = app.config.borrow_mut();
                            config.set_offset_ms(role, ms);
                            config.set_offset_on(role, on);
                        }
                        app.push_offset_live(role);
                        app.save_volume_soon();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_play_pause(move || {
                        app.toggle_pause();
                        app.wake_controls();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_fullscreen(move || app.toggle_fullscreen());
                }
                {
                    // Holding the icon, which shows or hides what is already
                    // chosen. Tapping it opens the chooser instead.
                    let app = self.clone();
                    controls.connect_subtitles(move || app.toggle_subtitles());
                }
                {
                    let app = self.clone();
                    controls.connect_subtitle_chosen(move |entry| app.choose_subtitle(entry));
                }
                {
                    let app = self.clone();
                    controls.connect_audio_chosen(move |role, entry| app.choose_audio(role, entry));
                }
                {
                    // Under a launcher there is no menu worth returning to:
                    // something else chose this video and is waiting for the
                    // playback to end, which stopping is a way of saying.
                    let app = self.clone();
                    controls.connect_stop(move || {
                        if app.external {
                            app.finish_playback(true);
                            app.window.close();
                        } else {
                            app.leave_playback();
                        }
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_settings(move || app.leave_playback());
                }
                {
                    // The same step the arrow keys take, through the same
                    // path, so a tap of either lands in the same place.
                    let app = self.clone();
                    controls.connect_skip(move |seconds| {
                        app.scrub(seconds);
                        app.end_scrub();
                    });
                }
                {
                    let app = self.clone();
                    controls.connect_double_click(move || app.toggle_fullscreen());
                }
                {
                    let app = self.clone();
                    controls.connect_motion(move || app.wake_controls());
                }
                {
                    // Dragging emits a value for every pointer movement, and
                    // seeking on each one asks the pipeline to decode to a
                    // position that is already out of date - which is what
                    // made dragging unusable on a Pi. Only the latest target
                    // is kept, and one timer does the work.
                    //
                    // That timer also decides when the drag is over, by asking
                    // whether the pointer button is still down. A release
                    // event is no use here: the scale claims the button
                    // sequence for its own dragging, and a claimed sequence
                    // stops reaching anything else, in any phase.
                    let app = self.clone();
                    let pending: Rc<Cell<Option<u64>>> = Rc::new(Cell::new(None));
                    let running = Rc::new(Cell::new(false));
                    let scrubbing = Rc::new(Cell::new(false));

                    // The end of a drag, taken from the raw event stream. Not
                    // a gesture: GtkScale claims the button sequence while it
                    // drags, and a claimed sequence never reaches another
                    // gesture in any phase - watching for a release that way
                    // saw the press and nothing after it. Asking the pointer
                    // for its button state instead goes stale as soon as it
                    // stops moving. A legacy controller is not a gesture, so
                    // nothing can claim the event away from it.
                    {
                        let app = self.clone();
                        let pending = pending.clone();
                        let scrubbing = scrubbing.clone();
                        let watcher = gtk::EventControllerLegacy::new();
                        watcher.set_propagation_phase(gtk::PropagationPhase::Capture);
                        watcher.connect_event(move |_, event| {
                            if event.event_type() != gdk::EventType::ButtonRelease
                                || !scrubbing.replace(false)
                            {
                                return glib::Propagation::Proceed;
                            }
                            if let Some(playback) = app.playback.borrow().as_ref() {
                                if let Some(target) = pending.take() {
                                    playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                                    playback.commit_seek();
                                    app.publish_now_playing();
                                }
                                playback.release_from_scrub();
                            }
                            glib::Propagation::Proceed
                        });
                        self.window.add_controller(watcher);
                    }
                    controls.connect_seek(move |fraction| {
                        let playback = app.playback.borrow().clone();
                        let Some(playback) = playback else { return };
                        let Some(duration) = playback.duration() else {
                            return;
                        };

                        let target = (duration.nseconds() as f64 * fraction) as u64;
                        // Aimed at straight away, so the readout follows the
                        // pointer rather than being pulled back to where
                        // playback still is by the next tick.
                        playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                        pending.set(Some(target));
                        app.wake_controls();

                        scrubbing.set(true);
                        if running.replace(true) {
                            return;
                        }
                        // Held still while the drag lasts, so the picture stays
                        // where the pointer puts it instead of running on
                        // underneath it.
                        playback.hold_for_scrub();

                        let app = app.clone();
                        let pending = pending.clone();
                        let running = running.clone();
                        let scrubbing = scrubbing.clone();
                        glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                            let playback = app.playback.borrow().clone();
                            let Some(playback) = playback else {
                                running.set(false);
                                return glib::ControlFlow::Break;
                            };
                            if let Some(target) = pending.take() {
                                playback.aim_at(gstreamer::ClockTime::from_nseconds(target));
                                playback.commit_seek();
                                app.publish_now_playing();
                            }
                            if scrubbing.get() {
                                return glib::ControlFlow::Continue;
                            }
                            // Releasing has already committed the last target
                            // and let playback go.
                            running.set(false);
                            glib::ControlFlow::Break
                        });
                    });
                }
                {
                    // The mark has to follow the state however it changed:
                    // this button, the menu's, the F key, or the window
                    // manager.
                    let weak = Rc::downgrade(&controls);
                    self.window.connect_fullscreened_notify(move |window| {
                        if let Some(controls) = weak.upgrade() {
                            controls.set_fullscreen(window.is_fullscreen());
                        }
                    });
                }
                // Carried across leaving playback and coming back, since the
                // pipeline is rebuilt each time and starts with them on.
                if self.subtitles_hidden.get() && playback.subtitles_showing() {
                    playback.toggle_subtitles();
                }
                self.show_subtitle_state(&playback, &controls);
                self.push_audio_entries(&playback, &controls);
                controls.update(&playback);
                // Where playback has reached, and nothing else. A film
                // opening with a full row of buttons over it announces the
                // interface rather than the video.
                controls.peek();
                let widget = controls.widget().clone();
                // Taken before the playback is moved into its cell, since the
                // reveal below watches it for the first frame that lands.
                let paintable = playback.widget().paintable();
                *self.controls.borrow_mut() = Some(controls);
                self.start_tick();
                self.window
                    .set_title(Some(&self.file_label().unwrap_or_default()));
                *self.playback.borrow_mut() = Some(playback);
                self.publish_now_playing();
                self.jellyfin_reported.set(0);
                self.report_to_jellyfin(JellyfinMoment::Started);
                // Playback begins playing, so the display is held from here
                // until it is paused or torn down.
                self.awake.set(true);

                // Held back until playback has actually reached the resume
                // point. The pipeline prerolls before the seek completes, so
                // revealing it straight away shows the opening frame and then
                // jumps - which reads as a glitch rather than as resuming.
                // Everything above has already happened; only what is on
                // screen waits.
                match resume {
                    Some(target) => self.reveal_when_resumed(widget, paintable, target),
                    None => self.window.set_child(Some(&widget)),
                }
                // Nothing to move a selection through here.
                self.set_nav(None, &[], &[]);
                *self.screen.borrow_mut() = Screen::Playing;
            }
            Err(e) => self.show_error(&format!("Couldn't play that file.\n\n{e}"), false),
        }
    }

    /// Centered rather than top-aligned, and a full screen rather than a
    /// modal dialog: it has to be readable at the same distance as
    /// everything else and navigable without a pointer.
    ///
    /// Skipped when something else launched us, which closes straight away.
    /// The question guards against losing your place by accident, and there is
    /// nothing to lose here: the launcher is waiting for this process to end,
    /// and under Kodi the position has already gone back to its library.
    fn show_confirm_quit(self: &Rc<Self>) {
        if self.external {
            self.window.close();
            return;
        }

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();

        page.append(&heading_label("Close the Player?"));

        let buttons = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(24)
            .halign(gtk::Align::Center)
            .build();

        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("tp-button");
        let quit = gtk::Button::with_label("Close");
        quit.add_css_class("tp-button");
        quit.add_css_class("tp-danger");
        buttons.append(&cancel);
        buttons.append(&quit);
        page.append(&buttons);

        {
            let app = self.clone();
            cancel.connect_clicked(move |_| {
                app.sounds.borrow().click();
                app.show_menu();
            });
        }
        {
            let app = self.clone();
            quit.connect_clicked(move |_| app.window.close());
        }

        // Nothing to move a selection through here.
        self.set_nav(None, &[], &[]);
        *self.screen.borrow_mut() = Screen::ConfirmQuit;
        self.window.set_child(Some(&self.dialog_column(&page)));
        // Cancel takes focus so a reflexive second Enter doesn't quit.
        cancel.grab_focus();
    }

    fn show_error(self: &Rc<Self>, message: &str, fatal: bool) {
        self.error_is_fatal.set(fatal);

        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(32)
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Center)
            .margin_start(48)
            .margin_end(48)
            .build();

        page.append(&heading_label("Something went wrong"));

        // Given the window's width rather than a fixed column: these messages
        // carry paths and URLs, which are long, and wrapping them into a
        // narrow strip makes them harder to read than they need to be.
        let label = gtk::Label::new(Some(message));
        label.add_css_class("tp-hint");
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_justify(gtk::Justification::Center);
        label.set_hexpand(true);
        // So a path or a URL that went wrong can be copied out and pasted
        // somewhere useful, which is most of what anyone wants from an error.
        // Not focusable: the selection is for a pointer, and leaving it in the
        // focus order would put a stop between the message and the way out.
        label.set_selectable(true);
        label.set_can_focus(false);
        page.append(&label);

        // Only an unopenable video named on the command line ends the session:
        // it was the whole reason the player was started, and under a launcher
        // there is no menu behind it worth returning to.
        let back = gtk::Button::with_label(if fatal { "Close" } else { "Back" });
        back.add_css_class("tp-button");
        back.set_halign(gtk::Align::Center);
        page.append(&back);

        let app = self.clone();
        back.connect_clicked(move |_| {
            if app.error_is_fatal.get() {
                app.window.close();
            } else {
                app.show_menu();
            }
        });

        // Nothing to move a selection through here.
        self.set_nav(None, &[], &[]);
        // A path or a message that went wrong is the thing most worth copying
        // in the whole application.
        *self.copy_root.borrow_mut() = Some(page.clone().upcast());
        *self.screen.borrow_mut() = Screen::Error;
        self.window.set_child(Some(&self.dialog_column(&page)));
        back.grab_focus();
    }
}

// --- Widget helpers ----------------------------------------------------

/// A screen laid out as fixed header, scrolling list, and whatever the
/// caller pins below. The list scrolls rather than the page as a whole, so
/// a long list can never push the header or a footer button off-screen.
/// Always builds the back button, even on screens that have nowhere to go
/// back to, where it's made invisible instead of omitted. Leaving it out
/// changes the header's height, which shifted the heading and the whole
/// list every time the user moved between the menu and a chooser.
fn list_page(title: &str, show_back: bool) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let heading = heading_label(title);
    heading.set_xalign(0.0);
    let page = list_page_with(&heading, show_back);
    // The list carries the page's title, so arriving on one says where you
    // are before it says what row you are on. A reader gives the container's
    // name, then the position, then the row - which is the whole context in
    // one breath, and none of it read out unasked.
    name_it(&page.1, title);
    page
}

/// The same page with a heading of the caller's choosing, for the browser's
/// path trail.
fn list_page_with(
    heading: &impl IsA<gtk::Widget>,
    show_back: bool,
) -> (gtk::Box, gtk::ListBox, gtk::Button, gtk::Box) {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(16)
        .css_classes(["tp-header"])
        .build();
    // Its own box rather than sizing the widgets themselves: a button adds
    // padding and borders to whatever minimum it is given, so the arrow and
    // the mark never agree on a size. An empty box takes exactly the size the
    // stylesheet asks for, and the child sits centered inside it.
    let slot = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["tp-leading"])
        .build();

    let back = back_button();
    if !show_back {
        // Kept in the layout so it still occupies its space, but invisible
        // and skipped by focus.
        back.set_opacity(0.0);
        back.set_sensitive(false);
        back.set_can_focus(false);
    }
    slot.append(&back);
    header.append(&slot);

    header.append(heading);
    page.append(&header);

    let (scroller, list) = scrolling_list();
    page.append(&scroller);

    (page, list, back, slot)
}

/// The scrolling list every screen built around one shares, wired the way
/// navigation here expects to find it.
fn scrolling_list() -> (gtk::ScrolledWindow, gtk::ListBox) {
    let list = gtk::ListBox::new();
    list.add_css_class("tp-menu");
    // Browse keeps exactly one row selected as focus moves, which is what
    // the boundary checks in wire_navigation rely on.
    list.set_selection_mode(gtk::SelectionMode::Browse);
    list.set_activate_on_single_click(true);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    // Tab has to land on the list itself. The rows cannot take focus, and a
    // ScrolledWindow will take it to scroll with the arrow keys, so without
    // this the stop is the scroller and every key goes to it instead.
    scroller.set_focusable(false);
    list.set_focusable(true);
    (scroller, list)
}

/// The application mark, decoded from the PNG compiled into the binary.
///
/// A PNG rather than the SVG it was drawn from, because GStreamer's Windows
/// distribution ships no gdk-pixbuf loaders at all and so cannot decode SVG
/// at runtime. The SVG is still what Linux installs, where librsvg is present.
fn logo_image(scale: f64) -> gtk::Image {
    const LOGO: &[u8] = include_bytes!("../data/ui/tineplayer.png");

    let image = gtk::Image::new();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(LOGO)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        Err(e) => eprintln!("Could not load the application icon: {e}"),
    }
    // Shares the back arrow's fixed slot, so the title beside it sits in the
    // same place on every screen instead of shifting as you move between
    // them. Drawn a little smaller than the slot so it cannot force it wider.
    image.set_valign(gtk::Align::Center);
    image.set_pixel_size((30.0 * scale).round() as i32);
    image
}

/// How much room the film's description may take, in interface units.
///
/// Interface units rather than pixels because that is the question actually
/// being asked. Everything on the page scales together, so what decides
/// whether the plot fits is not how many pixels tall the window is but how
/// many rows-worth of interface fit in it - and at 3x on a 1440px screen that
/// is a third of what it is at 1x on the same screen.
///
/// The reservation is what the page cannot do without: the choosers, the
/// footer that plays the film, and the margins around them. Whatever is left
/// over is what the description gets, and at 3x on a modest display that is
/// nothing - which is the right answer. A page that shows a plot summary and
/// no way to press play has its priorities backwards.
///
/// This is the plan's open question about `ui_scale` answered: no-scroll and
/// 3x cannot both hold, and what yields is the artwork and the prose.
/// A button face: a drawn mark, and the words beside it.
///
/// A box rather than a label with a mark in the text, and both halves are
/// centered on their own terms. The marks were glyphs to begin with, which
/// meant they took the label's font size and came out smaller than the type
/// they sat with; sizing them up through markup then made the whole *line*
/// taller, so the words sat on a baseline set by the mark and read as having
/// slipped downwards. An image beside a label has neither problem, and the
/// mark is drawn at whatever size suits rather than at whatever the text is.
fn marked_face(mark: gtk::Image, words: &str) -> gtk::Box {
    let face = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    mark.set_valign(gtk::Align::Center);
    face.append(&mark);

    if !words.is_empty() {
        let text = gtk::Label::new(Some(words));
        text.set_valign(gtk::Align::Center);
        face.append(&text);
    }
    face
}

/// How wide the media page is allowed to get, in interface units.
///
/// A ceiling rather than a shape. The reason to stop widening is that a line
/// of prose gets too long to read and a row's value drifts too far from its
/// label - both of which are about width alone, so nothing here consults the
/// height. Below this the page simply fills the window at any proportion.
///
/// **1920 is not an arbitrary round number.** The automatic interface size is
/// the display's height over 1080, so on any 16:9 screen shown fullscreen this
/// works out to `1920 * height / 1080`, which is `height * 16/9` - the screen's
/// own width. A television therefore fills edge to edge, which is what the
/// page is composed for and what the 16:9 rule this replaced did directly.
/// Anything wider than 16:9, or any window short of fullscreen, is where the
/// ceiling starts doing something, and what is left over goes to the backdrop
/// on either side.
///
/// Set at 1600 first, which quietly left a 1920px screen with 320px of
/// backdrop down the sides at fullscreen - the one case that most wants
/// filling.
const PAGE_MAX_UNITS: f64 = 1920.0;

/// How wide a dialog is allowed to get, in interface units.
///
/// The same 900 the notices page and the selector popovers already stop at,
/// and for the same reason: past about this much, a line of prose is longer
/// than the eye tracks back from. Shared by every panel that asks a question,
/// so two of them in a row are the same shape.
const DIALOG_MAX_UNITS: f64 = 900.0;

/// How much of the page's height the poster takes.
///
/// Wider than it was, and the width is the point: the poster and the column
/// beside it share one line, so a broader poster is what sets how wide the
/// summary runs. The extra depth on both sides is what fills a 16:9 screen
/// rather than leaving a band along the bottom.
const POSTER_SHARE: f64 = 0.58;

/// The padding `.tp-selector > contents` draws around a selector's list,
/// which its own width has to account for. Kept beside the stylesheet value it
/// mirrors - `panel_pad` - because the two have to agree.
const SELECTOR_PAD: f64 = 8.0;

/// How narrow a selector is allowed to get, in interface units.
///
/// A list of short entries - "None", "Stereo", a two-word device name - would
/// otherwise open as a sliver, which reads as something gone wrong rather than
/// as a deliberately small menu.
const SELECTOR_MIN_WIDTH: f64 = 300.0;

/// How wide a selector is allowed to get before its entries ellipsize.
const SELECTOR_MAX_WIDTH: f64 = 900.0;

/// How tall a selector is allowed to get before it scrolls instead.
///
/// Not a share of the window, deliberately: a popover that fills the screen is
/// the full-screen chooser this replaces. This is roughly a dozen rows, which
/// is enough for every device list and short enough that the page it belongs
/// to is still visible around it - which is the whole reason for a popover.
const SELECTOR_HEIGHT: f64 = 520.0;

/// Three lines of summary, in interface units, reserved whether the film has
/// a summary or not.
///
/// The one fixed height on the page, and the only one that earns it: a plot
/// runs from nothing to a paragraph while everything else here is one line or
/// absent, so it is the only thing that would move the rows underneath as you
/// step from one film to the next. A film with no summary gets the space as
/// blank rather than getting it back.
const PLOT_UNITS: f64 = 90.0;

/// What stands in for a poster when there is none, which is most of the time.
///
/// A PNG per theme rather than the SVG it was drawn from, for the reason
/// [`logo_image`] gives: GStreamer's Windows distribution ships no gdk-pixbuf
/// loaders, so nothing there can decode an SVG at runtime. The two versions
/// carry the same ink as the fullscreen marks beside them.
fn video_file_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/video-file.png");

    let image = gtk::Image::new();
    if let Ok(texture) = gdk::Texture::from_bytes(&glib::Bytes::from_static(ICON)) {
        image.set_paintable(Some(&texture));
    }
    // Drawn well inside the frame rather than filling it: the mark is saying
    // there is no artwork, and one that reached the edges would read as
    // artwork.
    image.set_pixel_size(size.round().max(1.0) as i32);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    // Expands to centre itself in the frame. The request stops at the poster
    // column, which sets its own `hexpand` explicitly.
    image.set_hexpand(true);
    image.set_vexpand(true);
    // Decoration beside a title that already names the file.
    image.set_accessible_role(gtk::AccessibleRole::Presentation);
    image
}

/// Uppercased here rather than with the `text-transform` CSS property,
/// which needs a newer GTK than this project's baseline.
/// Whether a scrap of clipboard text is worth offering as something to open.
///
/// Deliberately shallow: it is looking for a mistake worth not making, not
/// deciding whether the thing exists. A sentence someone happened to copy is
/// rejected, an address or a path is offered, and being wrong costs a
/// selected field the next keystroke replaces.
fn looks_openable(text: &str) -> bool {
    if text.is_empty() || text.lines().count() > 1 {
        return false;
    }
    text.contains("://") || text.starts_with("\\\\") || std::path::Path::new(text).is_absolute()
}

fn heading_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("tp-title");
    label
}

/// The four-corner mark for entering or leaving fullscreen.
///
/// The subtitle mark for the control bar.
///
/// One white version rather than a light and a dark one: unlike the menus,
/// the control strip draws its own dark background whatever the theme is, so
/// there is nothing for a second version to adapt to.
pub fn subtitles_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/subtitles.png");
    marked_image(ICON, 26.0 * scale)
}

/// The mark on the button that opens one output's soundtracks.
///
/// A note rather than a speaker, which is the whole reason the soundtracks and
/// the levels are on separate buttons: two speakers side by side say nothing
/// about which is which, and this one is a choice about the film rather than
/// about how loud it is.
///
/// Bundled rather than taken from the icon theme, like every other mark on this
/// strip: GStreamer's Windows bundle ships no icon theme at all, and a missing
/// icon draws as a broken-image box.
pub fn soundtrack_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/soundtrack.png");
    marked_image(ICON, 26.0 * scale)
}

/// The mark on the button that puts an output back in sync.
///
/// Drawn rather than taken from the icon theme, for the same reason the
/// fullscreen and subtitle marks are: nothing in the theme means "line these
/// up". `emblem-synchronizing-symbolic` comes closest and is in Adwaita, but
/// GStreamer's Windows bundle ships no icon theme at all - there is only the
/// set GTK compiles into itself - and a missing icon draws nothing rather
/// than failing, which is the worst way to find out.
///
/// One version rather than a light and a dark one, like the subtitle mark:
/// the control strip draws its own dark background whatever the theme is.
/// The size of the strip's icons, before scaling: the transport buttons, the
/// gear, and the buttons in the volume panel.
pub const ICON_PX: f64 = 24.0;

pub fn sync_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/sync.png");
    marked_image(ICON, SYNC_MARK_PX * scale)
}

/// The sync mark's size before scaling, which is deliberately not [`ICON_PX`]
/// like everything else in that panel.
///
/// Larger, so that it *looks* the same size. The speaker above it is a themed
/// icon whose glyph fills its box, while this is a drawn mark with clear space
/// around it - so at the same nominal size the stopwatch came out visibly the
/// smaller of the two.
///
/// The number is arithmetic rather than taste: the mark's ink fills 83% of its
/// canvas, the speaker draws 29px, and 29 / 0.83 / 1.25 lands here. Eyeballing
/// the screenshot first gave 32, which would have overshot to 34px - so if the
/// artwork is ever redrawn with different margins, measure the ink rather than
/// nudging this by feel.
const SYNC_MARK_PX: f64 = 28.0;

/// The fullscreen mark, in the direction it will take you.
///
/// Drawn for this application rather than taken from the icon theme: the
/// bundled theme has 157 icons and none of them mean fullscreen. The nearest,
/// `window-maximize-symbolic`, is a small square that reads as "maximize".
///
/// Drawn twice in each direction, once in each theme's foreground color,
/// because an embedded image cannot be recoloured the way a symbolic icon is.
/// A single compromise gray read poorly against both.
///
/// **`dark` is about the surface, not about the theme.** The control strip
/// draws its own near-black background under either theme, so it asks for the
/// dark-theme mark always - see [`marked_image`].
pub fn fullscreen_image(fullscreen: bool, scale: f64) -> gtk::Image {
    const ENTER: &[u8] = include_bytes!("../data/ui/fullscreen.png");
    const LEAVE: &[u8] = include_bytes!("../data/ui/restore.png");

    let bytes = match fullscreen {
        true => LEAVE,
        false => ENTER,
    };
    marked_image(bytes, CORNER_MARK_PX * scale)
}

/// The gear, for the settings screen.
///
/// A pair like the fullscreen marks, and for the same reason: it sits on the
/// page under either theme. It used to be `emblem-system-symbolic` from the
/// icon theme, which GTK recolors from the foreground - including dimming it
/// when the window loses focus, while the drawn mark beside it did not, so the
/// two came apart every time the window went to the back.
/// `size` is in real pixels and is the caller's to decide, because the gear
/// appears at two sizes: beside the fullscreen mark on the media page, where
/// the two have to agree, and among the transport icons on the control strip,
/// where it has to agree with those instead.
pub fn settings_image(size: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/settings.png");

    marked_image(ICON, size)
}

/// How large the two marks in the media page's corner are drawn, before
/// scaling. One number for both, so they cannot drift apart.
const CORNER_MARK_PX: f64 = 26.0;

/// The mark beside a name in the file browser, in interface units: the height
/// of the box a file mark is drawn into.
///
/// The marks are cropped to their ink before they are bundled, which is what
/// makes one number mean the same thing for all of them. Drawn as exported
/// they carried a wide empty margin - the page shape filled 54% of its canvas
/// across and 67% down - so a size set by eye against the icons that came
/// before was mostly padding, and the marks came out small however large the
/// number grew.
/// The operating system, written as people write it. `std::env::consts::OS`
/// answers in lowercase identifiers - "macos", "windows" - which read as a
/// build target rather than as a machine.
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// How much room the About text keeps inside its panel, in interface units.
const ABOUT_INSET: f64 = 18.0;

/// The mark beside the application's name on the About page, in interface
/// units.
const ABOUT_LOGO: f64 = 46.0;

/// How tall the notices are allowed to grow before they scroll, as a share of
/// the window. A dialog is a thing on top of a screen, and one that reaches
/// the edges is a screen wearing a border.
const NOTICES_SHARE: f64 = 0.8;

/// How wide the notices dialog is allowed to get, in interface units. About
/// the length of line prose is comfortable to read.
const NOTICES_WIDTH: f64 = 900.0;

/// How wide the settings screen's column of categories is, in interface
/// units. Fixed rather than sized to its contents, so the pane beside it does
/// not move when the longest category name changes.
const CATEGORY_WIDTH: f64 = 260.0;

const ROW_MARK_PX: f64 = 34.0;

/// The same, for a folder in a listing. A little smaller: a folder is a wide
/// shape where a page is a tall one, so an equal box fills more of the line
/// with ink and puts the folders ahead of the files in a list that is mostly
/// files.
const FOLDER_MARK_PX: f64 = 29.0;

/// The folder on the button that opens the system browser, which is smaller
/// again: a mark beside a line of text rather than one standing on its own.
const BUTTON_FOLDER_PX: f64 = 24.0;

/// How wide the marks' column is, whichever mark is in it. Wide enough for the
/// broadest of them with a little air, so the names line up down the list.
const MARK_COLUMN_PX: f64 = 32.0;

/// The triangle on the play button, and the arrow on restart.
///
/// White under either theme, because both sit on the blue button rather than
/// on the page. The play mark is deliberately not the one the control strip
/// uses: that one is the theme's own transport icon, and this is the button a
/// whole page is pointing at.
pub fn play_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/play.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

pub fn restart_image(scale: f64) -> gtk::Image {
    const ICON: &[u8] = include_bytes!("../data/ui/restart.png");
    marked_image(ICON, PLAY_MARK_PX * scale)
}

/// How large the marks on the play and restart buttons are drawn, before
/// scaling. Bigger than the strip's icons: these are the one action the page
/// exists to offer, and they are read from across a room.
const PLAY_MARK_PX: f64 = 26.0;

/// An image from bytes compiled into the binary, at a size in real pixels.
///
/// The size is set here rather than in the stylesheet because `-gtk-icon-size`
/// sizes icon *names*, and every mark in this application is a paintable - so
/// the CSS that catches a themed icon passes silently over these. A pixel or
/// two out and a button is a different width, which in the volume panel moves
/// the start of a bar and leaves the two bars visibly different lengths.
fn marked_image(bytes: &'static [u8], size: f64) -> gtk::Image {
    let image = gtk::Image::new();
    match gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        // Said out loud: a mark that silently fails to appear looks like a
        // button with nothing on it, which is not a clue anyone can act on.
        Err(e) => eprintln!("Could not load an interface mark: {e}"),
    }
    image.set_pixel_size(size.round().max(1.0) as i32);
    image
}

/// Publishes the current row as the list's `active-descendant`.
///
/// **This is not what makes a list audible.** Rows take focus and the focus
/// moves with the selection, and a screen reader speaks on focus changes and
/// on nothing else. Verified 2026-08-05 against Windows UI Automation:
/// stepping down the settings list moves the focused element from one
/// `ListItem` to the next, each named with its full row text.
///
/// Kept because the relation is correct by the specification and costs
/// nothing, but nothing should be built on it announcing anything. Publishing
/// the current item as state alone was tried twice - selection on the rows,
/// then this relation - and both were silent in practice.
///
/// Hung off `row-selected` rather than off the places that select, because
/// there are many of those - arrow keys, the gamepad, page keys, a pointer,
/// and every screen that opens on a remembered row - and one signal catches
/// them all.
fn announce_selection(list: &gtk::ListBox) {
    list.connect_row_selected(|list, row| match row {
        Some(row) => {
            list.update_relation(&[gtk::accessible::Relation::ActiveDescendant(
                row.upcast_ref(),
            )]);
        }
        None => list.reset_relation(gtk::AccessibleRelation::ActiveDescendant),
    });
}

/// Appends a row to a list and gives it a name.
///
/// The name goes on the row GTK wraps around the widget, not on the labels
/// inside it, because GTK derives a name from a child label but not from a
/// grandchild. A row built as a box of two labels therefore had no name, and
/// a screen reader announced it as "3 of 6" and nothing more.
fn append_named(list: &gtk::ListBox, child: &impl IsA<gtk::Widget>, name: &str) {
    list.append(child);
    if let Some(row) = child.as_ref().parent().and_downcast::<gtk::ListBoxRow>() {
        name_it(&row, name);
        // The list is one stop in the tab order, not one per row. A folder of
        // two hundred files is otherwise two hundred presses between you and
        // the button below it, which is the difference between usable and
        // not for anyone who navigates by Tab.
        //
        // Rows stay focusable all the same, and the focus follows the
        // selection. Making them unfocusable was the obvious way to get one
        // tab stop and it silenced the screen reader completely: focus
        // arrived at the list and never moved again, so Narrator read the
        // list and its first row and then nothing, however far down somebody
        // travelled. Selection alone is not enough - checked against Windows
        // UI Automation, which showed `IsSelected` moving correctly from row
        // to row while the focused element stayed the list throughout, and a
        // screen reader speaks on focus.
        //
        // One tab stop comes from `move_focus_stop` instead, which finds the
        // stop containing the focus and steps to the next one, so a focused
        // row still counts as being on its list.
    }
}

/// Opens a folder in whatever the machine browses files with.
///
/// **Not `AppInfo::launch_default_for_uri`, which was tried first.** GIO
/// answers "No application is registered as handling this file" for a
/// `file://` directory on Windows - measured 2026-08-12, with the link
/// reaching this code and the launch failing every time. There is nothing to
/// register: the shell is what opens folders, and GIO's table of URI handlers
/// does not know that.
///
/// So each platform is asked in its own words. The exit status is deliberately
/// not read: `explorer.exe` reports failure on success often enough to be
/// famous for it, and there is nothing useful to do with the answer anyway.
fn show_folder(folder: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let mut opener = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        // No console window for a GUI application to flash up behind itself.
        command.creation_flags(0x0800_0000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut opener = std::process::Command::new("open");
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let mut opener = std::process::Command::new("xdg-open");

    if let Err(e) = opener.arg(folder).spawn() {
        eprintln!("Could not open {}: {e}", folder.display());
    }
}

/// The explanation drawn under a settings row.
///
/// Never selectable and never focusable. It is not a control and not a value:
/// a caret landing in it, or an arrow key stopping on it, would be the
/// interface answering a question nobody asked.
fn row_note(text: &str, scale: f64) -> gtk::Label {
    let px = |base: f64| (base * scale).round() as i32;
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row-note");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_can_focus(false);
    // Lined up with the name above it, which sits inside the row's own
    // padding, and clear of the row below.
    label.set_margin_start(px(18.0));
    label.set_margin_end(px(18.0));
    label.set_margin_bottom(px(10.0));
    label
}

/// How a settings row reads aloud: the setting, then what it is set to.
fn row_name(label: &str, value: &str) -> String {
    if value.is_empty() {
        label.to_string()
    } else {
        format!("{label}, {value}")
    }
}

/// Gives a control a name for anyone who cannot see the picture on it. The
/// same reasoning as the copy in `controls`, which names the playback strip.
fn name_it(widget: &impl IsA<gtk::Accessible>, name: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(name)]);
}

fn back_button() -> gtk::Button {
    // An icon rather than a text glyph: a "‹" character sits off the
    // vertical center because it's positioned by font metrics rather than
    // by the icon's own bounding box.
    let button = gtk::Button::from_icon_name("go-previous-symbolic");
    button.add_css_class("tp-back");
    name_it(&button, "Back");
    button.set_valign(gtk::Align::Center);
    button
}

/// How a level reads in the settings menu. A silenced output says so rather
/// than showing the level it will return to, which is what the panel during
/// playback does too.
pub fn volume_label(level: f64, muted: bool) -> String {
    if muted {
        "Muted".to_string()
    } else {
        format!("{}%", (level * 100.0).round() as u32)
    }
}

/// The go-ahead button on a dialog: what it says, and whether pressing it
/// destroys something.
///
/// The two travel together because the second decides which button wears the
/// warning colour, and answering one without the other is what produced a red
/// Cancel sitting beside a plain Remove.
struct Confirm<'a> {
    label: &'a str,
    destructive: bool,
}

/// A panel over the screen that opened it: a heading, then whatever it has to
/// say, centered.
///
/// Named for the wizard it was written for. What is left of that is the shape:
/// a heading, some lines to read, and buttons - which is what a confirmation
/// and the sandbox instructions both are.
fn wizard_page(title: &str) -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .valign(gtk::Align::Center)
        .margin_top(40)
        .margin_bottom(40)
        .margin_start(56)
        .margin_end(56)
        .build();
    let heading = heading_label(title);
    heading.set_halign(gtk::Align::Center);
    // halign centers the label in the panel; justify centers the lines within
    // the label. Without it a heading that wraps, or one written across two
    // lines, sits centered as a block with its second line ragged left.
    heading.set_justify(gtk::Justification::Center);
    page.append(&heading);
    page
}

/// A line of explanation on a wizard panel. Selectable, so a command or a
/// path can be copied out with Ctrl+C, but never focusable: these are read,
/// not operated.
fn wizard_text(text: &str, command: bool) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(if command {
            gtk::pango::WrapMode::Char
        } else {
            gtk::pango::WrapMode::WordChar
        })
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .css_classes([if command { "tp-path" } else { "tp-hint" }])
        .build();
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// A settings row carrying a switch rather than the word "On" or "Yes".
///
/// The switch is a readout, not a control: it cannot be clicked or focused,
/// and the row it sits in is what gets activated. That keeps one way of
/// working the menu - move to a row, press it - rather than a second target
/// inside the row that only a pointer could reach.
fn switch_row(label: &str, on: bool) -> (gtk::Box, gtk::Switch) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    row.append(&name);

    let switch = gtk::Switch::new();
    switch.set_active(on);
    // A switch already reports whether it is on; without a name it reports
    // that about nothing in particular.
    name_it(&switch, label);
    switch.set_can_focus(false);
    switch.set_valign(gtk::Align::Center);
    row.append(&switch);

    (row, switch)
}

/// A settings row carrying a slider rather than a value and a chevron.
///
/// A level is a quantity, not a choice from a list, and a list of ten
/// percentages was a menu pretending to be a dial. Left and right move it
/// where they would otherwise do nothing on this screen, and the row keeps
/// the reading beside it so it can be set without looking at the bar.
/// A row with a bar, its reading, and for the ones that can be turned off, a
/// switch beyond it.
///
/// The switch rather than a value of its own: muted is not a quieter level
/// and an unapplied delay is not a shorter one, so both are a second thing
/// about the row, and the bar keeps saying what it will be when it is back
/// on.
fn slider_row(
    label: &str,
    width: i32,
    range: std::ops::RangeInclusive<f64>,
    now: f64,
    reading: &str,
    toggle: Option<bool>,
) -> (gtk::Box, gtk::Scale, gtk::Label, Option<gtk::Switch>) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    // The label takes the slack instead of the slider, which keeps the bar
    // over on the right where every other row shows its value. A bar the
    // width of the screen also reads as far more precision than a level has.
    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    name.set_hexpand(true);
    row.append(&name);

    let scale = gtk::Scale::with_range(
        gtk::Orientation::Horizontal,
        *range.start(),
        *range.end(),
        1.0,
    );
    scale.set_draw_value(false);
    scale.set_size_request(width, -1);
    scale.set_can_focus(false);
    scale.set_value(now);
    scale.add_css_class("tp-progress");
    // Settings bars only. The same class draws the video timeline and the
    // bars in the volume panel, which sit over a picture rather than on a
    // page of rows and are not the ones that disappear into the background.
    scale.add_css_class("tp-bar");
    name_it(&scale, label);
    row.append(&scale);

    // Wide enough for the longest reading any slider shows, so the bar beside
    // it never shifts as the value changes. `set_width_chars` is a minimum
    // rather than a maximum, so a reading longer than this would still push
    // the bar - which is what made the sync slider jump under the pointer
    // while it was being dragged, since "In sync" and "1000 ms earlier" are
    // eight characters apart.
    //
    // The same width for every slider rather than one each. A per-row width
    // would leave the bars ending at different places down the column, which
    // is worse to look at than the whitespace a short reading leaves here.
    let value = gtk::Label::new(Some(reading));
    value.add_css_class("tp-value");
    value.set_xalign(1.0);
    value.set_width_chars(READING_CHARS);
    row.append(&value);

    // The wheel scrolls the list it is in, rather than moving the bar under
    // the pointer. A settings screen is a list first: passing over a slider
    // on the way down it should not change a setting, and the value that
    // changes is the one nobody was looking at.
    //
    // Taken in the capture phase so the bar never sees it, and passed on to
    // the scroller by hand, since stopping the event stops it reaching the
    // list as well.
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(|controller, _, down| {
        let Some(scroller) = controller
            .widget()
            .and_then(|widget| widget.ancestor(gtk::ScrolledWindow::static_type()))
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return glib::Propagation::Stop;
        };
        let adjustment = scroller.vadjustment();
        // A row at a time, near enough: the step increment on a list is the
        // height of what it holds, and a tenth of a page where it is not set.
        let step = if adjustment.step_increment() > 0.0 {
            adjustment.step_increment()
        } else {
            adjustment.page_size() / 10.0
        };
        let wanted = adjustment.value() + down * step;
        adjustment.set_value(wanted.clamp(
            adjustment.lower(),
            (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
        ));
        glib::Propagation::Stop
    });
    scale.add_controller(wheel);

    let toggle = toggle.map(|on| {
        let switch = gtk::Switch::new();
        switch.set_active(on);
        name_it(&switch, label);
        switch.set_can_focus(false);
        switch.set_valign(gtk::Align::Center);
        row.append(&switch);
        // A bar that cannot be moved says so, rather than being moved to no
        // effect and leaving somebody to work out why nothing changed.
        scale.set_sensitive(on);
        value.set_sensitive(on);
        switch
    });

    (row, scale, value, toggle)
}

/// One piece of the notices page.
enum Notice {
    Heading(String),
    Text(String),
}

/// Turns THIRD-PARTY.md into something worth reading on a screen.
///
/// Not a Markdown renderer, and it does not need to be: the file is headings,
/// paragraphs and tables, and only the tables need doing anything to. A row of
/// pipes reads as punctuation rather than as a list, so the cells are joined
/// with a dash - `serde - 1.0.229 - MIT OR Apache-2.0` - and the rule under
/// each header is dropped, having nothing to say without the pipes around it.
///
/// Paragraphs are gathered rather than emitted line by line, so that text
/// wrapped at eighty columns in the file wraps to the window here instead.
fn notices_blocks(source: &str) -> Vec<Notice> {
    let mut blocks = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();

    let flush = |paragraph: &mut Vec<String>, blocks: &mut Vec<Notice>| {
        if !paragraph.is_empty() {
            blocks.push(Notice::Text(paragraph.join(" ")));
            paragraph.clear();
        }
    };

    for line in source.lines() {
        let line = line.trim();
        // The rule under a table header, which is pipes and dashes and no
        // words at all.
        if line.starts_with('|') && line.trim_matches(['|', '-', ':', ' ']).is_empty() {
            continue;
        }
        if let Some(heading) = line.strip_prefix("## ").or_else(|| line.strip_prefix("# ")) {
            flush(&mut paragraph, &mut blocks);
            blocks.push(Notice::Heading(heading.to_string()));
        } else if let Some(row) = line.strip_prefix('|') {
            // A table row stands alone rather than joining the paragraph
            // around it: two hundred crates read as a list, not as prose.
            flush(&mut paragraph, &mut blocks);
            let cells: Vec<&str> = row
                .trim_end_matches('|')
                .split('|')
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .collect();
            blocks.push(Notice::Text(cells.join("  -  ")));
        } else if line.is_empty() {
            flush(&mut paragraph, &mut blocks);
        } else {
            // Markdown decoration that would otherwise be read aloud as
            // punctuation, and the note marker, which is a label for a
            // renderer rather than words for a reader.
            let text = line
                .trim_start_matches("> ")
                .trim_start_matches('>')
                .trim_start_matches("- ")
                .replace("**", "")
                .replace('`', "");
            if text.trim() == "[!NOTE]" {
                continue;
            }
            paragraph.push(text.trim().to_string());
        }
    }
    flush(&mut paragraph, &mut blocks);
    blocks
}

/// A heading within a page of prose. Named rather than styled inline so the
/// About page reads as a document rather than as a form.
fn about_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about-heading");
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_selectable(true);
    label.set_can_focus(false);
    label
}

/// Where the address sits in relation to the sentence introducing it.
/// A line ending in a link that opens in the machine's browser. The address
/// is shown as written rather than hidden behind words, since on a screen
/// nobody can click there is still a use in being able to read it out.
///
/// Always on the same line as the sentence introducing it. There was a second
/// arrangement that put a long address on a line of its own, because one read
/// character by character is hard to pick back out of a wrapped paragraph -
/// and the only long address has gone, the notices it pointed at now being a
/// row directly below rather than a page on the web.
fn about_link(lead: &str, href: &str, shown: &str) -> gtk::Label {
    let label = about_text("");
    label.set_markup(&format!(
        "{} <a href=\"{}\">{}</a>",
        glib::markup_escape_text(lead),
        glib::markup_escape_text(href),
        glib::markup_escape_text(shown),
    ));
    label
}

fn about_text(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-about");
    label.set_xalign(0.0);
    label.set_wrap(true);
    // Selectable so a path or a version can be copied out rather than
    // transcribed, but never focusable: GTK gives a selectable label focus by
    // default, which would put a caret in the middle of a page navigated by
    // arrow keys.
    label.set_selectable(true);
    label.set_can_focus(false);
    // Long enough to read as paragraphs, short enough that the eye finds the
    // next line: a line the width of a television is unreadable.
    label.set_max_width_chars(72);
    label
}

/// Puts a list on a row, and scrolls it there once there is a page to scroll.
///
/// Focus alone is not enough. A screen is built and handed to the window in
/// one go, so at the moment a row is focused nothing has been laid out yet:
/// the scroller has no height, the row has no position, and the scroll that
/// would have followed the focus has nowhere to go. Coming back to a screen
/// therefore landed at the top of it, however far down you had been.
///
/// So the scroll is done by hand, and not until the row has been mapped -
/// which is the point at which it knows where it is.
fn settle_on(row: &gtk::ListBoxRow) {
    let ticket = claim_settling();
    // The row itself, so a screen reader has a focus change to announce.
    row.grab_focus();
    // Setting the window's child maps the new page there and then, so by the
    // time a screen picks its row the row is usually mapped already and
    // waiting for the signal would be waiting forever. Only the first screen
    // of a session arrives unmapped, because the window itself is not up yet.
    if row.is_mapped() {
        after_layout(row, ticket);
    } else {
        row.connect_map(move |row| after_layout(row, ticket));
    }
}

thread_local! {
    /// Which settling is the current one. See [`claim_settling`].
    static SETTLING: Cell<u64> = const { Cell::new(0) };
}

/// Claims the right to be the row the deferred work below settles on, and
/// supersedes whatever claimed it before.
///
/// Settling a row is not finished when [`settle_on`] returns: where the row is
/// and how far the scroller can travel are only known after a layout pass, so
/// the last of it waits for an idle. Nothing about that idle was tied to the
/// row still being the one wanted, and holding an arrow key queues one per
/// press - so the earlier ones came due against rows already left behind, and
/// scrolled them back into view. Arrowing quickly down a list threw the page
/// back to wherever the cursor had been a few presses ago.
///
/// A ticket rather than cancelling the pending idle: several things settle
/// rows - the arrow keys, a screen being built, a popover opening over one -
/// and none of them knows about the others. Each takes the next number, and
/// deferred work runs only while its number is still the current one, so the
/// most recent claim always wins without anybody having to be told.
///
/// This does not replace [`focus_is_outside`], which covers what a ticket
/// cannot: moving from the top row up to a header button focuses the button
/// without settling anything, so no new ticket is taken and only the focus
/// check sees that the row is no longer where the viewer is.
fn claim_settling() -> u64 {
    SETTLING.with(|settling| {
        let ticket = settling.get().wrapping_add(1);
        settling.set(ticket);
        ticket
    })
}

/// Whether this settling is still the one in force.
fn settling_is_current(ticket: u64) -> bool {
    SETTLING.with(|settling| settling.get() == ticket)
}

/// Runs once the page has been through a layout pass, which is when a row
/// finally knows where it is and the scroller knows how much of it there is
/// to move.
fn after_layout(row: &gtk::ListBoxRow, ticket: u64) {
    let row = row.clone();
    glib::idle_add_local_once(move || {
        // Only while this is still the row being settled on. Anything settled
        // since - the next row under a held arrow key, another screen, a
        // popover opening - has taken a later ticket and this one is stale.
        if !settling_is_current(ticket) {
            return;
        }
        // Only if the focus is still in this list. The grab below is a second
        // attempt, for the one case where the first one was too early to take
        // - and a second attempt that runs unconditionally is a second attempt
        // at stealing the focus back from wherever it has since gone.
        //
        // Arrowing up off the top row twice in quick succession did exactly
        // that: the first press selected the top row and queued this, the
        // second moved out to the Play button, and then this fired and pulled
        // the focus back down to the row. Slowly it was fine, because the idle
        // had already run before the second press arrived - which is the shape
        // of every bug that only happens when you are not being careful.
        if focus_is_outside(&row) {
            return;
        }
        // Only if it has not already taken. A focus grab inside a scroller
        // makes GTK scroll the row into view, so repeating one that already
        // succeeded sets a second scroll going against the one below.
        if !row.has_focus() {
            row.grab_focus();
        }
        show_row(&row);
    });
}

/// Whether the focus has left the list this row belongs to.
///
/// Nothing focused at all counts as inside: that is the state on the very
/// first screen of a session, before anything has taken focus, and it is
/// precisely when the deferred grab is needed.
fn focus_is_outside(row: &gtk::ListBoxRow) -> bool {
    let (Some(root), Some(list)) = (row.root(), row.parent()) else {
        return false;
    };
    match root.focus() {
        Some(focused) => focused != list && !focused.is_ancestor(&list),
        None => false,
    }
}

/// Moves the scroller so a row is fully on screen, by the smallest amount
/// that does it.
///
/// The minimum on purpose, and it used to place the row a third of the way
/// down the frame instead - which looks better in isolation and is the wrong
/// rule here, because this is not the only thing scrolling. Focusing a row
/// inside a scroller makes GTK bring it into view too, by the smallest amount.
/// Two rules that disagree about where a row belongs produce whichever answer
/// ran last: arrowing down kept the row at the bottom edge on the presses
/// where GTK's scroll had already satisfied this one, and threw the row up
/// near the top on the presses where it had not. Nothing about the input
/// differed, so it read as random.
///
/// Agreeing with GTK is what makes it predictable, and it is also the better
/// behaviour while arrowing: the row stays where it is and the list moves one
/// row under it, rather than the page jumping every time the edge is reached.
fn show_row(row: &gtk::ListBoxRow) {
    let Some(list) = row.parent() else { return };
    let mut ancestor = list.parent();
    let scroller = loop {
        match ancestor {
            Some(widget) => match widget.downcast::<gtk::ScrolledWindow>() {
                Ok(scroller) => break scroller,
                Err(widget) => ancestor = widget.parent(),
            },
            None => return,
        }
    };

    // The row's own allocation inside the list, which is where it sits in the
    // content and does not move when the content is scrolled.
    //
    // Asked of the widget tree with `translate_coordinates` before, which
    // looks equivalent and is not. The step above this one grabs the row's
    // focus, and GTK answers a focus grab inside a scroller by scrolling the
    // row into view itself - moving the adjustment and re-allocating the list
    // underneath us. `translate_coordinates` then reported whichever
    // allocation happened to be current: the row's place in the list on one
    // press, its place on screen on the next.
    //
    // On screen it is always the same place, hard against the bottom edge, so
    // every other press computed the same destination near the top of the list
    // and jumped there - and the two answers diverged further the further down
    // the list you had gone.
    let top = f64::from(row.allocation().y());
    let adjustment = scroller.vadjustment();
    let page = adjustment.page_size();
    // Already on screen: leave it where it is rather than jumping the page
    // about under someone who can see the row perfectly well.
    let value = adjustment.value();
    let bottom = top + f64::from(row.height());
    let wanted = if top < value {
        // Off the top: bring its top edge to the top of the frame.
        top
    } else if bottom > value + page {
        // Off the bottom: bring its bottom edge to the bottom of the frame.
        bottom - page
    } else {
        return;
    };
    adjustment.set_value(wanted.clamp(adjustment.lower(), (adjustment.upper() - page).max(0.0)));
}

/// Where a stored language code sits in the offered list.
fn language_position(code: Option<&str>) -> Option<usize> {
    let code = code?;
    crate::languages::LANGUAGES
        .iter()
        .position(|(stored, _, _, _)| *stored == code)
}

/// As far down the About page as it goes, which is the top of the last
/// screenful rather than the bottom of the text.
fn about_bottom(adjustment: &gtk::Adjustment) -> f64 {
    (adjustment.upper() - adjustment.page_size()).max(adjustment.lower())
}

/// Binds an action to each of `keys` under every modifier this platform
/// answers a shortcut on.
///
/// `<Primary>` everywhere, which is Control on all three platforms, plus
/// Command on macOS - where `<Primary>` is emphatically not it. See
/// `install_accelerators` for how that was measured.
fn bind_accels(gtk_app: &gtk::Application, action: &str, keys: &[&str]) {
    let mut accels = Vec::new();
    for key in keys {
        accels.push(format!("<Primary>{key}"));
        if cfg!(target_os = "macos") {
            accels.push(format!("<Meta>{key}"));
        }
    }
    let accels: Vec<&str> = accels.iter().map(String::as_str).collect();
    gtk_app.set_accels_for_action(action, &accels);
}

/// The modifiers a shortcut may be pressed with here, as one mask to test a
/// key event against.
///
/// `<Primary>` is asked of GTK rather than written out per platform, so the
/// keys matched by hand cannot drift from the ones bound as accelerators.
/// Command is added on macOS for the same reason it is bound there, and it is
/// why this is tested with `intersects` rather than `contains`: the mask holds
/// two modifiers on that platform and either one alone means yes.
fn primary_mask() -> gdk::ModifierType {
    let mut mask = gtk::accelerator_parse("<Primary>a")
        .map(|(_, mask)| mask)
        .unwrap_or(gdk::ModifierType::CONTROL_MASK);
    if cfg!(target_os = "macos") {
        mask |= gdk::ModifierType::META_MASK;
    }
    mask
}

fn last_row_index(list: &gtk::ListBox) -> i32 {
    let mut last = 0;
    while list.row_at_index(last + 1).is_some() {
        last += 1;
    }
    last
}

/// How a subtitle reads in a list.
///
/// The label of anything found beside the video is a language tag, written the
/// way the convention writes it - "en", "en.hi", "pt-BR" - and is put into
/// words. A file chosen by hand is labelled with its own name, which is not a
/// tag and would come out mangled if it were read as one.
fn subtitle_label(option: &Subtitle) -> String {
    match option {
        Subtitle::File { label, .. } => label.clone(),
        other => crate::languages::describe_tag(other.label()),
    }
}

fn describe_audio_track(track: &AudioTrack) -> String {
    // Checked against the title, which is where a language most often gets
    // named twice: a track tagged `eng` and titled "English Commentary" needs
    // no help, and would otherwise read "eng (English) - ... - English
    // Commentary".
    let mut text = format!(
        "{} — {} {}ch",
        crate::languages::describe_tag_unless(&track.language, &track.title),
        track.codec,
        track.channels
    );
    if !track.title.is_empty() {
        text.push_str(&format!(" — {}", track.title));
    }
    text
}

/// A stored alignment as a statement rather than as a signed number.
///
/// Which way the audio runs is the whole of what it says, and "+830ms" does
/// not say it. This is read by someone checking a correction they cannot see
/// the effect of, so it has to be unambiguous without a convention to look up.
fn describe_lateness(millis: f64) -> String {
    let rounded = millis.round();
    if rounded > 0.0 {
        format!("Audio {rounded:.0}ms late")
    } else if rounded < 0.0 {
        format!("Audio {:.0}ms early", -rounded)
    } else {
        "In step".to_string()
    }
}

/// A menu row: what the setting is on the left, its current value and a
/// chevron on the right.
/// The heading that opens a group of rows: which output the three rows under
/// it belong to.
///
/// A `GtkListBox` header rather than a row, which is what makes it
/// unselectable for free - headers sit outside the selection model and outside
/// the focus chain, so the arrow keys walk past without being told to.
///
/// Capitals with a little tracking, in the manner of a section label rather
/// than a title: it has to be legible enough to group what is under it and
/// quiet enough that the rows stay the thing being read. The tracking is a
/// Pango attribute rather than CSS `letter-spacing`, which GTK's stylesheet
/// parser accepts and does not apply.
/// What a group heading says under itself, for the groups that say anything.
///
/// Only Kodi's do. What belongs here is what is true of a whole installation
/// rather than of one setting under it - which file it is, and either why it
/// cannot be used or the thing every one of them shares: Kodi reads that file
/// once, at startup.
struct GroupNote {
    sentence: String,
    /// A folder the note offers to open. Offered rather than printed: a path
    /// read off a television is a path nobody is going to type.
    folder: Option<std::path::PathBuf>,
}

/// A group heading, and the line under it for the groups that have one.
///
/// The note is a `GtkLabel` styled like a row's own note and indented to the
/// same `pad_h` the heading is, so the three line up in one column.
fn group_header(title: &str, note: Option<&GroupNote>, scale: f64, first: bool) -> gtk::Widget {
    let heading = group_heading(title, scale, first);
    let Some(note) = note else {
        return heading.upcast();
    };

    let stack = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    stack.append(&heading);

    let text = row_note(&note.sentence, scale);
    if let Some(folder) = note.folder.clone() {
        // On the same line as the sentence it belongs to, the way Clear Saved
        // Playback Data offers its own folder.
        text.set_markup(&format!(
            "{}  <a href=\"{}\">Open File Location</a>",
            glib::markup_escape_text(&note.sentence),
            glib::markup_escape_text(&gtk::gio::File::for_path(&folder).uri()),
        ));
        // Reported rather than swallowed: a link that does nothing looks like
        // a link that was pressed wrongly.
        text.connect_activate_link(move |_, _| {
            show_folder(&folder);
            glib::Propagation::Stop
        });
    }
    stack.append(&text);
    stack.upcast()
}

fn group_heading(title: &str, scale: f64, first: bool) -> gtk::Label {
    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("tp-group");
    // Nothing above the first heading. It opens the list rather than dividing
    // it, and the buttons already sit above with room of their own.
    if first {
        heading.add_css_class("tp-group-first");
    }
    let attributes = gtk::pango::AttrList::new();
    attributes.insert(gtk::pango::AttrInt::new_letter_spacing(
        (1.5 * scale * gtk::pango::SCALE as f64) as i32,
    ));
    heading.set_attributes(Some(&attributes));
    heading
}

/// A heading's capitals turned back into words, for a screen reader.
///
/// "FIRST OUTPUT" read literally is a risk of being spelled out a letter at a
/// time, which is a real behaviour of several readers on all-capital text.
fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn menu_row(label: &str, value: &str, enabled: bool) -> gtk::Box {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(24)
        .build();
    row.add_css_class("tp-row");

    let name = gtk::Label::new(Some(label));
    name.set_xalign(0.0);
    row.append(&name);

    let value_label = gtk::Label::new(Some(value));
    value_label.add_css_class("tp-value");
    value_label.set_hexpand(true);
    value_label.set_xalign(1.0);
    value_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&value_label);

    let chevron = gtk::Label::new(Some("›"));
    chevron.add_css_class("tp-chevron");
    row.append(&chevron);

    row.set_sensitive(enabled);
    row
}

/// What a browsing screen is for: opening a video, or choosing a folder.
///
/// The two screens differ in what they list, what the footer holds and what a
/// row does. Everything else - the trail, the places column, the system
/// browser - is the same, and used to be written twice.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Browse {
    Videos,
    /// A separate soundtrack for the video already chosen: the same screen,
    /// listing audio files instead.
    Audio,
    /// A subtitle file from somewhere other than beside the video.
    Subtitles,
    Folders,
}

impl Browse {
    /// Whether only folders are worth showing. A folder is being chosen here,
    /// so the files inside it would be a list of things that cannot be picked.
    fn folders_only(self) -> bool {
        self == Browse::Folders
    }

    fn wants(self) -> crate::browser::Kind {
        match self {
            Browse::Audio => crate::browser::Kind::Audio,
            Browse::Subtitles => crate::browser::Kind::Subtitle,
            _ => crate::browser::Kind::Video,
        }
    }
}

/// The parts of a browsing screen its caller still has to finish.
struct BrowserPage {
    page: gtk::Box,
    list: gtk::ListBox,
    crumbs: Vec<gtk::Button>,
    browse: gtk::Button,
    open: gtk::Button,
    cancel: gtk::Button,
}

/// One row of a listing: what it says, what it is drawn with, where it goes,
/// and how it reads aloud. A path of `None` is the way up.
#[derive(Clone)]
struct BrowserEntry {
    /// Whether the Open button acts on this row: a file, rather than a folder,
    /// the way up, or a notice.
    openable: bool,
    label: String,
    icon: RowIcon,
    path: Option<std::path::PathBuf>,
    spoken: String,
    /// Something to read rather than somewhere to go: the line saying a
    /// folder holds nothing worth listing.
    notice: bool,
}

/// What sits behind a modal opened before there is a screen to sit behind it.
///
/// Blank on purpose. The alternative - building a menu page to stand in for
/// the real one - draws a screen nobody navigated to, which is worse than an
/// empty background because it looks like somewhere you could go back to.
fn empty_backdrop() -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, 0)
}

/// Fills a listing, and leaves the notice as a line of text.
///
/// A notice drawn like an entry invites being chosen, and choosing it walked
/// back up a level - which reads as a broken listing rather than as an empty
/// folder. Centred, dimmer, without an icon, and passed over by the cursor.
fn fill_browser_list(list: &gtk::ListBox, entries: &[BrowserEntry], scale: f64) {
    for entry in entries {
        if entry.notice {
            let label = gtk::Label::new(Some(&entry.label));
            label.add_css_class("tp-row");
            label.add_css_class("tp-hint");
            label.set_xalign(0.5);
            append_named(list, &label, &entry.spoken);
            if let Some(row) = label.parent().and_downcast::<gtk::ListBoxRow>() {
                row.set_selectable(false);
                row.set_activatable(false);
            }
        } else {
            append_named(
                list,
                &browser_row(entry.icon, &entry.label, scale),
                &entry.spoken,
            );
        }
    }
}

/// Opens a system dialog where the built-in browser already is.
///
/// Best effort: a folder that has since been unplugged or removed leaves the
/// dialog wherever it would have opened anyway, which is better than refusing
/// to open at all.
fn open_at(chooser: &gtk::FileChooserNative, start: &std::path::Path) {
    if start.is_dir() {
        let _ = chooser.set_current_folder(Some(&gtk::gio::File::for_path(start)));
    }
}

/// What a folder shows in a given mode: the way up, then what is inside.
fn browser_entries(directory: &std::path::Path, mode: Browse) -> Vec<BrowserEntry> {
    let mut entries = Vec::new();
    if let Some(parent) = directory.parent() {
        // Two dots rather than the word: it is what a file listing has always
        // called the folder above, and it needs no translating. Read aloud it
        // is punctuation and says nothing, so the spoken name says where it
        // goes instead.
        entries.push(BrowserEntry {
            openable: false,
            label: "..".to_string(),
            icon: RowIcon::Folder,
            path: None,
            spoken: match parent.file_name() {
                Some(name) => format!("Up to {}", name.to_string_lossy()),
                None => "Up to the list of drives".to_string(),
            },
            notice: false,
        });
    }
    for entry in crate::browser::read(directory, mode.wants()) {
        if mode.folders_only() && !entry.is_dir {
            continue;
        }
        // Which mark a file gets follows what this screen is for: the same
        // file is a video when a video is being chosen and a soundtrack when
        // one is. Nothing here inspects the file itself, which would mean
        // opening every one in the folder to draw a list.
        let icon = match (entry.is_dir, mode) {
            (true, _) => RowIcon::Folder,
            (false, Browse::Audio) => RowIcon::Audio,
            (false, Browse::Subtitles) => RowIcon::Subtitle,
            (false, _) => RowIcon::Video,
        };
        entries.push(BrowserEntry {
            openable: !entry.is_dir,
            label: entry.label.clone(),
            icon,
            path: Some(entry.path),
            spoken: entry.label,
            notice: false,
        });
    }
    // Only where the listing is what you came for. A folder with nothing to
    // play in it is worth saying, since the alternative reads as a folder
    // that failed to load; a folder with no folders under it is not empty at
    // all - it is full of files this screen has no reason to show, and
    // calling it empty would be wrong.
    //
    // Counting the way up as nothing, since it fills the list on its own and
    // is why this never appeared before.
    if mode == Browse::Videos && entries.iter().all(|entry| entry.path.is_none()) {
        entries.push(BrowserEntry {
            openable: false,
            label: "Nothing here".to_string(),
            icon: RowIcon::None,
            path: None,
            spoken: "Nothing here".to_string(),
            notice: true,
        });
    }
    entries
}

/// What a browser row draws beside its name.
///
/// The file marks are bundled rather than named from the desktop's icon set.
/// The set is what a row used to ask for, and the theme decides what turns up:
/// a generic video icon is absent from the Pi's theme entirely and fell back
/// to the missing-image glyph, which reads as a warning about the file. These
/// three are the same on every machine.
///
/// The folder is bundled with them. It could have stayed the theme's - a
/// folder is the one icon every theme has - but then one mark in a column of
/// four would be drawn in somebody else's hand, and which one would depend on
/// the machine.
#[derive(Clone, Copy, PartialEq)]
enum RowIcon {
    Folder,
    Video,
    Audio,
    Subtitle,
    /// A notice rather than a file - "Nothing here" - which draws no mark.
    None,
}

impl RowIcon {
    /// The mark at the size a listing draws it.
    fn image(self, scale: f64) -> gtk::Image {
        let size = match self {
            Self::Folder => FOLDER_MARK_PX,
            _ => ROW_MARK_PX,
        };
        self.image_at(size, scale)
    }

    /// The mark at a size of the caller's choosing, for the places that are
    /// not a row in a listing.
    fn image_at(self, size: f64, scale: f64) -> gtk::Image {
        const VIDEO: &[u8] = include_bytes!("../data/ui/file-video.png");
        const AUDIO: &[u8] = include_bytes!("../data/ui/file-audio.png");
        const SUBTITLE: &[u8] = include_bytes!("../data/ui/file-subtitle.png");
        const FOLDER: &[u8] = include_bytes!("../data/ui/folder.png");

        let bytes = match self {
            Self::Video => VIDEO,
            Self::Audio => AUDIO,
            Self::Subtitle => SUBTITLE,
            Self::Folder => FOLDER,
            Self::None => return gtk::Image::new(),
        };
        marked_image(bytes, size * scale)
    }
}

/// A browser row: a mark, then the name.
///
/// Icons rather than emoji, because emoji depend on a color font being
/// installed. The Pi has none, so a folder character rendered as an empty box
/// with the codepoint inside it.
fn browser_row(icon: RowIcon, text: &str, scale: f64) -> gtk::Box {
    // The padding goes on the row rather than the label, so it applies
    // before the icon as well as around the text.
    //
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["tp-row"])
        .build();

    let image = icon.image(scale);
    image.add_css_class("tp-row-icon");
    // A column of its own, the same width whatever is drawn in it, so every
    // name in the list starts at the same place. The marks are cropped to
    // their ink and no two are the same shape - a page is tall and narrow, a
    // folder wide - so left to size themselves the folder rows and the file
    // rows put their names a couple of pixels apart, which is the sort of
    // thing that reads as sloppiness without being obvious enough to name.
    image.set_size_request((MARK_COLUMN_PX * scale).round() as i32, -1);
    image.set_halign(gtk::Align::Center);
    row.append(&image);

    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

/// Lines a selector's right edge up with the right edge of the row that opened
/// it, leaving the vertical placement to GTK.
///
/// GTK positions a popover by centering it on a rectangle you nominate, in the
/// parent's coordinates. Here that is a one-pixel sliver at
/// `row_width - popover_width / 2`, so centering the popover on it lands the
/// two right edges together. The entries inside are right-aligned because they
/// are alternatives to the value on the right of the row, and a centered
/// popover would sit just left of it - close enough to read as a mistake
/// rather than a margin.
///
/// The rectangle spans the row's full height, which is GTK's own default and
/// gives its ordinary vertical behaviour: below the row when there is room,
/// flipped above it when there is not.
///
/// Aligning an edge of the popover to an edge of the row was tried and taken
/// out again. It is possible - a zero-height rectangle at `y` puts the
/// popover's near edge on that line - but it requires predicting which way GTK
/// will open, and the popover then covers the row it belongs to, which leaves
/// a choice sitting under the pointer where the row used to be. Clicking again
/// to dismiss picks that choice instead. macOS avoids this by aligning the
/// *selected* entry to the row rather than the first one, so a second click
/// picks what was already set; without that, overlapping is worse than not.
///
/// **The width cannot come from measuring the popover.** A popover is a
/// `GtkNative`: it takes no room in the widget that parents it, so measuring
/// it as a child answers zero however wide it will actually open. That zero is
/// what left an earlier attempt at this centered. The number has to come from
/// what is inside it, plus the padding the stylesheet puts around that.
fn aim_right(popover: &gtk::Popover, anchor: &gtk::ListBoxRow, width: i32) {
    if width <= 0 || anchor.width() <= 0 {
        return;
    }
    let center = anchor.width() - width / 2;
    popover.set_pointing_to(Some(&gdk::Rectangle::new(center, 0, 1, anchor.height())));
}

fn chooser_row(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("tp-row");
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label
}

/// GTK rings the system bell when a keyboard move can't go anywhere - at
/// the ends of a list, which happens constantly when navigating by
/// arrow key or D-pad. The application provides its own click instead.
fn suppress_error_bell() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_error_bell(false);
        // Clicking the timeline jumps to that point. Left to the platform
        // default this differs by system: on macOS a click on the trough
        // steps toward the pointer instead of going there, which reads as a
        // seek that ignored where you clicked.
        settings.set_gtk_primary_button_warps_slider(true);
        // Holding the timeline still for a moment otherwise puts GtkRange into
        // its fine-adjustment mode: the trough grows and the slider starts
        // moving a fraction of the distance the pointer does. That is a useful
        // affordance for choosing an exact value in a settings dialog, and a
        // baffling one on a video timeline, where it looks like the playhead
        // has come unstuck from the mouse. Nothing here wants a long press, so
        // the threshold is put beyond reach rather than the behavior fought.
        // An hour, not u32::MAX: the property has a range and refuses
        // anything past it, which panics on the way up.
        settings.set_gtk_long_press_time(60 * 60 * 1000);
    }
}

/// Sizes are set here rather than left to the theme because the interface
/// is meant to be read from across a room. Everything scales from one
/// factor so it can be dialled down for close-range use.
/// Starting window size, in the same units as the interface inside it.
///
/// A fixed size would mean a 2x menu opening into a 1x frame, which is how a
/// 4K display ends up with a window too small for its own contents. Capped to
/// most of the monitor so a large scale on a modest screen still opens
/// something that fits, panels and decoration included.
fn default_window_size(
    scale: f64,
    monitor: Option<&gdk::Monitor>,
    saved: (Option<i32>, Option<i32>),
) -> (i32, i32) {
    // Sixteen by nine, and a good deal larger than it was. The old size was
    // 1100x700 - close to 11:7, and so a shape no film is - which left the
    // media page holding a column of empty air down the sides of its artwork
    // before anybody had touched a window edge.
    const BASE_WIDTH: f64 = 1600.0;
    const BASE_HEIGHT: f64 = 900.0;
    const MAX_FRACTION: f64 = 0.9;

    // Where it was left, if it was left anywhere. Held to the same fraction of
    // the screen as the default below: a size remembered from a larger monitor
    // would otherwise open off the edge of a smaller one.
    let (mut width, mut height) = match saved {
        (Some(width), Some(height)) if width > 0 && height > 0 => (width as f64, height as f64),
        _ => (BASE_WIDTH * scale, BASE_HEIGHT * scale),
    };
    if let Some(monitor) = monitor {
        let geometry = monitor.geometry();
        width = width.min(geometry.width() as f64 * MAX_FRACTION);
        height = height.min(geometry.height() as f64 * MAX_FRACTION);
    }
    (width.round() as i32, height.round() as i32)
}

/// Registers the provider the interface's sizes are loaded into. Kept so the
/// sizes can be replaced later without stacking up providers, which is what
/// makes re-scaling on a different monitor possible.
fn install_styles() -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    provider
}

fn style_css(scale: f64) -> String {
    let px = |base: f64| (base * scale).round() as i32;

    format!(
        "
        /* The font TinePlayer ships, so its own text is the same on every
           platform rather than three different system faces.

           Naming it matters for more than looks. Without this the interface
           asks for the platform's default font and ours is only ever reached
           as a fallback, one character at a time - which is what left Cyrillic
           on macOS with gaps between the letters, each one resolved separately
           from whatever happened to cover it. Named, the whole line comes from
           one face with its own metrics.

           Every script face is named too, and that is not belt and braces.
           Listed only as \"TinePlayer Sans\", the others are reachable solely
           as fallback, and fallback prefers whatever the machine already has:
           Arabic and Armenian came out as three different system faces on
           three platforms, while Telugu and Bengali were consistent purely
           because nobody else had them. Naming them puts ours first.

           The generic sans-serif at the end is what draws anything ours does
           not carry: file names, device names and track titles, in scripts
           nobody can predict. The list is written out rather than generated,
           so it has to be updated when a script is added - which
           packaging/fonts/build-fonts.py will refuse to build without. */
        window, .tp-menu, .tp-controls {{
            font-family:
                \"TinePlayer Sans\",
                \"TinePlayer Sans Arabic\", \"TinePlayer Sans Armenian\",
                \"TinePlayer Sans Bengali\", \"TinePlayer Sans Cjk\",
                \"TinePlayer Sans Devanagari\", \"TinePlayer Sans Georgian\",
                \"TinePlayer Sans Gurmukhi\", \"TinePlayer Sans Hangul\",
                \"TinePlayer Sans Hebrew\", \"TinePlayer Sans Malayalam\",
                \"TinePlayer Sans Symbols\",
                \"TinePlayer Sans Tamil\", \"TinePlayer Sans Telugu\",
                \"TinePlayer Sans Thai\",
                sans-serif;
        }}
        .tp-title {{
            font-size: {title}px;
            font-weight: bold;
            opacity: 0.75;
            letter-spacing: {tracking}px;
        }}
        .tp-row {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        .tp-value {{ opacity: 0.7; }}
        /* A line under a row saying what it does. Smaller and dimmer than the
           setting it explains, so a column of them reads as annotation rather
           than as more rows. */
        .tp-row-note {{ font-size: {note}px; opacity: 0.55; }}
        /* Every link in the application, in the interface's own ink rather
           than a theme's blue. There is one palette here and blue is the
           accent that marks where the cursor is - a link wearing it reads as a
           selection, and a purple visited one reads as nothing else in the
           interface has ever used.

           Named twice because GTK has said it both ways: each markup link gets
           its own node called `link`, and the widget also carries the `:link`
           state. Not `a`, which is HTML - the parser accepts it, matches
           nothing, and leaves the theme colour exactly where it was.

           Slightly held back at rest so it does not shout over the sentence it
           sits in, and full strength under the pointer. */
        link, *:link, *:visited {{ color: rgba(255, 255, 255, 0.78); }}
        link:hover, *:link:hover, *:visited:hover {{ color: #ffffff; }}
        /* Except inside a note, which is dimmed as a whole: a link held back
           again there would end up quieter than the text around it, which is
           backwards for the one part of the line that can be pressed. */
        .tp-row-note link,
        .tp-row-note *:link {{ color: #ffffff; }}
        /* Sized with the rest of the interface: the theme's default switch is
           drawn for a mouse at a desk, and is a smudge from a sofa.

           Monochrome rather than the theme's accent, which is the same blue as
           the row highlight and disappeared into it. On is read from the fill
           being solid, not from its hue, so it competes with nothing. Off the
           full foreground colour in both, which was the loudest thing on a
           screen of settings most people set once, but not by the same
           amount: the dark theme needs the fill near white to read as on at
           all, where the light one wants a good deal less than black.
           Literal
           colors picked from the theme here rather than `@theme_fg_color`, for
           the reason the cancel button gives. */
        .tp-row switch {{
            min-width: {switch_w}px;
            min-height: {switch_h}px;
            border-radius: {switch_h}px;
            background-color: {trough};
            background-image: none;
            border-color: transparent;
        }}
        .tp-row switch > slider {{
            min-width: {slider}px;
            min-height: {slider}px;
            border-radius: {switch_h}px;
            /* The same knob as a slider carries, for the same reason. Only
               while the switch is off: checked, the knob sits on the lit fill
               and needs to be the dark one below. */
            background-color: {knob};
        }}
        .tp-row switch:checked {{
            background-color: {fill};
            border-color: {fill};
        }}
        /* A switch that cannot be worked, saying so. The colours above are
           stated outright rather than taken from the theme, so the theme's own
           insensitive styling has nothing to dim - which left a disabled row
           with a lit switch on it, reading as a control that simply refused
           the press. Faded whole, so trough, fill and knob go together. */
        .tp-row switch:disabled {{ opacity: 0.35; }}
        .tp-chevron {{ font-size: {row}px; opacity: 0.5; }}
        .tp-hint {{ font-size: {hint}px; opacity: 0.7; }}
        /* The one screen made of paragraphs. Looser than a row of settings,
           since it is read rather than scanned. */
        .tp-about {{ font-size: {hint}px; opacity: 0.8; }}
        .tp-about-heading {{
            font-size: {row}px;
            font-weight: bold;
            margin-top: {pad_v}px;
        }}
        /* The name at the top of About, which opens the page and so has
           nothing above it to be spaced from. The margin every other heading
           carries is inside the label's own box, so centering it against the
           mark beside it centered the margin too and left the text sitting
           low by exactly that much. */
        .tp-about-title {{ margin-top: 0; }}
        /* Every button in the interface: one size, one padding, one corner.
           The corner matches a menu row's, so a button and the rows it sits
           over read as parts of one page. */
        .tp-button {{
            font-size: {row}px;
            padding: {pad_v}px {pad_h}px;
            border-radius: {radius}px;
        }}
        /* The one action every screen is pointing at.

           Deliberately *not* the blue the selected row is drawn in, which is
           what it was first. Two things were wrong with that. A blue button
           sitting directly above a blue selected row read as two halves of
           one control rather than as an action and a choice; and when the
           button took focus it had nothing left to say so with, because it
           was already wearing the color that means this one is selected. A
           different hue gives the focus ring something to be seen against,
           and lets blue go on meaning one thing throughout.

           Blue, and the clash is resolved from the other side: what changed
           is the *focus* color, which is now a neutral rather than the same
           accent. Blue means an action throughout, and white means where you
           currently are. Literal, and with the
           gradient cleared: the theme paints
           buttons with one, and a flat color underneath it comes out as a tint
           of whatever the theme wanted rather than as this color. */
        .tp-action {{
            font-weight: bold;
            background-image: none;
            background-color: {play_fill};
            color: {play_ink};
            border-color: transparent;
        }}
        .tp-action:hover {{ background-color: {play_hover}; }}
        /* Focus said loudly, because this is read from a distance and these
           buttons no longer sit in a list whose highlight does the saying. A
           ring around the button rather than a change of fill: the fill is
           what the button *is*, and swapping one green for another is a
           difference nobody can see across a room. */
        /* Drawn as a shadow rather than an outline. `outline` is what a focus
           ring is normally, and GTK parsed it here without complaint and drew
           nothing - so it is not something to spend an afternoon on when a
           spread shadow does the same job and demonstrably works. */

        /* The corner marks had no focus state at all, which on a screen meant
           to be driven by a gamepad from a sofa means arrowing onto one and
           having nothing tell you. Same ring, drawn round the icon. */
        .tp-gear:focus {{
            background-color: rgba(128, 128, 128, 0.22);
            box-shadow: 0 0 0 {focus_ring}px {focus};
        }}
        /* Nothing to press it about: a disabled action keeps its shape and
           loses its insistence, rather than staying the loudest thing on a
           page it cannot act on. */
        .tp-action:disabled {{
            background-color: {trough};
            color: inherit;
            opacity: 0.5;
        }}
        /* Restart, once Resume has taken the words. Square rather than merely
           narrow, so it reads as the mark's button rather than as a button
           whose label went missing. */
        .tp-action-icon {{ padding: {pad_v}px {pad_v}px; min-width: {play_icon}px; }}
        /* Half again the height, on the media page's pair alone. They are the
           one thing the page is for, and on a television they are pressed from
           across a room - so they are worth more than the height a line of
           text happens to need. Declared after `.tp-action-icon` so the
           restart button takes this padding rather than that one. */
        .tp-tall {{ padding-top: {tall_v}px; padding-bottom: {tall_v}px; }}
        /* The media page.

           The ground the whole page is drawn on, and - through `color` - the
           ground the backdrop screen-blends against. See src/backdrop.rs for
           why the background arrives as a foreground property: a widget
           cannot read its own CSS background from inside `snapshot`, and
           every other color in this application is declared here rather than
           in Rust. Literal, for the reason the highlight is literal. */
        .tp-backdrop {{ color: {page_bg}; }}
        /* The rows sit on the artwork, so everything between them and it has
           to get out of the way. A GtkListBox and a GtkScrolledWindow both
           paint the theme's view background by default, which came out as an
           opaque slab over the backdrop in the shape of the list. */
        /* Transparent only where something else is already drawing the ground:
           a panel, a selector's own box, or the media page with the film's
           backdrop behind it. Everywhere else a list keeps the theme's own
           background, which is what sets it apart from the page around it.

           Written unscoped to begin with, and that took the ground out from
           under every list in the application - the browser's two columns
           merged into the page behind them, and there was no longer anything
           to say where one ended.

           `.tp-menu-panel` earns its place here for the opposite reason: a
           list inside a panel that painted its own background drew a grey box
           within the black one, which reads as two panels where there is one.
           `.tp-bare` is for a list standing on no ground at all. */
        .tp-menu-panel .tp-menu, .tp-menu-panel .tp-menu > row,
        .tp-bare .tp-menu, .tp-bare .tp-menu > row,
        .tp-media .tp-menu, .tp-media .tp-menu > row,
        .tp-selector .tp-menu, .tp-selector .tp-menu > row {{
            background-color: transparent;
        }}
        .tp-menu-panel scrolledwindow, .tp-menu-panel viewport,
        .tp-bare scrolledwindow, .tp-bare viewport,
        .tp-media scrolledwindow, .tp-media viewport,
        .tp-selector scrolledwindow, .tp-selector viewport {{
            background-color: transparent;
        }}
        /* The two marks in the corner are affordances rather than actions:
           no fill and no border until the pointer is on them, so they carry
           no weight beside the button the page is actually pointing at. */
        .tp-gear {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
        }}
        /* No color, and none needed. The gear was a symbolic icon, which GTK
           recolors from the foreground - including dimming it when the window
           loses focus, while the drawn mark beside it did not, so the pair came
           apart every time the window went to the back. It is a drawn mark
           itself now, in the same ink, so the two behave alike without being
           told to. */
        .tp-gear:hover {{ background-color: rgba(128, 128, 128, 0.22); }}
        /* No focus ring on the playback controls. Every button there already
           says where the cursor is by filling with the accent - see
           `.tp-selected` - so the shared ring was a second mark for the same
           fact, drawn inside the fill and reading as an outline around it. The
           menus keep theirs, where there is no fill to say it instead. */
        .tp-transport-button:focus {{ box-shadow: none; }}
        /* The frame the poster sits in, which is also what is seen when there
           is no poster. A flat panel a shade off the page rather than an
           outline: at a distance a thin border on a dark ground disappears,
           and the shape is what says a picture belongs here. */
        .tp-poster {{
            background-color: {panel};
            border-radius: {radius}px;
        }}
        /* The film's name. The largest thing on the page by a good margin,
           because from across a room it is the one thing being checked. */
        .tp-film-title {{
            font-size: {film_title}px;
            font-weight: bold;
        }}
        .tp-film-facts {{ font-size: {film_facts}px; opacity: 0.65; }}
        .tp-film-plot {{ font-size: {film_plot}px; opacity: 0.92; }}
        /* The label-and-reading lines: under the poster, and the languages
           above the rows. The label's own dimming is set per-span in the
           markup, so this carries only the size. */
        .tp-fact {{ font-size: {fact}px; }}
        /* The name of a reading rather than the reading itself, dimmed so a
           column of these scans as values with labels rather than as a block
           of text of one weight. */
        .tp-fact-name {{ opacity: 0.6; }}
        .tp-empty-prompt {{ font-size: {row}px; opacity: 0.7; }}
        /* The line that stands where the Connect button stands when there is
           nothing to connect. Sized like the buttons it sits under rather than
           like small print, because it answers the same question they ask -
           and at full strength, so the green mark beside it stays green
           instead of being dimmed along with the words. */
        .tp-connected {{ font-size: {row}px; }}
        /* Backing out, on every screen that offers it. A literal red for the
           same reason the highlight is literal: a theme name that does not
           exist makes the whole declaration fail to parse. */
        .tp-danger {{
            background-image: none;
            background-color: #c01c28;
            color: #ffffff;
        }}
        .tp-danger:hover {{ background-color: #a51d2d; }}
        /* Beside a main action rather than being one: smaller type and far
           less padding than the buttons it sits with, so it reads as a way to
           reach something else rather than as the thing to press. */
        .tp-secondary {{ font-size: {small}px; padding: {tight_v}px {tight_h}px; }}
        .tp-menu > row {{ border-radius: {radius}px; }}
        /* The ground the rows sit on. Black at a fraction rather than a
           lighter grey: it has to read as a panel over whatever backdrop the
           film brought, and a tint that darkens works over every one of them
           where a fixed colour only works over some. */
        .tp-menu-panel {{
            background-color: rgba(0, 0, 0, 0.2);
            border-radius: {panel_radius}px;
            padding: {panel_pad}px;
        }}
        /* No ground of its own, but the same inset as the panel beside it.
           Without it the categories sat a few pixels above and to the left of
           the settings they name, which reads as two lists that were laid out
           separately rather than as one screen. */
        .tp-bare {{ padding: {panel_pad}px; }}
        /* Gray rather than a theme color, so it lifts off the background in
           both light and dark without needing two rules. */
        .tp-menu > row:hover {{ background-color: rgba(128, 128, 128, 0.18); }}
        .tp-menu:focus-within > row:selected:hover {{ background-color: {focus_row}; }}
        .tp-menu > row.tp-section-start {{ margin-top: {section}px; }}
        /* A group heading. Quiet on purpose: smaller than a row and dimmed,
           so it labels what is under it without competing with it. Indented to
           `pad_h` so it starts exactly where the row labels below it do. */
        .tp-group {{
            font-size: {group}px;
            font-weight: bold;
            opacity: 0.55;
            margin: {group_top}px {pad_h}px {group_gap}px {pad_h}px;
        }}
        .tp-group-first {{ margin-top: {group_first_top}px; }}
        /* A selector opened over the page. `contents` is the node GTK puts
           inside a popover; styling the popover itself leaves the theme's own
           background drawn underneath. */
        /* Smaller than a row on the page behind it. A selector is a list of
           variations on one value rather than a set of destinations, and at
           the page's own size it reads as a second menu that has landed on
           top of the first. */
        .tp-selector .tp-row {{
            font-size: {selector_row}px;
            padding: {selector_row_pad_v}px {selector_row_pad_h}px;
        }}
        .tp-selector separator {{
            margin: {rule_gap}px 0;
            background-color: rgba(255, 255, 255, 0.14);
        }}
        .tp-selector > contents {{
            background-color: {selector_bg};
            border-radius: {panel_radius}px;
            padding: {panel_pad}px;
            box-shadow: 0 {shadow_drop}px {shadow_blur}px rgba(0, 0, 0, 0.55);
        }}
        /* Which row is in force, as opposed to which row the cursor is on.
           Two different facts that a list has only one highlight for, and
           conflating them is actively misleading in the places column: moving
           the cursor there would appear to change the folder being shown.

           A bar down the leading edge rather than a fill, so it reads as
           'you are here' beside the focus rather than competing with it.
           Drawn with an inset shadow rather than a border so that marking a
           row does not shift its text. */
        .tp-menu > row.tp-current {{
            box-shadow: inset {mark}px 0 0 0 {highlight};
        }}
        /* Belongs to the row above it: indented so the group reads as one
           thing without every label having to name the output again. */
        .tp-menu > row.tp-subrow {{ margin-left: {subrow}px; }}
        /* A selection is only shown while the list it belongs to holds the
           focus. A list keeps its selected row either way, so that returning
           to it lands where you left - but showing that on a list you are
           not on reads as a second cursor, and with two lists side by side
           it is genuinely unclear which one an arrow key would move.

           The cost is that stepping down to the buttons leaves the list with
           nothing marked. That is the right trade: the buttons show their own
           focus, so there is still exactly one thing highlighted on screen. */
        .tp-menu:focus-within > row:selected {{
            background-image: none;
            background-color: {focus_row};
            color: {on_focus};
        }}
        .tp-menu:focus-within > row:selected .tp-value,
        .tp-menu:focus-within > row:selected .tp-chevron {{
            color: {on_focus};
            opacity: 0.85;
        }}
        /* A ring rather than a fill. Recoloring a focused button changes what
           it looks like it does - a Cancel that turns blue reads as the one
           to press - and beside another button the pair stop looking like
           peers. An inset shadow rather than a border so nothing shifts, and
           rather than an outline so it follows the rounded corners. */
        button:focus {{
            box-shadow: 0 0 0 {focus_ring}px {focus};
        }}
        /* Chrome-less until pointed at, but the arrow itself stays visible
           so the way back is always apparent. */
        /* One fixed footprint for whatever leads the header, the back arrow
           or the application mark. Without it the two screens allocate
           different widths and everything after them moves. */
        .tp-leading {{
            min-width: {leading}px;
            min-height: {leading}px;
            padding: 0px;
        }}
        /* Fixed too, so a header of buttons is no taller than one holding a
           plain label and the list below starts in the same place. */
        .tp-header {{ min-height: {leading}px; }}
        .tp-back {{
            padding: 0px;
            min-width: 0px;
            min-height: 0px;
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            opacity: 0.6;
        }}
        .tp-back:hover {{
            background-color: rgba(128, 128, 128, 0.25);
            opacity: 1;
        }}
        .tp-back:focus {{ opacity: 1; }}
        /* Laid over the picture, so it sets its own colors rather than
           inheriting theme ones that may be light. */
        .tp-controls {{
            background-color: rgba(0, 0, 0, 0.75);
            padding: {pad_v}px {pad_h}px;
        }}
        /* The buttons sit under the timeline rather than beside it, so the
           row a controller is moving along is unambiguous. */
        .tp-buttons {{ padding: 0px; }}
        /* Tabular figures, so the digits are all one width. A proportional
           1 is narrower than a 0, which makes a running clock twitch even
           when the number of characters does not change. */
        .tp-time {{
            font-size: {hint}px;
            color: #ffffff;
            font-feature-settings: \"tnum\" 1;
        }}
        .tp-transport {{ -gtk-icon-size: {icon}px; color: #ffffff; }}
        /* Play, drawn bigger than what sits around it. */
        .tp-transport-main {{ -gtk-icon-size: {icon_main}px; }}
        /* Flat over the picture: the strip already reads as a control bar,
           and button chrome on top of video looks like a mistake. */
        .tp-transport-button {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 0px {crumb_pad}px;
        }}
        .tp-transport-button:hover {{ background-color: rgba(255, 255, 255, 0.15); }}
        /* A control that is there but not doing anything: the sync button
           while an output's delay is switched off. Dimmed rather than hidden,
           since it is what turns the delay back on. */
        .tp-off {{ opacity: 0.35; }}
        /* Where a controller is, drawn boldly enough to be found from across a
           room rather than as the hairline a focus ring would give. */
        .tp-selected {{
            background-color: {highlight};
            border-radius: {radius}px;
        }}
        /* Darker than the strip it sits on, so it reads as a panel laid over
           the bar rather than as more of the bar. */
        .tp-volume-panel {{
            background-color: rgba(0, 0, 0, 0.75);
            border-radius: {radius}px;
            padding: {crumb_pad}px;
            margin-bottom: {crumb_pad}px;
            margin-right: {pad_h}px;
        }}
        /* Padded so the selection mark has room around a row rather than
           sitting tight against the words. */
        .tp-volume-panel > box {{
            padding: {crumb_pad}px;
            border-radius: {radius}px;
        }}
        /* Less underneath a block than above it. The panel already spaces the
           groups apart and pads its own bottom edge, so a group's own padding
           was a third helping and left each set of bars floating well clear of
           the divider under it. */
        /* Written against the rule above rather than beside it. A bare
           `.tp-menu-group` is one class where that is a class and a type, so it
           loses on specificity and the padding never changes - which looks
           exactly like a rule that was never added. */
        .tp-volume-panel > box.tp-menu-group {{ padding-bottom: 0px; }}
        /* The last group is the panel's bottom edge rather than a join between
           two blocks, so it keeps what the others give up: the space under the
           final bar answers the space above the first heading, where the space
           between groups only has to separate them.

           An explicit class rather than `:last-child`, because a selector GTK
           will not parse is discarded whole and says so only in the log - and
           the failure looks identical to a rule nobody added. Later in the
           sheet than the rule above, which is what settles the tie between two
           selectors of the same weight. */
        .tp-volume-panel > box.tp-menu-foot {{ padding-bottom: {crumb_pad}px; }}
        .tp-volume-panel label {{ color: #ffffff; }}
        /* The same size as the transport icons, which are drawn to be read
           from a sofa rather than a desk. A button built from an icon name
           has no image to class, so the size is set on the descendant. */
        .tp-volume-panel button image {{
            -gtk-icon-size: {icon}px;
            color: #ffffff;
        }}
        /* The 1 and 2 on the two output buttons, tucked into the lower
           trailing corner of the speaker. Drawn on its own dark disc: the
           strip sits over a moving picture, and a bare numeral lands on
           whatever the film happens to be showing.

           Sized off the icon rather than off the body text, so it stays a mark
           on an icon at every ui_scale instead of growing into a second
           glyph. */
        .tp-output-badge {{
            font-size: {badge}px;
            font-weight: bold;
            color: #ffffff;
            background-color: rgba(0, 0, 0, 0.8);
            border-radius: {badge}px;
            padding: 0 {badge_pad}px;
            margin: 0;
        }}
        /* Between the soundtracks and the device below them. Faint rather than
           a rule: it separates two kinds of thing inside one menu, and a hard
           line would read as two panels that had run together. */
        .tp-menu-divider {{
            background-color: rgba(255, 255, 255, 0.25);
            margin-top: {crumb_pad}px;
            margin-bottom: {crumb_pad}px;
        }}
        /* A list taller than the window scrolls rather than running off the
           top of it. The scrolled window itself carries no background: the
           panel behind it already has one, and a second would show as a
           lighter block wherever the list did not fill its box. */
        .tp-volume-panel scrolledwindow {{ background-color: transparent; }}
        /* Drawn over the list rather than beside it, so that a film with more
           soundtracks than fit does not come out a different width from one
           that fits - the panel is opened and closed constantly, and a width
           that moved with the content would read as the menu jumping. */
        .tp-volume-panel scrollbar,
        .tp-subtitle-panel scrollbar {{
            background-color: transparent;
            border: none;
        }}
        .tp-volume-panel scrollbar slider,
        .tp-subtitle-panel scrollbar slider {{
            background-color: rgba(255, 255, 255, 0.35);
            border-radius: {radius}px;
        }}
        /* The subtitle chooser, laid over the bar exactly as the volume panel
           is and out of the same corner: they are the two things the strip
           opens rather than does, and a list that arrived looking like
           something else would read as a different kind of thing. */
        .tp-subtitle-panel {{
            background-color: rgba(0, 0, 0, 0.75);
            border-radius: {radius}px;
            padding: {crumb_pad}px;
            margin-bottom: {crumb_pad}px;
            margin-right: {pad_h}px;
        }}
        /* Padded so the selection mark has room around a row rather than
           sitting tight against the words, the same as the panel beside it.

           At the interface's own row size rather than the size a stock label
           comes out at, which is drawn for a desk. This is a list of languages
           read from a sofa while a film runs, so it is sized like every other
           list in the application and not like the panel's device names, which
           are a caption above a control rather than the thing being chosen. */
        .tp-subtitle-row {{
            font-size: {row}px;
            padding: {crumb_pad}px;
            border-radius: {radius}px;
            color: #ffffff;
        }}
        /* The subtitle in force, marked apart from where the cursor is - the
           two part company as soon as anybody moves, which is the point of
           marking them separately. The same bar down the leading edge the
           menus draw, so 'you are here' and 'this is what is on' read the
           same way over a film as they do on a page. */
        .tp-subtitle-row.tp-current {{
            box-shadow: inset {mark}px 0 0 0 {highlight};
        }}
        /* The handle, not the whole bar: filling the trough drew over the
           very thing that says where playback is. */
        .tp-progress.tp-selected {{ background-color: transparent; }}
        .tp-progress.tp-selected slider {{
            background-color: {knob};
            outline: {outline}px solid {highlight};
            outline-offset: {outline}px;
            min-width: {handle}px;
            min-height: {handle}px;
        }}
        /* Faded while subtitles are off and solid while they are on, so the
           button reports the state as well as offering to change it. Opacity
           rather than color: the mark is an image, which a color cannot
           tint. */
        /* Darkens the menu the browser opens over, so the panel reads as
           being in front of it rather than beside it. */
        .tp-scrim {{ background-color: rgba(0, 0, 0, 0.55); }}
        /* Inset from the window edges, so the dimmed menu shows around all
           four sides and the panel looks like a window over it. Literal
           colors rather than theme names, for the reason given by the
           highlight color below. */
        .tp-modal {{
            background-color: #1e1e1e;
            border: 1px solid rgba(255, 255, 255, 0.14);
            border-radius: {radius}px;
            margin: {modal}px;
            padding: {modal_pad}px;
        }}
        /* Taller than a stock entry: this is the one thing on its panel, and
           it is read from the same distance as everything else. */
        .tp-path {{ font-size: {row}px; padding: {pad_v}px {pad_h}px; }}
        /* The Quick Connect code, which is copied off the screen a character
           at a time into a phone across the room. Sized like a film's title
           because that is the one other thing in the interface meant to be
           read from that far away, and spaced out so no two characters run
           together - a 6 beside a G at a glance is what turns this into two
           attempts. */
        .tp-code {{
            font-size: {film_title}px;
            font-weight: bold;
            letter-spacing: {code_tracking}px;
        }}
        /* A soundtrack icon whose output is silenced, faded the same way and
           for the same reason as the subtitle mark: the button reports the
           state as well as offering to change it. It is the only thing on
           screen that can, now that the levels have a menu of their own -
           holding one of these mutes that output, and without this the gesture
           worked and looked as though it had not. */
        .tp-soundtrack-muted {{ opacity: 0.45; }}
        .tp-subtitles-button {{ opacity: 0.45; }}
        .tp-subtitles-on {{ opacity: 1; }}
        .tp-subtitles-button:disabled {{ opacity: 0.2; }}
        .tp-progress {{ min-height: {bar}px; }}
        .tp-progress progress {{ background-color: {highlight}; }}
        /* The alignment panel's bar, thicker than the playback scrubber: it
           is the only thing moving on that screen, and it is read from across
           a room rather than aimed at with a pointer. Its own class, so the
           scrubber and the settings sliders keep the weight they were given.
           The height has to sit on both nodes - a GtkProgressBar draws the
           fill inside the trough, and raising only one leaves a thick bar with
           a thin fill rattling around in it. */
        .tp-align-bar, .tp-align-bar trough, .tp-align-bar progress {{
            min-height: {align_bar}px;
            border-radius: {align_bar_radius}px;
        }}
        /* Styled in full rather than by borrowing `tp-bar`, whose dim fill is
           meant for a slider with a handle on it to point at. There is no
           handle here, so the fill is the whole of what is being read and it
           takes the highlight colour. `background-image: none` first, or the
           theme's gradient sits over any colour set under it. */
        .tp-align-bar trough {{
            background-color: {trough};
            background-image: none;
        }}
        .tp-align-bar progress {{
            background-color: {highlight};
            background-image: none;
        }}
        /* Settings bars, drawn to be found rather than to be tasteful. The
           theme's own colours put a faint handle on a faint trough, which on
           a dark background is a bar that has to be looked for.

           Three steps apart, so the parts stay told from each other: the
           handle brightest, the part behind it dimmer, the rest dimmer again.
           Deliberately not the highlight colour, which is what a selected row
           is painted with - a blue bar on a blue row is the one case where
           the theme's choice vanishes completely. */
        .tp-bar trough {{ background-color: {trough}; background-image: none; }}
        .tp-bar trough > highlight, .tp-bar progress {{
            background-color: {fill};
            background-image: none;
        }}
        /* `background-image: none` first, or none of the colour below shows:
           the theme paints handles and troughs with a gradient image, which
           sits over any background colour set under it. The same trap the
           transport buttons work around. */
        .tp-bar slider, .tp-row switch > slider {{
            background-image: none;
            background-color: {knob};
            box-shadow: none;
            /* A ring against the knob's own brightness, so one knob colour
               reads both on the dim trough and on the lit fill it travels
               onto - sliders and switches alike. */
            border: {edge}px solid {knob_edge};
        }}
        .tp-bar slider {{
            min-width: {handle}px;
            min-height: {handle}px;
        }}
        /* An output that is silenced or a delay not being applied: the row
           still says what it is set to, quietly. */
        .tp-bar:disabled trough > highlight,
        .tp-bar:disabled progress {{ background-color: {trough}; }}
        /* Reads as a path rather than a row of buttons, until one takes
           focus and the shared button:focus rule highlights it. */
        .tp-crumb {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 2px {crumb_pad}px;
            font-size: {title}px;
            font-weight: bold;
            opacity: 0.75;
        }}
        .tp-crumb:focus {{ opacity: 1; }}
        .tp-crumb-separator {{ font-size: {title}px; opacity: 0.4; }}
        /* Kept small enough that the header stays the height every other
           screen's header is. */
        .tp-browse {{
            background-image: none;
            background-color: transparent;
            border-color: transparent;
            box-shadow: none;
            min-height: 0px;
            min-width: 0px;
            padding: 2px {crumb_pad}px;
            opacity: 0.6;
        }}
        .tp-browse:hover {{ opacity: 1; }}
        .tp-browse image {{ -gtk-icon-size: {back_icon}px; }}
        /* A new version is waiting. A dot rather than a count or a word: it
           says only that something is here, which is all it knows, and it
           reads at the distance this interface is built for.

           Drawn in the accent colour on the button that opens Settings, and
           on the row that names the version. The button's mark goes as soon
           as the row has been reached; the row keeps its own. */
        /* The dot is placed inside the gradient rather than with
           background-position, which GTK will not take two values for: it
           rejects the whole declaration as junk at the end of a value, and
           falls back to the top left corner. Windows tolerated it and
           macOS did
           not, so it looked correct on the machine it was written on and
           wrong everywhere else. The size comes from the colour stops for the
           same reason - fewer properties, fewer things to be refused. */
        .tp-badge {{
            background-image: radial-gradient(circle at 88% 14%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
        }}
        .tp-badge-row {{
            background-image: radial-gradient(circle at {badge_left}px 50%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
            padding-left: {badge_indent}px;
        }}
        /* The selection highlight is this same blue, so a blue dot on the
           selected row is a blue dot on blue. It has to change colour for the
           one moment it matters most - the row is selected the instant it is
           reached. */
        .tp-menu > row.tp-badge-row:selected {{
            background-image: radial-gradient(circle at {badge_left}px 50%,
                {on_highlight} 0, {on_highlight} {badge_r}px, transparent {badge_r}px);
        }}
        .tp-gear {{ padding: {pad_v}px {pad_h}px; }}
        /* Only where it sits beside the tall pair, and the same height as it.
           `tall_pad` across is what `tall_v` down was before the height came
           back ten percent - it is kept because the widths were to stay put,
           which leaves these marginally wider than tall rather than square. */
        .tp-gear.tp-tall {{ padding: {tall_v}px {tall_pad}px; }}
        .tp-row-icon {{ -gtk-icon-size: {row_icon}px; opacity: 0.65; }}
        .tp-back image {{ -gtk-icon-size: {back_icon}px; }}
        .{video} {{ background-color: black; }}
        ",
        title = px(20.0),
        tracking = px(2.0).max(1),
        // Scaled like everything else: a dot sized for a monitor is invisible
        // on a television across a room.
        // Big enough to read from a sofa, which is the distance this whole
        // interface is sized for. The first attempt was five pixels and
        // looked like a rendering artefact.
        badge_r = px(7.0).max(5),
        badge_left = px(14.0),
        badge_indent = px(24.0),
        // What reads against the selection highlight rather than into it.
        on_highlight = "#ffffff",
        row = px(21.0),
        hint = px(20.0),
        small = px(17.0),
        note = px(16.0),
        tight_v = px(7.0),
        tight_h = px(10.0),
        pad_v = px(9.0),
        pad_h = px(18.0),
        radius = px(8.0),
        // Larger than a row's corner, in proportion to the box it rounds. At
        // the row radius a panel this size reads as a rectangle with the
        // corners knocked off.
        panel_radius = px(16.0),
        panel_pad = px(8.0),
        outline = px(2.0).max(1),
        handle = px(18.0),
        // Bright on dark; on light a white knob held in by its ring rather
        // than a dark disc, which read as heavy against a white row and
        // heavier still in a column of them.
        knob = "#dcdcdc",
        fill = "#b9b9b9",
        trough = "rgba(255, 255, 255, 0.13)",
        knob_edge = "rgba(0, 0, 0, 0.55)",
        edge = px(1.0).max(1),
        switch_w = px(64.0),
        switch_h = px(32.0),
        slider = px(26.0),
        section = px(28.0),
        group = px(16.0),
        group_top = px(24.0),
        group_gap = px(4.0),
        group_first_top = px(10.0),
        // About three quarters of the page's row. Clearly subordinate to the
        // menu behind it, and still a size anyone can read from a sofa - which
        // half size was not, on the one list in the interface made of
        // near-identical strings where a misread picks the wrong track.
        rule_gap = px(6.0),
        selector_row = px(17.0),
        selector_row_pad_v = px(7.0),
        selector_row_pad_h = px(14.0),
        shadow_drop = px(4.0),
        shadow_blur = px(18.0),
        subrow = px(28.0),
        mark = px(4.0),
        // A shade larger than `ICON_PX`, which is what every other icon in
        // the interface uses. The gear sits beside the fullscreen mark, and
        // that mark is a picture with clear space drawn into it - so at the
        // same nominal size the gear's own glyph came out visibly the smaller
        // of the two. Matched by eye rather than by number, because what has
        // to agree is the drawn marks and not the boxes around them.
        icon = px(ICON_PX + 3.5),
        icon_main = px(38.4),
        crumb_pad = px(6.0),
        // The 1 and 2 on the output buttons. About half the icon, which is
        // what keeps it a mark on a speaker rather than a numeral with a
        // speaker behind it.
        badge = px(ICON_PX * 0.5),
        badge_pad = px(3.0),
        leading = px(38.0),
        back_icon = px(22.0),
        row_icon = px(18.0),
        bar = px(6.0),
        align_bar = px(14.0),
        align_bar_radius = px(7.0),
        // A literal color rather than a theme name: GTK's named colors
        // differ between themes and libadwaita, and an undefined one makes
        // the whole declaration fail to parse - which silently leaves the
        // highlighted row unreadable. Both foreground and background are
        // set for the same reason: overriding only the background left the
        // theme's white selection text on a pale color.
        modal = px(48.0),
        modal_pad = px(16.0),
        highlight = "#3584e4",
        film_title = px(48.0),
        code_tracking = px(8.0).max(2),
        film_facts = px(24.0),
        film_plot = px(22.0),
        fact = px(20.0),
        // Half again as tall as the type inside it would otherwise leave it,
        // less ten percent. Vertical and horizontal are separate numbers
        // because that ten percent came off the height alone: the marks in the
        // button bar keep the width that had them square.
        tall_v = px(17.0),
        tall_pad = px(20.0),
        play_icon = px(46.0),
        focus_ring = px(3.0).max(2),
        // The blue the play and restart buttons are drawn in - the same accent
        // as everything else the application colors deliberately.
        play_fill = "#3584e4",
        play_hover = "#4a90e8",
        play_ink = "#ffffff",
        // What shows where you are: the selected row, and the ring round a
        // focused button.
        //
        // **Not the accent, and that is the point.** With blue meaning both
        // "this is the action" and "this is where you are", a blue button
        // sitting above a blue selected row read as one continuous thing, and
        // a focused button had nothing left to distinguish itself with. The
        // highest-contrast neutral against the page says "here" without
        // competing with any color that means something - white on the dark
        // theme, and near-black on the light one, where white would be
        // invisible against a near-white page.
        focus = "#ffffff",
        // The same white, backed off, for the row the cursor is on. Only the
        // row: the ring on a focused button stays at full strength, being a
        // thin outline that has nothing like the area to spare.
        focus_row = "rgba(255, 255, 255, 0.7)",
        on_focus = "#1c1c1c",
        // The ink both corner marks share. The fullscreen image is drawn in
        // it as a picture; the gear is told to match.
        // The page's own ground. Matched to what each theme's window is
        // already drawn in, since the backdrop paints over the whole page and
        // a mismatch would show as a rectangle behind the content.
        page_bg = "#242424",
        // Darker than the page and all but opaque. It sits over artwork here
        // and will sit over a moving picture later, and neither is a ground
        // anyone can read a list against.
        selector_bg = "rgba(26, 26, 26, 0.98)",
        // The poster's frame, a shade off the page in whichever direction
        // there is room to go.
        panel = "rgba(255, 255, 255, 0.07)",
        video = crate::player::VIDEO_CSS_CLASS,
    )
}

#[cfg(test)]
mod notices {
    use super::*;

    /// The real file, since that is what ships and what the transform has to
    /// cope with. A table that still has its pipes in it is the failure this
    /// is watching for: it reads as punctuation rather than as a list.
    #[test]
    fn the_shipped_notices_read_as_text() {
        let blocks = notices_blocks(include_str!("../THIRD-PARTY.md"));
        assert!(!blocks.is_empty(), "nothing was produced");

        let mut headings = Vec::new();
        for block in &blocks {
            match block {
                Notice::Heading(text) => headings.push(text.as_str()),
                Notice::Text(text) => {
                    assert!(!text.contains('|'), "table pipe left in: {text:?}");
                    assert!(!text.contains("**"), "bold marker left in: {text:?}");
                    assert!(!text.starts_with('>'), "quote marker left in: {text:?}");
                    assert!(text.trim() != "[!NOTE]", "note marker left in");
                }
            }
        }

        for wanted in ["Fonts", "Native libraries", "Rust dependencies"] {
            assert!(
                headings.contains(&wanted),
                "no {wanted:?} heading in {headings:?}"
            );
        }
    }

    /// A crate row keeps all three of its cells, joined rather than dropped.
    #[test]
    fn a_crate_row_keeps_its_columns() {
        let blocks = notices_blocks(
            "## Rust dependencies\n\n| Crate | Version | License |\n|---|---|---|\n| serde | 1.0.229 | MIT OR Apache-2.0 |\n",
        );
        let rows: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("serde")
                && row.contains("1.0.229")
                && row.contains("MIT OR Apache-2.0")),
            "the crate row lost a column: {rows:?}"
        );
        // The rule under the header carries no words and should be gone.
        assert!(!rows.iter().any(|row| row.contains("---")), "{rows:?}");
    }

    /// Paragraphs wrapped in the file wrap to the window instead.
    #[test]
    fn wrapped_prose_is_rejoined() {
        let blocks = notices_blocks("one line\nand its continuation\n\na second paragraph\n");
        let texts: Vec<&String> = blocks
            .iter()
            .filter_map(|block| match block {
                Notice::Text(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[0], "one line and its continuation");
    }
}

#[cfg(test)]
mod summary_lines {
    use super::{MOST_LANGUAGES, summary_markup};

    fn languages(count: usize) -> Vec<String> {
        (0..count).map(|i| format!("Lang{i}")).collect()
    }

    /// The rule applies to whichever line is handed too many, not to the one
    /// that happens to show it most. The subtitle line is usually the long
    /// one, so an audio line built by different code would go untested.
    #[test]
    fn either_line_counts_what_it_left_out() {
        for name in ["Audio", "Subtitles"] {
            let line = summary_markup(name, &languages(MOST_LANGUAGES + 5));
            assert!(line.contains("+5 more"), "{name} did not count the rest");
            assert!(line.starts_with(&format!("<span alpha='60%'>{name}:</span>")));
        }
    }

    /// Exactly at the limit is a complete list, and saying "+0 more" about it
    /// would be both noise and a lie.
    #[test]
    fn nothing_left_over_is_said_about() {
        let line = summary_markup("Audio", &languages(MOST_LANGUAGES));
        assert!(!line.contains("more"));
        assert!(line.contains("Lang0"));
        assert!(line.contains(&format!("Lang{}", MOST_LANGUAGES - 1)));
    }

    /// Only as many as fit are named, whatever it was given.
    #[test]
    fn never_names_more_than_the_limit() {
        let line = summary_markup("Subtitles", &languages(40));
        assert_eq!(line.matches("Lang").count(), MOST_LANGUAGES);
        assert!(line.contains(&format!("+{} more", 40 - MOST_LANGUAGES)));
    }

    /// A file with no such track says so, rather than showing an empty line
    /// where a list should be.
    #[test]
    fn nothing_at_all_says_so() {
        let line = summary_markup("Subtitles", &[]);
        assert!(line.contains(super::NO_TRACKS));
        assert!(!line.contains("more"));
    }

    /// Track titles come from files and can hold anything. An unescaped
    /// ampersand is not a stray character here - it makes the markup invalid,
    /// and GTK draws nothing at all for the whole line.
    #[test]
    fn a_language_named_with_markup_cannot_break_the_line() {
        let line = summary_markup("Audio", &["Ol' <b>Bill</b> & Ben".to_string()]);
        assert!(line.contains("&amp;"));
        assert!(line.contains("&lt;b&gt;"));
    }
}

#[cfg(test)]
mod readings {
    use super::{offset_label, volume_label};

    /// The sign is the whole reading: it says which way the sound moves, and
    /// it is the only thing separating the two directions now that the words
    /// are gone.
    #[test]
    fn a_shifted_output_reads_with_its_direction() {
        assert_eq!(offset_label(150.0), "+150ms");
        assert_eq!(offset_label(-150.0), "-150ms");
        assert_eq!(offset_label(crate::config::MAX_OFFSET_MS), "+1000ms");
        assert_eq!(offset_label(-crate::config::MAX_OFFSET_MS), "-1000ms");
    }

    /// Unshifted is a plain zero and never a signed one. Rounding a small
    /// negative gives -0, which formats as "-0ms" and claims a shift that is
    /// not there.
    #[test]
    fn an_unshifted_output_reads_without_a_sign() {
        assert_eq!(offset_label(0.0), "0ms");
        assert_eq!(offset_label(-0.0), "0ms");
        assert_eq!(offset_label(-0.4), "0ms");
        assert_eq!(offset_label(0.4), "0ms");
    }

    /// Sliders move in tens but a stored value can be anything, including
    /// something written into the config file by hand.
    #[test]
    fn a_reading_is_rounded_to_the_millisecond() {
        assert_eq!(offset_label(149.6), "+150ms");
        assert_eq!(offset_label(-149.6), "-150ms");
    }

    /// Every reading has to fit the space kept for it, or it widens the label
    /// and moves the bar beside it.
    #[test]
    fn every_reading_fits_the_space_kept_for_it() {
        let longest = [
            offset_label(-crate::config::MAX_OFFSET_MS),
            offset_label(crate::config::MAX_OFFSET_MS),
            volume_label(1.0, false),
            volume_label(0.0, true),
        ];
        for reading in longest {
            assert!(
                reading.chars().count() <= super::READING_CHARS as usize,
                "{reading:?} is wider than the space kept for it"
            );
        }
    }

    /// A silenced output says so rather than showing the level it will come
    /// back to, which would read as though it were playing.
    #[test]
    fn a_silenced_output_says_so_whatever_its_level() {
        assert_eq!(volume_label(0.75, true), "Muted");
        assert_eq!(volume_label(0.0, true), "Muted");
        assert_eq!(volume_label(0.75, false), "75%");
        assert_eq!(volume_label(0.0, false), "0%");
        assert_eq!(volume_label(1.0, false), "100%");
    }
}

#[cfg(test)]
mod settings_rows {
    use super::*;

    use crate::kodi_setup::Confinement;

    /// The Kodi installations these tests pretend were found. Two, so that the
    /// repeated rows are actually repeated - with one there is no difference
    /// between "a group per installation" and "a group" - and both ordinary,
    /// so the count below is not also counting a sandbox's extra row.
    fn kodis() -> Vec<KodiPane> {
        vec![
            KodiPane {
                heading: "KODI 21.1 (STANDARD)".to_string(),
                confinement: Confinement::None,
            },
            KodiPane {
                heading: "KODI 20.5 (CUSTOM)".to_string(),
                confinement: Confinement::None,
            },
        ]
    }

    /// How many rows an ordinary installation contributes: what type of player
    /// it is, and what it does when it hands a video over.
    const ROWS_PER_KODI: usize = 2;

    /// Every row the whole screen holds, for one state of the Jellyfin
    /// pairing.
    fn every_row(jellyfin: JellyfinPane) -> Vec<Item> {
        Category::ALL
            .iter()
            .flat_map(|category| category.items(&kodis(), jellyfin))
            .map(|(_, item)| item)
            .collect()
    }

    /// Every setting is somewhere, and nowhere twice.
    ///
    /// This is what the old numbering could not promise. Rows were positions
    /// in one list, so a stale number silently built a control onto the wrong
    /// row and left another as a plain line of text - which is what happened
    /// to Preferred Language, and it read as a missing setting rather than a
    /// bug. Categories make losing one easy in a new way: an item can simply
    /// be left out of every list and never appear at all.
    #[test]
    fn every_item_appears_in_exactly_one_category() {
        // Both states of the pairing, because the Jellyfin pane shows
        // different rows in each - and a row placed in neither would be a row
        // nobody can ever reach.
        for jellyfin in [JellyfinPane::NotConnected, JellyfinPane::Connected] {
            let all = every_row(jellyfin);
            for item in &all {
                let count = all.iter().filter(|other| *other == item).count();
                assert_eq!(count, 1, "an item appears {count} times");
            }
            // Written out rather than derived, so adding a setting and
            // forgetting to place it fails here instead of at a glance. It is
            // not the number of `Item` variants: the five an output has are
            // placed once for each output, and the Kodi category holds a group
            // of rows per Kodi found, plus the one row that belongs to no
            // installation and names another by hand.
            let elsewhere = 24;
            let kodi = ROWS_PER_KODI * kodis().len() + 1;
            // One row either way: the way in, or the way out.
            let paired = 1;
            assert_eq!(all.len(), elsewhere + kodi + paired);
        }

        // And between the two states, every Jellyfin row is reachable.
        let both: Vec<Item> = every_row(JellyfinPane::NotConnected)
            .into_iter()
            .chain(every_row(JellyfinPane::Connected))
            .collect();
        for item in [Item::JellyfinConnect, Item::JellyfinDisconnect] {
            assert!(both.contains(&item), "{item:?} is on no pane at all");
        }
    }

    /// The pane says one thing or the other, and never both: a Connect on a
    /// pane that is already connected, or a Disconnect on one with nothing to
    /// disconnect from, would each mean the opposite of what it says.
    #[test]
    fn the_jellyfin_pane_takes_two_shapes() {
        let rows = |state| -> Vec<Item> {
            Category::Jellyfin
                .items(&[], state)
                .into_iter()
                .map(|(_, item)| item)
                .collect()
        };
        assert_eq!(
            rows(JellyfinPane::NotConnected),
            vec![Item::JellyfinConnect]
        );
        assert_eq!(
            rows(JellyfinPane::Connected),
            vec![Item::JellyfinDisconnect]
        );
        // One row under one heading, in both.
        for state in [JellyfinPane::NotConnected, JellyfinPane::Connected] {
            let rows = Category::Jellyfin.items(&[], state);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0.as_deref(), Some("JELLYFIN"));
        }
    }

    /// Every installation heads its own group, and the row that adds one by
    /// hand belongs to none of them.
    ///
    /// This is the shape the wizard's five screens came down to. The three
    /// things it asked - which Kodi, what type of player, what to do on
    /// handover - are the heading and the two rows under it.
    #[test]
    fn each_installation_heads_its_own_group() {
        let rows = Category::Kodi.items(&kodis(), JellyfinPane::NotConnected);
        let headed: Vec<(String, Item)> = rows
            .iter()
            .filter_map(|(heading, item)| heading.as_ref().map(|text| (text.to_string(), *item)))
            .collect();
        assert_eq!(
            headed,
            vec![
                ("KODI 21.1 (STANDARD)".to_string(), Item::KodiType(0)),
                ("KODI 20.5 (CUSTOM)".to_string(), Item::KodiType(1)),
                ("OTHER".to_string(), Item::KodiAdd),
            ]
        );
        // Each installation's rows carry its own index, or a change made on
        // one group would land on another.
        let items: Vec<Item> = rows.iter().map(|(_, item)| *item).collect();
        assert_eq!(
            items,
            vec![
                Item::KodiType(0),
                Item::KodiHandover(0),
                Item::KodiType(1),
                Item::KodiHandover(1),
                Item::KodiAdd,
            ]
        );
    }

    /// How an installation was made decides which rows it gets. A Snap cannot
    /// start an external player at all, so it has nothing to set; a Flatpak
    /// can, once it is given permission, so it has somewhere to say so.
    #[test]
    fn a_sandbox_changes_which_rows_an_installation_has() {
        let sandboxed = |confinement| {
            Category::Kodi
                .items(
                    &[KodiPane {
                        heading: "KODI".to_string(),
                        confinement,
                    }],
                    JellyfinPane::NotConnected,
                )
                .into_iter()
                .map(|(_, item)| item)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sandboxed(Confinement::Snap),
            vec![Item::KodiType(0), Item::KodiAdd]
        );
        assert_eq!(
            sandboxed(Confinement::Flatpak),
            vec![
                Item::KodiType(0),
                Item::KodiHandover(0),
                Item::KodiPermission(0),
                Item::KodiAdd,
            ]
        );
    }

    /// With nothing found the pane says so, rather than offering only a way to
    /// add something and leaving open whether it ever looked.
    #[test]
    fn an_empty_pane_says_why_it_is_empty() {
        let rows = Category::Kodi.items(&[], JellyfinPane::NotConnected);
        let items: Vec<Item> = rows.iter().map(|(_, item)| *item).collect();
        assert_eq!(items, vec![Item::KodiNone, Item::KodiAdd]);
        // One heading over both, since the row saying nothing was found and
        // the row that does something about it are the same subject.
        assert_eq!(rows[0].0.as_deref(), Some("KODI"));
        assert_eq!(rows[1].0, None);
    }

    /// The version sits under the switch that decides whether anything is
    /// said about newer ones. Read the other way round it is a status with no
    /// stated relationship to the control above it.
    #[test]
    fn the_version_follows_the_update_switch() {
        let general: Vec<Item> = Category::General
            .items(&kodis(), JellyfinPane::NotConnected)
            .into_iter()
            .map(|(_, item)| item)
            .collect();
        let switch = general.iter().position(|item| *item == Item::Updates);
        let status = general.iter().position(|item| *item == Item::UpdateStatus);
        assert_eq!(status, switch.map(|at| at + 1));
    }

    /// Clear Data destroys something, and was asked to sit at the end of
    /// General rather than among the everyday toggles.
    #[test]
    fn clearing_data_comes_last() {
        let general = Category::General.items(&kodis(), JellyfinPane::NotConnected);
        assert_eq!(general.last().map(|(_, item)| *item), Some(Item::ClearData));
    }

    /// A row carries a switch or a bar or neither, and the two that carry
    /// both - the pair whose bar can be turned off - are deliberate. What must
    /// not happen is a row claiming a switch it was never built with, since
    /// activating it would then do nothing at all.
    #[test]
    fn every_switch_row_has_something_to_switch() {
        for (_, item) in Category::ALL
            .iter()
            .flat_map(|category| category.items(&kodis(), JellyfinPane::Connected))
        {
            if item.has_switch() {
                assert!(
                    item.setting().is_none(),
                    "a row cannot both open a chooser and hold a switch"
                );
            }
        }
    }
}

/// Brings a widget up from nothing over a quarter of a second.
///
/// Artwork is loaded after the page is already up, so without this it appears
/// at full strength between one frame and the next - which reads as a fault
/// rather than as something finishing loading.
///
/// Opacity rather than a `Revealer`, which is what the controls use for their
/// panels: a revealer that is not revealed takes no space, and a poster
/// collapsing its frame and then pushing it back open would be a worse jolt
/// than the one being fixed. Opacity never touches the layout.
///
/// Driven by the frame clock rather than a timer, so it runs at whatever rate
/// the screen is actually drawing and finishes on a frame rather than between
/// two.
fn fade_in(widget: &impl IsA<gtk::Widget>) {
    const OVER: f64 = 0.5;

    let widget = widget.clone().upcast::<gtk::Widget>();
    widget.set_opacity(0.0);
    let started = std::time::Instant::now();
    widget.add_tick_callback(move |widget, _| {
        let progress = (started.elapsed().as_secs_f64() / OVER).clamp(0.0, 1.0);
        // Eased out, so it arrives softly rather than stopping dead.
        widget.set_opacity(1.0 - (1.0 - progress).powi(3));
        match progress >= 1.0 {
            true => glib::ControlFlow::Break,
            false => glib::ControlFlow::Continue,
        }
    });
}

/// Which of Jellyfin's three reporting endpoints a moment belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
enum JellyfinMoment {
    Started,
    Progress,
    Stopped,
}
