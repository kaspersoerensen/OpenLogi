//! Host I/O loop for [`crate::presenter`]'s click-to-toggle detection.
//!
//! Named `presenter_tap`, not `presenter`, to avoid colliding with
//! [`openlogi_device::presenter`] re-exported at this crate's root by its
//! blanket `pub use openlogi_device::*`.
//!
//! Bypasses `openlogi-hidpp` and the `HidBackend` trait entirely: this reads
//! a plain HID mouse collection, not a HID++ vendor one, and there is no
//! protocol semantic here for the scripted/replay test backends to mock — see
//! the module doc on `openlogi_device::presenter` for why the Spotlight needs
//! this path instead of the usual `0x1b04` divert.

use async_hid::AsyncHidRead;
use futures_lite::StreamExt as _;
use openlogi_device::presenter::PresenterTapState;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

/// The plain HID mouse collection's usage page/id — not the HID++ vendor
/// collection (`0xff43`/`0xff00`). Logitech's generic-desktop mouse usage,
/// present on every plain HID mouse regardless of vendor.
const MOUSE_USAGE_PAGE: u16 = 0x0001;
const MOUSE_USAGE_ID: u16 = 0x0002;

/// Why [`run_presenter_tap`] could not run, or stopped running.
#[derive(Debug, Error)]
pub enum PresenterTapError {
    /// No HID node matched the requested identity and usage collection —
    /// the device isn't connected, or it doesn't expose a plain mouse
    /// collection at all.
    #[error("no plain-HID mouse node found for {vendor_id:04x}:{product_id:04x}")]
    NotFound {
        /// USB/HID vendor ID that was requested.
        vendor_id: u16,
        /// USB/HID product ID that was requested.
        product_id: u16,
    },
    /// The `async-hid` transport failed to enumerate or open the node, or a
    /// read failed. Not typed further: this path has no protocol to
    /// classify errors against, only a transport to report.
    #[error("presenter HID tap failed: {0}")]
    Hid(String),
}

/// Passively tap `vendor_id:product_id`'s plain HID mouse collection and send
/// on `sink` each time its button is pressed — one toggle press per send,
/// never one per hold — until `shutdown` resolves.
///
/// A passive, non-exclusive read: nothing here changes the device's
/// reporting mode or firmware state, so there is no cleanup owed on any exit
/// path (unlike the HID++ capture sessions in `openlogi_device::session`,
/// which must restore diverted controls before returning).
pub async fn run_presenter_tap(
    vendor_id: u16,
    product_id: u16,
    sink: &mpsc::UnboundedSender<()>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<(), PresenterTapError> {
    let backend = async_hid::HidBackend::default();
    let devices: Vec<async_hid::Device> = backend
        .enumerate()
        .await
        .map_err(|error| PresenterTapError::Hid(error.to_string()))?
        .collect()
        .await;
    let node = devices
        .into_iter()
        .find(|device| {
            device.vendor_id == vendor_id
                && device.product_id == product_id
                && device.usage_page == MOUSE_USAGE_PAGE
                && device.usage_id == MOUSE_USAGE_ID
        })
        .ok_or(PresenterTapError::NotFound {
            vendor_id,
            product_id,
        })?;
    let (mut reader, _writer) = node
        .open()
        .await
        .map_err(|error| PresenterTapError::Hid(error.to_string()))?;

    let mut state = PresenterTapState::default();
    let mut buf = [0u8; 64];
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => return Ok(()),
            result = reader.read_input_report(&mut buf) => {
                let len = result.map_err(|error| PresenterTapError::Hid(error.to_string()))?;
                if state.observe(&buf[..len]) {
                    debug!("presenter tap: toggle press");
                    let _ = sink.send(());
                }
            }
        }
    }
}
