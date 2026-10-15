use std::{
    process::Child,
    sync::{Arc, OnceLock},
};

use foxie_base::sandboxer::ISandboxService;
use libbinder::object::B;
use parking_lot::Mutex;

pub static SANDBOXER: OnceLock<Arc<B<dyn ISandboxService>>> = OnceLock::new();
pub static SANDBOXER_CHILD: Mutex<Option<Child>> = Mutex::new(None);

pub fn get_sandboxer() -> &'static Arc<B<dyn ISandboxService>> {
    SANDBOXER.get().unwrap()
}
