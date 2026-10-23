#![feature(oneshot_channel)]
#![feature(once_cell_try)]

use std::os::fd::{FromRawFd, OwnedFd};

use anyhow::Context;
use caps::CapSet;
use clap::Parser;
use foxie_base::{sandboxer::ISandboxService, uids};
use nix::{
    errno::Errno,
    sys::{eventfd::EventFd, prctl},
    unistd::{setresuid, setuid},
};

use crate::{
    constants::SANDBOXER, service_manager::get_manager, services_registry::SERVICE_MANAGER,
};

mod constants;
mod init_task;
mod root_dev;
mod service_manager;
mod services;
mod services_registry;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    ready_event_fd: u32,
    #[arg(long)]
    shutdown_req_to_init: u32,
    #[arg(long)]
    shutdown_req_acked_from_init: u32,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Assume caler make it correct
    let ready_event_fd = unsafe {
        EventFd::from_owned_fd(OwnedFd::from_raw_fd(
            cli.ready_event_fd
                .try_into()
                .context("--ready-event-fd is not valid fd")?,
        ))
    };
    let shutdown_req_eventfd = unsafe {
        EventFd::from_owned_fd(OwnedFd::from_raw_fd(
            cli.shutdown_req_to_init
                .try_into()
                .context("--shutdown-req-to-init is not valid fd")?,
        ))
    };
    let shutdown_ack_eventfd = unsafe {
        EventFd::from_owned_fd(OwnedFd::from_raw_fd(
            cli.shutdown_req_acked_from_init
                .try_into()
                .context("--shutdown-req-acked-from-init is not valid fd")?,
        ))
    };

    // Service manager dont have any right to the system
    prctl::set_keepcaps(false).context("Cant ask to retain capability")?;
    caps::clear(None, CapSet::Ambient).context("Clearining effective capabilities")?;
    caps::clear(None, CapSet::Bounding).context("Clearining effective capabilities")?;
    caps::clear(None, CapSet::Inheritable).context("Clearining effective capabilities")?;
    setresuid(
        uids::SERVICE_MANAER,
        uids::SERVICE_MANAER,
        uids::SERVICE_MANAER,
    )
    .context("Cannot drop UID")?;
    caps::clear(None, CapSet::Permitted).context("Clearining effective capabilities")?;
    caps::clear(None, CapSet::Effective).context("Clearining effective capabilities")?;

    root_dev::discover_root().context("Cannot find root device")?;

    println!("Root device was found at {}", root_dev::get_root_dev());
    println!("Basic preboot rootfs is ready");

    services_registry::run(|rt| {
        // Deprivilege service_manager
        setuid(foxie_base::uids::SERVICE_MANAER).context("Cannot drop privileges")?;

        // Service manager is ready, wake up the sandboxer to register
        ready_event_fd.write(1).context("Cannot write to eventfd")?;

        let sandboxer_service =
            foxie_base::service_registry::wait_and_get_service::<dyn ISandboxService>(
                &***SERVICE_MANAGER.get().unwrap(),
                foxie_base::sandboxer::SERVICE_ID,
            )
            .context("Cannot send transaction for waiting sandboxer")?
            .context("Cannot waits for sandboxer")?
            .context("Cannot cast IService to ISandboxService")?;
        SANDBOXER
            .set(sandboxer_service)
            .map_err(|_| ())
            .expect("Shouldnt be set");

        // Start init tasks
        init_task::run(rt).context("Running init tasks")?;

        init_task::DO_SHUTDOWN_COND.wait_while(&mut init_task::DO_SHUTDOWN.lock(), |x| *x != true);
        get_manager().shutdown();

        // Telling init that we want to shutdown
        // so it knows that some process that exited :3
        // that shouldnt exit is intended
        loop {
            match shutdown_req_eventfd.write(1) {
                Ok(_) => break,
                Err(Errno::EINTR) => continue,
                Err(e) => Err(e).context("Cannot write to shutdown req eventfd")?,
            }
        }

        // Now wait until init acked it
        loop {
            match shutdown_ack_eventfd.read() {
                Ok(_) => break,
                Err(Errno::EINTR) => continue,
                Err(e) => Err(e).context("Cannot read from shutdown req ack from init eventfd")?,
            }
        }

        println!("service_manager: init acked it! lets shutdown");

        constants::get_sandboxer()
            .stop()
            .context("Cannot send stop transaction to sandboxer")?
            .context("Cannot request sandboxer to stop")?;
        Ok(())
    })?;

    Ok(())
}
