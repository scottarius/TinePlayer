//! The pictures on the media page: the backdrop behind it, and the poster in
//! its frame.
//!
//! One widget for both, because the hard part is the same for each and
//! `GtkPicture` does not do it at either size. The picture has to *cover* its
//! box - filled without being stretched, cropped wherever it does not fit -
//! and `GtkPicture` on this project's GTK 4.6 baseline offers only "keep the
//! aspect ratio", which letterboxes instead. `GtkContentFit`, which would say
//! this in a property, arrived in 4.8.
//!
//! The backdrop adds two things to that: it paints the page's own ground
//! underneath, so there is a known color behind everything rather than
//! whatever the theme left there, and it draws the picture over it at a
//! fraction of full strength.
//!
//! **It was a *screen* blend and is not any more.** Screen never darkens, so
//! it lifted the page where the picture was bright and left it alone where the
//! picture was dark, which is a better-behaved ground for large type than
//! plain transparency gives. It also looked correct on Windows and came out
//! all but invisible on the Pi, where the same `GskBlendNode` goes through a
//! different renderer. A backdrop that depends on which machine is drawing it
//! is not a backdrop, and this is decoration rather than anything load
//! bearing - so it composites plainly now, the same way everywhere.
//!
//! **The base color comes from the stylesheet, through `color`.** A widget
//! cannot read its own CSS background from inside `snapshot`, and hardcoding
//! one here would put a color in Rust that every other color in this
//! application declares in `style_css`, where the light and dark pair sit
//! side by side. `color` is the one paintable property that can be read back,
//! and this widget draws no text, so it is free to carry the background
//! instead. See `.tp-backdrop` in the stylesheet.

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

