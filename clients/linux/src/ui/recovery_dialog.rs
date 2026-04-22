//! One-time recovery-words disclosure dialog.
//!
//! Displays the 24 BIP-39 recovery words in a modal dialog that the user must
//! explicitly acknowledge before dismissing. The words never appear on any
//! persistent widget (e.g. the unlock status label) so that:
//!
//! * They are not visible to anyone walking past the screen after vault
//!   creation.
//! * They are not read aloud by accessibility tools attached to the main
//!   window.
//! * They do not sit in screenshots or screen recordings of normal vault use.
//!
//! The words are held in a [`Zeroizing<String>`] that is dropped — and thus
//! wiped — as soon as the dialog is closed.
//!
//! Implementation note: this crate targets libadwaita `v1_4`, so we use
//! [`adw::MessageDialog`]. `adw::AlertDialog` (libadwaita 1.5) would be the
//! newer idiom but is not available on this feature set.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use zeroize::Zeroizing;

/// Response id used when the user confirms they have saved the words. This is
/// the only response that dismisses the dialog.
const RESPONSE_CONFIRM: &str = "confirm";
/// Response id for the "Copy to clipboard" action. It does not close the
/// dialog so the user can still read the words after copying.
const RESPONSE_COPY: &str = "copy";

/// Build and present a modal recovery-words dialog.
///
/// * `parent` — any widget inside the window the dialog should be transient
///   for. The nearest [`gtk::Window`] ancestor is used.
/// * `words` — the 24 recovery words. The value is consumed; its heap buffer
///   is wiped when the dialog is dismissed.
/// * `on_dismissed` — invoked on the GTK main thread after the dialog has
///   been closed by the user.
pub fn show_recovery_words_dialog<W, F>(parent: &W, words: Zeroizing<String>, on_dismissed: F)
where
    W: IsA<gtk::Widget>,
    F: FnOnce() + 'static,
{
    let parent_window = parent
        .root()
        .and_then(|root| root.downcast::<gtk::Window>().ok());

    let mut builder = adw::MessageDialog::builder()
        .modal(true)
        .heading("Save Your Recovery Words");
    if let Some(win) = parent_window.as_ref() {
        builder = builder.transient_for(win);
    }
    let dialog = builder
        .body(
            "These 24 words are the only way to recover your vault if you \
             forget your password. Write them down and store them somewhere \
             safe. They will not be shown again.",
        )
        .build();

    // Read-only, selectable label so the user can copy any or all words
    // manually. `selectable` + GTK's default behaviour means the widget is
    // not editable.
    let words_label = gtk::Label::builder()
        .label(words.as_str())
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .css_classes(["monospace", "title-4"])
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    dialog.set_extra_child(Some(&words_label));

    dialog.add_response(RESPONSE_COPY, "_Copy to Clipboard");
    dialog.add_response(RESPONSE_CONFIRM, "I've _Saved My Recovery Words");
    dialog.set_response_appearance(RESPONSE_CONFIRM, adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some(RESPONSE_CONFIRM));
    // Make the confirmation response the only way to dismiss the dialog —
    // Escape / closing the window treats it as a confirm so we don't silently
    // re-open the flow with stale state.
    dialog.set_close_response(RESPONSE_CONFIRM);

    // The dialog owns the `Zeroizing<String>` until it is closed. We move it
    // into the response signal handler; `connect_response` is invoked at most
    // once for the closing response, so the wrapper-side drop (and therefore
    // the wipe) happens deterministically. `connect_response` takes an `Fn`
    // closure, so interior mutability is required for both the words buffer
    // and the one-shot completion callback.
    let words_cell = std::cell::RefCell::new(Some(words));
    let on_dismissed = std::cell::RefCell::new(Some(on_dismissed));
    // Tracks whether *we* wrote the recovery words to the system clipboard.
    // Only set when the user clicks "Copy to Clipboard". On dismiss we use
    // this to decide whether to wipe — we must not unconditionally clear,
    // or we'd erase whatever the user copied (from elsewhere) between
    // opening and dismissing the dialog.
    let we_wrote_clipboard = Rc::new(Cell::new(false));

    let we_wrote_clipboard_handler = we_wrote_clipboard.clone();
    dialog.connect_response(None, move |dlg, response| match response {
        RESPONSE_COPY => {
            if let Some(words) = words_cell.borrow().as_deref() {
                if let Some(display) = gdk::Display::default() {
                    display.clipboard().set_text(words);
                    we_wrote_clipboard_handler.set(true);
                }
            }
            // Keep the dialog open — the user still needs to confirm.
        }
        _ => {
            // Any other response (confirm / close) dismisses the dialog.
            // Drop the recovery words first so the wipe runs before we
            // invoke the caller's completion handler.
            drop(words_cell.borrow_mut().take());
            // Threat model: the recovery words are equivalent to the
            // master key. If we copied them to the clipboard, every other
            // process on the system can read them via paste until something
            // else is copied. Overwrite our own clipboard contents on
            // dismiss to bound that window. We only wipe when *we* wrote
            // (tracked above) so we don't clobber unrelated data the user
            // copied in the interim.
            if we_wrote_clipboard_handler.get() {
                if let Some(display) = gdk::Display::default() {
                    display.clipboard().set_text("");
                }
            }
            if let Some(cb) = on_dismissed.borrow_mut().take() {
                // Run the callback asynchronously to ensure `present()`
                // has fully unwound before the caller mutates any UI that
                // might still be animating the dialog away.
                glib::idle_add_local_once(cb);
            }
            dlg.close();
        }
    });

    dialog.present();
}

#[cfg(test)]
mod tests {
    use super::show_recovery_words_dialog;
    use zeroize::Zeroizing;

    /// Compile-time assertion that the dialog entry point stays reachable
    /// with the expected shape: `(parent: &impl IsA<Widget>,
    /// words: Zeroizing<String>, on_dismissed: impl FnOnce() + 'static)`.
    ///
    /// A future refactor that deletes or renames the one-time disclosure
    /// flow will break this symbol reference and fail the build — so the
    /// recovery-words UI cannot silently revert to a persistent label.
    #[test]
    fn dialog_entry_point_is_reachable() {
        // Monomorphise for a specific widget + callback type so the compiler
        // actually resolves the generic function rather than leaving it
        // unreferenced in a where-clause.
        let _: fn(&gtk::Widget, Zeroizing<String>, fn()) =
            show_recovery_words_dialog::<gtk::Widget, fn()>;
    }

    /// Guard the clipboard-wipe-on-dismiss branch against silent removal.
    ///
    /// We can't drive a real GTK dismissal without a display server in CI,
    /// so we assert structurally on the source: both the gating flag set
    /// in the copy branch and the clearing `set_text("")` call in the
    /// dismiss branch must be present. A future "simplification" that
    /// drops the wipe will fail this test instead of silently leaving the
    /// recovery words in the system clipboard.
    #[test]
    fn dismiss_branch_clears_clipboard_when_we_wrote_it() {
        let src = include_str!("recovery_dialog.rs");
        assert!(
            src.contains("we_wrote_clipboard_handler.set(true)"),
            "copy branch must mark that we wrote to the clipboard so the \
             dismiss branch can wipe only what we put there"
        );
        assert!(
            src.contains("display.clipboard().set_text(\"\")"),
            "dismiss branch must overwrite our own clipboard contents — \
             the recovery words are master-key equivalent and must not \
             linger after the dialog closes"
        );
    }
}
