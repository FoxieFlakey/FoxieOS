use std::sync::{Arc, Weak};

use anyhow::anyhow;
use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::binder_ipc_object;
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
