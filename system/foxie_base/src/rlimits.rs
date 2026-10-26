use anyhow::{Context, bail};
use nix::{errno::Errno, libc};
use rlimit::Resource;
use serde::{Deserialize, Serialize};

use crate::util::retry_eintr;

pub static INIT: RLimits = RLimits { nice: Some(-20) };
pub static SANDBOXER: RLimits = RLimits { nice: Some(-20) };
pub static SERVICE_MANAGER: RLimits = RLimits { nice: Some(-10) };
pub static SHELL: RLimits = RLimits { nice: Some(0) };
pub static GRAPHICS_SERVER: RLimits = RLimits { nice: Some(-1) };

// None on each field mean leave as it is
#[derive(Serialize, Deserialize, Default, Clone, Copy)]
pub struct RLimits {
    pub nice: Option<i32>,
}

impl RLimits {
    pub fn apply(&self) -> anyhow::Result<()> {
        match self.nice {
            Some(niceness) => {
                if niceness < -20 || niceness > 19 {
                    bail!("Invalid niceness: {niceness}")
                }

                retry_eintr(|| {
                    rlimit::setrlimit(
                        Resource::NICE,
                        (20 - niceness) as u64,
                        (20 - niceness) as u64,
                    )
                })?;

                // Update current priority to clamp or nothing
                let current_prio = loop {
                    Errno::from_raw(0).set();
                    let ret = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
                    if Errno::last_raw() != 0 {
                        if Errno::last() == Errno::EINTR {
                            continue;
                        }
                        return Err(Errno::last()).context("Cannot get current process priority");
                    } else {
                        break ret;
                    }
                };

                if current_prio < niceness {
                    // Process is out of rlimit range, clamp it to max
                    loop {
                        let ret = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, niceness) };
                        if ret == -1 {
                            if Errno::last() == Errno::EINTR {
                                continue;
                            }

                            return Err(Errno::last())
                                .context("Cannot set current process priority to be inside limit");
                        } else if ret == 0 {
                            break;
                        }
                    }
                }
            }
            None => todo!(),
        }
        Ok(())
    }
}
