//! Agent-owned presenter highlight state.
//!
//! The Spotlight's front button is detected as a discrete toggle press (see
//! `openlogi_device::presenter` for why), not a hold — so unlike the Actions
//! Ring's per-invocation session, there is nothing here to open and close
//! around one gesture. [`HighlightSession`] is just the current on/off state,
//! flipped by [`HighlightSession::toggle`] each time
//! [`watchers::presenter`](crate::watchers) reports a press, and observed by
//! whoever needs to react to it (the IPC layer, eventually) via
//! [`HighlightSession::watch`]'s `tokio::sync::watch` receiver — the same
//! "current value plus wake on change" shape an IPC long-poll needs, for
//! free.

use openlogi_core::binding::HighlightMode;
use tokio::sync::watch;

/// The highlight's current on-screen state, when it is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightState {
    /// Which effect is showing.
    pub mode: HighlightMode,
}

/// Agent-owned toggle state for the on-screen presenter highlight.
///
/// `None` means hidden. There is exactly one of these per agent run — the
/// Spotlight (and this feature) supports one active presenter at a time.
pub struct HighlightSession {
    state: watch::Sender<Option<HighlightState>>,
}

impl Default for HighlightSession {
    fn default() -> Self {
        Self {
            state: watch::channel(None).0,
        }
    }
}

impl HighlightSession {
    /// Flip the highlight: hidden → showing `mode`, showing (in any mode) →
    /// hidden. A press while a *different* mode is already showing hides it
    /// rather than switching modes — this device has only the one button to
    /// press, so "toggle" is the only gesture available; changing modes
    /// belongs to a future double-click (P.S. not implemented here).
    pub fn toggle(&self, mode: HighlightMode) {
        self.state.send_modify(|current| {
            *current = match current {
                Some(_) => None,
                None => Some(HighlightState { mode }),
            };
        });
    }

    /// The current state, without subscribing to further changes.
    #[must_use]
    pub fn current(&self) -> Option<HighlightState> {
        *self.state.borrow()
    }

    /// Subscribe to every future change, starting from the current value.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Option<HighlightState>> {
        self.state.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_shows_the_pressed_mode_from_hidden() {
        let session = HighlightSession::default();
        session.toggle(HighlightMode::Highlight);
        assert_eq!(
            session.current(),
            Some(HighlightState {
                mode: HighlightMode::Highlight
            })
        );
    }

    #[test]
    fn a_second_press_hides_it_regardless_of_mode_argument() {
        let session = HighlightSession::default();
        session.toggle(HighlightMode::Highlight);
        session.toggle(HighlightMode::Spotlight);
        assert_eq!(session.current(), None);
    }

    #[test]
    fn watch_observes_the_toggle() {
        let session = HighlightSession::default();
        let mut watcher = session.watch();
        assert_eq!(*watcher.borrow(), None);
        session.toggle(HighlightMode::Spotlight);
        assert!(watcher.has_changed().unwrap_or(false));
        assert_eq!(
            *watcher.borrow_and_update(),
            Some(HighlightState {
                mode: HighlightMode::Spotlight
            })
        );
    }
}
