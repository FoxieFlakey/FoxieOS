use std::{fs::File, hash_map, io::Write, path::PathBuf, process::Command};

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

    println!("Building libdrm");
    meson::build(
        &get_syslib_src_dir().join("libdrm"),
        &get_syslib_build_dir().join("libdrm"),
        &hash_map! {
            "tests".to_string() => "false".to_string(),
            "intel".to_string() => "disabled".to_string(),
            "radeon".to_string() => "disabled".to_string(),
            "amdgpu".to_string() => "disabled".to_string(),
            "nouveau".to_string() => "disabled".to_string(),
            "vmwgfx".to_string() => "disabled".to_string(),
            "omap".to_string() => "disabled".to_string(),
            "exynos".to_string() => "disabled".to_string(),
            "tegra".to_string() => "disabled".to_string(),
            "vc4".to_string() => "disabled".to_string(),
            "etnaviv".to_string() => "disabled".to_string(),

            // Miscelanous
            "man-pages".to_string() => "disabled".to_string(),
            "valgrind".to_string() => "disabled".to_string(),
            "udev".to_string() => "false".to_string(),
            "install-test-programs".to_string() => "false".to_string(),
        },
        vec![("libdrm.so", "libdrm.so.*")],
    )
    .context("Building libdrm")?;

    println!("Building zlib-ng");
    cmake::build(
        &get_syslib_src_dir().join("zlib-ng"),
        &get_syslib_build_dir().join("zlib-ng"),
        &hash_map! {
            "ZLIB_COMPAT".to_string() => "ON".to_string(),
            "BUILD_TESTING".to_string() => "OFF".to_string(),
            "ZLIB_ENABLE_TESTS".to_string() => "OFF".to_string(),
            "WITH_GTEST".to_string() => "OFF".to_string(),
        },
        &[],
        &["Unspecified"],
        &[],
        &[],
    )
    .context("Building zlib-ng")?;

    // Due limitation in zlib-ng? apparently cant install header seperately
    // so install everything to dev sysroot first then copies the libs
    Command::new("sh")
        .arg("-c")
        .arg(&format!(
            "cp --remove-destination -- {0}/system/lib/libz* {1}/system/lib/; exit $?",
            get_syslib_dev_sysroot_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",
                ))?,
            crate::rootfs_dir()
                .canonicalize()
                .context("Canonicalizing rootfs dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to rootfs dir is not valid UTF-8 string: {x:#?}",
                ))?
        ))
        .status()
        .context("Cannot wait for shell while copying zlib-ng to final rootfs")?
        .exit_ok()
        .context("Cannot zlib-ng to final rootfs")?;

    println!("Building mesa");
    meson::build(
        &get_syslib_src_dir().join("mesa"),
        &get_syslib_build_dir().join("mesa"),
        &hash_map! {
            "build-tests".to_string() => "false".to_string(),
            "gallium-rusticl".to_string() => "false".to_string(),
            "platforms".to_string() => "".to_string(),
            "glx".to_string() => "disabled".to_string(),
            "glvnd".to_string() => "disabled".to_string(),
            "opengl".to_string() => "false".to_string(),
            "gbm".to_string() => "disabled".to_string(),
            "gles1".to_string() => "disabled".to_string(),
            "gles2".to_string() => "disabled".to_string(),
            "egl".to_string() => "disabled".to_string(),
            "auto_features".to_string() => "disabled".to_string(),

            // No software rasterization only virtio-gpu supported
            "gallium-drivers".to_string() => "".to_string(),
            "vulkan-drivers".to_string() => "virtio".to_string(),

            // LLVM currently borked, cant be cross compiled sanely
            "llvm".to_string() => "disabled".to_string(),
            "c_args".to_string() => format!("['-target', 'x86_64-unknown-linux-musl', '--sysroot={0}', '-I{1}','-I{1}/libdrm']", get_syslib_dev_sysroot_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",
                ))?, get_syslib_include_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot include dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to dev sysroot include  dir is not valid UTF-8 string: {x:#?}",
                ))?,
            ),
            "cpp_args".to_string() => format!("['-target', 'x86_64-unknown-linux-musl', '--sysroot={0}', '-I{1}/c++/v1', '-I{1}','-I{1}/libdrm']", get_syslib_dev_sysroot_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to dev sysroot dir is not valid UTF-8 string: {x:#?}",
                ))?, get_syslib_include_dir()
                .canonicalize()
                .context("Canonicalizing dev sysroot include dir")?
                .into_os_string()
                .into_string()
                .map_err(|x| anyhow::anyhow!(
                    "Path to dev sysroot include  dir is not valid UTF-8 string: {x:#?}",
                ))?,
            ),
        },
        vec![
        ],
    )
    .context("Building mesa")?;

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
