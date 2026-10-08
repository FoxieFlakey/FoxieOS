use std::{
    env,
    num::NonZero,
    path::{Path, PathBuf},
    process::exit,
    thread,
};

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

fn main() {
    env::set_current_dir(project_dir()).unwrap();

    let cli = Cli::parse();
    match cli.command {
        Commands::Build(build_args) => build::main(build_args),
        Commands::Boot(boot_args) => {
            build::main(boot_args.build);
            println!("Booting");
            let status = std::process::Command::new("qemu-system-x86_64")
                .arg("-m")
                .arg("256")
                .arg("-cpu")
                .arg("EPYC-Rome-v5")
                .arg("-kernel")
                .arg(&*crate::kernel_file().to_string_lossy())
                .arg("-drive")
                .arg(format!(
                    "file={},format=raw,if=none,id=hd0",
                    crate::rootfs_image_file().display()
                ))
                .arg("-device")
                .arg("virtio-blk-pci,drive=hd0")
                .arg("-smp")
                .arg(format!(
                    "{}",
                    thread::available_parallelism().unwrap_or(NonZero::new(1).unwrap())
                ))
                .arg("-accel")
                .arg("kvm")
                .arg("-device")
                .arg("virtio-serial-pci,id=virtio-serial0")
                .arg("-chardev")
                .arg("stdio,id=charconsole0,mux=on,signal=off")
                .arg("-device")
                .arg("virtconsole,chardev=charconsole0,id=console0")
                .arg("-append")
                .arg(format!(
                    "console=hvc0 root=/dev/vda rootfstype=erofs init=/init {}",
                    boot_args.kernel_args.unwrap_or_default()
                ))
                .status()
                .expect("Failed to boot VM");

            if !status.success() {
                exit(status.code().unwrap_or(1));
            };
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
