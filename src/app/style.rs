//! The interface's stylesheet, and the provider it is loaded into.
//!
//! One sheet for the whole application, rebuilt at whatever scale is in force
//! and reloaded into the same provider, which is what lets the interface
//! re-scale when it moves to a different monitor.

use gtk::gdk;

use super::{ICON_PX, PANEL_RADIUS};
use crate::appearance;

/// Registers the provider the interface's sizes are loaded into. Kept so the
/// sizes can be replaced later without stacking up providers, which is what
/// makes re-scaling on a different monitor possible.
pub(super) fn install_styles() -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();

    // **A rejected declaration is discarded in silence.** GTK keeps parsing,
    // the property falls back to its default, and the only trace is a line in
    // the log nobody is reading - which is how `circle at 14pxpx` shipped,
    // taking the badge dot out in *both* directions while looking like a
    // styling decision. Saying so on stderr costs nothing and turns "the dot
    // is missing" into a sentence naming the line.
    provider.connect_parsing_error(|_, section, error| {
        log::error!("Stylesheet rejected at {}: {error}", section.to_str());
    });

    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    provider
}

pub(super) fn style_css(scale: f64) -> String {
    let px = |base: f64| (base * scale).round() as i32;
    // GTK's CSS has no logical properties, so which physical side
    // begins a line has to be worked out and written in. Everything
    // else in this sheet is symmetric and needs no such care.
    let start = appearance::css_start();
    let end = appearance::css_end();
    let badge_inset = match appearance::css_start() {
        "right" => format!("calc(100% - {}px)", px(14.0)),
        _ => format!("{}px", px(14.0)),
    };

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
        /* Jellyfin's own gradient, on the one button that connects to one.
           `background-image` carries it rather than `background-color`, which
           takes a single value - and the theme paints its own gradient image
           there, so anything set as a colour alone is drawn over.

           Their permission covers the gradient on a logo, so a control wearing
           it is beyond what is written down - kept only while it is being
           looked at. The hex values are theirs exactly. */
        .tp-jellyfin {{
            font-weight: bold;
            /* Restated rather than inherited from `.tp-button`. A background
               *image* is not clipped to that rule's radius the way a colour
               is, so the gradient came out square-cornered inside a rounded
               button. */
            border-radius: {radius}px;
            background-image: linear-gradient(to right, {jf_start}, {jf_end});
            background-color: transparent;
            color: {play_ink};
            border-color: transparent;
        }}
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
        /* The same heading on the playback strip, whose rows are padded to
           `crumb_pad` rather than to the page's `pad_h` - so it starts where
           they start rather than a dozen pixels inside them, and takes a
           smaller gap above, that list being a short one read over a film.

           Two classes against one, so it wins on specificity rather than on
           where it happens to sit in the sheet. */
        .tp-group.tp-strip-group {{
            margin: {strip_group_top}px {crumb_pad}px {group_gap}px {crumb_pad}px;
        }}
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
        /* A group heading inside a selector. The page's heading size is the
           selector's *row* size, near enough, so a heading set at it does not
           read as one - it takes a step down of its own, keeping the relation
           to the rows below that it has on the page. Indented to the row
           padding rather than the page's, so it starts where they do.

           Two classes against one, which is how it wins: a rule that loses on
           specificity is discarded in silence. */
        .tp-selector .tp-group {{
            font-size: {selector_group}px;
            margin: {selector_group_top}px {selector_row_pad_h}px {group_gap}px
                {selector_row_pad_h}px;
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

           The same backed-off white the settings categories rest at, so the
           whole application says 'in force but not where the keys are' one
           way. It was a blue bar down the leading edge, which read as a
           different kind of thing entirely - an accent mark rather than a
           quieter version of the highlight beside it - and left two idioms
           for one idea.

           Hover deliberately does not override it, which falls out of this
           rule coming after the hover one. That is what the focused row does
           too: a row carrying a mark keeps it under the pointer. */
        .tp-menu > row.tp-current {{
            background-color: {resting_row};
        }}
        /* Belongs to the row above it: indented so the group reads as one
           thing without every label having to name the output again. */
        .tp-menu > row.tp-subrow {{ margin-{start}: {subrow}px; }}
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
        /* The exception to the rule above, for the lists that keep their place
           marked once the keyboard has gone somewhere else: the settings
           categories, which are the heading of everything in the pane beside
           them, and the browser's listing, which is where you will be put
           back when you come out of the places column.

           Backed well off the focused row's white so the two read in order
           rather than as a pair - the bright one is where the keys are going,
           this one is only saying where you were. Without it the row falls
           through to the theme's own selection color, which is a blue nothing
           else on the screen is drawn in and reads as an accent rather than
           as a quieter cursor. */
        .tp-resting > row:selected {{
            background-image: none;
            background-color: {resting_row};
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
        /* The key list. Darker and more opaque than the volume panel beside
           it, because that one is read against a bar and this one is read
           against whatever frame the film happens to be on - which may be
           anything at all. */
        .tp-shortcuts {{
            background-color: rgba(0, 0, 0, 0.9);
            border-radius: {radius}px;
            padding: {crumb_pad}px;
        }}
        .tp-shortcuts label {{ color: #ffffff; }}
        /* The keys themselves, set apart from what they do so the two columns
           are told apart at a glance from across a room.

           Sized here rather than left to the theme's default, which is what a
           label with no size of its own gets: a size nobody chose, and one
           that does not follow the interface when a fullscreen television
           doubles everything else. Row size, the same as the menus, because
           this is read from the same distance they are. */
        .tp-shortcut-keys {{
            font-size: {row}px;
            font-weight: bold;
            padding: {tight_v}px {pad_h}px;
        }}
        .tp-shortcut-means {{
            font-size: {row}px;
            opacity: 0.85;
            padding: {tight_v}px {pad_h}px;
        }}
        /* Every other row shaded rather than ruled. A line between rows draws
           the eye across the page as much as the row does; a band under the
           words is read as one line without being looked at. */
        .tp-shortcut-row {{ border-radius: {radius}px; }}
        .tp-shortcut-stripe {{ background-color: rgba(255, 255, 255, 0.07); }}
        /* Darker than the strip it sits on, so it reads as a panel laid over
           the bar rather than as more of the bar. */
        .tp-volume-panel {{
            background-color: rgba(0, 0, 0, 0.75);
            border-radius: {radius}px;
            padding: {crumb_pad}px;
            margin-bottom: {crumb_pad}px;
            margin-{end}: {pad_h}px;
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
            margin-{end}: {pad_h}px;
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
           marking them separately. The same backed-off white the menus rest
           at, so 'this is what is on' reads the same way over a film as it
           does on a page. */
        .tp-subtitle-row.tp-current {{
            background-color: {resting_row};
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
            background-image: radial-gradient(circle at {badge_corner} 14%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
        }}
        .tp-badge-row {{
            background-image: radial-gradient(circle at {badge_left} 50%,
                {highlight} 0, {highlight} {badge_r}px, transparent {badge_r}px);
            padding-{start}: {badge_indent}px;
        }}
        /* The selection highlight is this same blue, so a blue dot on the
           selected row is a blue dot on blue. It has to change colour for the
           one moment it matters most - the row is selected the instant it is
           reached. */
        .tp-menu > row.tp-badge-row:selected {{
            background-image: radial-gradient(circle at {badge_left} 50%,
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
        // Measured from the edge the line starts at, so the dot stays
        // beside the words rather than on the far side of the row.
        badge_left = badge_inset,
        badge_corner = match appearance::css_start() {
            "right" => "12%",
            _ => "88%",
        },
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
        panel_radius = px(PANEL_RADIUS),
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
        strip_group_top = px(12.0),
        // About three quarters of the page's row. Clearly subordinate to the
        // menu behind it, and still a size anyone can read from a sofa - which
        // half size was not, on the one list in the interface made of
        // near-identical strings where a misread picks the wrong track.
        rule_gap = px(6.0),
        selector_row = px(17.0),
        selector_group = px(13.0),
        // Less than the page's 24: a popover is a short list read in one
        // glance, and the gap that separates groups on a full screen only
        // makes this one taller.
        selector_group_top = px(14.0),
        selector_row_pad_v = px(7.0),
        selector_row_pad_h = px(14.0),
        shadow_drop = px(4.0),
        shadow_blur = px(18.0),
        subrow = px(28.0),
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
        jf_start = "#AA5CC3",
        jf_end = "#00A4DC",
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
        // The same white again, right back, for a row that is still in force
        // while the keyboard is somewhere else - the settings category on
        // show. Weak enough that the focused row is plainly the louder of the
        // two, since one of them is where the next key goes and the other is
        // only saying what is being looked at.
        resting_row = "rgba(255, 255, 255, 0.16)",
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
