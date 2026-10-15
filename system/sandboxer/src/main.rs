#![feature(linux_pidfd)]

use std::sync::{Arc, Weak};

use anyhow::Context;
use foxie_base::{
    IObject, iobject,
    sandboxer::{ISandbox, ISandboxService, isandboxservice},
    service_manager::{IService, IServiceManager, iservice, iservicemanager},
};
use libbinder::{
    ContextManagerInfo, Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::{TryFromProxy, packetable::Serde};
use parking_lot::{Condvar, Mutex};

mod constants;
mod root_dev;
mod sandbox_profiles;
mod sandboxer;

static CAN_SHUTDOWN: Mutex<bool> = Mutex::new(false);
static SHUTDOWN_CHANGED: Condvar = Condvar::new();

// Sandbox service never shutdown. It runs forever
// till end of system
fn main() -> anyhow::Result<()> {
    let runtime = Runtime::new(
        foxie_base::MAIN_BINDER_PATH,
        ContextManagerInfo::Remote(Box::new(|x| Ok(<dyn IServiceManager>::try_from_proxy(x)?))),
    )
    .context("Cannot initialize binder runtime")?;

    root_dev::discover_root().context("Cannot find root device")?;
    sandboxer::init().context("Initializing sandboxer")?;

    let service_manager = runtime
        .get_manager()
        .downcast_ref::<iservicemanager::ProxyIServiceManager>()
        .unwrap();
    let registration_cookie = service_manager
        .register_service(
            foxie_base::sandboxer::SERVICE_ID,
            Arc::new(B::new(Ctl {
                rt: Arc::downgrade(&runtime),
            })),
        )
        .context("Cannot send transaction")?
        .map_err(|Serde(x)| x)
        .context("Cannot register sandboxer")?;

    // Doenst care, jsut sleep forever
    SHUTDOWN_CHANGED.wait_while(&mut CAN_SHUTDOWN.lock(), |&mut x| !x);

    // Allow sandboxer to be unregistered
    drop(registration_cookie);

    sandboxer::shutdown();
    println!("sandboxer: Shutting down");
    Ok(())
}

struct Ctl {
    rt: Weak<Runtime>,
}

impl ObjectTrait for Ctl {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> Arc<Runtime> {
        self.rt.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: libbinder_basic::enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn ISandboxService>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Ctl {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            isandboxservice::ID => Ok(true),
            iservice::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IService for Ctl {
    fn stop(&self) -> anyhow::Result<anyhow::Result<()>> {
        *CAN_SHUTDOWN.lock() = true;
        SHUTDOWN_CHANGED.notify_all();
        Ok(Ok(()))
    }
}

impl ISandboxService for Ctl {
    fn create_sandbox(
        &self,
        config: Serde<foxie_base::sandboxer::Config>,
    ) -> anyhow::Result<anyhow::Result<Arc<B<dyn ISandbox>>>> {
        let profile = config.0.profile;
        let ret = sandboxer::create_sandbox(
            sandboxer::Config {
                prep: Box::new(|uid| sandbox_profiles::apply_profile(profile, uid)),
                uid: config.0.uid.into(),
            },
            self.rt.clone(),
        );

        match ret {
            Ok(x) => Ok(Ok(Arc::new(B::new(x)))),
            Err(e) => Ok(Err(e)),
        }
    }
}
