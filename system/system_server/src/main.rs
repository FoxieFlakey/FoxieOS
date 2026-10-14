#![feature(linux_pidfd)]
#![feature(oneshot_channel)]

use std::process::Command;

use anyhow::Context;

mod constants;
mod init_task;
mod prep_fs;
mod process_services;
mod root_dev;
#[allow(unused)]
mod sandbox_profiles;
#[allow(unused)]
mod sandboxer;
mod service_manager;

fn do_main() -> anyhow::Result<()> {
    prep_fs::run().context("Preparing rootfs")?;

    println!("Root device was found at {}", root_dev::get_root_dev());
    println!("Basic preboot rootfs is ready");

    service_manager::run(|rt| {
        // Init sandboxer
        sandboxer::init().context("Initializing sandboxer")?;

        // Start init tasks
        init_task::run(rt).context("Running init tasks")?;

        init_task::DO_SHUTDOWN_COND.wait_while(&mut init_task::DO_SHUTDOWN.lock(), |x| *x != true);
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
