use std::{fs, sync::OnceLock};

use anyhow::{Context, bail};
use nix::sys::stat::{SFlag, major, minor, stat};

static ROOT_DEV_PATH: OnceLock<String> = OnceLock::new();

pub fn get_root_dev() -> &'static str {
    ROOT_DEV_PATH.get().unwrap()
}

pub fn discover_root() -> anyhow::Result<()> {
    let stats = stat("/").context("Cannot get information about /")?;
    let major_id = major(stats.st_dev);
    let minor_id = minor(stats.st_dev);

    for entry in fs::read_dir("/dev")? {
        let entry = entry.context("Cannot get directory entry in /dev")?;
        let path = entry.path();

        if let Ok(stats) = stat(&path) {
            let file_type = SFlag::from_bits_truncate(stats.st_mode);
            if file_type.contains(SFlag::S_IFBLK) {
                if major(stats.st_rdev) == major_id && minor(stats.st_rdev) == minor_id {
                    let path = path
                        .to_str()
                        .ok_or(anyhow::anyhow!(
                            "The path to root device is not valid Rust UTF8 string"
                        ))?
                        .to_string();
                    ROOT_DEV_PATH
                        .set(path.clone())
                        .expect("Must not be initialized already");
                    return Ok(());
                }
            }
        }
    }

    bail!("Cannot find root device")
}
