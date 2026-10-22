use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

pub struct Listeners<T> {
    id: u64,
    registry: HashMap<u64, Box<dyn FnMut(&T) + Send + Sync>>,
    next_callback_id: u64,
}

pub struct UnregisterToken {
    listener_id: u64,
    callback_id: u64,
}

impl<T> Default for Listeners<T> {
    fn default() -> Self {
        Self::new()
    }
}

static LISTENER_ID_GENERATOR: AtomicU64 = AtomicU64::new(0);

impl<T> Listeners<T> {
    pub fn new() -> Self {
        Self {
            id: LISTENER_ID_GENERATOR.fetch_add(1, Ordering::Relaxed),
            registry: HashMap::new(),
            next_callback_id: 0,
        }
    }

    pub fn register<F>(&mut self, func: F) -> UnregisterToken
    where
        F: FnMut(&T) + Send + Sync + 'static,
    {
        self.next_callback_id += 1;
        assert!(
            self.registry
                .insert(self.next_callback_id, Box::new(func))
                .is_none(),
            "no function can share same id"
        );

        UnregisterToken {
            listener_id: self.id,
            callback_id: self.next_callback_id,
        }
    }

    pub fn unregister(&mut self, token: UnregisterToken) {
        assert_eq!(
            self.id, token.listener_id,
            "Attempting to use token belonging to other listener!"
        );
        assert!(
            self.registry.remove(&token.callback_id).is_some(),
            "Token is duplicated"
        );
    }

    pub fn call_all(&mut self, val: T) {
        for callback in self.registry.values_mut() {
            callback(&val);
        }
    }
}
