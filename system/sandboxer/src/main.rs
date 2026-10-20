#![feature(linux_pidfd)]

use std::{
    os::fd::{FromRawFd, OwnedFd},
    sync::{Arc, Weak},
};

use anyhow::{Context, anyhow};
use caps::CapSet;
use clap::Parser;
use foxie_base::{
    IObject, iobject,
    sandboxer::{ISandbox, ISandboxService, isandboxservice},
    service_manager::{IService, IServiceManager, iservice, iservicemanager},
    uids,
};
use libbinder::{
    ContextManagerInfo, Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::{TryFromProxy, packetable::Serde};
use nix::{
    sys::{eventfd::EventFd, prctl},
    unistd::setuid,
};
use parking_lot::{Condvar, Mutex};

mod constants;
mod process_handle;
mod root_dev;
mod sandbox_profiles;
mod sandboxer;

static CAN_SHUTDOWN: Mutex<bool> = Mutex::new(false);
static SHUTDOWN_CHANGED: Condvar = Condvar::new();

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    service_manager_ready_event_fd: u32,
}

// Sandbox service never shutdown. It runs forever
// till end of system
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Assume caler make it correct
    let service_manager_ready_event_fd = unsafe {
        EventFd::from_owned_fd(OwnedFd::from_raw_fd(
            cli.service_manager_ready_event_fd
                .try_into()
                .context("--ready-event-fd is not valid fd")?,
        ))
    };

    // Lets drop UID before registering and retaining all capabilities

    prctl::set_keepcaps(true).context("Cant ask to retain capability")?;
    caps::set(None, CapSet::Effective, &caps::all())
        .context("Setting effective capability before setuid")?;
    caps::set(None, CapSet::Inheritable, &caps::all())
        .context("Setting inheritable capability before setuid")?;
    caps::set(None, CapSet::Ambient, &caps::all())
        .context("Setting ambient capability before setuid")?;
    setuid(uids::SANDBOXER).context("Cannot drop UID")?;
    caps::set(None, CapSet::Effective, &caps::all())
        .context("Cannot restore effective capability after setuid")?;
    caps::set(None, CapSet::Inheritable, &caps::all())
        .context("Setting inheritable capability after setuid")?;
    caps::set(None, CapSet::Ambient, &caps::all())
        .context("Cannot restore ambient capability after setuid")?;

    let runtime = Runtime::new(
        foxie_base::MAIN_BINDER_PATH,
        ContextManagerInfo::Remote(Box::new(|x| Ok(<dyn IServiceManager>::try_from_proxy(x)?))),
    )
    .context("Cannot initialize binder runtime")?;

    root_dev::discover_root().context("Cannot find root device")?;
    sandboxer::init().context("Initializing sandboxer")?;

    // Service manager is ready, lets do register sandboxer
    service_manager_ready_event_fd
        .read()
        .context("Cannot wait for service manager readyness")?;

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
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }

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
