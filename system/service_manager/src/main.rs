#![feature(linux_pidfd)]
#![feature(oneshot_channel)]

use std::{os::linux::process::CommandExt, process::Command};

use anyhow::Context;
use foxie_base::sandboxer::ISandboxService;

use crate::{
    constants::{SANDBOXER, SANDBOXER_CHILD},
    service_manager::SERVICE_MANAGER,
};

mod constants;
mod init_task;
mod pidfd;
mod process_services;
mod root_dev;
mod service_manager;

fn do_main() -> anyhow::Result<()> {
    root_dev::discover_root().context("Cannot find root device")?;

    println!("Root device was found at {}", root_dev::get_root_dev());
    println!("Basic preboot rootfs is ready");

    service_manager::run(|rt| {
        // Spawn sandboxer which is needed by server manager
        // to be able spawn jobs
        *SANDBOXER_CHILD.lock() = Some(
            Command::new("/system/bin/sandboxer")
                .create_pidfd(true)
                .spawn()
                .context("Cannot spawn sandboxer service")?,
        );

        let sandboxer_service =
            foxie_base::service_manager::wait_and_get_service::<dyn ISandboxService>(
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

        constants::get_sandboxer()
            .stop()
            .context("Cannot send stop transaction to sandboxer")?
            .context("Cannot request sandboxer to stop")?;
        let status = constants::SANDBOXER_CHILD
            .lock()
            .take()
            .unwrap()
            .wait()
            .context("Cannot wait for sandboxer to exit")?;
        println!("service_manager: Sandboxer exited with code {status}");
        Ok(())
    })?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    if let Err(e) = do_main() {
        println!("init crashed with error: {e:#?}");
        println!("Entering rescue shell. Exiting will continue error handling");
        Command::new("/system/bin/busybox")
            .arg("sh")
            .status()
            .context("Cannot start shell")?;
        Err(e)
    } else {
        Ok(())
    }
}
