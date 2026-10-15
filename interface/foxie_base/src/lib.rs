#![feature(linux_pidfd)]

use libbinder::object::{B, ObjectTrait};
use libbinder_basic::binder_ipc_object;

pub mod common;
pub mod impls;
pub mod sandboxer;
pub mod service_manager;
pub mod uids;

pub static MAIN_BINDER_PATH: &str = "/dev/binder";

#[binder_ipc_object(root = true, interface_id = "foxie.IObject")]
pub trait IObject: ObjectTrait {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool>;
}

// Pretty basic, token interface. it doesnt do anything. But it will be
// very useful to assert that there right to something
#[binder_ipc_object(interface_id = "foxie.IToken")]
pub trait IToken: IObject {
    // This does nothing, mainly because currently still
    // have not tested macro for no methods
    fn stub(&self) -> anyhow::Result<()>;
}
