use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, Weak},
};

use anyhow::Context;
use foxie_base::{
    IObject, IToken,
    impls::Token,
    init::IInit,
    iobject,
    service_manager::{GetInitError, IService, IServiceManager, RegisterError, iservicemanager},
};
use libbinder::{
    ContextManagerInfo, Runtime,
    object::{B, ObjectTrait},
};
use libbinder_basic::packetable::Serde;
use parking_lot::RwLock;

use crate::init_task;

pub static SERVICE_MANAGER: OnceLock<Arc<B<ServiceManager>>> = OnceLock::new();

pub fn run<F>(manager_ready: F) -> anyhow::Result<()>
where
    F: FnOnce(&Arc<Runtime>) -> anyhow::Result<()>,
{
    let runtime = Runtime::new(
        foxie_base::MAIN_BINDER_PATH,
        ContextManagerInfo::Concrete(Box::new(|rt| {
            Ok(Arc::new_cyclic(|this| {
                B::new(ServiceManager {
                    rt: Arc::downgrade(rt),
                    this: this.clone(),
                    state: RwLock::new(State {
                        is_shutting_down: true,
                        services: HashMap::new(),
                    }),
                })
            }))
        })),
    )
    .context("Cannot initialize binder runtime")?;

    SERVICE_MANAGER
        .set(
            runtime
                .get_manager()
                .downcast_ref::<ServiceManager>()
                .unwrap()
                .this
                .upgrade()
                .unwrap(),
        )
        .ok()
        .unwrap();

    println!("service_manager: Ready!");
    manager_ready(&runtime)?;
    drop(runtime);
    Ok(())
}

struct State {
    services: HashMap<String, Service>,
    is_shutting_down: bool,
}

struct Service {
    service: Arc<B<dyn IService>>,
}

pub struct ServiceManager {
    rt: Weak<Runtime>,
    this: Weak<B<ServiceManager>>,
    state: RwLock<State>,
}

impl ServiceManager {
    fn unregister(&self, service: &str) {
        let mut state = self.state.write();
        if state.services.contains_key(service) {
            state.services.remove(service);
            println!("service_manager: Service {service} unregistered");
        }
    }
}

impl IObject for ServiceManager {
    fn has_interface(&self, name: &str) -> anyhow::Result<bool> {
        match name {
            iservicemanager::ID | iobject::ID => Ok(true),
            _ => Ok(false),
        }
    }
}

impl IServiceManager for ServiceManager {
    fn register_service(
        &self,
        name: &str,
        service: Arc<B<dyn IService>>,
    ) -> anyhow::Result<Result<Arc<B<dyn IToken>>, Serde<RegisterError>>> {
        let mut state = self.state.write();
        if state.is_shutting_down {
            return Ok(Err(Serde(RegisterError::PermissionDenied)));
        }

        if state.services.contains_key(name) {
            return Ok(Err(Serde(RegisterError::AlreadyExist)));
        }

        state.services.insert(name.to_string(), Service { service });
        drop(state);

        let this = self.this.clone();
        let name_cloned = name.to_string();

        let token = Token::new_with_death(self.rt.clone(), move || {
            if let Some(x) = this.upgrade() {
                x.unregister(&name_cloned)
            }
        });

        Ok(Ok(token))
    }

    fn get_service(&self, name: &str) -> anyhow::Result<Option<Arc<B<dyn IService>>>> {
        let state = self.state.read();
        if let Some(service) = state.services.get(name) {
            Ok(Some(service.service.clone()))
        } else {
            Ok(None)
        }
    }

    fn get_init(&self) -> anyhow::Result<Result<Arc<B<dyn IInit>>, Serde<GetInitError>>> {
        if let Some(init) = init_task::INIT.get() {
            Ok(Ok(init.clone()))
        } else {
            Ok(Err(Serde(GetInitError::InitNotReady)))
        }
    }
}

impl ObjectTrait for ServiceManager {
    fn get_remote<'a>(&'a self) -> Option<&'a libbinder::proxy::Proxy> {
        None
    }

    fn get_runtime(&self) -> std::sync::Arc<Runtime> {
        self.rt.upgrade().unwrap()
    }

    fn on_transaction(
        &self,
        code: u32,
        flags: enumflags2::BitFlags<libbinder::object::Flag>,
        message: &mut libbinder::packet::Packet,
    ) -> Result<Option<(u32, libbinder::packet::Packet)>, libbinder::object::TransactionError> {
        <dyn IServiceManager>::decode_and_dispatch(self, code, flags, message)
    }
}
