use std::{
    ffi::c_int,
    ops::{Deref, DerefMut},
    os::{
        fd::{AsFd, OwnedFd},
        linux::process::PidFd,
    },
};

use anyhow::{Context, bail};
use libbinder_basic::packetable::Packetable;
use nix::sys::{signal::Signal, statfs::fstatfs};

pub struct SafePidFd(PidFd);

impl SafePidFd {
    pub fn into_inner(self) -> PidFd {
        self.0
    }
}

impl Deref for SafePidFd {
    type Target = PidFd;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SafePidFd {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<PidFd> for SafePidFd {
    fn from(value: PidFd) -> Self {
        Self(value)
    }
}

impl TryFrom<OwnedFd> for SafePidFd {
    type Error = anyhow::Error;

    fn try_from(fd: OwnedFd) -> Result<Self, Self::Error> {
        // Check if the fd is indeed proper pidfd. Checking with fstatfs
        // to check if fs type is pidfd there magic from include/uapi/linux/magic.h
        const PID_FS_MAGIC: u64 = 0x50494446;
        let stat = fstatfs(fd.as_fd()).context("Cannot fstatfs")?;
        if stat.filesystem_type().0 != PID_FS_MAGIC {
            bail!("Fd received is not pidfd");
        }

        Ok(SafePidFd(PidFd::from(fd)))
    }
}

impl Packetable<'_> for SafePidFd {
    fn serialize(&self, writer: &mut libbinder_basic::writer::Writer) -> anyhow::Result<()> {
        writer.write_fd(self.0.as_fd())
    }

    fn deserialize(reader: &mut libbinder_basic::reader::Reader<'_>) -> anyhow::Result<Self> {
        Self::try_from(reader.read_fd()?)
    }
}

pub struct SerdeSignal(pub Signal);

impl<'a> Packetable<'a> for SerdeSignal {
    fn serialize(&self, writer: &mut libbinder_basic::writer::Writer) -> anyhow::Result<()> {
        writer.write_bytes_raw(&(self.0 as c_int).to_ne_bytes());
        Ok(())
    }

    fn deserialize(reader: &mut libbinder_basic::reader::Reader<'a>) -> anyhow::Result<Self> {
        let mut sig = [0; size_of::<c_int>()];
        reader.read_bytes_raw(&mut sig)?;
        Ok(Self(
            Signal::try_from(c_int::from_ne_bytes(sig)).context("Invalid signal")?,
        ))
    }
}
