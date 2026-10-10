// This contains set of functions usable for prep in sandbox creatoin
// for different profiles like app_profile, unconfined_profile, etc

use std::path::Path;

use anyhow::Context;
use nix::{
    mount::{MsFlags, mount},
    unistd::Uid,
};

// This hids (in addition to base_profile and restrict_proc_profile)
// /app (should be mounted over later)
// /app_data (should be mounted over later)
//
// This create per sandbox's /tmp tmpfs limited at 32 MiB
// and 1000 inodes
pub fn app_profile(uid: Uid) -> anyhow::Result<()> {
    base_profile(uid).context("Applying base profile")?;
    restrict_proc_profile(uid).context("Applying restrict proc profile")?;

    hide_mount("/mnt")?;
    hide_mount("/data")?;
    hide_mount("/app")?;
    hide_mount("/app_data")?;

    Ok(())
}

pub fn restrict_proc_profile(_uid: Uid) -> anyhow::Result<()> {
    mount(
        None::<&Path>,
        "/proc",
        Some("proc"),
        MsFlags::MS_NOATIME
            | MsFlags::MS_NODIRATIME
            | MsFlags::MS_NODEV
            | MsFlags::MS_NOEXEC
            | MsFlags::MS_NOSUID,
        Some("hidepid=2"),
    )
    .context("Cannot mount proc")?;
    Ok(())
}

pub fn base_profile(uid: Uid) -> anyhow::Result<()> {
    let uid = uid.as_raw();
    mount(
        None::<&Path>,
        "/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
        Some(&*format!("size=32M,nr_inodes=1000,uid={uid},gid={uid}")),
    )
    .context("Cannot create per sandbox /tmp")?;
    Ok(())
}

fn hide_mount(path: &str) -> Result<(), anyhow::Error> {
    let flags = MsFlags::MS_NOATIME
        | MsFlags::MS_NODIRATIME
        | MsFlags::MS_NODEV
        | MsFlags::MS_NOEXEC
        | MsFlags::MS_NOSUID;
    let empty_tmpfs_data = "size=0,nr_inodes=1,uid=0,gid=0";
    mount(
        None::<&Path>,
        path,
        Some("tmpfs"),
        flags,
        Some(empty_tmpfs_data),
    )
    .with_context(|| format!("Cannot hide {path}"))?;
    Ok(())
}
