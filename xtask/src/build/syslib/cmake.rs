use std::{collections::HashMap, fs::File, num::NonZero, path::Path, process::Command, thread};

use anyhow::Context;

use crate::{build::syslib::get_syslib_dev_sysroot_dir, util};

// All commands arguments are ChatGPT suggested to make it works

pub fn build<P1, P2>(
    src_dir: &P1,
    build_dir: &P2,
    cmake_cfg: &HashMap<String, String>,
    runtime_components: &[&str],
    dev_components: &[&str],
    runtime_targets: &[&str],
    dev_targets: &[&str],
) -> anyhow::Result<()>
where
    P1: AsRef<Path> + ?Sized,
    P2: AsRef<Path> + ?Sized,
{
    let src_dir = src_dir.as_ref();
    let build_dir = build_dir.as_ref();
    util::mkdir(build_dir).context("Creating musl build dir")?;

    let build_dir = build_dir
        .canonicalize()
        .context("Canonicalize build dir path")?;
    if !build_dir.join("uwu").exists() {
        let mut cmd = Command::new("cmake");
        cmd.current_dir(&build_dir)
            .arg("-G")
            .arg("Ninja")
            .arg(&format!(
                "-DCMAKE_TOOLCHAIN_FILE={}",
                crate::project_dir()
                    .join("x86_64-foxieos-linux-musl.cmake")
                    .canonicalize()
                    .context("Canonicalizing path to toolchain file")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to cmake toolchain file is not valid UTF-8 string: {x:#?}",
                    ))?
            ))
            .arg(&format!(
                "-DFOXIE_ROOTFS={}",
                crate::rootfs_dir()
                    .canonicalize()
                    .context("Canonicalizing path to rootfs")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to cmake rootfs is not valid UTF-8 string: {x:#?}",
                    ))?
            ))
            .arg(&format!(
                "-DFOXIE_DEV_SYSROOT={}",
                get_syslib_dev_sysroot_dir()
                    .canonicalize()
                    .context("Canonicalizing dev sysroot dir to toolchain file")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to cmake dev sysroot is not valid UTF-8 string: {x:#?}",
                    ))?
            ))
            .arg("-DCMAKE_BUILD_TYPE=Release")
            .arg(&format!("-DCMAKE_INSTALL_PREFIX=/system"))
            .arg(&format!("-DCMAKE_INSTALL_LIBDIR=lib"))
            .arg(&format!("-DCMAKE_INSTALL_BINDIR=bin"))
            .arg(&format!("-BUILD_SHARED_LIBS=ON"));

        for (k, v) in cmake_cfg {
            cmd.arg(format!("-D{k}={v}"));
        }

        cmd.arg("-B")
            .arg(".")
            .arg("-S")
            .arg(src_dir.canonicalize().context("Canonicalizing src dir")?)
            .status()
            .context("Waiting for cmake result")?
            .exit_ok()
            .context("Running cmake setup")?;

        // Mark that cmake meson setup has ran
        File::create(build_dir.join("uwu")).context("Marking if meson setup is done")?;
    }

    Command::new("cmake")
        .current_dir(&build_dir)
        .arg("--build")
        .arg(".")
        .arg(&format!(
            "-j{}",
            thread::available_parallelism().unwrap_or(NonZero::new(1).unwrap())
        ))
        .arg("--config")
        .arg("Release")
        .status()
        .context("Waiting for cmake build result")?
        .exit_ok()
        .context("Building cmake project")?;

    for runtime in runtime_components {
        Command::new("cmake")
            .current_dir(&build_dir)
            .env(
                "DESTDIR",
                crate::rootfs_dir()
                    .canonicalize()
                    .context("Canonicalizing rootfs dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| {
                        anyhow::anyhow!("Path to rootfs dir is not valid UTF-8 string: {x:#?}",)
                    })?,
            )
            .arg("--install")
            .arg(".")
            .arg("--component")
            .arg(runtime)
            .status()
            .context("Waiting for cmake install result")?
            .exit_ok()
            .context("Installing cmake project's runtime files")?;
    }

    for runtime in runtime_targets {
        Command::new("cmake")
            .current_dir(&build_dir)
            .env(
                "DESTDIR",
                crate::rootfs_dir()
                    .canonicalize()
                    .context("Canonicalizing rootfs dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| {
                        anyhow::anyhow!("Path to rootfs dir is not valid UTF-8 string: {x:#?}",)
                    })?,
            )
            .arg("--build")
            .arg(".")
            .arg("--target")
            .arg(runtime)
            .status()
            .context("Waiting for cmake install result")?
            .exit_ok()
            .context("Installing cmake project's runtime files")?;
    }

    for dev in dev_components {
        Command::new("cmake")
        .current_dir(&build_dir)
        .env(
            "DESTDIR",
            get_syslib_dev_sysroot_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| {
                    anyhow::anyhow!("Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",)
                })?,
        )
        .arg("--install")
        .arg(".")
        .arg("--component")
        .arg(dev)
        .status()
        .context("Waiting for cmake install result")?
        .exit_ok()
        .context("Installing cmake project's dev files")?;
    }

    for dev in dev_targets {
        Command::new("cmake")
        .current_dir(&build_dir)
        .env(
            "DESTDIR",
            get_syslib_dev_sysroot_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| {
                    anyhow::anyhow!("Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",)
                })?,
        )
        .arg("--build")
        .arg(".")
        .arg("--target")
        .arg(dev)
        .status()
        .context("Waiting for cmake install result")?
        .exit_ok()
        .context("Installing cmake project's dev files")?;
    }

    Ok(())
}
