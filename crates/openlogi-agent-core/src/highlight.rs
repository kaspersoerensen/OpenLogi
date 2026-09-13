//! Agent-owned presenter highlight state.
//!
//! The Spotlight's front button is detected as a discrete toggle press (see
//! `openlogi_device::presenter` for why), not a hold — so unlike the Actions
//! Ring's per-invocation session, there is nothing here to open and close
//! around one gesture. [`HighlightSession`] is just the current on/off state,
//! flipped by [`HighlightSession::toggle`] each time
//! [`watchers::presenter`](crate::watchers) reports a press, and served to
//! [`Agent::observe_highlight`](openlogi_ipc::Agent::observe_highlight) by
//! [`HighlightSession::observe`] — the same generation-stamped long-poll
//! shape [`crate::action_ring::ActionRingManager::observe`] serves for the
//! Actions Ring, over the highlight's own cell.

use openlogi_core::binding::HighlightMode;
use openlogi_ipc::{Generation, HighlightObservation, HighlightState, OBSERVE_HOLD};
use tokio::sync::watch;

/// Agent-owned toggle state for the on-screen presenter highlight.
///
/// There is exactly one of these per agent run — the Spotlight (and this
/// feature) supports one active presenter at a time.
pub struct HighlightSession {
    /// What the overlay observes. Derived after every [`Self::toggle`], so
    /// "hidden" is simply the absence of an active mode — there is no closed
    /// message to invent, and an overlay that restarts mid-highlight reads
    /// the live state instead of having missed the toggle.
    published: watch::Sender<HighlightObservation>,
}

impl Default for HighlightSession {
    fn default() -> Self {
        // Generation 1: 0 is the observer's "seen nothing" sentinel.
        let (published, _) = watch::channel(HighlightObservation {
            generation: 1,
            active: None,
        });
        Self { published }
    }
}

impl HighlightSession {
    /// Flip the highlight: hidden → showing `mode`, showing (in any mode) →
    /// hidden. A press while a *different* mode is already showing hides it
    /// rather than switching modes — this device has only the one button to
    /// press, so "toggle" is the only gesture available; changing modes
    /// belongs to a future double-click (not implemented here).
    pub fn toggle(&self, mode: HighlightMode) {
        self.published.send_modify(|observed| {
            observed.active = match observed.active {
                Some(_) => None,
                None => Some(HighlightState { mode }),
            };
            observed.generation += 1;
        });
    }

    /// Serve one [`Agent::observe_highlight`](openlogi_ipc::Agent::observe_highlight).
    pub async fn observe(&self, since: Generation) -> HighlightObservation {
        let mut rx = self.published.subscribe();
        let changed = rx.wait_for(|observed| observed.generation != since);
        match tokio::time::timeout(OBSERVE_HOLD, changed).await {
            Ok(Ok(observed)) => *observed,
            // Hold elapsed, or the manager is gone: answer with what we have.
            Ok(Err(_)) | Err(_) => *self.published.borrow(),
        }
    }

    /// The current state, without waiting for a change. Used by tests and by
    /// anything that only needs a snapshot, not a long-poll.
    #[must_use]
    pub fn current(&self) -> Option<HighlightState> {
        self.published.borrow().active
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

    #[tokio::test]
    async fn observe_returns_immediately_for_a_stale_generation() {
        let session = HighlightSession::default();
        session.toggle(HighlightMode::Spotlight);
        let observed = tokio::time::timeout(Duration::from_millis(50), session.observe(1))
            .await
            .expect("a changed generation must not wait out the hold");
        assert_eq!(
            observed.active,
            Some(HighlightState {
                mode: HighlightMode::Spotlight
            })
        );
        assert_eq!(observed.generation, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn observe_holds_and_then_answers_unchanged() {
        let session = HighlightSession::default();
        // Already at generation 1 with nothing to report — the call must not
        // return before OBSERVE_HOLD elapses, and then answers with the same
        // (unchanged) state as a liveness heartbeat.
        let observed = session.observe(1).await;
        assert_eq!(observed.generation, 1);
        assert_eq!(observed.active, None);
    }

    #[tokio::test]
    async fn a_toggle_wakes_a_pending_observe() {
        let session = HighlightSession::default();
        let observe = session.observe(1);
        tokio::pin!(observe);
        assert!(
            futures_lite::future::poll_once(&mut observe)
                .await
                .is_none(),
            "nothing has changed yet"
        );
        session.toggle(HighlightMode::Highlight);
        let observed = tokio::time::timeout(Duration::from_millis(50), observe)
            .await
            .expect("the toggle above must wake the pending observe");
        assert_eq!(
            observed.active,
            Some(HighlightState {
                mode: HighlightMode::Highlight
            })
        );
    }
}
