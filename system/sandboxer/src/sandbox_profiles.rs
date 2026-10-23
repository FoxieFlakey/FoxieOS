// This contains set of functions usable for prep in sandbox creatoin
// for different profiles like app_profile, unconfined_profile, etc

use std::{fs, path::Path};

use anyhow::Context;
use foxie_base::util::retry_eintr;
use nix::{
    mount::{MsFlags, mount},
    unistd::Uid,
};

pub fn apply_profile(profile: foxie_base::sandboxer::Profile, uid: Uid) -> anyhow::Result<()> {
    match profile {
        foxie_base::sandboxer::Profile::Unconfined => unconfined_profile(uid),
        foxie_base::sandboxer::Profile::App => app_profile(uid),
    }
}

// This hids (in addition to base_profile and restrict_proc_profile)
// /app (should be mounted over later)
// /app_data (should be mounted over later)
//
// This create per sandbox's /tmp tmpfs limited at 32 MiB
// and 1000 inodes
fn app_profile(uid: Uid) -> anyhow::Result<()> {
    base_profile(uid).context("Applying base profile")?;
    restrict_proc_profile(uid).context("Applying restrict proc profile")?;

    hide_mount("/mnt")?;
    hide_mount("/data")?;
    hide_mount("/app")?;
    hide_mount("/app_data")?;

    Ok(())
}

fn restrict_proc_profile(_uid: Uid) -> anyhow::Result<()> {
    retry_eintr(|| {
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
    })
    .context("Cannot mount proc")?;
    Ok(())
}

fn unconfined_profile(_uid: Uid) -> anyhow::Result<()> {
    bind("/orig_root/dev", "/dev", true).context("Passing thru host devtmpfs")?;
    bind("/orig_root/proc", "/proc", true).context("Passing thru host devtmpfs")?;
    bind("/orig_root/sys", "/sys", true).context("Passing thru host devtmpfs")?;

    Ok(())
}

fn base_dev_stuffs<P>(uid: Uid, dev_path: P) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    let dev_path = dev_path.as_ref().to_path_buf();
    retry_eintr(|| fs::create_dir_all(dev_path.join("pts"))).context("Creating pts directory")?;
    retry_eintr(|| {
        mount(
            None::<&Path>,
            &dev_path.join("pts"),
            Some("devpts"),
            MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_NOSUID,
            Some(&*format!("max=500,mode=600,uid={uid},gid={uid}")),
        )
    })
    .context("Cannot create per sandbox devpts")?;

    bind("/orig_root/dev/binder", dev_path.join("binder"), false)
        .context("Mount bin /dev/binder")?;
    Ok(())
}

fn base_profile(uid: Uid) -> anyhow::Result<()> {
    retry_eintr(|| {
        mount(
            None::<&Path>,
            "/tmp",
            Some("tmpfs"),
            MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_NODEV | MsFlags::MS_NOSUID,
            Some(&*format!("size=32M,nr_inodes=1000,uid={uid},gid={uid}")),
        )
    })
    .context("Cannot create per sandbox /tmp")?;

    retry_eintr(|| {
        mount(
            None::<&Path>,
            "/dev",
            Some("tmpfs"),
            MsFlags::MS_NOATIME | MsFlags::MS_NODIRATIME | MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID,
            None::<&Path>,
        )
    })
    .context("Cannot mount dev")?;
    base_dev_stuffs(uid, "/dev").context("Mounting stuffs under /dev")?;
    Ok(())
}

fn bind<P1, P2>(src: P1, dest: P2, do_recurse: bool) -> anyhow::Result<()>
where
    P1: AsRef<Path>,
    P2: AsRef<Path>,
{
    let mut flags = MsFlags::MS_BIND;
    if do_recurse {
        flags |= MsFlags::MS_REC;
    }

    retry_eintr(|| {
        mount(
            Some(src.as_ref()),
            dest.as_ref(),
            None::<&Path>,
            flags,
            None::<&Path>,
        )
    })
    .with_context(|| {
        format!(
            "Bind mounting {} to {}",
            src.as_ref().display(),
            dest.as_ref().display()
        )
    })?;
    Ok(())
}

fn hide_mount(path: &str) -> Result<(), anyhow::Error> {
    let flags = MsFlags::MS_NOATIME
        | MsFlags::MS_NODIRATIME
        | MsFlags::MS_NODEV
        | MsFlags::MS_NOEXEC
        | MsFlags::MS_NOSUID;
    let empty_tmpfs_data = "size=0,nr_inodes=1,uid=0,gid=0";
    retry_eintr(|| {
        mount(
            None::<&Path>,
            path,
            Some("tmpfs"),
            flags,
            Some(empty_tmpfs_data),
        )
    })
    .with_context(|| format!("Cannot hide {path}"))?;
    Ok(())
}
