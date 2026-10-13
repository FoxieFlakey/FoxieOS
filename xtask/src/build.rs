use std::process::Command;

use anyhow::Context;
use clap::Args;

use crate::util;

mod syslib;

#[derive(Args)]
pub struct BuildArgs {}

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

    // Completely new /dev tmpfs, filled only necessary stuffs like
    // /dev/null, /dev/full, /dev/zero, /dev/random, /dev/urandom
    // and few others. Does not use kernel's builtin devtmpfs
    util::mkdir(crate::rootfs_dir().join("dev")).context("Creating dev dir")?;

    // Heavily censored with subset=pid hidepid=2
    // Does not apply to /init
    util::mkdir(crate::rootfs_dir().join("proc")).context("Creating proc dir")?;

    // Also censored by userspace FUSE daemon, only allowing few
    // sysfs exposed
    // Does not apply to /init
    util::mkdir(crate::rootfs_dir().join("sys")).context("Creating sys dir")?;

    // Each app gets own limited tmpfs
    util::mkdir(crate::rootfs_dir().join("tmp")).context("Creating tmp dir")?;

    // tmpfs containing mountpoints for each data
    // the /data/primary is primary storage, system
    // will not boot without it
    // while /data/{uuid}/ is expanded storage
    util::mkdir(crate::rootfs_dir().join("data")).context("Creating data dir")?;

    // Yo, gcc, you're bedrock overything
    util::mkdir(crate::rootfs_dir().join("lib")).context("Creating lib dir")?;
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

    // tmpfs, containing mountpoints. Only visible to system_server
    util::mkdir(crate::rootfs_dir().join("mnt")).context("Creating mnt dir")?;

    // most of time its empty, only mounted during initializing sandbox
    util::mkdir(crate::rootfs_dir().join("sandbox_root")).context("Creating sandbox_root dir")?;

    // Each sandboxed app/daemon gets private /app_data to store its data
    // and /app is for static read only resources
    // For init, these are empty
    util::mkdir(crate::rootfs_dir().join("app")).context("Creating app dir")?;
    util::mkdir(crate::rootfs_dir().join("app_data")).context("Creating app_data dir")?;

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
    util::mkdir(crate::rootfs_dir().join("etc")).unwrap();
    dircpy::CopyBuilder::new("./extra/terminfo", crate::rootfs_dir().join("etc/terminfo"))
        .overwrite_if_newer(true)
        .overwrite_if_size_differs(true)
        .run()
        .context("Copying terminfo for htop")?;

    util::copy_file(
        crate::target_dir().join("x86_64-foxieos-linux-musl/debug/init"),
        crate::rootfs_dir().join("init"),
    )
    .context("Copying init")?;
    util::copy_file(
        crate::target_dir().join("x86_64-foxieos-linux-musl/debug/system_server"),
        crate::rootfs_dir().join("system/bin/system_server"),
    )
    .context("Copying system_server")?;
    util::copy_file(
        crate::target_dir().join("x86_64-foxieos-linux-musl/debug/tty_spawner"),
        crate::rootfs_dir().join("system/bin/tty_spawner"),
    )
    .context("Copying tty_spawner")?;
    util::copy_file(
        crate::target_dir().join("x86_64-foxieos-linux-musl/debug/initctl"),
        crate::rootfs_dir().join("system/bin/initctl"),
    )
    .context("Copying initctl")?;
    util::copy_file(
        crate::target_dir().join("x86_64-foxieos-linux-musl/debug/sandbox_helper"),
        crate::rootfs_dir().join("system/bin/sandbox_helper"),
    )
    .context("Copying sandbox_helper")?;

    Command::new("mkfs.erofs")
        .arg("--all-root")
        .arg(crate::rootfs_image_file())
        .arg(crate::rootfs_dir().to_str().unwrap())
        .env("RUSTFLAGS", "-C relocation-model=static")
        .status()
        .context("Waiting for mkfs.erofs result")?
        .exit_ok()
        .context("Executing mkfs.erofs")?;

    Ok(())
}
