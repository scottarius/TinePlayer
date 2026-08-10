//! Holds the media page to a maximum width, centered, however wide the window
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
//! **A maximum width rather than a 16:9 column, which is what this was
//! first.** Deriving the width from the window's *height* meant that making
//! the window shorter also made it narrower, so the page pinched inwards into
//! a tall thin strip down the middle while the space either side went to
//! backdrop. Nothing about the design wants that: the reason to stop widening
//! is that lines get too long to read, and how tall the window is has no
//! bearing on that. A ceiling is the whole rule, and below it the page simply
//! fills what it is given.
//!
//! **A widget rather than a resize handler, because the handler does not
//! work.** Setting a size request from `notify::default-width` looks right and
//! is wrong twice over: the property tracks the *requested* size rather than
//! the real one, so it never moves at all while the window is maximized -
//! which is the case this exists for - and a size request is a minimum, so a
//! page whose natural width exceeds it widens anyway. Measuring and allocating
//! is the only place the answer can be enforced rather than suggested.

use std::cell::Cell;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Column {
        /// The ceiling, in pixels, already scaled by the caller.
        pub most: Cell<i32>,
    }

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
            let column = super::held(width, self.most.get());
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
    /// Wraps a page, holding it to `most` pixels wide at the most.
    pub fn around(child: &impl IsA<gtk::Widget>, most: i32) -> Self {
        let column: Self = glib::Object::builder().build();
        column.imp().most.set(most);
        child.as_ref().set_parent(&column);
        column
    }
}

/// How wide the page is allowed to be in a window of this width.
///
/// Never wider than the window: a narrow window is simply filled, because the
/// alternative is asking for room that is not there and being clipped for it.
/// A ceiling of zero or less means no ceiling, so a caller that has not
/// decided one yet gets the window rather than nothing.
fn held(width: i32, most: i32) -> i32 {
    let width = width.max(0);
    match most > 0 {
        true => width.min(most),
        false => width,
    }
}

#[cfg(test)]
mod tests {
    use super::held;

    /// The case this exists for: a maximized ultrawide, where the page keeps
    /// its width and the backdrop takes the rest.
    #[test]
    fn a_wide_window_stops_at_the_ceiling() {
        assert_eq!(held(3440, 2000), 2000);
        // What is left over goes to the backdrop, evenly on both sides.
        assert_eq!((3440 - held(3440, 2000)) / 2, 720);
    }

    /// Anything narrower than the ceiling is filled rather than pinched. This
    /// is what the 16:9 rule got wrong: it took the width from the *height*,
    /// so shrinking a window vertically squeezed the page into a strip down
    /// the middle with backdrop either side of it.
    #[test]
    fn a_smaller_window_is_filled_whatever_its_shape() {
        assert_eq!(held(1400, 2000), 1400);
        // The same width at three very different heights, because height has
        // nothing to do with how long a line should be.
        for _height in [400, 900, 1440] {
            assert_eq!(held(1400, 2000), 1400);
        }
    }

    /// A window with no size yet, which is what the first allocation looks
    /// like, must not produce a negative width.
    #[test]
    fn nothing_yet_is_not_a_negative_column() {
        assert_eq!(held(0, 2000), 0);
        assert_eq!(held(0, 0), 0);
    }

    /// No ceiling set means no ceiling applied.
    #[test]
    fn an_unset_ceiling_lets_the_page_fill() {
        assert_eq!(held(3440, 0), 3440);
        assert_eq!(held(3440, -1), 3440);
    }
}
