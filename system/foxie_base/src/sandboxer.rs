use std::sync::Arc;

use libbinder::object::{B, ObjectTrait};
use libbinder_basic::{binder_ipc_object, packetable::Serde};
use serde::{Deserialize, Serialize};

use crate::{
    IObject,
    common::{SafePidFd, SerdeSignal},
    iobject,
    service_manager::{IService, iservice},
};

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum Profile {
    Unconfined,
    App,
}

#[derive(Serialize, Deserialize)]
pub struct SpawnArgs {
    pub path: String,
    pub argv: Vec<String>,
    pub arg0: Option<String>,
}

#[binder_ipc_object(interface_id = "foxie.ISandbox")]
pub trait ISandbox: IObject {
    fn kill_all(&self) -> anyhow::Result<Serde<()>>;
    fn spawn(
        &self,
        spawn_args: Serde<SpawnArgs>,
    ) -> anyhow::Result<anyhow::Result<(SafePidFd, Arc<B<dyn IProcessHandle>>, nix::libc::pid_t)>>;
}

#[binder_ipc_object(interface_id = "foxie.ISandboxProcess")]
pub trait IProcessHandle: IObject {
    fn send_signal(&self, signal: SerdeSignal) -> anyhow::Result<anyhow::Result<Serde<()>>>;
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    // Uid of what all tasks will spawn as
    pub uid: u32,
    // Security profile for the tasks
    pub profile: Profile,
}

#[binder_ipc_object(interface_id = "foxie.ISandboxService")]
pub trait ISandboxService: IService {
    fn create_sandbox(
        &self,
        config: Serde<Config>,
    ) -> anyhow::Result<anyhow::Result<Arc<B<dyn ISandbox>>>>;
}

pub const SERVICE_ID: &str = "foxieos.Sandboxer";
