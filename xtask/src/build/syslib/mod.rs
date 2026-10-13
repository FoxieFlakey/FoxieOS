use std::path::PathBuf;

// Musl libc, the base of the system
mod musl;

pub fn build() {
    println!("Building musl libc");
    musl::build();
}

pub fn get_syslib_src_dir() -> PathBuf {
    crate::project_dir().join("syslibs")
}

pub fn get_syslib_build_dir() -> PathBuf {
    crate::target_dir().join("syslib_build_uwu")
}
