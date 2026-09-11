//! Presenter click-to-toggle detection from a plain HID mouse report.
//!
//! The Logitech Spotlight does not honor `0x1b04` control diversion over
//! Bluetooth LE at all (verified on hardware — see the notes on issue #279):
//! diverting any of its control IDs, plain or raw-XY, produces zero
//! `divertedButtonsEvent`/`rawXYEvent` notifications, even while the front
//! button is actively held. Cross-checked against a known-good device (an MX
//! Master 3S's real gesture-button CID) on the identical transport ruled out
//! a general BLE bug, and cross-referencing the Projecteur project's own
//! reverse-engineered protocol notes confirmed we tested the *documented*
//! mechanism (the same CIDs, the same `DivertedButtonEvent`/
//! `DivertedRawXYEvent` functions) — it simply doesn't fire over BLE. This is
//! a firmware limitation specific to the Spotlight, almost certainly gated to
//! its dedicated eQUAD receiver.
//!
//! A hold-to-point gesture is *also* unreliable to detect passively: the
//! Spotlight also exposes an ordinary HID mouse collection (`usage_page=
//! 0x0001`, `usage_id=0x0002`) whose reports never arrive at all while idle,
//! but whether it keeps emitting zero-motion "heartbeat" reports while held
//! genuinely still turned out to depend on how still the hand actually was —
//! confirmed on hardware both ways, including total silence for 6+ seconds
//! during a deliberately steady hold. There is no bit anywhere in this report
//! that means "held, not moving" as opposed to "released"; Logi Options+
//! clearly has a real signal for this (indefinite still-hold tolerance,
//! zero-lag release) but it is almost certainly reading the proprietary
//! `0x1a01 Sensor3D` feature, which — per Projecteur, a project dedicated to
//! reverse-engineering this exact device — has never been documented by
//! anyone. Reproducing it would mean capturing genuine Bluetooth HID++
//! traffic from Options+ itself; out of reach here.
//!
//! What *is* reliable, using the same plain-HID report: the button bit is
//! clear for the *entire* duration of a hold-and-point gesture (confirmed
//! repeatedly on hardware) and is only ever set by a genuine, discrete click.
//! So a click and a hold-to-point gesture can never be confused with each
//! other at this byte, and detecting "the button was just pressed" needs no
//! timeout or debounce heuristics at all — a plain rising-edge check on the
//! button bit is exactly correct. [`PresenterTapState`] uses that as a
//! **toggle**: one click turns the highlight on, the next turns it off,
//! sidestepping the unsolvable "is this still held" question entirely rather
//! than approximating it.
//!
//! This module is the sans-I/O decision half of that — no time, no host
//! dependency, exercised entirely with synthetic byte sequences. The
//! host-specific read loop that feeds it lives in `openlogi_hid`.

/// Byte offset of the button bitmask in the tapped report — the only byte
/// this decoder reads, so it also doubles as the minimum report length.
const BUTTONS_OFFSET: usize = 1;

/// Sans-I/O decision object: turns a stream of raw HID mouse reports into
/// toggle-press edges. One instance per tap; the only state is whether the
/// button bit was set on the previous report, so a rising edge can be told
/// apart from every report while the button stays held.
#[derive(Debug, Default)]
pub struct PresenterTapState {
    was_pressed: bool,
}

impl PresenterTapState {
    /// Feed one raw input report. Returns `true` exactly on the button bit's
    /// 0→1 transition — the instant a real click begins — and `false` for
    /// every other report, including every one of a hold-and-point gesture
    /// (whose button bit never sets at all) and every report after the first
    /// while a click is still physically pressed.
    ///
    /// A report too short to carry a button byte is inert.
    pub fn observe(&mut self, report: &[u8]) -> bool {
        let Some(&buttons) = report.get(BUTTONS_OFFSET) else {
            return false;
        };
        let pressed = buttons != 0;
        let rising_edge = pressed && !self.was_pressed;
        self.was_pressed = pressed;
        rising_edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report with the button bit set — a click, whatever the (irrelevant)
    /// motion fields say.
    const PRESSED: [u8; 8] = [0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    /// A report with the button bit clear — this is what *every* report of a
    /// hold-and-point gesture looks like, moving or not.
    const RELEASED: [u8; 8] = [0x02, 0x00, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x00];

    #[test]
    fn a_click_reports_exactly_one_rising_edge() {
        let mut state = PresenterTapState::default();
        assert!(state.observe(&PRESSED), "the button just went down");
        assert!(
            !state.observe(&PRESSED),
            "still held from the report above — not a new click"
        );
        assert!(
            !state.observe(&RELEASED),
            "the release itself is not a rising edge"
        );
    }

    #[test]
    fn a_hold_and_point_gesture_never_reports_an_edge() {
        // Every report of a real hold-and-point gesture has the button bit
        // clear — this must never be mistaken for a click.
        let mut state = PresenterTapState::default();
        for _ in 0..50 {
            assert!(!state.observe(&RELEASED));
        }
    }

    #[test]
    fn two_separate_clicks_each_report_their_own_edge() {
        let mut state = PresenterTapState::default();
        assert!(state.observe(&PRESSED));
        assert!(!state.observe(&RELEASED));
        assert!(
            state.observe(&PRESSED),
            "a fresh press after a real release is a new edge"
        );
        assert!(!state.observe(&RELEASED));
    }

    #[test]
    fn idle_at_start_reports_no_edge() {
        let mut state = PresenterTapState::default();
        assert!(!state.observe(&RELEASED));
    }

    #[test]
    fn a_report_too_short_to_carry_a_button_byte_is_inert() {
        let mut state = PresenterTapState::default();
        assert!(!state.observe(&[]));
        assert!(!state.observe(&[0x02]));
    }

    #[test]
    fn a_two_byte_report_is_the_shortest_valid_one() {
        let mut state = PresenterTapState::default();
        assert!(state.observe(&[0x02, 0x01]), "buttons byte alone is enough");
    }
}
