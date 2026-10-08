use libbinder::object::{B, ObjectTrait};
use libbinder_basic::binder_ipc_object;

pub mod impls;
pub mod init;
pub mod service_manager;

pub static MAIN_BINDER_PATH: &str = "/dev/binderfs/binder";

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
