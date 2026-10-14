use std::{
    fs::{self, File},
    path::Path,
};

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

    // NOTE: devtmpfs is mounted by init, we dont need to do anything here

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

    File::create_new("/dev/binder").context("Cannot create empty file at /dev/binder")?;

    mount(
        Some("/dev/binderfs/binder"),
        "/dev/binder",
        None::<&Path>,
        MsFlags::MS_BIND,
        None::<&Path>,
    )
    .context("Mount binding primary binder")?;

    // for mnt in ["/dev", "/sys", "/proc"] {
    //     mount(
    //         None::<&Path>,
    //         mnt,
    //         None::<&Path>,
    //         MsFlags::MS_SHARED,
    //         None::<&Path>,
    //     )
    //     .with_context(|| format!("Making {mnt} shareable"))?;
    // }

    root_dev::discover_root().context("Cannot find root device")?;
    Ok(())
}
