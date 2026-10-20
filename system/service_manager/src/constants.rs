use std::sync::{Arc, OnceLock};

use foxie_base::sandboxer::ISandboxService;
use libbinder::object::B;

pub static SANDBOXER: OnceLock<Arc<B<dyn ISandboxService>>> = OnceLock::new();

pub fn get_sandboxer() -> &'static Arc<B<dyn ISandboxService>> {
    SANDBOXER.get().unwrap()
}
