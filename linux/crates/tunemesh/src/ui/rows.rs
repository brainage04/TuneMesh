//! The list model behind every list and grid of the app, and the diff that
//! keeps an update down to the rows that changed.
//!
//! GTK makes a widget for every item in a fixed window around what is on
//! screen: 200 rows in a list view, `max_columns` x 31 tiles in a grid
//! view. Making, styling and laying out all of them in one frame took 30-50
//! ms. `LazyRows` shows its items a screenful per frame up to that window
//! (`grow`), and takes them away the same way (`drain_step`), so no frame
//! makes or destroys more than about a screenful of rows. An update changes
//! only the range `diff` finds; every other row keeps its widget.
//!
//! The items are placeholders: a row reads what it shows from its page's
//! data by its position (`gtk::ListItem::position`).

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

/// The range an update replaces: `(position, removed, added)`, from the
/// longest common prefix and suffix of `old` and `new` (which never
/// overlap).
pub fn diff<T: PartialEq>(old: &[T], new: &[T]) -> (usize, usize, usize) {
    diff_by(old.len(), new.len(), |i, j| old[i] == new[j])
}

/// `diff` over lengths, with `same(i, j)` comparing `old[i]` and `new[j]`.
pub fn diff_by(
    old: usize,
    new: usize,
    same: impl Fn(usize, usize) -> bool,
) -> (usize, usize, usize) {
    let prefix = (0..old.min(new)).take_while(|&i| same(i, i)).count();
    let suffix = (0..old.min(new) - prefix)
        .take_while(|&k| same(old - 1 - k, new - 1 - k))
        .count();
    (prefix, old - prefix - suffix, new - prefix - suffix)
}

/// Rows GTK keeps a widget for in a list view (`GTK_LIST_VIEW_MAX_LIST_ITEMS`).
pub const LIST_WINDOW: u32 = 200;
/// Rows a list shows per frame while it fills or empties: about a screen.
pub const LIST_CHUNK: u32 = 40;

mod imp {
    use std::cell::{Cell, RefCell};

    use gtk::subclass::prelude::*;
    use gtk::{gio, glib, prelude::*};

    #[derive(Default)]
    pub struct LazyRows {
        /// Items in the model's data.
        pub len: Cell<u32>,
        /// Items GTK sees: `len` once grown.
        pub shown: Cell<u32>,
        /// GTK's widget window, and the rows added per frame below it.
        pub window: Cell<u32>,
        pub chunk: Cell<u32>,
        pub growing: Cell<bool>,
        pub draining: Cell<bool>,
        /// The view whose frames pace the growth.
        pub view: RefCell<Option<glib::WeakRef<gtk::Widget>>>,
        /// The list items GTK made for this model's view.
        pub items: RefCell<Vec<glib::WeakRef<gtk::ListItem>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LazyRows {
        const NAME: &'static str = "TmLazyRows";
        type Type = super::LazyRows;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for LazyRows {}

    impl ListModelImpl for LazyRows {
        fn item_type(&self) -> glib::Type {
            glib::Object::static_type()
        }

        fn n_items(&self) -> u32 {
            self.shown.get()
        }

        fn item(&self, position: u32) -> Option<glib::Object> {
            (position < self.shown.get()).then(|| glib::Object::new::<glib::Object>())
        }
    }
}

glib::wrapper! {
    pub struct LazyRows(ObjectSubclass<imp::LazyRows>) @implements gio::ListModel;
}

impl LazyRows {
    /// An empty model for a view that keeps `window` widgets, filled
    /// `chunk` rows per frame.
    pub fn new(window: u32, chunk: u32) -> Self {
        let rows: Self = glib::Object::new();
        rows.imp().window.set(window);
        rows.imp().chunk.set(chunk.max(1));
        rows
    }

    /// Sets the widget window and chunk (a grid's change with its columns).
    pub fn set_window(&self, window: u32, chunk: u32) {
        self.imp().window.set(window);
        self.imp().chunk.set(chunk.max(1));
    }

