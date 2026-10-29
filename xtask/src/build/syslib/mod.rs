use std::{fs::File, hash_map, io::Write, path::PathBuf};

use anyhow::Context;

use crate::util;

// Musl libc, the base of the system
mod musl;

// Common stuff for Meson and CMake
mod cmake;
mod meson;

pub fn build() -> anyhow::Result<()> {
    println!("Building creating include and pkgconfig dir");
    util::mkdir(get_syslib_include_dir()).context("Cannot create syslibs include dir")?;
    util::mkdir(get_syslib_pkgconfig_dir()).context("Cannot create syslibs pkgconfig dir")?;

    println!("Copying Linux kernel headers");

    dircpy::CopyBuilder::new("./extra/linux-headers", get_syslib_include_dir())
        .overwrite_if_newer(true)
        .overwrite_if_size_differs(true)
        .run()
        .context("Copying linux headers to syslib include dir")?;

    println!("Building musl libc");
    musl::build().context("Building musl libc")?;

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

    util::copy_file(
        "./extra/empty_librt.a",
        crate::rootfs_dir().join("system/lib/librt.a"),
    )
    .context("Copying empty librt.a")?;

    util::copy_file(
        "./extra/libssp_nonshared.a",
        crate::rootfs_dir().join("system/lib/libssp_nonshared.a"),
    )
    .context("Copying libssp_nonshared.a")?;

    util::copy_file(
        crate::rootfs_dir().join("system/lib/libc.so"),
        crate::rootfs_dir().join("system/lib/libssp_shared.so"),
    )
    .context("Copying empty libc.so to libssp_shared")?;

    File::create(get_syslib_build_dir().join("meson-cross-file.ini"))
        .context("Creating meson's cross file")?
        .write_all(
            format!(
                "
[constants]
sysroot_path = '{}'
dev_sysroot_path = '{}'
include_dir = '{}'
project_dir = '{}'
",
                crate::rootfs_dir()
                    .canonicalize()
                    .context("Canonicalizing rootfs dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to rootfs dir is not valid UTF-8 string: {x:#?}",
                    ))?,
                get_syslib_dev_sysroot_dir()
                    .canonicalize()
                    .context("Canonicalizing dev sysroot include dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",
                    ))?,
                get_syslib_include_dir()
                    .canonicalize()
                    .context("Canonicalizing syslib include dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to syslib include dir is not valid UTF-8 string: {x:#?}",
                    ))?,
                crate::project_dir()
                    .canonicalize()
                    .context("Canonicalizing project dir")?
                    .into_os_string()
                    .into_string()
                    .map_err(|x| anyhow::anyhow!(
                        "Path to project dir is not valid UTF-8 string: {x:#?}",
                    ))?,
            )
            .as_bytes(),
        )
        .context("Cannot initialize meson cross file")?;

    println!("Building LLVM's libc++");
    cmake::build(
        &get_syslib_src_dir().join("llvm-stripped/runtimes"),
        &get_syslib_build_dir().join("llvm-libcxx-rt"),
        // ChatGPT suggested to make it works
        &hash_map! {
            "LLVM_ENABLE_RUNTIMES".to_string() => "libcxx;libcxxabi;libunwind".to_string(),
            "CMAKE_CXX_COMPILER_WORKS".to_string() => "1".to_string(),
            "LIBCXX_HAS_MUSL_LIBC".to_string() => "1".to_string(),
            "CMAKE_ASM_FLAGS".to_string() => format!("-isystem '{}'", get_syslib_include_dir().canonicalize().context("Cannot canonicalize include dir")?.into_os_string().into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to include dir is not valid UTF-8 string: {x:#?}",
                ))?),
            "CMAKE_C_FLAGS".to_string() => format!("-nostdinc -isystem '{}'", get_syslib_include_dir().canonicalize().context("Cannot canonicalize include dir")?.into_os_string().into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to include dir is not valid UTF-8 string: {x:#?}",
                ))?),
            "CMAKE_CXX_FLAGS".to_string() => format!("-nostdinc++ -nostdinc -nostdlib++ -isystem '{}'", get_syslib_include_dir().canonicalize().context("Cannot canonicalize include dir")?.into_os_string().into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to include dir is not valid UTF-8 string: {x:#?}",
                ))?),
        },
        &[
            "unwind",
            "cxxabi",
            "cxx",
        ],
        // ChatGPT suggested to make it works
        &["unwind-headers", "cxxabi-headers", "cxx-headers"],
        &[],
        &[],
    )
    .context("Building libc++")?;

    Ok(())
}

pub fn get_syslib_src_dir() -> PathBuf {
    crate::project_dir().join("syslibs")
}

pub fn get_syslib_build_dir() -> PathBuf {
    crate::target_dir().join("syslib_build_uwu")
}

pub fn get_syslib_dev_sysroot_dir() -> PathBuf {
    get_syslib_build_dir().join("dev_sysroot")
}

pub fn get_syslib_include_dir() -> PathBuf {
    get_syslib_dev_sysroot_dir().join("system").join("include")
}

pub fn get_syslib_pkgconfig_dir() -> PathBuf {
    get_syslib_dev_sysroot_dir()
        .join("system")
        .join("pkgconfig")
}
