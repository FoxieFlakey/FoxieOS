use std::{fs::File, num::NonZero, path::PathBuf, process::Command, thread};

use anyhow::Context;

use crate::{build::syslib, util};

pub fn build() -> anyhow::Result<()> {
    util::mkdir(get_musl_build_dir()).context("Creating musl build dir")?;

    let build_dir = get_musl_build_dir()
        .canonicalize()
        .context("Canonicalize Musl build dir path")?;
    if !build_dir.join("uwu").exists() {
        // ./configure isnt ran yet
        Command::new(get_musl_src_dir().join("configure"))
            .current_dir(&build_dir)
            .arg("--prefix=/system")
            .arg("--disable-static")
            .arg("--syslibdir=/system/lib")
            .arg("--libdir=/system/lib")
            .arg("--bindir=/system/bin")
            .arg("--target=x86_64")
            .arg("--host=x86_64")
            .arg("--enable-wrapper=no")
            .arg("CC=clang -target x86_64-unknown-linux-musl")
            .arg("LLVM=1")
            .arg("AR=llvm-ar")
            .arg("RANLIB=llvm-ranlib")
            .status()
            .context("Waiting for ./configure result")?
            .exit_ok()
            .context("Running configure")?;

        // Mark that configure has ran
        File::create(build_dir.join("uwu")).context("Marking if ./configure is done")?;
    }

    Command::new("make")
        .current_dir(&build_dir)
        .arg(&format!(
            "-j{}",
            thread::available_parallelism().unwrap_or(NonZero::new(1).unwrap())
        ))
        .arg(&format!(
            "DESTDIR={}",
            crate::rootfs_dir()
                .canonicalize()
                .context("Canonicalizing rootfs dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to rootfs dir is not valid UTF-8 string: {x:#?}",
                ))?
        ))
        .arg("install")
        .status()
        .context("Waiting for 'make install' result")?
        .exit_ok()
        .context("Compiling Musl libc")?;

    Ok(())
}

fn get_musl_src_dir() -> PathBuf {
    syslib::get_syslib_src_dir().join("musl")
}

fn get_musl_build_dir() -> PathBuf {
    syslib::get_syslib_build_dir().join("musl")
}
