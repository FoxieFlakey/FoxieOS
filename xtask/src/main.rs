#![feature(exit_status_error)]

use std::{
    env,
    num::NonZero,
    path::{Path, PathBuf},
    thread,
};

use anyhow::{Context, anyhow};
use clap::{Args, Parser, Subcommand};

use crate::build::BuildArgs;

mod build;
mod util;

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build(BuildArgs),
    Boot(BootArgs),
}

#[derive(Args)]
struct BootArgs {
    #[command(flatten)]
    build: BuildArgs,
    kernel_args: Option<String>,
}

fn main() -> anyhow::Result<()> {
    env::set_current_dir(project_dir()).unwrap();

    let cli = Cli::parse();
    match cli.command {
        Commands::Build(build_args) => build::main(build_args).context("Building system"),
        Commands::Boot(boot_args) => {
            build::main(boot_args.build).context("Building system")?;
            println!("Booting");
            std::process::Command::new("qemu-system-x86_64")
                .arg("-m")
                .arg("256")
                .arg("-cpu")
                .arg("EPYC-Rome-v5")
                .arg("-kernel")
                .arg(crate::kernel_file())
                .arg("-drive")
                .arg(format!(
                    "file={},format=raw,if=none,id=hd0",
                    crate::rootfs_image_file().into_os_string()
                        .into_string()
                        .map_err(|x| anyhow!("Path contained non UTF-8 sequence: {x:#?}"))?
                ))
                .arg("-device")
                .arg("virtio-gpu-pci")
                .arg("-vga")
                .arg("none")
                .arg("-display")
                .arg("gtk,gl=on")
                .arg("-device")
                .arg("virtio-blk-pci,drive=hd0")
                .arg("-smp")
                .arg(format!(
                    "{}",
                    thread::available_parallelism().unwrap_or(NonZero::new(1).unwrap())
                ))
                .arg("-accel")
                .arg("kvm")
                
                // Serial controller
                .arg("-device")
                .arg("virtio-serial-pci,id=virtio-serial0")

                // /dev/hvc0
                .arg("-device")
                .arg("virtconsole,chardev=charconsole0,id=console0")

                // /dev/hvc1
                .arg("-device")
                .arg("virtconsole,chardev=charconsole1,id=console1")

                .arg("-chardev")
                .arg("stdio,id=charconsole0,mux=on,signal=off")
                .arg("-chardev")
                .arg("vc,id=charconsole1")

                .arg("-append")
                .arg(format!(
                    "root=/dev/vda console=hvc0 rootfstype=erofs init=/system/bin/init PATH=/system/bin/:/system/busybox_bins/ {}",
                    boot_args.kernel_args.unwrap_or_default()
                ))
                .status()
                .context("Boot VM with QEMU")?
                .exit_ok()
                .context("QEMU VM exited")
        }
    }
}

fn project_dir() -> PathBuf {
    Path::new(&env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .unwrap()
        .to_path_buf()
}

fn target_dir() -> PathBuf {
    project_dir().join("target")
}

fn kernel_file() -> PathBuf {
    project_dir().join("extra/kernel")
}

fn rootfs_image_file() -> PathBuf {
    project_dir().join("target/tmp/rootfs.erofs")
}

fn rootfs_dir() -> PathBuf {
    project_dir().join("target/tmp/rootfs")
}
