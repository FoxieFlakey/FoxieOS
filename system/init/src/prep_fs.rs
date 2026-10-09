use std::{fs, path::Path};

use anyhow::Context;
use nix::{
    fcntl::AT_FDCWD,
    mount::{MsFlags, mount},
    sys::stat::{FchmodatFlags, Mode, fchmodat},
};

use crate::root_dev;

pub fn run() -> anyhow::Result<()> {
    let flags = MsFlags::MS_NOATIME
        | MsFlags::MS_NODIRATIME
        | MsFlags::MS_NODEV
        | MsFlags::MS_NOEXEC
        | MsFlags::MS_NOSUID;
    mount(
        None::<&Path>,
        "/tmp",
        Some("tmpfs"),
        flags,
        Some("size=32M"),
    )
    .context("Mounting init's /tmp")?;
    mount(
        None::<&Path>,
        "/mnt",
        Some("tmpfs"),
        flags,
        Some("size=32M"),
    )
    .context("Mounting /mnt tmpfs")?;

    mount(
        None::<&Path>,
        "/dev",
        Some("devtmpfs"),
        // Devtmpfs well contains devices
        flags & !MsFlags::MS_NODEV,
        None::<&Path>,
    )
    .context("Mounthing /dev")?;
    mount(None::<&Path>, "/proc", Some("proc"), flags, None::<&Path>).context("Mounthing /proc")?;
    mount(None::<&Path>, "/sys", Some("sysfs"), flags, None::<&Path>).context("Mounthing /sys")?;
    mount(
        None::<&Path>,
        "/sys/fs/cgroup",
        Some("cgroup2"),
        flags,
        None::<&Path>,
    )
    .context("Mounthing /sys/fs/cgroup")?;

    fs::create_dir_all("/dev/binderfs").context("Creating binderfs")?;

    mount(
        None::<&Path>,
        "/dev/binderfs",
        Some("binder"),
        // The files under binderfs is inherently special device
        flags & !MsFlags::MS_NODEV,
        Some("stats=global"),
    )
    .context("Mounting /dev/binderfs")?;

    fchmodat(
        AT_FDCWD,
        "/dev/binderfs/binder",
        Mode::from_bits_retain(0o666),
        FchmodatFlags::NoFollowSymlink,
    )
    .context("Cannot chmod binder file")?;

    root_dev::discover_root().context("Cannot find root device")?;
    Ok(())
}
