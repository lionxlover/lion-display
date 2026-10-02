//! Stacking and focus policy: the shell's window layers.
//!
//! Distinct from the compositor's per-parent stacking-list scene
//! order (ldp-compositor), this is the *policy* layer:
//! per-space back-to-front ordering of toplevels, the focus policy
//! with modal gating, and activation attribution — the *cause* of
//! every focus change is recorded for a11y and audit (architecture
//! §8). The integrator maps policy decisions onto the scene graph.
//!
//! Layering within one space (back to front):
//!
//! 1. normal toplevels (their [`crate::stack::WindowStack`] order),
//! 2. modeless dialogs (above their parent),
//! 3. modal dialogs (above their parent's tree),
//! 4. popups (grab-held, always front).
//!
//! The stack tracks (1)–(3); popups are grab-scoped and arrive with
//! the seat layer's grab model — the shell only never stacks them
//! *below* their parent, which [`WindowStack::add_dialog`]'s raise
//! guarantees structurally.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::dialog::DialogModality;
use crate::spaces::Spaces;
use crate::WindowKey;

/// Why a window gained (or lost) focus — recorded per activation for
/// a11y announcements and the audit chain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActivationCause {
    /// The user clicked the window.
    PointerClick,
    /// An Alt-Tab-style focus switch.
    KeyboardShortcut,
    /// The workspace switched and the window is topmost there.
    WorkspaceSwitch,
    /// A shell command (window list, launch activation).
    ShellCommand,
    /// A modal dialog opened over the window.
    DialogOpened,
    /// The window was restored from minimized.
    Restore,
    /// An assistive-technology driven activation.
    A11y,
}

impl ActivationCause {
    /// Whether the cause was a direct user action (a11y-sensitive:
    /// these are announced as "user" activations).
    #[must_use]
    pub const fn is_user(self) -> bool {
        matches!(
            self,
            ActivationCause::PointerClick | ActivationCause::KeyboardShortcut
        )
    }
}

/// One recorded activation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Activation {
    /// The window activated.
    pub window: WindowKey,
    /// The cause.
    pub cause: ActivationCause,
}

/// Back-to-front toplevel ordering per space, with dialog layering and
/// focus.
///
/// Invariants maintained by construction:
///
/// * a window appears in exactly one space's stack (its home space in
///   the [`Spaces`] model — sticky windows appear in *their* space's
///   stack and are *rendered* on every space; re-stacking policy
///   applies once),
/// * dialogs always sit above their parent,
/// * the keyboard focus is a stack member or `None`,
/// * destroying a window removes it (and its dialogs) from every
///   structure.
#[derive(Clone, Debug, Default)]
pub struct WindowStack {
    /// Space → back-to-front toplevel order.
    per_space: HashMap<u32, Vec<WindowKey>>,
    /// Dialog → parent (and modality).
    dialogs: HashMap<WindowKey, (WindowKey, DialogModality)>,
    /// Parent → open dialogs (front-most last).
    dialogs_of: HashMap<WindowKey, Vec<WindowKey>>,
    /// MRU activation order across all spaces (front = most recent).
    mru: Vec<WindowKey>,
    /// The keyboard focus.
    focus: Option<WindowKey>,
    /// Activation log (bounded, most recent last).
    log: Vec<Activation>,
}

/// The activation log bound (a11y needs the recent tail, not history).
const LOG_BOUND: usize = 64;

