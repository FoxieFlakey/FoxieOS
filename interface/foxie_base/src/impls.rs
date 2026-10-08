use std::{
    hash::Hash,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use libbinder::{
    Runtime,
    object::{B, ObjectTrait},
};

use crate::{IObject, IToken, iobject, itoken};

pub struct Token(Weak<Runtime>, u64, Option<Box<dyn FnOnce() + Send + Sync>>);

impl Drop for Token {
    fn drop(&mut self) {
        if let Some(x) = self.2.take() {
            x();
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl Token {
    pub fn new(runtime: Weak<Runtime>) -> Arc<B<Self>> {
        Arc::new(B::new(Self(
            runtime,
            COUNTER.fetch_add(1, Ordering::Relaxed),
            None,
        )))
    }

    pub fn new_with_death<F>(runtime: Weak<Runtime>, callback: F) -> Arc<B<Self>>
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        Arc::new(B::new(Self(
            runtime,
            COUNTER.fetch_add(1, Ordering::Relaxed),
            Some(Box::new(callback)),
        )))
    }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0) && self.1 == other.1
    }
}

impl Eq for Token {}

impl Hash for Token {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_ptr().addr().hash(state);
        self.1.hash(state);
    }
}

impl ObjectTrait for Token {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> Arc<Runtime> {
        self.0.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: libbinder_basic::enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn IToken>::decode_and_dispatch(self, code, flags, message)
    }
}

impl IObject for Token {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iobject::ID | itoken::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IToken for Token {
    fn stub(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
