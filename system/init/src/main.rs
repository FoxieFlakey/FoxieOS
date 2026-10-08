use std::process::Command;

use anyhow::{Context, bail};
use nix::{
    sys::reboot::{RebootMode, reboot},
    unistd::Pid,
};

mod init_task;
mod prep_fs;
mod service_manager;

fn do_main() -> anyhow::Result<()> {
    if Pid::this().as_raw() != 1 {
        bail!("Must run as PID 1");
    }

    prep_fs::run().context("Preparing rootfs")?;

    println!("Basic preboot rootfs is ready");

    service_manager::run(|rt| {
        // Start init tasks
        init_task::run(rt)?;

        init_task::DO_SHUTDOWN_COND.wait_while(&mut init_task::DO_SHUTDOWN.lock(), |x| *x != true);
        Ok(())
    })?;
    println!("init: going to shutdown, bye bye!");

    reboot(RebootMode::RB_POWER_OFF).context("Cannot perform shutdown")?;
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
