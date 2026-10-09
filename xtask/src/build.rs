use std::process::{Command, exit};

use clap::Args;

use crate::util;

#[derive(Args)]
pub struct BuildArgs {}

pub fn main(_args: BuildArgs) {
    let status = Command::new("cargo")
        .arg("build")
        .arg("--target")
        .arg("x86_64-unknown-linux-musl")
        .env("RUSTFLAGS", "-C relocation-model=static")
        .status()
        .expect("Failed to execute cargo build");

    if !status.success() {
        exit(status.code().unwrap_or(1));
    }

    println!("Creating rootfs");
    util::mkdir(crate::rootfs_dir()).unwrap();

    // Completely new /dev tmpfs, filled only necessary stuffs like
    // /dev/null, /dev/full, /dev/zero, /dev/random, /dev/urandom
    // and few others. Does not use kernel's builtin devtmpfs
    util::mkdir(crate::rootfs_dir().join("dev")).unwrap();

    // Heavily censored with subset=pid hidepid=2
    // Does not apply to /init
    util::mkdir(crate::rootfs_dir().join("proc")).unwrap();

    // Also censored by userspace FUSE daemon, only allowing few
    // sysfs exposed
    // Does not apply to /init
    util::mkdir(crate::rootfs_dir().join("sys")).unwrap();

    // Each app gets own limited tmpfs
    util::mkdir(crate::rootfs_dir().join("tmp")).unwrap();

    // tmpfs containing mountpoints for each data
    // the /data/primary is primary storage, system
    // will not boot without it
    // while /data/{uuid}/ is expanded storage
    util::mkdir(crate::rootfs_dir().join("data")).unwrap();

    // tmpfs, containing mountpoints. Only visible to /init
    util::mkdir(crate::rootfs_dir().join("mnt")).unwrap();

    // Each sandboxed app/daemon gets private /app_data to store its data
    // and /app is for static read only resources
    // For init, these are empty
    util::mkdir(crate::rootfs_dir().join("app")).unwrap();
    util::mkdir(crate::rootfs_dir().join("app_data")).unwrap();

    println!("Copying busybox");
    util::copy_file(
        "./extra/busybox",
        crate::rootfs_dir().join("system/bin/busybox"),
    )
    .unwrap();
    dircpy::CopyBuilder::new(
        "./extra/busybox_overlay",
        crate::rootfs_dir().join("system/busybox_bins"),
    )
    .overwrite_if_newer(true)
    .overwrite_if_size_differs(true)
    .run()
    .unwrap();

    println!("Copying htop");
    util::copy_file("./extra/htop", crate::rootfs_dir().join("system/bin/htop")).unwrap();

    println!("Copying terminfo");
    util::mkdir(crate::rootfs_dir().join("etc")).unwrap();
    dircpy::CopyBuilder::new("./extra/terminfo", crate::rootfs_dir().join("etc/terminfo"))
        .overwrite_if_newer(true)
        .overwrite_if_size_differs(true)
        .run()
        .unwrap();

    println!("Copying init");
    util::copy_file(
        crate::target_dir().join("x86_64-unknown-linux-musl/debug/init"),
        crate::rootfs_dir().join("init"),
    )
    .unwrap();
    util::copy_file(
        crate::target_dir().join("x86_64-unknown-linux-musl/debug/initctl"),
        crate::rootfs_dir().join("system/bin/initctl"),
    )
    .unwrap();
    util::copy_file(
        crate::target_dir().join("x86_64-unknown-linux-musl/debug/sandbox_helper"),
        crate::rootfs_dir().join("system/bin/sandbox_helper"),
    )
    .unwrap();

    println!("Generating EROFS");
    let status = Command::new("mkfs.erofs")
        .arg("--all-root")
        .arg(crate::rootfs_image_file())
        .arg(crate::rootfs_dir().to_str().unwrap())
        .env("RUSTFLAGS", "-C relocation-model=static")
        .status()
        .expect("Failed to execute mkfs.erofs");

    if !status.success() {
        exit(status.code().unwrap_or(1));
    }
}
