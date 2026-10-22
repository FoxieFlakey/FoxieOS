use std::{borrow::Cow, fmt::Display, sync::Arc};

use libbinder::object::{B, ObjectTrait};
use libbinder_basic::{binder_ipc_object, packetable::Serde};
use serde::{Deserialize, Serialize};

use crate::{
    IObject,
    common::{SerdePid, SerdeSignal},
    iobject,
    service_registry::{IService, iservice},
    util::IFnOnce,
};

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum Profile {
    Unconfined,
    App,
}

#[derive(Serialize, Deserialize)]
pub struct SpawnArgs<'a> {
    pub path: Cow<'a, str>,
    pub argv: Vec<String>,
    pub arg0: Option<Cow<'a, str>>,
    pub tty: Option<Cow<'a, str>>,
}

#[binder_ipc_object(interface_id = "foxie.ISandbox")]
pub trait ISandbox: IObject {
    fn kill_all(&self) -> anyhow::Result<anyhow::Result<Serde<()>>>;
    fn spawn(
        &self,
        spawn_args: Serde<SpawnArgs>,
    ) -> anyhow::Result<anyhow::Result<Arc<B<dyn IProcessHandle>>>>;
}

#[derive(Deserialize, Serialize, Copy, Clone)]
pub enum ExitReason {
    Signal(SerdeSignal),
    Exit(i32),
}

impl Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExitReason::Signal(signal) => write!(f, "signal {}", signal.0),
            ExitReason::Exit(code) => write!(f, "code {code}"),
        }
    }
}

#[binder_ipc_object(interface_id = "foxie.ISandboxProcess")]
pub trait IProcessHandle: IObject {
    fn get_pid(&self) -> anyhow::Result<SerdePid>;
    fn get_exit_reason(&self) -> anyhow::Result<anyhow::Result<Serde<Option<ExitReason>>>>;
    fn send_signal(&self, signal: SerdeSignal) -> anyhow::Result<anyhow::Result<Serde<()>>>;
    fn on_death(&self, callback: Arc<B<dyn IFnOnce>>) -> anyhow::Result<anyhow::Result<()>>;
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
