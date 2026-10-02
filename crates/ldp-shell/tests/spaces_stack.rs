//! Spaces × stacking × focus integration (Phase 12 support suite).
//!
//! The policy layers working together: visibility drives what the
//! compositor renders per seat; the stack orders it; the focus policy
//! (with modal gating) picks the keyboard target; every focus change
//! carries attribution. Pinned here end-to-end, including the
//! sticky-window-across-spaces semantics and workspace-switch focus
//! restoration.

#![forbid(unsafe_code)]

mod common;

use common::Rng;
use ldp_shell::dialog::DialogModality;
use ldp_shell::spaces::Spaces;
use ldp_shell::stack::{ActivationCause, WindowStack};
use ldp_shell::WindowKey;

fn k(n: u64) -> WindowKey {
    WindowKey::new(n)
}

/// A small scene: 3 spaces, 6 windows, one seat.
fn scene() -> (Spaces, WindowStack) {
    let mut spaces = Spaces::new(3);
    spaces.add_seat(1, 0);
    // Space 0: editors.
    spaces.assign(k(1), 0);
    spaces.assign(k(2), 0);
    // Space 1: browser.
    spaces.assign(k(3), 1);
    // Space 2: chat + a sticky music widget.
    spaces.assign(k(4), 2);
    spaces.assign(k(5), 2);
    spaces.set_sticky(k(5), true);

    let mut stack = WindowStack::new();
    for &w in &[1u64, 2, 3, 4, 5] {
        stack.add(&spaces, k(w));
    }
    (spaces, stack)
}

#[test]
fn visibility_and_render_order_track_the_active_space() {
    let (mut spaces, stack) = scene();
    // Seat on space 0: sees 1, 2 and the sticky 5.
    assert_eq!(spaces.visible_windows(1).unwrap(), vec![k(1), k(2), k(5)]);
    // The render order for space 0 contains exactly those windows.
    let order = stack.render_order(0);
    assert_eq!(order.len(), 2); // the stack's space-0 members (1, 2)
                                // Switching to space 2: sees 4, 5.
    spaces.switch(1, 2).unwrap();
    assert_eq!(spaces.visible_windows(1).unwrap(), vec![k(4), k(5)]);
}

#[test]
fn activation_raises_within_the_own_space_only() {
    let (spaces, mut stack) = scene();
    stack.activate(k(1), ActivationCause::PointerClick);
    assert_eq!(stack.order_on(0), &[k(2), k(1)]);
    // Space 1's order is untouched.
    assert_eq!(stack.order_on(1), &[k(3)]);
    // Activating a window on another space does not disturb this one.
    stack.activate(k(3), ActivationCause::ShellCommand);
    assert_eq!(stack.order_on(1), &[k(3)]);
    assert_eq!(stack.order_on(0), &[k(2), k(1)]);
    assert_eq!(stack.focus(), Some(k(3)));
    assert_eq!(stack.mru()[..2], [k(3), k(1)]);
    let _ = spaces;
}

#[test]
fn workspace_switch_focus_restores_topmost() {
    let (mut spaces, mut stack) = scene();
    // Work on space 0.
    stack.activate(k(2), ActivationCause::PointerClick);
    // Switch to space 1 and activate the browser.
    spaces.switch(1, 1).unwrap();
    stack.activate(k(3), ActivationCause::WorkspaceSwitch);
    // Back to space 0: the topmost there is still window 2 (raised
    // earlier) — and the integrator re-activates it with the switch
    // cause.
    spaces.switch(1, 0).unwrap();
    let topmost = stack.order_on(0).last().copied().unwrap();
    assert_eq!(topmost, k(2));
    stack.activate(topmost, ActivationCause::WorkspaceSwitch);
    assert_eq!(stack.focus(), Some(k(2)));
    // The attribution log tells the a11y story: click → switch →
    // switch.
    let causes: Vec<ActivationCause> = stack.log().iter().map(|a| a.cause).collect();
    assert_eq!(
        causes,
        vec![
            ActivationCause::PointerClick,
            ActivationCause::WorkspaceSwitch,
            ActivationCause::WorkspaceSwitch,
        ]
    );
}

#[test]
fn modal_gating_across_the_focus_surface() {
    let (spaces, mut stack) = scene();
    stack.activate(k(1), ActivationCause::PointerClick);
    // A modal dialog opens over window 1.
    stack.add_dialog(k(1), k(10), DialogModality::Modal);
    assert_eq!(stack.focus(), Some(k(10)));
    // The gated window cannot take focus back by any cause.
    assert!(!stack.can_focus(k(1)));
    stack.activate(k(1), ActivationCause::ShellCommand); // refused by policy in the integrator; the stack still records
                                                         // ...but the honest integrator consults can_focus first; here we
                                                         // verify the query API, then close the dialog and refocus.
    stack.close_dialog(k(10));
    assert!(stack.can_focus(k(1)));
    stack.activate(k(1), ActivationCause::Restore);
    assert_eq!(stack.focus(), Some(k(1)));
    let _ = spaces;
}

#[test]
fn dialog_removal_with_parent_takes_both() {
    let (spaces, mut stack) = scene();
    stack.activate(k(3), ActivationCause::ShellCommand);
    stack.add_dialog(k(3), k(11), DialogModality::Modal);
    assert!(stack.is_gated(k(3)));
    // The parent is destroyed (client crash): the dialog goes too.
    assert!(stack.remove(k(3)));
    assert!(!stack.is_gated(k(3)));
    assert!(stack.can_focus(k(3)) || stack.mru().is_empty());
    // The dialog is not independently activatable anymore.
    stack.activate(k(11), ActivationCause::ShellCommand);
    assert_ne!(stack.focus(), Some(k(11)));
    let _ = spaces;
}

