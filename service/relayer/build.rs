#![allow(clippy::unwrap_used)]

fn main() {
    println!("cargo::rerun-if-changed=redstone-bridge/");

    let mut npm = std::process::Command::new("npm");
    npm.arg("run");
    npm.arg("build");
    npm.current_dir("redstone-bridge");
    npm.spawn().unwrap().wait().unwrap();
}
