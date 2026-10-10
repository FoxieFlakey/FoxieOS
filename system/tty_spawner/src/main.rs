use std::{
    convert::Infallible,
    os::{
        fd::{AsFd, AsRawFd},
        unix::process::CommandExt,
    },
    process::Command,
};

use anyhow::Context;
use clap::Parser;
use nix::{
    fcntl::{OFlag, open},
    ioctl_write_int_bad,
    sys::stat::Mode,
    unistd::{dup2_stderr, dup2_stdin, dup2_stdout, setsid},
};

#[derive(Parser)]
pub struct Cli {
    #[arg(long)]
    binary: String,

    #[arg(long)]
    tty_dev: String,

    #[arg(long)]
    arg0: Option<String>,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    argv: Vec<String>,
}

ioctl_write_int_bad!(tiocsctty, nix::libc::TIOCSCTTY);

fn main() -> anyhow::Result<Infallible> {
    let cli = Cli::parse();
    let mut cmd = Command::new(&cli.binary);
    if let Some(arg0) = &cli.arg0 {
        cmd.arg0(arg0);
    }

    cmd.args(&cli.argv);

    let tty = open(&*cli.tty_dev, OFlag::O_RDWR, Mode::empty())
        .with_context(|| format!("Cannot open tty at '{}'", cli.tty_dev))?;

    setsid().context("Cannot call setsid")?;
    unsafe { tiocsctty(tty.as_raw_fd(), 0) }.context("Cannot do ioctl TIOCSCTTY")?;

    dup2_stderr(tty.as_fd()).context("Cannot do dup2 for stderr")?;
    dup2_stdout(tty.as_fd()).context("Cannot do dup2 for stdout")?;
    dup2_stdin(tty.as_fd()).context("Cannot do dup2 for stdin")?;

    Err(cmd.exec()).with_context(|| {
        format!(
            "Cannot exec command '{}' (args not placed here)",
            cli.binary
        )
    })
}
