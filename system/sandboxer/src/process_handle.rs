use std::{os::linux::process::PidFd, sync::Weak};

use anyhow::{Context, anyhow};
use foxie_base::{
    IObject,
    common::SerdeSignal,
    iobject,
    sandboxer::{IProcessHandle, iprocesshandle},
    uids,
};
use libbinder::{Runtime, object::ObjectTrait};
use libbinder_basic::packetable::Serde;
use pidfd_util::PidFdExtFoxie;

pub struct ProcessHandle {
    pub pidfd: PidFd,
    pub rt: Weak<Runtime>,
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
}
