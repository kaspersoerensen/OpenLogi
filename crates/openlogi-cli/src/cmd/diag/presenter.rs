//! `openlogi diag presenter` — click-to-toggle detection for the Spotlight's
//! presenter button, over the passive plain-HID tap rather than `0x1b04`
//! divert. See `openlogi_device::presenter` for why this device needs it.

use anyhow::{Context, Result, bail};
use clap::Args;
use openlogi_hid::DeviceRoute;
use openlogi_hid::presenter_tap::run_presenter_tap;
use tokio::sync::{mpsc, oneshot};

use crate::cmd::diag::select_device;

#[derive(Debug, Args)]
pub struct PresenterArgs {
    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting.
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,

    /// Seconds to listen before stopping.
    #[arg(long, default_value_t = 60)]
    pub seconds: u64,
}

pub async fn run(args: PresenterArgs) -> Result<()> {
    let (route, name) = select_device(args.device.as_deref(), &[]).await?;
    let DeviceRoute::Direct {
        vendor_id,
        product_id,
    } = route
    else {
        bail!(
            "the presenter tap only supports a directly-connected device (USB cable or \
             Bluetooth), not one behind a receiver"
        );
    };
    println!(
        "device: {name} ({vendor_id:04x}:{product_id:04x}) — click the front button to \
         toggle the highlight on/off; Ctrl-C to stop early\n"
    );

    let (presses_tx, mut presses_rx) = mpsc::unbounded_channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let mut shutdown_tx = Some(shutdown_tx);

    let tap = run_presenter_tap(vendor_id, product_id, &presses_tx, shutdown_rx);
    tokio::pin!(tap);
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(args.seconds));
    tokio::pin!(deadline);
    let mut highlight_on = false;

    let outcome = loop {
        tokio::select! {
            result = &mut tap => break Some(result),
            () = &mut deadline => {
                if let Some(tx) = shutdown_tx.take() {
                    let _ = tx.send(());
                }
                break None;
            }
            Some(()) = presses_rx.recv() => {
                highlight_on = !highlight_on;
                println!("  highlight: {}", if highlight_on { "ON" } else { "off" });
            }
        }
    };
    // Whether the deadline fired or the tap itself ended, drain it to
    // completion so a genuine transport failure still surfaces.
    let outcome = match outcome {
        Some(result) => result,
        None => tap.await,
    };
    outcome.context("presenter tap")?;
    Ok(())
}
