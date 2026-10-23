use std::{
    io,
    sync::{Arc, Weak},
};

use anyhow::anyhow;
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::binder_ipc_object;
use nix::errno::Errno;
use parking_lot::Mutex;

use crate::{IObject, iobject};

pub mod listeners;

#[binder_ipc_object(interface_id = "foxieos.IFnOnce")]
pub trait IFnOnce: IObject {
    fn call(&self) -> anyhow::Result<anyhow::Result<()>>;
}

pub struct FnOnceObject(
    Mutex<Option<Box<dyn FnOnce() + Sync + Send>>>,
    Weak<Runtime>,
);

impl ObjectTrait for FnOnceObject {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> std::sync::Arc<Runtime> {
        self.1.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: libbinder_basic::enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn IFnOnce>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for FnOnceObject {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID => Ok(true),
            ifnonce::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IFnOnce for FnOnceObject {
    fn call(&self) -> anyhow::Result<anyhow::Result<()>> {
        match self.0.lock().take() {
            Some(x) => Ok(Ok((x)())),
            None => Ok(Err(anyhow!("This function already called"))),
        }
    }
}

impl FnOnceObject {
    pub fn new<F>(rt: Weak<Runtime>, closure: F) -> Arc<B<dyn IFnOnce>>
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        Arc::new(B::new(FnOnceObject(
            Mutex::new(Some(Box::new(closure))),
            rt,
        )))
    }
}

mod private {
    pub trait Sealed {}
}

pub trait IsEIntr: private::Sealed {
    fn is_eintr(&self) -> bool;
}

impl private::Sealed for Errno {}
impl IsEIntr for Errno {
    fn is_eintr(&self) -> bool {
        matches!(self, Errno::EINTR)
    }
}

impl private::Sealed for io::Error {}
impl IsEIntr for io::Error {
    fn is_eintr(&self) -> bool {
        matches!(self.kind(), io::ErrorKind::Interrupted)
    }
}

pub fn retry_eintr<T, E, F>(mut func: F) -> Result<T, E>
where
    F: FnMut() -> Result<T, E>,
    E: IsEIntr,
{
    loop {
        match func() {
            Ok(x) => return Ok(x),
            Err(e) => {
                if e.is_eintr() {
                    continue;
                } else {
                    return Err(e);
                }
            }
        }
    }
}
