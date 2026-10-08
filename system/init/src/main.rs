use std::process::Command;

fn main() {
    println!("Hello from init!");
    Command::new("/system/bin/busybox")
        .arg("sh")
        .status()
        .unwrap();
}
