#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    println!("cargo:rerun-if-changed=js/src/");
    println!("cargo:rerun-if-changed=js/package.json");
    println!("cargo:rerun-if-changed=js/package-lock.json");
    println!("cargo:rerun-if-changed=js/tsconfig.json");
    println!("cargo:rerun-if-changed=js/dist/bundle.js");

    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let js_dir = manifest_dir.join("js");

    #[allow(clippy::expect_used)]
    let npm_command = if std::env::var("CI").is_ok() {
        "ci"
    } else {
        "install"
    };
    let status = std::process::Command::new("npm")
        .args([npm_command])
        .current_dir(&js_dir)
        .status()
        .expect("Failed to install npm dependencies for redstone-bridge");
    assert!(status.success(), "npm {npm_command} failed");

    let status = std::process::Command::new("npm")
        .args(["run", "build"])
        .current_dir(&js_dir)
        .status()
        .expect("Failed to build redstone-bridge");
    assert!(status.success(), "npm run build failed");

    let status = std::process::Command::new("npm")
        .args(["run", "bundle"])
        .current_dir(&js_dir)
        .status()
        .expect("Failed to bundle redstone-bridge");
    assert!(status.success(), "npm run bundle failed");

    let bundle = js_dir.join("dist/bundle.js");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundle.js");
    std::fs::copy(&bundle, &dest).unwrap_or_else(|e| {
        panic!(
            "Failed to copy {} to {}: {e}",
            bundle.display(),
            dest.display()
        )
    });
}
