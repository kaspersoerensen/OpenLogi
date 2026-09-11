//! Watches capture plans for a device whose `PresenterHighlight` button is
//! bound to [`Action::ToggleHighlight`], and runs the presenter tap for it.
//!
//! Unlike the other HID++ watchers in this module, the tap owns no firmware
//! state to restore on shutdown (see `openlogi_hid::presenter_tap`'s docs) —
//! it's a passive, non-exclusive plain-HID read, so tearing it down is just
//! dropping the task. That's also why this watcher's `spawn` signature is
//! much smaller than gesture/host_switch/keyboard's: no channel registry, no
//! receiver access, no device I/O gate — none of those exist for a
//! connection this crate doesn't open through `HidBackend` at all.

use std::sync::Arc;

use openlogi_core::binding::{Action, ButtonId, HighlightMode};
use openlogi_hid::DeviceRoute;
use openlogi_hid::presenter_tap::run_presenter_tap;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::capture_plan::{DeviceCapturePlan, SharedCapturePlans};
use crate::highlight::HighlightSession;

/// The device identity and mode a presenter tap should currently run for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    vendor_id: u16,
    product_id: u16,
    mode: HighlightMode,
}

/// The first online device (there is realistically ever at most one
/// Spotlight) whose `PresenterHighlight` button resolves to
/// `Action::ToggleHighlight`, or `None` if no such binding exists right now.
fn wanted_from_plans(plans: &[DeviceCapturePlan]) -> Option<Wanted> {
    plans.iter().find_map(|plan| {
        let DeviceRoute::Direct {
            vendor_id,
            product_id,
        } = plan.target.route
        else {
            return None;
        };
        let binding = plan.dispatch.bindings.get(&ButtonId::PresenterHighlight)?;
        let Action::ToggleHighlight(mode) = binding.click_action() else {
            return None;
        };
        Some(Wanted {
            vendor_id,
            product_id,
            mode,
        })
    })
}

/// Run one presenter tap for `wanted`, flipping `highlight` on each press,
/// until the tap ends (transport error/disconnect) or `shutdown` resolves.
async fn run_one(wanted: Wanted, highlight: &HighlightSession, shutdown: oneshot::Receiver<()>) {
    let (presses_tx, mut presses_rx) = mpsc::unbounded_channel();
    let tap = run_presenter_tap(wanted.vendor_id, wanted.product_id, &presses_tx, shutdown);
    tokio::pin!(tap);
    loop {
        tokio::select! {
            result = &mut tap => {
                if let Err(error) = result {
                    debug!(?error, "presenter tap ended");
                }
                return;
            }
            Some(()) = presses_rx.recv() => highlight.toggle(wanted.mode),
        }
    }
}

/// Watch `capture_plans` for a device bound to `ToggleHighlight` and run its
/// presenter tap for as long as that binding holds — restarting on any
/// change to which device or mode is wanted, and retrying against the latest
/// plan snapshot if the tap itself ends (e.g. the device went to sleep or
/// disconnected) rather than waiting for an unrelated plan change to notice.
#[must_use]
pub fn spawn(
    mut capture_plans: SharedCapturePlans,
    highlight: Arc<HighlightSession>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut current: Option<(Wanted, oneshot::Sender<()>)> = None;
        let mut task: Option<JoinHandle<()>> = None;
        loop {
            let plans = capture_plans.borrow_and_update().clone();
            let wanted = wanted_from_plans(&plans);
            let unchanged = matches!(
                (&current, wanted),
                (Some((running, _)), Some(w)) if *running == w
            );
            if !unchanged {
                if let Some((_, stop)) = current.take() {
                    let _ = stop.send(());
                }
                if let Some(task) = task.take() {
                    let _ = task.await;
                }
                task = wanted.map(|w| {
                    let (stop_tx, stop_rx) = oneshot::channel();
                    current = Some((w, stop_tx));
                    let highlight = Arc::clone(&highlight);
                    tokio::spawn(async move { run_one(w, &highlight, stop_rx).await })
                });
            }

            match task.as_mut() {
                Some(running) => {
                    tokio::select! {
                        result = capture_plans.changed() => {
                            if result.is_err() {
                                break;
                            }
                        }
                        _ = running => {
                            // The tap ended on its own — clear it and let the
                            // top of the loop re-derive `wanted` from the
                            // same (unchanged) snapshot, retrying it.
                            current = None;
                            task = None;
                        }
                    }
                }
                None => {
                    if capture_plans.changed().await.is_err() {
                        break;
                    }
                }
            }
        }
        if let Some((_, stop)) = current.take() {
            let _ = stop.send(());
        }
        if let Some(task) = task.take() {
            let _ = task.await;
        }
    })
}

#[cfg(test)]
mod tests {
    use openlogi_core::binding::{Action, Binding, HighlightMode};
    use openlogi_core::config::Config;
    use openlogi_core::device_order::PhysicalDeviceKey;
    use openlogi_hid::DeviceRoute;

    use super::*;
    use crate::capture_plan::plan_for_device;

    const CONFIG_KEY: &str = "spotlight-a";

    fn physical_key() -> PhysicalDeviceKey {
        PhysicalDeviceKey::parse("unit:00000001").expect("fixture should be a physical key")
    }

    fn direct_route() -> DeviceRoute {
        DeviceRoute::Direct {
            vendor_id: 0x046d,
            product_id: 0xb503,
        }
    }

    fn plan_with(config: &Config, route: DeviceRoute) -> DeviceCapturePlan {
        plan_for_device(config, physical_key(), CONFIG_KEY, route, None, 0, true)
    }

    #[test]
    fn a_toggle_highlight_binding_on_a_direct_device_is_wanted() {
        let mut config = Config::default();
        config.set_binding(
            CONFIG_KEY,
            ButtonId::PresenterHighlight,
            Binding::Single(Action::ToggleHighlight(HighlightMode::Spotlight)),
        );
        let plans = [plan_with(&config, direct_route())];
        assert_eq!(
            wanted_from_plans(&plans),
            Some(Wanted {
                vendor_id: 0x046d,
                product_id: 0xb503,
                mode: HighlightMode::Spotlight,
            })
        );
    }

    #[test]
    fn an_unbound_presenter_button_is_not_wanted() {
        let plans = [plan_with(&Config::default(), direct_route())];
        assert_eq!(wanted_from_plans(&plans), None);
    }

    #[test]
    fn a_presenter_button_bound_to_a_different_action_is_not_wanted() {
        let mut config = Config::default();
        config.set_binding(
            CONFIG_KEY,
            ButtonId::PresenterHighlight,
            Binding::Single(Action::LeftClick),
        );
        let plans = [plan_with(&config, direct_route())];
        assert_eq!(wanted_from_plans(&plans), None);
    }

    #[test]
    fn a_toggle_highlight_binding_behind_a_receiver_is_not_wanted() {
        // The presenter tap only ever targets a directly-connected device —
        // there is no plain-HID collection to read behind a Bolt/Unifying
        // receiver route.
        let mut config = Config::default();
        config.set_binding(
            CONFIG_KEY,
            ButtonId::PresenterHighlight,
            Binding::Single(Action::ToggleHighlight(HighlightMode::Highlight)),
        );
        let plans = [plan_with(
            &config,
            DeviceRoute::Bolt {
                receiver_uid: "abc123".into(),
                slot: 1,
            },
        )];
        assert_eq!(wanted_from_plans(&plans), None);
    }
}
