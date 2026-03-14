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
    if std::env::var("CI").is_ok() {
        let status = std::process::Command::new("npm")
            .args(["ci"])
            .current_dir(&js_dir)
            .status()
            .expect("Failed to install npm dependencies for redstone-bridge");
        assert!(status.success(), "npm ci failed");

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
    }

    let bundle = js_dir.join("dist/bundle.js");
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundle.js");
    std::fs::copy(&bundle, &dest).unwrap_or_else(|_| {
        panic!(
            "Failed to copy {} to {}. Run `npm run bundle` in service/redstone-bridge/js",
            bundle.display(),
            dest.display(),
        )
    });
}
