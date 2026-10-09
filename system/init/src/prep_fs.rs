use std::{fs, path::Path};

use anyhow::Context;
use nix::mount::{MsFlags, mount};

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
        flags,
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
    Ok(())
}
