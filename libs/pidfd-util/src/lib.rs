#![feature(linux_pidfd)]

use std::{
    ffi::{c_int, c_uint, c_void},
    os::{
        fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd},
        linux::process::PidFd,
    },
};

use anyhow::Context;
use nix::{
    errno::Errno,
    libc::{self, SYS_pidfd_send_signal, syscall},
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::{signal::Signal, wait::WaitStatus},
    unistd::{Pid, dup},
};

mod private {
    pub trait Sealed {}
}

pub trait PidFdExtFoxie: private::Sealed + Sized {
    fn get_termination_reason(&self) -> Result<WaitStatus, Errno>;
    fn wait_for_termination_reason(&self) -> anyhow::Result<WaitStatus>;
    fn send_signal(&self, sig: Signal) -> Result<(), Errno>;
    fn dup(&self) -> Result<Self, Errno>;
}

impl private::Sealed for PidFd {}
impl PidFdExtFoxie for PidFd {
    fn dup(&self) -> Result<Self, Errno> {
        let duped = dup(self.as_fd())?;

        // SAFETY: This is valid fd, we just dup'ed it
        Ok(unsafe { PidFd::from_raw_fd(duped.into_raw_fd()) })
    }

    fn wait_for_termination_reason(&self) -> anyhow::Result<WaitStatus> {
        let mut pollfds = [PollFd::new(
            self.as_fd(),
            PollFlags::POLLIN | PollFlags::POLLHUP,
        )];

        loop {
            loop {
                let Err(e) = poll(&mut pollfds, PollTimeout::NONE) else {
                    break;
                };

                if e == Errno::EINTR {
                    // Interrupted
                    continue;
                }
                Err::<(), _>(e).context("Cannot poll for pidfd")?;
            }

            if !pollfds[0].revents().unwrap().is_empty() {
                // Process either reaped or dead
                break;
            }
        }

        Ok(self
            .get_termination_reason()
            .context("Cannot get termination reason")?)
    }

    fn get_termination_reason(&self) -> Result<WaitStatus, Errno> {
        let mut info: libc::pidfd_info = unsafe { std::mem::zeroed() };
        info.mask = libc::__u64::from(libc::PIDFD_INFO_EXIT | libc::PIDFD_INFO_PID);

        loop {
            if unsafe { libc::ioctl(self.as_raw_fd(), libc::PIDFD_GET_INFO, &mut info) } < 0 {
                if Errno::last() == Errno::EINTR {
                    continue;
                }
                return Err(Errno::last());
            }
            break;
        }

        if info.mask & libc::__u64::from(libc::PIDFD_INFO_EXIT | libc::PIDFD_INFO_PID) == 0 {
            return Err(Errno::EINVAL);
        }

        Ok(WaitStatus::from_raw(
            Pid::from_raw(info.pid as i32),
            info.exit_code,
        )?)
    }

    fn send_signal(&self, sig: Signal) -> Result<(), Errno> {
        loop {
            let res = unsafe {
                syscall(
                    SYS_pidfd_send_signal,
                    c_int::from(self.as_raw_fd()),
                    c_int::from(sig as i32),
                    0 as *mut c_void,
                    c_uint::from(0u32),
                )
            };

            if res == -1 {
                if Errno::last() == Errno::EINTR {
                    continue;
                }
                return Err(Errno::last());
            } else {
                assert!(res == 0);
                return Ok(());
            }
        }
    }
}
