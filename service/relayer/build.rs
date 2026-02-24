#![allow(clippy::unwrap_used)]

fn main() {
    println!("cargo::rerun-if-changed=redstone-bridge/");

    let install = std::process::Command::new("npm")
        .arg("install")
        .current_dir("redstone-bridge")
        .status()
        .unwrap();
    assert!(install.success());

    let build = std::process::Command::new("npm")
        .arg("run")
        .arg("build")
        .current_dir("redstone-bridge")
        .status()
        .unwrap();
    assert!(build.success());
}
