use std::{fs::File, num::NonZero, path::PathBuf, process::Command, thread};

use crate::{build::syslib, util};

pub fn build() {
    util::mkdir(get_musl_build_dir()).unwrap();

    let build_dir = get_musl_build_dir().canonicalize().unwrap();
    if !build_dir.join("uwu").exists() {
        // ./configure isnt ran yet
        Command::new(get_musl_src_dir().join("configure"))
            .current_dir(&build_dir)
            .arg("--prefix=/system")
            .arg("--disable-static")
            .arg("--syslibdir=/system/lib")
            .arg("--libdir=/system/lib")
            .arg("--bindir=/system/musl_bin")
            .arg("--target=x86_64")
            .arg("--host=x86_64")
            .arg("--enable-wrapper=no")
            .arg("CC=clang -target x86_64-unknown-linux-musl")
            .arg("LLVM=1")
            .arg("AR=llvm-ar")
            .arg("RANLIB=llvm-ranlib")
            .status()
            .unwrap()
            .exit_ok()
            .unwrap();

        // Mark that configure has ran
        File::create(build_dir.join("uwu")).unwrap();
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
                .unwrap()
                .into_os_string()
                .into_string()
                .map_err(|_| "Contains not UTF-8 valid characters")
                .unwrap()
        ))
        .arg("install")
        .status()
        .unwrap()
        .exit_ok()
        .unwrap();
}

fn get_musl_src_dir() -> PathBuf {
    syslib::get_syslib_src_dir().join("musl")
}

fn get_musl_build_dir() -> PathBuf {
    syslib::get_syslib_build_dir().join("musl")
}
