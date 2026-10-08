use libbinder::object::{B, ObjectTrait};
use libbinder_basic::{binder_ipc_object, packetable::Serde};
use serde::{Deserialize, Serialize};

use crate::{IObject, iobject};

#[derive(Serialize, Deserialize, Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("Permission denied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IInit")]
pub trait IInit: IObject {
    fn request_shutdown(&self) -> anyhow::Result<Result<(), Serde<ShutdownError>>>;
    fn ping(&self) -> anyhow::Result<Serde<String>>;
}