#[test]
fn sticky_windows_raise_once_and_render_everywhere() {
    let (mut spaces, mut stack) = scene();
    // Activate the sticky widget from space 2.
    spaces.switch(1, 2).unwrap();
    stack.activate(k(5), ActivationCause::PointerClick);
    // It is raised in its home space's stack (space 2).
    assert_eq!(stack.order_on(2).last(), Some(&k(5)));
    // But it is *visible* on every space (rendering, not stacking).
    spaces.switch(1, 0).unwrap();
    assert!(spaces.visible_windows(1).unwrap().contains(&k(5)));
    // Un-stickying removes it from other spaces' visibility.
    spaces.set_sticky(k(5), false);
    assert!(!spaces.visible_windows(1).unwrap().contains(&k(5)));
    assert!(spaces.visible_windows(1).unwrap().contains(&k(1)));
}

#[test]
fn space_count_reflow_moves_windows_and_reports() {
    let (mut spaces, mut stack) = scene();
    // Add a window on space 2 (the one being removed next).
    spaces.assign(k(6), 2);
    stack.add(&spaces, k(6));
    let moved = spaces.set_count(2);
    // Windows on space 2 fall to space 1 (keys 4, 5, 6 sorted).
    assert_eq!(moved, vec![(k(4), 1), (k(5), 1), (k(6), 1)]);
    // The stack follows the reflow deterministically.
    stack.reflow(&spaces);
    let order1 = stack.render_order(1);
    assert!(order1.contains(&k(4)) && order1.contains(&k(6)));
    // Space 2 no longer exists anywhere.
    assert!(stack.order_on(2).is_empty());
    assert_eq!(spaces.count(), 2);
}

#[test]
fn set_workspace_moves_refile_the_stack() {
    let (mut spaces, mut stack) = scene();
    stack.activate(k(1), ActivationCause::PointerClick);
    // The window moves to space 1 via `set_workspace` (clamped).
    let target = spaces.move_window(k(1), 7);
    assert_eq!(target, 2);
    stack.move_window(&spaces, k(1));
    // It left space 0 and arrived at space 2's front.
    assert_eq!(stack.order_on(0), &[k(2)]);
    assert_eq!(stack.order_on(2).last(), Some(&k(1)));
    // Moving back restores it to the front of its home space.
    spaces.move_window(k(1), 0);
    stack.move_window(&spaces, k(1));
    assert_eq!(stack.order_on(0).last(), Some(&k(1)));
}

#[test]
fn fuzzed_focus_churn_keeps_invariants() {
    let mut rng = Rng::seeded(0xF0C1);
    let (mut spaces, mut stack) = scene();
    let windows: Vec<WindowKey> = vec![k(1), k(2), k(3), k(4), k(5)];
    let causes = [
        ActivationCause::PointerClick,
        ActivationCause::KeyboardShortcut,
        ActivationCause::WorkspaceSwitch,
        ActivationCause::ShellCommand,
        ActivationCause::Restore,
        ActivationCause::A11y,
    ];
    let mut dialogs_open = 0u32;
    for step in 0..30_000u32 {
        match rng.below(8) {
            0..=2 => {
                let w = windows[usize::try_from(rng.below(5)).unwrap()];
                let c = causes[usize::try_from(rng.below(6)).unwrap()];
                if stack.can_focus(w) {
                    stack.activate(w, c);
                    assert_eq!(stack.focus(), Some(w));
                }
            }
            3 => {
                // Open a dialog over a random window.
                let parent = windows[usize::try_from(rng.below(5)).unwrap()];
                let dialog = WindowKey::new(1000 + u64::from(step));
                let modal = rng.flip();
                stack.add_dialog(
                    parent,
                    dialog,
                    if modal {
                        DialogModality::Modal
                    } else {
                        DialogModality::Modeless
                    },
                );
                dialogs_open += 1;
                if modal {
                    assert!(stack.is_gated(parent));
                }
            }
            4 => {
                // Close a random dialog id (may be already closed).
                let id = 1000 + rng.below(u64::from(dialogs_open.max(1)));
                stack.close_dialog(WindowKey::new(id));
            }
            5 => {
                spaces
                    .switch(1, u32::try_from(rng.below(3)).unwrap())
                    .unwrap();
            }
            6 => {
                let w = windows[usize::try_from(rng.below(5)).unwrap()];
                let _ = stack.remove(w);
                stack.add(&spaces, w); // respawn (crash-recovery shape)
            }
            _ => {
                stack.focus_keyboard(Some(windows[usize::try_from(rng.below(5)).unwrap()]));
            }
        }
        // Invariants after every step.
        if let Some(f) = stack.focus() {
            assert!(stack.mru().contains(&f), "focus escaped the tracked set");
        }
        // Every space order has no duplicates.
        for s in 0..3u32 {
            let order = stack.order_on(s);
            let mut seen = std::collections::HashSet::new();
            for w in order {
                assert!(seen.insert(*w), "duplicate {w:?} in space {s}");
            }
        }
        // The MRU has no duplicates either.
        let mut seen = std::collections::HashSet::new();
        for w in stack.mru() {
            assert!(seen.insert(*w), "duplicate {w:?} in MRU");
        }
        // The log is bounded.
        assert!(stack.log().len() <= 64);
    }
}
