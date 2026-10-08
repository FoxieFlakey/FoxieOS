use std::{
    process::Command,
    sync::{Arc, OnceLock, Weak},
};

use anyhow::Context;
use foxie_base::{
    IObject,
    init::{IInit, ShutdownError, iinit},
    iobject,
};
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::packetable::Serde;
use parking_lot::{Condvar, Mutex};

pub static INIT: OnceLock<Arc<B<dyn IInit>>> = OnceLock::new();
pub static DO_SHUTDOWN: Mutex<bool> = Mutex::new(false);
pub static DO_SHUTDOWN_COND: Condvar = Condvar::new();

pub fn run(runtime: &Arc<Runtime>) -> anyhow::Result<()> {
    INIT.set(Arc::new(B::new(Init {
        rt: Arc::downgrade(runtime),
    })))
    .ok()
    .unwrap();

    // Lets just pop cmdline for testing
    Command::new("/system/bin/busybox")
        .arg("sh")
        .spawn()
        .context("Cannot spawn test shell :<")?;

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
        <dyn IInit>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Init {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            iinit::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IInit for Init {
    fn ping(&self) -> anyhow::Result<Serde<String>> {
        Ok(Serde("Hiiii from init >w> i am alive!".to_string()))
    }

    fn request_shutdown(&self) -> anyhow::Result<Result<(), Serde<ShutdownError>>> {
        println!("init: Shutdown requested!");
        *DO_SHUTDOWN.lock() = true;
        DO_SHUTDOWN_COND.notify_all();
        Ok(Ok(()))
    }
}
