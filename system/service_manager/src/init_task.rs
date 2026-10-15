use std::{
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};

use anyhow::Context;
use foxie_base::{
    IObject, iobject,
    system_server::{ISystemServer, ShutdownError, isystemserver},
};
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::packetable::Serde;
use nix::{sys::signal::Signal, unistd::Uid};
use parking_lot::{Condvar, Mutex};

use crate::{
    process_services::{self, Request},
    sandbox_profiles,
};

pub static INIT: OnceLock<Arc<B<dyn ISystemServer>>> = OnceLock::new();
pub static DO_SHUTDOWN: Mutex<bool> = Mutex::new(false);
pub static DO_SHUTDOWN_COND: Condvar = Condvar::new();

pub fn run(runtime: &Arc<Runtime>) -> anyhow::Result<()> {
    INIT.set(Arc::new(B::new(Init {
        rt: Arc::downgrade(runtime),
    })))
    .ok()
    .unwrap();

    // Starting the process service manager (not binder)
    process_services::start_watcher().context("Cannot start process service manager")?;
    process_services::send_request(Request::AddService(process_services::Config {
        name: "shell".to_string(),
        binary: "/system/bin/tty_spawner".into(),
        arg0: None,
        argv: vec![
            "--binary".to_string(),
            "/system/busybox_bins/sh".to_string(),
            "--tty-dev".to_string(),
            "/dev/hvc1".to_string(),
        ],
        uid: Uid::from_raw(0),
        sandbox_profile: Some(sandbox_profiles::unconfined_profile),
        retry_delay: Duration::from_secs(0),
        retry_count: 5,
        retry_window: Duration::from_mins(2),
        graceful_signal: Signal::SIGTERM,
        graceful_delay: Duration::from_secs(5),
    }))
    .context("Cannot add shell service")?;
    process_services::send_request(Request::Start("shell".to_string()))
        .context("Cannot start shell service")?;

    Ok(())
}

struct Init {
    rt: Weak<Runtime>,
}

impl ObjectTrait for Init {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> std::sync::Arc<Runtime> {
        self.rt.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn ISystemServer>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Init {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            isystemserver::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl ISystemServer for Init {
    fn ping(&self) -> anyhow::Result<Serde<String>> {
        Ok(Serde("Hiiii from init >w> i am alive!".to_string()))
    }

    fn request_shutdown(&self) -> anyhow::Result<Result<(), Serde<ShutdownError>>> {
        println!("system_server: Shutdown requested!");
        *DO_SHUTDOWN.lock() = true;
        DO_SHUTDOWN_COND.notify_all();
        Ok(Ok(()))
    }
}
