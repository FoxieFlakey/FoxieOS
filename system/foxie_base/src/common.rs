use std::ffi::c_int;

use anyhow::Context;
use libbinder_basic::packetable::Packetable;
use nix::{sys::signal::Signal, unistd::Pid};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone)]
pub struct SerdeSignal(pub Signal);

impl<'a> Deserialize<'a> for SerdeSignal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a>,
    {
        let sig = <c_int as Deserialize>::deserialize(deserializer)?;
        Ok(Self(Signal::try_from(sig).map_err(|x| {
            serde::de::Error::custom(format!("Invalid signal: {x}"))
        })?))
    }
}

impl Serialize for SerdeSignal {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        <c_int as Serialize>::serialize(&(self.0 as c_int), serializer)
    }
}

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

pub struct SerdePid(pub Pid);

impl<'a> Packetable<'a> for SerdePid {
    fn serialize(&self, writer: &mut libbinder_basic::writer::Writer) -> anyhow::Result<()> {
        writer.write_bytes_raw(&self.0.as_raw().to_ne_bytes());
        Ok(())
    }

    fn deserialize(reader: &mut libbinder_basic::reader::Reader<'a>) -> anyhow::Result<Self> {
        let mut pid = [0; size_of::<nix::libc::pid_t>()];
        reader.read_bytes_raw(&mut pid)?;
        Ok(Self(Pid::from_raw(i32::from_ne_bytes(pid))))
    }
}
