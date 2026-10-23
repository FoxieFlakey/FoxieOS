use std::sync::{Arc, OnceLock, Weak};

use foxie_base::{
    IObject, iobject,
    service_registry::{IServiceManagerCtl, ShutdownError, iservicemanagerctl},
};
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::packetable::Serde;
use parking_lot::{Condvar, Mutex};

use crate::{service_manager, services};

pub static INIT: OnceLock<Arc<B<dyn IServiceManagerCtl>>> = OnceLock::new();
pub static DO_SHUTDOWN: Mutex<bool> = Mutex::new(false);
pub static DO_SHUTDOWN_COND: Condvar = Condvar::new();

pub fn run(runtime: &Arc<Runtime>) -> anyhow::Result<()> {
    INIT.set(Arc::new(B::new(Init {
        rt: Arc::downgrade(runtime),
    })))
    .ok()
    .unwrap();

    // Starting the process service manager (not binder)
    service_manager::init(runtime.clone());

    // Register & start services
    services::run()
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
        <dyn IServiceManagerCtl>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Init {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            iservicemanagerctl::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IServiceManagerCtl for Init {
    fn ping(&self) -> anyhow::Result<Serde<String>> {
        Ok(Serde("Hiiii from init >w> i am alive!".to_string()))
    }

    fn request_shutdown(&self) -> anyhow::Result<Result<(), Serde<ShutdownError>>> {
        println!("service_manager: Shutdown requested!");
        *DO_SHUTDOWN.lock() = true;
        DO_SHUTDOWN_COND.notify_all();
        Ok(Ok(()))
    }
}
