use std::{borrow::Cow, fmt::Display, os::fd::OwnedFd, sync::Arc};

use anyhow::Context;
use libbinder::object::{B, ObjectTrait, TransactionError};
use libbinder_basic::{
    binder_ipc_object,
    packetable::{Packetable, Serde},
};
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

pub struct SpawnArgs<'a> {
    pub path: Cow<'a, str>,
    pub argv: Vec<String>,
    pub arg0: Option<Cow<'a, str>>,
    pub tty: Option<Cow<'a, str>>,
    // These will be prepended first to argv
    pub fd_args: Vec<(String, OwnedFd)>,
}

impl<'a> Packetable<'a> for SpawnArgs<'a> {
    fn serialize(&self, mut writer: &mut libbinder_basic::writer::Writer) -> anyhow::Result<()> {
        serde::Serialize::serialize(&self.path, &mut writer).context("Cannot serialize 'path'")?;
        serde::Serialize::serialize(&self.argv, &mut writer).context("Cannot serialize 'argv'")?;
        serde::Serialize::serialize(&self.arg0, &mut writer).context("Cannot serialize 'arg0'")?;
        serde::Serialize::serialize(&self.tty, &mut writer).context("Cannot serialize 'tty'")?;
        Packetable::serialize(&self.fd_args.len(), &mut writer)
            .context("Cannot serialize fd_args len")?;

        for v in self.fd_args.iter() {
            Packetable::serialize(v, &mut writer).context("Cannot serialize fd args")?;
        }
        Ok(())
    }

    fn deserialize(mut reader: &mut libbinder_basic::reader::Reader<'a>) -> anyhow::Result<Self> {
        let path =
            serde::Deserialize::deserialize(&mut reader).context("Cannot deserialize 'path'")?;
        let argv =
            serde::Deserialize::deserialize(&mut reader).context("Cannot deserialize 'argv'")?;
        let arg0 =
            serde::Deserialize::deserialize(&mut reader).context("Cannot deserialize 'arg0'")?;
        let tty =
            serde::Deserialize::deserialize(&mut reader).context("Cannot deserialize 'tty'")?;

        let len = Packetable::deserialize(&mut reader).context("Cannot read length of fd_args")?;
        let mut fd_args = Vec::with_capacity(len);

        for _ in 0..len {
            fd_args
                .push(Packetable::deserialize(&mut reader).context("Cannot deserialize fd args")?);
        }

        Ok(Self {
            path,
            argv,
            arg0,
            tty,
            fd_args,
        })
    }
}

#[binder_ipc_object(interface_id = "foxie.ISandbox")]
pub trait ISandbox: IObject {
    fn kill_all(&self) -> Result<anyhow::Result<Serde<()>>, TransactionError>;
    fn spawn(
        &self,
        spawn_args: SpawnArgs,
    ) -> Result<anyhow::Result<Arc<B<dyn IProcessHandle>>>, TransactionError>;
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
    fn get_pid(&self) -> Result<SerdePid, TransactionError>;
    fn get_exit_reason(
        &self,
    ) -> Result<anyhow::Result<Serde<Option<ExitReason>>>, TransactionError>;
    fn send_signal(
        &self,
        signal: SerdeSignal,
    ) -> Result<anyhow::Result<Serde<()>>, TransactionError>;
    fn on_death(
        &self,
        callback: Arc<B<dyn IFnOnce>>,
    ) -> Result<anyhow::Result<()>, TransactionError>;
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
    ) -> Result<anyhow::Result<Arc<B<dyn ISandbox>>>, TransactionError>;
}

pub const SERVICE_ID: &str = "foxieos.Sandboxer";