/// How much of the picture reaches the page.
///
/// Low on purpose, and lower than it looks: screen blending only ever
/// lightens, so twenty percent of a bright frame is a good deal more visible
/// than twenty percent of a dark one. The number that matters is not how well
/// the artwork reads but whether the focus highlight is still the loudest
/// thing on screen, which is what a deliberately high-contrast interface
/// stands to lose behind a picture.
const STRENGTH: f64 = 0.2;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct Artwork {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// Whether this is the backdrop, which paints the page's ground and
        /// blends into it, rather than a picture drawn as it stands.
        pub behind: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Artwork {
        const NAME: &'static str = "TinePlayerArtwork";
        type Type = super::Artwork;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // Decoration, and a screen reader should walk straight past it.
            // Without this it is announced as an unnamed group sitting in
            // front of everything on the page. The page names the film in
            // text directly above, so nothing is lost by staying quiet.
            klass.set_accessible_role(gtk::AccessibleRole::Presentation);
        }
    }

    impl ObjectImpl for Artwork {}

    impl WidgetImpl for Artwork {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let bounds = graphene::Rect::new(0.0, 0.0, width, height);
            let behind = self.behind.get();
            let texture = self.texture.borrow().clone();

            // The page's own ground, painted here so the blend below has
            // something to blend against. Drawn whether or not there is a
            // picture, which is what lets the backdrop sit behind every media
            // page rather than only the ones with artwork.
            //
            // `GtkWidget.get_color` would say this in one call and arrived in
            // GTK 4.10, which is four versions past this project's baseline.
            // The style context is how 4.6 answers the same question.
            if behind {
                snapshot.append_color(&widget.style_context().color(), &bounds);
            }
            let Some(texture) = texture else { return };

            if !behind {
                // A poster, drawn as it is: cropped to its frame and nothing
                // else done to it.
                snapshot.push_clip(&bounds);
                snapshot.append_texture(&texture, &fitted(&texture, width, height));
                snapshot.pop();
                return;
            }

            // The picture over the ground, cropped and faded back. Plainly
            // composited: no blend node.
            //
            // It was a *screen* blend to begin with, which never darkens and
            // so lifted the page only where the picture was bright. It looked
            // right on Windows and came out all but invisible on the Pi, where
            // the same node goes through a different renderer. A backdrop that
            // depends on which machine is drawing it is not a backdrop, and
            // this is decoration - not worth a per-platform branch or the
            // afternoon it would take to find out why. Straight alpha does the
            // same job the same way everywhere.
            snapshot.push_opacity(STRENGTH);
            snapshot.push_clip(&bounds);
            snapshot.append_texture(&texture, &fitted(&texture, width, height));
            snapshot.pop();
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Artwork(ObjectSubclass<imp::Artwork>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Artwork {
    /// The picture behind the whole page: the ground, and a film's fanart
    /// screen-blended into it.
    pub fn backdrop() -> Self {
        let artwork: Self = glib::Object::builder().build();
        artwork.imp().behind.set(true);
        artwork.add_css_class("tp-backdrop");
        artwork
    }

    /// A picture filling a frame, at its own strength.
    pub fn poster() -> Self {
        glib::Object::builder().build()
    }

    /// Hangs a picture, or takes one away.
    pub fn set_texture(&self, texture: Option<gdk::Texture>) {
        *self.imp().texture.borrow_mut() = texture;
        self.queue_draw();
    }
}

/// Where to draw the picture so that it covers the widget without being
/// stretched: scaled by whichever axis needs the most, centered across, and
/// flush with the top.
///
/// Top rather than centered because of what is in the picture. A film's
/// backdrop is composed with its subject in the upper half and ground or
/// shadow along the bottom, so overflowing downwards loses the part nobody
/// looks at. Centering it crops the top and bottom evenly and takes the sky
/// off every wide image.
fn fitted(texture: &gdk::Texture, width: f32, height: f32) -> graphene::Rect {
    let (picture_w, picture_h) = (texture.width() as f32, texture.height() as f32);
    if picture_w <= 0.0 || picture_h <= 0.0 {
        return graphene::Rect::new(0.0, 0.0, width, height);
    }
    // The larger of the two ratios is the one that leaves no gap.
    let scale = (width / picture_w).max(height / picture_h);
    let (drawn_w, drawn_h) = (picture_w * scale, picture_h * scale);
    graphene::Rect::new((width - drawn_w) / 2.0, 0.0, drawn_w, drawn_h)
}

#[cfg(test)]
mod tests {
    /// The rectangle is worked out without touching GTK, so the rule can be
    /// checked without a display - which is what makes it testable in CI at
    /// all, since none of the three runners has one.
    fn at(picture: (f32, f32), into: (f32, f32)) -> (f32, f32, f32, f32) {
        let (picture_w, picture_h) = picture;
        let (width, height) = into;
        let scale = (width / picture_w).max(height / picture_h);
        let (drawn_w, drawn_h) = (picture_w * scale, picture_h * scale);
        ((width - drawn_w) / 2.0, 0.0, drawn_w, drawn_h)
    }

    /// A 16:9 backdrop in a 16:9 window is the case that must not move at
    /// all: no crop, no offset, no scaling artefacts from a near-miss.
    #[test]
    fn a_matching_shape_fills_exactly() {
        let (x, y, w, h) = at((1920.0, 1080.0), (1280.0, 720.0));
        assert_eq!((x, y), (0.0, 0.0));
        assert_eq!((w, h), (1280.0, 720.0));
    }

    /// A wide window: the picture grows to span it and overflows downwards,
    /// which is the crop that keeps the subject.
    #[test]
    fn a_wider_window_crops_the_bottom() {
        let (x, y, w, h) = at((1920.0, 1080.0), (3440.0, 1440.0));
        assert_eq!(x, 0.0, "no horizontal gap");
        assert_eq!(y, 0.0, "pinned to the top");
        assert_eq!(w, 3440.0);
        assert!(h > 1440.0, "overflows downwards, not upwards");
    }

    /// A tall or narrow window crops the sides instead, and centers what is
    /// left so the subject stays in the middle.
    #[test]
    fn a_narrower_window_crops_evenly_across() {
        let (x, y, w, h) = at((1920.0, 1080.0), (800.0, 900.0));
        assert!(x < 0.0, "overflows both sides");
        assert_eq!(y, 0.0);
        assert_eq!(h, 900.0);
        // Symmetrical: what hangs off the left equals what hangs off the
        // right, which is the whole of "horizontally centered".
        assert!((x + w - 800.0 + x).abs() < 0.01);
    }
}
