use std::{collections::HashMap, fs::File, path::Path, process::Command};

use anyhow::Context;

use crate::{
    build::syslib::{get_syslib_build_dir, get_syslib_dev_sysroot_dir},
    util,
};

pub fn build<P1, P2>(
    src_dir: &P1,
    build_dir: &P2,
    meson_cfg: &HashMap<String, String>,

    // Creates these symlink at destination
    // NOTE: The 'dest' can be globbed with shell
    // with first result is taken. While src cant
    symlink_fix: Vec<(&str, &str)>,
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
        let mut cmd = Command::new("meson");
        cmd.current_dir(&build_dir)
            .arg("setup")
            .arg("--buildtype")
            .arg("release")
            .arg("--cross-file")
            .arg(
                crate::project_dir()
                    .canonicalize()
                    .context("Canonicalizing project dir")?
                    .join("x86_64-foxieos-linux-musl-meson-crosscompile.ini"),
            )
            .arg("--cross-file")
            .arg(
                get_syslib_build_dir()
                    .canonicalize()
                    .context("Canonicalizing syslib build dir")?
                    .join("meson-cross-file.ini"),
            )
            .arg("--prefix")
            .arg("/system");

        for (k, v) in meson_cfg {
            cmd.arg(format!("-D{k}={v}"));
        }

        cmd.arg(".")
            .arg(src_dir.canonicalize().context("Canonicalizing src dir")?)
            .status()
            .context("Waiting for meson setup result")?
            .exit_ok()
            .context("Running meson setup")?;

        // Mark that meson setup has ran
        File::create(build_dir.join("uwu")).context("Marking if meson setup is done")?;
    }

    Command::new("meson")
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
        .arg("install")
        .arg("--tags")
        .arg("runtime,data")
        .status()
        .context("Waiting for 'meson install' result")?
        .exit_ok()
        .context("Compiling meson project")?;

    Command::new("meson")
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
        .arg("install")
        .arg("--tags")
        .arg("devel")
        .status()
        .context("Waiting for 'meson install' result")?
        .exit_ok()
        .context("Installing meson project's dev files")?;

    // Fixing symlinks
    let rootfs = crate::rootfs_dir()
        .canonicalize()
        .context("Canonicalizing rootfs dir")?
        .into_os_string()
        .into_string()
        .map_err(|x| anyhow::anyhow!("Path to rootfs dir is not valid UTF-8 string: {x:#?}",))?;
    for (src, dest) in symlink_fix {
        Command::new("sh")
            .arg("-c")
            .arg(format!(
                "set -e; dest=$(find '{rootfs}/system/lib' -name '{dest}' -printf '%f\n' | head -1); ln -f -s -T \"$dest\" '{rootfs}/system/lib/{src}'"
            ))
            .status()
            .context("Cannot wait fo symlink fix shell to complete")?
            .exit_ok()
            .context("Cannot fix symlinks")?;
    }

    Ok(())
}
