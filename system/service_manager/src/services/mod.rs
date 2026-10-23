use std::time::Duration;

use anyhow::Context;
use foxie_base::sandboxer;
use nix::sys::signal::Signal;

use crate::service_manager::{self, service::Service};

pub fn run() -> anyhow::Result<()> {
    let shell_service = Service::new(service_manager::service::Config {
        name: "shell".to_string(),
        binary: "/system/busybox_bins/sh".into(),
        arg0: None,
        argv: vec![],
        tty: Some("/dev/hvc0".into()),
        uid: foxie_base::uids::SHELL,
        sandbox_profile: sandboxer::Profile::Unconfined,
        retry_delay: Duration::from_secs(0),
        retry_count: 5,
        retry_window: Duration::from_mins(2),
        graceful_signal: Signal::SIGTERM,
        graceful_delay: Duration::from_secs(5),
        start_timeout: Duration::from_secs(20),
        ready_eventfd_arg: None,
    });

    shell_service.start().context("Cannot start shell")?;
    Ok(())
}
