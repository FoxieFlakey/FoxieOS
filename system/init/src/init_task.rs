use std::{
    path::Path,
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
use nix::{
    mount::{MsFlags, mount},
    unistd::Uid,
};
use parking_lot::{Condvar, Mutex};

use crate::sandboxer::{
    Config,
    sandbox::{Sandbox, SpawnArgs},
};

pub static INIT: OnceLock<Arc<B<dyn IInit>>> = OnceLock::new();
pub static DO_SHUTDOWN: Mutex<bool> = Mutex::new(false);
pub static DO_SHUTDOWN_COND: Condvar = Condvar::new();

pub fn run(runtime: &Arc<Runtime>) -> anyhow::Result<()> {
    INIT.set(Arc::new(B::new(Init {
        rt: Arc::downgrade(runtime),
    })))
    .ok()
    .unwrap();

    let sandbox = Sandbox::new(Config {
        prep: || {
            mount(
                None::<&Path>,
                "/proc",
                Some("proc"),
                MsFlags::MS_NOATIME
                    | MsFlags::MS_NODIRATIME
                    | MsFlags::MS_NODEV
                    | MsFlags::MS_NOEXEC
                    | MsFlags::MS_NOSUID,
                Some("hidepid=2,subset=pid"),
            )
            .context("Cannot remount proc")?;
            Ok(())
        },
        uid: Uid::from_raw(1000),
    })
    .context("Cannot create sandbox for shell")?;

    // Lets just pop cmdline for testing
    let exit_code = sandbox
        .spawn(&SpawnArgs {
            path: "/system/bin/busybox".to_string(),
            argv: vec![],
            arg0: Some("sh".to_string()),
        })
        .context("Cannot spawn test shell :<")?
        .wait()
        .context("Cannot wait shell")?;
    println!("Shell exited with {exit_code}");

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
