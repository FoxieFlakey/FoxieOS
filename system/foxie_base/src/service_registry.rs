use std::sync::{Arc, oneshot};

use anyhow::Context;
use libbinder::{
    object::{B, ObjectTrait},
    proxy::Proxy,
};
use libbinder_basic::{TryFromProxy, binder_ipc_object, packetable::Serde};
use serde::{Deserialize, Serialize};

use crate::{
    IObject, IToken, iobject,
    util::{FnOnceObject, IFnOnce},
};

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

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum WaitServiceError {
    #[error("Too many waits")]
    TooManyWaits,
    #[error("Permission dnied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IServiceRegistry")]
pub trait IServiceRegistry: IObject {
    #[must_use = "Service would be automatically unregistred if token dropped"]
    fn register_service(
        &self,
        name: &str,
        service: Arc<B<dyn IService>>,
    ) -> anyhow::Result<Result<Arc<B<dyn IToken>>, Serde<RegisterError>>>;
    fn get_service(&self, name: &str) -> anyhow::Result<Option<Arc<B<dyn IService>>>>;
    fn wait_service(
        &self,
        name: &str,
        callback: Arc<B<dyn IFnOnce>>,
    ) -> anyhow::Result<Result<(), Serde<WaitServiceError>>>;
    fn get_ctl(
        &self,
    ) -> anyhow::Result<Result<Arc<B<dyn IServiceManagerCtl>>, Serde<GetServiceManagerCtlError>>>;
}

pub fn wait_and_get_service<T>(
    manager: &dyn IServiceRegistry,
    service: &str,
) -> anyhow::Result<Result<anyhow::Result<Arc<B<T>>>, WaitServiceError>>
where
    T: TryFromProxy + ?Sized,
{
    loop {
        let (sender, receiver) = oneshot::channel();
        let ret = manager
            .wait_service(
                service,
                FnOnceObject::new(Arc::downgrade(&manager.get_runtime()), || {
                    // Dont really care if waiter is uninterested
                    let _ = sender.send(());
                }),
            )
            .context("Cannot send transacton for waiting")?
            .map_err(|Serde(x)| x);
        if let Err(e) = ret {
            return Ok(Err(e));
        }

        receiver.recv().expect("The waker disappeared");

        if let Some(service) = manager
            .get_service(service)
            .context("Cannot send transaction for getting service")?
        {
            let proxy = match service.get_remote() {
                Some(x) => x.clone(),
                None => Proxy::from_object(service),
            };

            return Ok(Ok(T::try_from_proxy(proxy).map_err(|e| e.into())));
        }
    }
}

#[derive(Serialize, Deserialize, thiserror::Error, Debug)]
pub enum StopError {
    #[error("Permission denied")]
    PermissionDenied,
}

#[binder_ipc_object(interface_id = "foxie.IService")]
pub trait IService: IObject {
    fn stop(&self) -> anyhow::Result<anyhow::Result<()>>;
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
