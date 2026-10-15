use std::sync::Arc;

use libbinder::object::{B, ObjectTrait};
use libbinder_basic::{binder_ipc_object, packetable::Serde};
use serde::{Deserialize, Serialize};

use crate::{IObject, IToken, iobject};

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum GetServiceManagerCtlError {
    #[error("Permission denied")]
    PermissionDenied,
    #[error("Service manager is not ready")]
    ServiceManagerNotReady,
}

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum RegisterError {
    #[error("Service already existed")]
    AlreadyExist,
    #[error("Permission dnied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IServiceManager")]
pub trait IServiceManager: IObject {
    #[must_use = "Service would be automatically unregistred if token dropped"]
    fn register_service(
        &self,
        name: &str,
        service: Arc<B<dyn IService>>,
    ) -> anyhow::Result<Result<Arc<B<dyn IToken>>, Serde<RegisterError>>>;
    fn get_service(&self, name: &str) -> anyhow::Result<Option<Arc<B<dyn IService>>>>;
    fn get_ctl(
        &self,
    ) -> anyhow::Result<Result<Arc<B<dyn IServiceManagerCtl>>, Serde<GetServiceManagerCtlError>>>;
}

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum StopError {
    #[error("Permission denied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IService")]
pub trait IService: IObject {
    fn stop(&self) -> anyhow::Result<Result<(), Serde<GetServiceManagerCtlError>>>;
}

#[derive(Serialize, Deserialize, Debug, thiserror::Error)]
pub enum ShutdownError {
    #[error("Permission denied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IServiceManagerCtl")]
pub trait IServiceManagerCtl: IObject {
    fn request_shutdown(&self) -> anyhow::Result<Result<(), Serde<ShutdownError>>>;
    fn ping(&self) -> anyhow::Result<Serde<String>>;
}
