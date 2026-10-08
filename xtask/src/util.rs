use std::{fs, io, path::Path};

pub fn copy_file<P1: AsRef<Path>, P2: AsRef<Path>>(src: P1, dest: P2) -> io::Result<()> {
    let src = src.as_ref();
    let dest = dest.as_ref();
    let mut dest_dir = dest.to_path_buf();
    dest_dir.pop();
    mkdir(dest_dir)?;

    fs::copy(src, dest)?;
    Ok(())
}

pub fn mkdir<P: AsRef<Path>>(dir: P) -> io::Result<()> {
    fs::create_dir_all(dir)
}
