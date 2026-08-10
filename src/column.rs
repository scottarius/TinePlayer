//! Holds the media page to a 16:9 column, centered, however wide the window
//! gets.
//!
//! A maximized window on a 21:9 monitor is an ordinary desktop case, and
//! letting the layout track the window means one composition at 16:9 and a
//! progressively looser one either side of it: a plot line three thousand
//! pixels across is not a paragraph anyone reads, and a row whose value has
//! drifted that far from its label has stopped reading as one row. Holding the
//! column and letting the backdrop fill what is left keeps a single design
//! honest at every shape.
//!
//! **A widget rather than a resize handler, because the handler does not
//! work.** Setting a size request from `notify::default-width` looks right and
//! is wrong twice over: the property tracks the *requested* size rather than
//! the real one, so it never moves at all while the window is maximized -
//! which is the case this exists for - and a size request is a minimum, so a
//! page whose natural width exceeds it widens anyway. Measuring and allocating
//! is the only place the answer can be enforced rather than suggested.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// The shape the page is composed for.
const RATIO: f64 = 16.0 / 9.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Column;

    #[glib::object_subclass]
    impl ObjectSubclass for Column {
        const NAME: &'static str = "TinePlayerColumn";
        type Type = super::Column;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Column {
        /// A widget must let go of its children itself: GTK 4 does not
        /// unparent them on dispose, and leaving one parented is a warning on
        /// every screen change.
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Column {
        /// Measured as the child measures, so the window can still be made
        /// as small as the page genuinely needs. This narrows a page that is
        /// given too much room; it does not claim room it has not got.
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.obj().first_child() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.obj().first_child() else {
                return;
            };
            let column = super::held(width, height);
            child.size_allocate(
                &gtk::Allocation::new((width - column) / 2, 0, column, height),
                baseline,
            );
        }
    }
}

glib::wrapper! {
    pub struct Column(ObjectSubclass<imp::Column>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Column {
    /// Wraps a page in the column it is composed for.
    pub fn around(child: &impl IsA<gtk::Widget>) -> Self {
        let column: Self = glib::Object::builder().build();
        child.as_ref().set_parent(&column);
        column
    }
}

/// How wide the page is allowed to be in a window of this size.
///
/// Never wider than the window: a tall or square window is simply filled,
/// because the alternative is asking for room that is not there and being
/// clipped for it.
fn held(width: i32, height: i32) -> i32 {
    let wanted = (height.max(0) as f64 * RATIO).round() as i32;
    wanted.min(width).max(0)
}

#[cfg(test)]
mod tests {
    use super::held;

    /// The case this exists for: a maximized ultrawide, where the page keeps
    /// its shape and the backdrop takes the rest.
    #[test]
    fn a_wide_window_keeps_the_column() {
        assert_eq!(held(3440, 1440), 2560);
        // What is left over goes to the backdrop, evenly on both sides.
        assert_eq!((3440 - held(3440, 1440)) / 2, 440);
    }

    /// A 16:9 window is exactly the composition, so nothing is held back.
    #[test]
    fn a_matching_window_is_filled() {
        assert_eq!(held(1920, 1080), 1920);
        assert_eq!(held(1280, 720), 1280);
    }

    /// Anything squarer or taller is filled rather than clipped. Asking for
    /// more than the window has would put the page's right edge off-screen.
    #[test]
    fn a_narrow_window_is_never_overrun() {
        assert_eq!(held(1000, 900), 1000);
        assert_eq!(held(600, 800), 600);
    }

    /// A window with no size yet, which is what the first allocation looks
    /// like, must not produce a negative width.
    #[test]
    fn nothing_yet_is_not_a_negative_column() {
        assert_eq!(held(0, 0), 0);
        assert_eq!(held(0, 500), 0);
    }
}