    /// Items in the data (shown or not yet).
    pub fn len(&self) -> u32 {
        self.imp().len.get()
    }

    /// Remembers a list item the view made for this model (from the
    /// factory's `setup`), for `drain_step`.
    pub fn track(&self, li: &gtk::ListItem) {
        let mut items = self.imp().items.borrow_mut();
        items.retain(|w| w.upgrade().is_some());
        items.push(li.downgrade());
    }

    fn changed(&self, position: u32, removed: u32, added: u32) {
        if removed > 0 || added > 0 {
            self.items_changed(position, removed, added);
        }
    }

    /// Replaces every item with `len` new ones; they appear a chunk per
    /// frame of `view`.
    pub fn reset(&self, len: u32, view: &impl IsA<gtk::Widget>) {
        *self.imp().view.borrow_mut() = Some(view.upcast_ref::<gtk::Widget>().downgrade());
        self.replace_all(len);
        self.grow_later();
    }

    /// `reset` without growing: the first chunk only.
    fn replace_all(&self, len: u32) {
        let imp = self.imp();
        imp.draining.set(false);
        let old = imp.shown.get();
        imp.len.set(len);
        imp.shown.set(len.min(imp.chunk.get()));
        self.changed(0, old, imp.shown.get());
    }

    fn grow_later(&self) {
        let imp = self.imp();
        if imp.shown.get() >= imp.len.get() || imp.growing.get() {
            return;
        }
        let Some(view) = imp.view.borrow().as_ref().and_then(|w| w.upgrade()) else {
            return;
        };
        imp.growing.set(true);
        let weak = self.downgrade();
        view.add_tick_callback(move |_, _| {
            let Some(rows) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !rows.imp().draining.get() && rows.grow() {
                glib::ControlFlow::Continue
            } else {
                rows.imp().growing.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// Shows the next chunk; whether there is more to show.
    fn grow(&self) -> bool {
        let imp = self.imp();
        let (shown, len) = (imp.shown.get(), imp.len.get());
        if shown >= len {
            return false;
        }
        // Past GTK's window no row gets a widget: show the rest at once.
        let next = if shown < imp.window.get() {
            (shown + imp.chunk.get()).min(len)
        } else {
            len
        };
        imp.shown.set(next);
        self.changed(shown, 0, next - shown);
        next < len
    }

    /// Replaces `removed` items at `position` of the data with `added` new
    /// ones. Rows not shown yet change without GTK seeing them; many new
    /// rows at the end of what is shown appear a chunk per frame.
    pub fn splice(&self, position: u32, removed: u32, added: u32) {
        let imp = self.imp();
        let (shown, old_len) = (imp.shown.get(), imp.len.get());
        imp.len.set(old_len - removed + added);
        // The shown rows stay a prefix of the data.
        if position > shown || (position == shown && shown < old_len) {
            return;
        }
        let gone = removed.min(shown - position);
        let after = shown - position - gone;
        let chunk = imp.chunk.get();
        let added_now = if after == 0 && added > chunk {
            chunk
        } else {
            added
        };
        imp.shown.set(shown - gone + added_now);
        self.changed(position, gone, added_now);
        self.grow_later();
    }

    /// Removes up to a chunk of rows that have widgets, after dropping at
    /// once the rows that have none; whether the model is empty. Run once a
    /// frame, this tears a list down without a long frame.
    pub fn drain_step(&self) -> bool {
        let imp = self.imp();
        imp.draining.set(true);
        imp.len.set(imp.shown.get());
        let mut shown = imp.shown.get();
        let bound = imp
            .items
            .borrow()
            .iter()
            .filter_map(|w| w.upgrade())
            .filter(|li| li.item().is_some())
            .map(|li| li.position())
            .filter(|&p| p < shown)
            .fold(None, |r: Option<(u32, u32)>, p| {
                Some(r.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))))
            });
        if let Some((lo, hi)) = bound {
            if hi + 1 < shown {
                imp.shown.set(hi + 1);
                self.changed(hi + 1, shown - hi - 1, 0);
                shown = hi + 1;
            }
            if lo > 0 {
                imp.shown.set(shown - lo);
                self.changed(0, lo, 0);
                shown -= lo;
            }
        }
        let k = imp.chunk.get().min(shown);
        imp.shown.set(shown - k);
        self.changed(shown - k, k, 0);
        imp.len.set(imp.shown.get());
        imp.shown.get() == 0
    }
}

/// The page position of a list item, if it shows one.
pub fn position(li: &glib::WeakRef<gtk::ListItem>) -> Option<u32> {
    let li = li.upgrade()?;
    li.item()?;
    Some(li.position()).filter(|&p| p != gtk::INVALID_LIST_POSITION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_keeps_the_common_prefix_and_suffix() {
        // Next track with the queue open: now-playing moves from 10 to 11,
        // and only the rows between "now" and the rest change.
        let old = ["now", "10", "next", "11", "12", "13"];
        let new = ["now", "11", "next", "12", "13"];
        assert_eq!(diff(&old, &new), (1, 3, 2));
        // A track arrives in the middle of an album.
        assert_eq!(diff(&["a", "b", "d"], &["a", "b", "c", "d"]), (2, 0, 1));
        assert_eq!(diff(&["a", "b"], &["a", "b"]), (2, 0, 0));
        assert_eq!(diff::<&str>(&[], &["a"]), (0, 0, 1));
        assert_eq!(diff(&["a", "b"], &[]), (0, 2, 0));
        // Prefix and suffix never overlap.
        assert_eq!(diff(&["a", "a"], &["a", "a", "a"]), (2, 0, 1));
        assert_eq!(diff(&["x", "a", "y"], &["z"]), (0, 3, 1));
    }

    #[test]
    fn a_long_queue_changes_in_a_short_range() {
        let old: Vec<u32> = (0..40_000).collect();
        let new: Vec<u32> = (1..40_000).collect();
        assert_eq!(diff(&old, &new), (0, 1, 0));
    }

    /// What GTK is told adds up to what the model has, the rows shown stay
    /// a prefix of the data, and a change past them is not seen at all.
    #[test]
    fn lazy_rows_show_a_prefix_and_report_every_change() {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        let rows = LazyRows::new(200, 40);
        let seen = Rc::new(Cell::new(0u32));
        let log: Rc<RefCell<Vec<(u32, u32, u32)>>> = Rc::default();
        let (s, l) = (seen.clone(), log.clone());
        rows.connect_items_changed(move |m, p, r, a| {
            assert!(p + r <= s.get(), "removed rows GTK never had");
            s.set(s.get() - r + a);
            assert_eq!(s.get(), m.n_items());
            l.borrow_mut().push((p, r, a));
        });
        let last = || *log.borrow().last().unwrap();

        rows.replace_all(1000);
        assert_eq!((rows.n_items(), rows.len()), (40, 1000));
        // Past what is shown: GTK sees nothing.
        rows.splice(500, 1, 2);
        assert_eq!(
            (rows.n_items(), rows.len(), log.borrow().len()),
            (40, 1001, 1)
        );
        rows.splice(10, 1, 1);
        assert_eq!(last(), (10, 1, 1));
        // Reaching past the shown rows: only those go.
        rows.splice(30, 20, 0);
        assert_eq!((last(), rows.n_items(), rows.len()), ((30, 10, 0), 30, 981));
        while rows.grow() {}
        assert_eq!(rows.n_items(), 981);
        // Fully shown: exactly the change.
        rows.splice(5, 0, 1);
        assert_eq!((last(), rows.n_items()), ((5, 0, 1), 982));
        // Many new rows at the end come a chunk at a time.
        rows.replace_all(1);
        rows.splice(0, 1, 1000);
        assert_eq!((last(), rows.n_items(), rows.len()), ((0, 1, 40), 40, 1000));
        // Taken away a chunk at a time.
        let mut steps = 1;
        while !rows.drain_step() {
            steps += 1;
        }
        assert_eq!((rows.n_items(), steps), (0, 1));
        assert_eq!(seen.get(), 0);
    }
}
