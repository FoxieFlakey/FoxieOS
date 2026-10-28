use std::os::fd::{FromRawFd, OwnedFd};

use anyhow::Context;
use clap::Parser;
use foxie_base::util::retry_eintr;
use nix::sys::eventfd::EventFd;
use parking_lot::{Condvar, Mutex};

use crate::graphics_server::GraphicsServer;

mod graphics_server;

static CAN_SHUTDOWN: Mutex<bool> = Mutex::new(false);
static SHUTDOWN_CHANGED: Condvar = Condvar::new();

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    ready_event_fd: u32,
}

fn main() -> anyhow::Result<()> {
    foxie_base::init_log("graphics_server");
    let cli = Cli::parse();
    // Assume caler make it correct
    let ready_event_fd = unsafe {
        EventFd::from_owned_fd(OwnedFd::from_raw_fd(
            cli.ready_event_fd
                .try_into()
                .context("--ready-event-fd is not valid fd")?,
        ))
    };
    let server = GraphicsServer::new().context("Cannot initialze graphics server")?;

    retry_eintr(|| ready_event_fd.write(1)).context("Cannot send ready notification")?;

    SHUTDOWN_CHANGED.wait_while(&mut CAN_SHUTDOWN.lock(), |&mut x| !x);
    drop(server);
    Ok(())
}
