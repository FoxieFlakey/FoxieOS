use std::os::fd::AsRawFd;

use nix::{errno::Errno, libc, sys::wait::WaitStatus, unistd::Pid};
use pidfd_util::PidFd;

mod private {
    pub trait Sealed {}
}

pub trait PidFdExtFoxie: private::Sealed {
    fn get_termination_reason(&self) -> Result<WaitStatus, Errno>;
}

impl private::Sealed for PidFd {}
impl PidFdExtFoxie for PidFd {
    fn get_termination_reason(&self) -> Result<WaitStatus, Errno> {
        let mut info: libc::pidfd_info = unsafe { std::mem::zeroed() };
        info.mask = libc::PIDFD_INFO_EXIT.into();

        if unsafe { libc::ioctl(self.as_raw_fd(), libc::PIDFD_GET_INFO, &mut info) } < 0 {
            return Err(Errno::last());
        }

        if info.mask & libc::__u64::from(libc::PIDFD_INFO_EXIT) == 0 {
            return Err(Errno::EINVAL);
        }

        Ok(WaitStatus::from_raw(
            Pid::from_raw(info.pid as i32),
            info.exit_code,
        )?)
    }
}
