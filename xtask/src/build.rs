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
    util::mkdir(crate::rootfs_dir().join("dev")).unwrap();
    util::mkdir(crate::rootfs_dir().join("proc")).unwrap();
    util::mkdir(crate::rootfs_dir().join("sys")).unwrap();
    util::mkdir(crate::rootfs_dir().join("tmp")).unwrap();

    println!("Copying busybox");
    util::copy_file(
        "./extra/busybox",
        crate::rootfs_dir().join("system/bin/busybox"),
    )
    .unwrap();

    println!("Copying init");
    util::copy_file(
        crate::target_dir().join("x86_64-unknown-linux-musl/debug/init"),
        crate::rootfs_dir().join("init"),
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
