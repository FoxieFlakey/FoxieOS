use std::process::Command;

use anyhow::Context;
use clap::Args;

use crate::util;

mod syslib;

#[derive(Args)]
pub struct BuildArgs {
    #[arg(long, default_value = "false")]
    build_syslib: bool,
}

static CRATES: &[&str] = &[
    "init",
    "service",
    "sandbox_helper",
    "service_manager",
    "tty_spawner",
    "sandboxer",
    "graphics_server",
    "vk_hewwo",
];

static DIRS: &[&str] = &[
    "proc",
    "sys",
    "dev",
    "app",
    "app_data",
    "sandbox_root",
    "orig_root",
    "etc",
    "system",
    "tmp",
    "mnt",
];

pub fn main(args: BuildArgs) -> anyhow::Result<()> {
    println!("Creating rootfs");
    util::mkdir(crate::rootfs_dir()).context("Creating rootfs dir")?;

    for dir in DIRS {
        util::mkdir(crate::rootfs_dir().join(dir))
            .with_context(|| format!("Creating dir {dir}"))?;
    }

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

    if args.build_syslib {
        println!("Building syslibs");
        syslib::build().context("Building syslib")?;
    } else {
        println!(
            "Not building syslibs (IF this is first time, you must build syslib once or build if you change. The rootfs image will not be bootlable without it)"
        );
    }

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

    util::copy_file("./extra/htop", crate::rootfs_dir().join("system/bin/htop"))
        .context("Copying htop")?;

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
