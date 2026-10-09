use std::{
    convert::Infallible,
    fs::File,
    io::Write,
    os::{
        fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::Command,
};

use anyhow::Context;
use caps::CapSet;
use clap::Parser;
use nix::{
    fcntl::{FcntlArg, FdFlag, fcntl},
    sched::{CloneFlags, setns},
    sys::prctl,
    unistd::{chroot, fchdir, setgid, setgroups, setresuid},
};

#[derive(Parser)]
pub struct Cli {
    #[arg(long)]
    binary: String,

    #[arg(long)]
    uid: u32,
    #[arg(long)]
    gid: u32,
    #[arg(long)]
    arg0: Option<String>,

    #[arg(long)]
    procs_fd: u32,
    #[arg(long)]
    mnt_namespace: u32,
    #[arg(long)]
    root_fd: u32,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    argv: Vec<String>,
}

fn set_cloexec(fd: BorrowedFd<'_>) -> anyhow::Result<()> {
    let mut flags =
        FdFlag::from_bits_retain(fcntl(fd, FcntlArg::F_GETFD).context("Getting FD flags")?);
    flags |= FdFlag::FD_CLOEXEC;
    fcntl(fd, FcntlArg::F_SETFD(flags)).context("Setting O_CLOEXEC flags")?;
    Ok(())
}

fn main() -> Result<Infallible, anyhow::Error> {
    let cli = Cli::parse();
    let mut cmd = Command::new(&cli.binary);
    if let Some(arg0) = &cli.arg0 {
        cmd.arg0(arg0);
    }

    cmd.args(&cli.argv);

    prctl::set_no_new_privs().context("Cannot set PR_SET_NO_NEW_PRIVS")?;
    caps::securebits::set_keepcaps(false).context("Cannot clear keepcaps flag")?;

    // SAFETY: Caller passed the FD, we own it
    let procs_fd =
        unsafe { OwnedFd::from_raw_fd(cli.procs_fd.try_into().context("Checking procs_fd fd")?) };
    set_cloexec(procs_fd.as_fd()).context("Setting close on exec for procs_fd")?;

    // SAFETY: Caller passed the FD, we own it
    let mnt_namespace = unsafe {
        OwnedFd::from_raw_fd(
            cli.mnt_namespace
                .try_into()
                .context("Checking mnt_namespace fd")?,
        )
    };
    set_cloexec(mnt_namespace.as_fd()).context("Setting close on exec for mount namespace")?;

    let mut procs_fd = File::from(procs_fd);
    procs_fd
        .write_all(b"0\n")
        .context("Cannot enter sandboxed cgroup")?;

    setns(mnt_namespace.as_fd(), CloneFlags::CLONE_NEWNS).context("Cannot set mount namespace")?;
    let root_fd =
        unsafe { OwnedFd::from_raw_fd(cli.root_fd.try_into().context("Checking root_fd fd")?) };
    set_cloexec(procs_fd.as_fd()).context("Setting close on exec for root_fd")?;

    // now chroot
    fchdir(root_fd.as_fd()).context("Cannot fchdir to new root")?;
    chroot(".").context("Cannot chroot to new root")?;

    // Drop groups
    setgroups(&[]).context("Cannot clear supplementary groups")?;
    setgid(cli.gid.into()).context("Cannot call setgid")?;

    // Drops all capability
    caps::clear(None, CapSet::Ambient).context("Cannot clear ambient capabilities")?;
    caps::clear(None, CapSet::Bounding).context("Cannot clear bounding capabilities")?;
    caps::clear(None, CapSet::Inheritable).context("Cannot clear inheritable capabilities")?;

    // Then finally drop the UID
    setresuid(cli.uid.into(), cli.uid.into(), cli.uid.into()).context("Cannot call setresuid")?;

    // And clear the SETUID capability
    caps::clear(None, CapSet::Permitted).context("Cannot clear permitted capabilities")?;
    caps::clear(None, CapSet::Effective).context("Cannot clear effective capabilities")?;
    Err(cmd.exec()).context("Cannot exec")?
}