impl WindowStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> WindowStack {
        WindowStack::default()
    }

    /// A space's back-to-front order.
    #[must_use]
    pub fn order_on(&self, space: u32) -> &[WindowKey] {
        self.per_space.get(&space).map_or(&[], Vec::as_slice)
    }

    /// The keyboard focus.
    #[must_use]
    pub fn focus(&self) -> Option<WindowKey> {
        self.focus
    }

    /// The MRU order (most recent first).
    #[must_use]
    pub fn mru(&self) -> &[WindowKey] {
        &self.mru
    }

    /// The activation log (most recent last).
    #[must_use]
    pub fn log(&self) -> &[Activation] {
        &self.log
    }

    /// Map a window (raising into its home space) at the front and
    /// register it as known (least-recently-used until first
    /// activated).
    pub fn add(&mut self, spaces: &Spaces, window: WindowKey) {
        if let Ok(space) = spaces.space_of(window) {
            self.per_space.entry(space).or_default().push(window);
        }
        if !self.mru.contains(&window) {
            self.mru.push(window);
        }
    }

    /// Follow a `set_workspace` move: re-file the window into its new
    /// home space at the front of that space's stack.
    pub fn move_window(&mut self, spaces: &Spaces, window: WindowKey) {
        for order in self.per_space.values_mut() {
            order.retain(|w| *w != window);
        }
        if let Ok(space) = spaces.space_of(window) {
            self.per_space.entry(space).or_default().push(window);
        }
    }

    /// Rebuild the per-space filing after a bulk assignment change
    /// (space-count reflow). Relative back-to-front order among the
    /// survivors is preserved; cross-space merges resolve by ascending
    /// old-space index, so the rebuild is fully deterministic.
    pub fn reflow(&mut self, spaces: &Spaces) {
        let old: std::collections::BTreeMap<u32, Vec<WindowKey>> =
            std::mem::take(&mut self.per_space).into_iter().collect();
        let mut new: HashMap<u32, Vec<WindowKey>> = HashMap::new();
        for order in old.values() {
            for w in order {
                if let Ok(space) = spaces.space_of(*w) {
                    new.entry(space).or_default().push(*w);
                }
            }
        }
        self.per_space = new;
    }

    /// Remove a window everywhere: from its space stack, the MRU list,
    /// the focus, and the dialog graph (its dialogs go too). Returns
    /// whether the focus was held by the window or any of its removed
    /// dialogs (the integrator must re-focus).
    #[must_use]
    pub fn remove(&mut self, window: WindowKey) -> bool {
        for order in self.per_space.values_mut() {
            order.retain(|w| *w != window);
        }
        self.mru.retain(|w| *w != window);
        let mut held_focus = self.focus == Some(window);
        if held_focus {
            self.focus = None;
        }
        // Its dialogs (if it was a parent) are removed with it.
        if let Some(children) = self.dialogs_of.remove(&window) {
            for d in children {
                held_focus |= self.remove_dialog_only(d);
            }
        }
        // If it was itself a dialog, detach it.
        held_focus |= self.remove_dialog_only(window);
        held_focus
    }

    fn remove_dialog_only(&mut self, dialog: WindowKey) -> bool {
        if let Some((parent, _)) = self.dialogs.remove(&dialog) {
            if let Some(v) = self.dialogs_of.get_mut(&parent) {
                v.retain(|d| *d != dialog);
            }
            self.mru.retain(|w| *w != dialog);
            if self.focus == Some(dialog) {
                self.focus = None;
                return true;
            }
        }
        false
    }

    /// Register a dialog above its parent and focus it (`DialogOpened`
    /// attribution). The dialog stacks with the parent's space.
    pub fn add_dialog(&mut self, parent: WindowKey, dialog: WindowKey, modality: DialogModality) {
        self.dialogs.insert(dialog, (parent, modality));
        self.dialogs_of.entry(parent).or_default().push(dialog);
        if !self.mru.contains(&dialog) {
            self.mru.push(dialog);
        }
        self.activate(dialog, ActivationCause::DialogOpened);
    }

    /// Close a dialog: detach it; if it held focus, focus falls back to
    /// the parent (Restore attribution).
    pub fn close_dialog(&mut self, dialog: WindowKey) {
        if let Some((parent, _)) = self.dialogs.remove(&dialog) {
            if let Some(v) = self.dialogs_of.get_mut(&parent) {
                v.retain(|d| *d != dialog);
            }
            self.mru.retain(|w| *w != dialog);
            if self.focus == Some(dialog) {
                self.activate(parent, ActivationCause::Restore);
            }
        }
    }

    /// Whether `window` is gated (input may not reach it): a live modal
    /// dialog is open over it.
    #[must_use]
    pub fn is_gated(&self, window: WindowKey) -> bool {
        self.dialogs_of.get(&window).is_some_and(|ds| {
            ds.iter().any(|d| {
                self.dialogs
                    .get(d)
                    .is_some_and(|(_, m)| *m == DialogModality::Modal)
            })
        })
    }

    /// Whether focusing `window` is allowed: not gated itself, and not
    /// a modeless sibling under an open modal dialog of the same
    /// parent (modal gates the *whole* parent tree).
    #[must_use]
    pub fn can_focus(&self, window: WindowKey) -> bool {
        if self.is_gated(window) {
            return false;
        }
        if let Some((parent, _)) = self.dialogs.get(&window) {
            // A sibling dialog is blocked while a modal dialog of the
            // same parent is open (the dialog itself is fine).
            if self.is_gated(*parent) {
                return false;
            }
        }
        true
    }

    /// Activate a window: raise it to its space's front, move it to
    /// the MRU head, make it the keyboard focus, record the cause.
    /// Unknown (never-added) windows are ignored — a spurious
    /// activation from a late input event must not corrupt the stack.
    pub fn activate(&mut self, window: WindowKey, cause: ActivationCause) {
        if !self.tracked(window) {
            return;
        }
        // Raise in its space.
        for order in self.per_space.values_mut() {
            if order.contains(&window) {
                order.retain(|w| *w != window);
                order.push(window);
            }
        }
        // MRU head.
        self.mru.retain(|w| *w != window);
        self.mru.insert(0, window);
        self.focus = Some(window);
        self.log.push(Activation { window, cause });
        if self.log.len() > LOG_BOUND {
            self.log.remove(0);
        }
    }

    /// Focus follows a non-activating gesture: set the focus without
    /// raising (unknown ids ignored).
    pub fn focus_keyboard(&mut self, window: Option<WindowKey>) {
        if let Some(w) = window {
            if !self.tracked(w) {
                return;
            }
        }
        self.focus = window;
    }

    fn tracked(&self, window: WindowKey) -> bool {
        self.mru.contains(&window)
    }

    /// The rendering order for a space: normal toplevels, then
    /// modeless dialogs, then modal dialogs (each group in stack
    /// order; within dialogs, the most recently opened is frontmost).
    #[must_use]
    pub fn render_order(&self, space: u32) -> Vec<WindowKey> {
        let mut v: Vec<WindowKey> = self.order_on(space).to_vec();
        let mut modeless: Vec<WindowKey> = Vec::new();
        let mut modal: Vec<WindowKey> = Vec::new();
        for (d, (_p, m)) in &self.dialogs {
            if self.dialog_space(*d) == Some(space) {
                if *m == DialogModality::Modal {
                    modal.push(*d);
                } else {
                    modeless.push(*d);
                }
            }
        }
        v.extend(modeless);
        v.extend(modal);
        v
    }

    fn dialog_space(&self, dialog: WindowKey) -> Option<u32> {
        self.dialogs.get(&dialog).and_then(|(p, _)| {
            self.per_space
                .iter()
                .find(|(_, order)| order.contains(p))
                .map(|(s, _)| *s)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::Spaces;

    fn k(n: u64) -> WindowKey {
        WindowKey::new(n)
    }

    fn two_spaces() -> (Spaces, WindowStack) {
        let mut sp = Spaces::new(2);
        sp.add_seat(1, 0);
        sp.assign(k(1), 0);
        sp.assign(k(2), 0);
        sp.assign(k(3), 1);
        let mut st = WindowStack::new();
        st.add(&sp, k(1));
        st.add(&sp, k(2));
        st.add(&sp, k(3));
        (sp, st)
    }

    #[test]
    fn activate_raises_and_focuses_with_attribution() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(2), ActivationCause::PointerClick);
        assert_eq!(st.focus(), Some(k(2)));
        assert_eq!(st.order_on(0), &[k(1), k(2)]);
        assert_eq!(st.mru(), &[k(2), k(1), k(3)]);
        assert_eq!(
            st.log(),
            &[Activation {
                window: k(2),
                cause: ActivationCause::PointerClick
            }]
        );
        assert!(ActivationCause::PointerClick.is_user());
        assert!(!ActivationCause::ShellCommand.is_user());
    }

    #[test]
    fn unknown_activations_are_ignored() {
        let (_, mut st) = two_spaces();
        st.activate(k(99), ActivationCause::ShellCommand);
        assert_eq!(st.focus(), None);
        assert!(st.log().is_empty());
        st.focus_keyboard(Some(k(99)));
        assert_eq!(st.focus(), None);
        st.focus_keyboard(None);
        assert_eq!(st.focus(), None);
    }

    #[test]
    fn modal_dialog_gates_the_parent_tree() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(1), ActivationCause::ShellCommand);
        st.add_dialog(k(1), k(10), DialogModality::Modal);
        // The dialog took focus.
        assert_eq!(st.focus(), Some(k(10)));
        // The parent is gated; focus cannot return to it.
        assert!(st.is_gated(k(1)));
        assert!(!st.can_focus(k(1)));
        // Unrelated windows remain focusable.
        assert!(st.can_focus(k(2)));
        // Closing the dialog restores the parent.
        st.close_dialog(k(10));
        assert!(!st.is_gated(k(1)));
        assert!(st.can_focus(k(1)));
        assert_eq!(st.focus(), Some(k(1)));
        assert_eq!(
            st.log().last(),
            Some(&Activation {
                window: k(1),
                cause: ActivationCause::Restore
            })
        );
    }

    #[test]
    fn modeless_dialog_does_not_gate() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(1), ActivationCause::ShellCommand);
        st.add_dialog(k(1), k(11), DialogModality::Modeless);
        assert!(!st.is_gated(k(1)));
        assert!(st.can_focus(k(1)));
        assert!(st.can_focus(k(11)));
    }

    #[test]
    fn render_order_layers_dialogs_above_windows() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(2), ActivationCause::WorkspaceSwitch);
        st.add_dialog(k(1), k(11), DialogModality::Modeless);
        st.add_dialog(k(2), k(10), DialogModality::Modal);
        // Space 0: normal [1, 2], then modeless dialogs [11], then
        // modal dialogs [10].
        assert_eq!(st.render_order(0), vec![k(1), k(2), k(11), k(10)]);
        // Space 1 untouched.
        assert_eq!(st.render_order(1), vec![k(3)]);
    }

    #[test]
    fn remove_takes_windows_and_their_dialogs() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(1), ActivationCause::ShellCommand);
        st.add_dialog(k(1), k(10), DialogModality::Modal);
        assert!(st.remove(k(1)));
        // The dialog went with the parent.
        assert!(!st.is_gated(k(1)));
        assert_eq!(st.focus(), None);
        // k2 survives on space 0.
        assert_eq!(st.order_on(0), &[k(2)]);
        // The dialog itself is gone from everywhere.
        st.activate(k(10), ActivationCause::ShellCommand); // ignored
        assert_eq!(st.focus(), None);
        // Removing a dialog alone leaves the parent focused path
        // intact.
        st.activate(k(2), ActivationCause::ShellCommand);
        st.add_dialog(k(2), k(12), DialogModality::Modal);
        assert!(st.remove(k(12)));
        assert_eq!(st.focus(), None);
    }

    #[test]
    fn activation_log_is_bounded() {
        let (sp, mut st) = two_spaces();
        st.add(&sp, WindowKey::new(50));
        for i in 0..100 {
            let w = if i % 2 == 0 { k(1) } else { k(2) };
            st.activate(w, ActivationCause::KeyboardShortcut);
        }
        assert_eq!(st.log().len(), 64);
        // The newest entry is the last activation.
        assert_eq!(
            st.log().last().map(|a| a.cause),
            Some(ActivationCause::KeyboardShortcut)
        );
    }

    #[test]
    fn focus_keyboard_sets_without_raising() {
        let (_sp, mut st) = two_spaces();
        st.activate(k(1), ActivationCause::ShellCommand);
        st.focus_keyboard(Some(k(2)));
        assert_eq!(st.focus(), Some(k(2)));
        // No raise happened: k1 stays front-most (last in the
        // back-to-front order).
        assert_eq!(st.order_on(0), &[k(2), k(1)]);
        assert_eq!(st.mru()[0], k(1));
    }
}
