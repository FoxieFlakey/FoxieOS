use std::thread;

use nix::sys::wait::{WaitStatus, waitpid};

pub fn run_reaper() {
    thread::spawn(|| {
        println!("init: Reapear running");
        loop {
            if let Ok(WaitStatus::Exited(pid, code)) = waitpid(None, None) {
                println!("init: (repear) Process {pid} exited with {code}");
            }
        }
    });
}
