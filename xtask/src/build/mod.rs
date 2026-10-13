use std::process::Command;

use anyhow::Context;
use clap::Args;

use crate::util;

mod syslib;

#[derive(Args)]
pub struct BuildArgs {}

static CRATES: &[&str] = &[
    "init",
    "initctl",
    "sandbox_helper",
    "system_server",
    "tty_spawner",
];

static DIRS: &[&str] = &[
    "proc",
    "sys",
    "dev",
    "app",
    "app_data",
    "sandbox_root",
    "etc",
    "system",
    "tmp",
    "mnt",
];

pub fn main(_args: BuildArgs) -> anyhow::Result<()> {
    println!("Creating rootfs");
    util::mkdir(crate::rootfs_dir()).context("Creating rootfs dir")?;

    println!("Building syslibs");
    syslib::build().context("Building syslib")?;

    Command::new("cargo")
        .arg("build")
        .arg("-Zjson-target-spec")
        .arg("-Zbuild-std=core,alloc,std,panic_unwind")
        .arg("--target")
        .arg("x86_64-foxieos-linux-musl.json")
        .status()
        .context("Waiting for cargo build result")?
        .exit_ok()
        .context("Building system binaries")?;

    for dir in DIRS {
        util::mkdir(crate::rootfs_dir().join(dir))
            .with_context(|| format!("Creating dir {dir}"))?;
    }

    util::copy_file(
        "./extra/libgcc_s.so",
        crate::rootfs_dir().join("lib/libgcc_s.so.1"),
    )
    .context("Copying libgcc_s.so")?;
    util::copy_file(
        "./extra/libgcc_s.so",
        crate::rootfs_dir().join("lib/libgcc_s.so"),
    )
    .context("Copying libgcc_s.so")?;

    println!("Copying busybox");
    util::copy_file(
        "./extra/busybox",
        crate::rootfs_dir().join("system/bin/busybox"),
    )
    .context("Copying busybox")?;
    dircpy::CopyBuilder::new(
        "./extra/busybox_overlay",
        crate::rootfs_dir().join("system/busybox_bins"),
    )
    .overwrite_if_newer(true)
    .overwrite_if_size_differs(true)
    .run()
    .context("Copying busybox_overlay")?;

    println!("Copying htop");
    util::copy_file("./extra/htop", crate::rootfs_dir().join("system/bin/htop"))
        .context("Copying htop")?;

    println!("Copying terminfo");
    dircpy::CopyBuilder::new("./extra/terminfo", crate::rootfs_dir().join("etc/terminfo"))
        .overwrite_if_newer(true)
        .overwrite_if_size_differs(true)
        .run()
        .context("Copying terminfo for htop")?;

    for c in CRATES {
        util::copy_file(
            crate::target_dir().join(format!("x86_64-foxieos-linux-musl/debug/{}", c)),
            crate::rootfs_dir().join(format!("system/bin/{c}")),
        )
        .with_context(|| format!("Copying {c}"))?;
    }

    Command::new("mkfs.erofs")
        .arg("--all-root")
        .arg(crate::rootfs_image_file())
        .arg(crate::rootfs_dir())
        .env("RUSTFLAGS", "-C relocation-model=static")
        .status()
        .context("Waiting for mkfs.erofs result")?
        .exit_ok()
        .context("Executing mkfs.erofs")?;

    Ok(())
}
