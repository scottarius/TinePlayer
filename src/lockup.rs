//! The application logo: the mark with the name beside or beneath it.
//!
//! **A widget of its own because `GtkImage` cannot draw a picture that is not
//! square.** It has one size property, and that property does two jobs: it is
//! the widget's measured footprint *and* the box the paintable is fitted
//! inside before being centered in whatever the widget was allocated. A mark
//! never notices, because the two agree for a square. A lockup four and a half
//! times wider than it is tall cannot satisfy both at once, and each way round
//! fails visibly:
//!
//! - pixel size as the width gives a header 145 units deep to hold a logo 32
//!   units tall, with the heading beside it stranded in all that room;
//! - pixel size as the height, with a size request supplying the width,
//!   measures correctly and then draws the artwork at a third of the size in
//!   the middle of the space it correctly reserved.
//!
//! The second of those is worth knowing about, because measuring the widget
//! says it is right. Only what is on screen says otherwise.
//!
//! So this widget declares no size of its own and takes the one it is given,
//! the way [`crate::artwork`] already does, and paints the texture across the
//! whole of it. The caller sets a size request in the artwork's own
//! proportions, and what is asked for is what is drawn.
//!
//! A PNG rather than the SVG it was drawn from, because GStreamer's Windows
//! distribution ships no gdk-pixbuf loaders at all and so cannot decode SVG at
//! runtime. Both lockups are rendered from the dark artwork, since the light
//! theme went in PR #14 and this ink has to read against a dark ground.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct Lockup {
        pub texture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Lockup {
        const NAME: &'static str = "TinePlayerLockup";
        type Type = super::Lockup;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // A picture of the application's name, so a reader that meets it
            // should say the name rather than announce a picture. The label
            // itself is set on the instance, since it is the same words in
            // both places it is drawn.
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for Lockup {}

    impl WidgetImpl for Lockup {
        /// No `measure`, deliberately: the size request is the whole of what
        /// this widget asks for, so a caller cannot be surprised by artwork
        /// having an opinion about how big it should be.
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            snapshot.append_texture(&texture, &graphene::Rect::new(0.0, 0.0, width, height));
        }
    }
}

glib::wrapper! {
    pub struct Lockup(ObjectSubclass<imp::Lockup>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Lockup {
    /// The logo at `width`, with the height taken from the artwork.
    ///
    /// The proportions are read off the texture rather than written down at
    /// the call sites, where they would be a second copy of something the
    /// picture already knows and a way for the two lockups to end up scaled
    /// differently.
    ///
    /// Centered on both axes, because the widget is exactly the size it asked
    /// for and stretching it to fill a taller row would distort the artwork.
    pub fn new(bytes: &'static [u8], width: f64) -> Self {
        let lockup: Self = glib::Object::new();
        let mut height = width;
        match gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)) {
            Ok(texture) => {
                height = width * f64::from(texture.height()) / f64::from(texture.width());
                lockup.imp().texture.replace(Some(texture));
            }
            // Said out loud: a logo that silently fails to appear looks like a
            // screen that has not finished drawing.
            Err(e) => eprintln!("Could not load the application logo: {e}"),
        }
        lockup.set_size_request(
            width.round().max(1.0) as i32,
            height.round().max(1.0) as i32,
        );
        lockup.set_halign(gtk::Align::Center);
        lockup.set_valign(gtk::Align::Center);
        lockup.update_property(&[gtk::accessible::Property::Label("TinePlayer")]);
        lockup
    }
}
