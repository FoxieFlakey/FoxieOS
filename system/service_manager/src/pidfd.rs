use std::os::fd::{AsFd, AsRawFd};

use anyhow::Context;
use nix::{
    errno::Errno,
    libc,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::wait::WaitStatus,
    unistd::Pid,
};
use pidfd_util::PidFd;

mod private {
    pub trait Sealed {}
}

pub trait PidFdExtFoxie: private::Sealed {
    fn get_termination_reason(&self) -> Result<WaitStatus, Errno>;
    fn wait_for_termination_reason(&self) -> anyhow::Result<WaitStatus>;
}

impl private::Sealed for PidFd {}
impl PidFdExtFoxie for PidFd {
    fn wait_for_termination_reason(&self) -> anyhow::Result<WaitStatus> {
        let mut pollfds = [PollFd::new(
            self.as_fd(),
            PollFlags::POLLIN | PollFlags::POLLHUP,
        )];

        loop {
            poll(&mut pollfds, PollTimeout::NONE).context("Cannot poll for pidfd")?;

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

        if unsafe { libc::ioctl(self.as_raw_fd(), libc::PIDFD_GET_INFO, &mut info) } < 0 {
            return Err(Errno::last());
        }

        if info.mask & libc::__u64::from(libc::PIDFD_INFO_EXIT | libc::PIDFD_INFO_PID) == 0 {
            return Err(Errno::EINVAL);
        }

        Ok(WaitStatus::from_raw(
            Pid::from_raw(info.pid as i32),
            info.exit_code,
        )?)
    }
}
