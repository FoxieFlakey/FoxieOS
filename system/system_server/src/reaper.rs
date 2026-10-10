use std::thread;

use nix::sys::wait::{WaitStatus, waitpid};

pub fn run_reaper() {
    thread::spawn(|| {
        println!("init: Reapear running");
        loop {
            match waitpid(None, None) {
                Ok(WaitStatus::Exited(pid, code)) => {
                    println!("init: (repear) Process {pid} exited with {code}");
                }
                Ok(_) | Err(_) => {
                    nix::unistd::sleep(5);
                }
            };
        }
    });
}
