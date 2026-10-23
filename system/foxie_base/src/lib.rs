#![feature(oneshot_channel)]

use std::{
    io::{Write, stderr},
    sync::OnceLock,
};

use libbinder::object::{B, ObjectTrait};
use libbinder_basic::binder_ipc_object;
use log::Log;

use crate::util::retry_eintr;

pub mod common;
pub mod impls;
pub mod sandboxer;
pub mod service_registry;
pub mod uids;
pub mod util;

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

pub fn init_log<T>(name: T)
where
    T: Into<String>,
{
    if let Ok(()) = LOGGER.set(LoggerImpl(name.into())) {
        log::set_logger(LOGGER.get().unwrap()).expect("Cannot initialize logger");
        log::set_max_level(log::LevelFilter::Debug);
    }
}

static LOGGER: OnceLock<LoggerImpl> = OnceLock::new();
struct LoggerImpl(String);

impl Log for LoggerImpl {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        match record.level() {
            log::Level::Error => eprintln!("{}: [ERROR] {}", self.0, record.args().to_string()),
            log::Level::Warn => eprintln!("{}: [WARN] {}", self.0, record.args().to_string()),
            log::Level::Info => eprintln!("{}: [INFO] {}", self.0, record.args().to_string()),
            log::Level::Debug => eprintln!("{}: [DEBUG] {}", self.0, record.args().to_string()),
            log::Level::Trace => eprintln!("{}: [TRACE] {}", self.0, record.args().to_string()),
        }
    }

    fn flush(&self) {
        let _ = retry_eintr(|| stderr().flush());
    }
}
