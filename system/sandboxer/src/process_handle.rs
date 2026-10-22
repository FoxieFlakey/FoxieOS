use std::{
    os::linux::process::PidFd,
    sync::{Arc, Weak},
};

use anyhow::{Context, anyhow};
use foxie_base::{
    IObject,
    common::{SerdePid, SerdeSignal},
    iobject,
    sandboxer::{ExitReason, IProcessHandle, iprocesshandle},
    uids,
    util::IFnOnce,
};
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::packetable::Serde;
use nix::unistd::Pid;
use parking_lot::Mutex;
use pidfd_util::PidFdExtFoxie;

pub struct ProcessHandle {
    pub death_reason: Mutex<Option<ExitReason>>,
    pub pid: Pid,
    pub pidfd: PidFd,
    pub rt: Weak<Runtime>,
    pub death_listeners: Mutex<Vec<Arc<B<dyn IFnOnce>>>>,
}

impl ObjectTrait for ProcessHandle {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> std::sync::Arc<libbinder::Runtime> {
        self.rt.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: libbinder_basic::enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn IProcessHandle>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for ProcessHandle {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            iprocesshandle::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IProcessHandle for ProcessHandle {
    fn get_pid(&self) -> anyhow::Result<foxie_base::common::SerdePid> {
        Ok(SerdePid(self.pid))
    }

    fn get_exit_reason(&self) -> anyhow::Result<anyhow::Result<Serde<Option<ExitReason>>>> {
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }
        Ok(Ok(Serde(self.death_reason.lock().clone())))
    }

    fn send_signal(&self, signal: SerdeSignal) -> anyhow::Result<anyhow::Result<Serde<()>>> {
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }

        Ok(self
            .pidfd
            .send_signal(signal.0)
            .with_context(|| format!("Cannot send {} this process handle", signal.0.as_str()))
            .map(|()| Serde(())))
    }

    fn on_death(&self, callback: Arc<B<dyn IFnOnce>>) -> anyhow::Result<anyhow::Result<()>> {
        if !uids::can_access_sandboxer(self.get_runtime().get_caller_identity().sender_euid) {
            return Ok(Err(anyhow!("Caller not allowed to access sandboxer")));
        }

        let guard = self.death_reason.lock();
        if guard.is_some() {
            // Best effort only
            let _ = callback.call();
            return Ok(Ok(()));
        }

        self.death_listeners.lock().push(callback);
        drop(guard);
        Ok(Ok(()))
    }
}
