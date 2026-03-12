#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    println!("cargo:rerun-if-changed=js/src/**/*.ts");
    println!("cargo:rerun-if-changed=js/package*.json");
    println!("cargo:rerun-if-changed=js/tsconfig.json");
    println!("cargo:rerun-if-changed=js/dist/bundle.js");

    #[allow(clippy::expect_used)]
    if std::env::var("CI").is_ok() {
        std::process::Command::new("npm")
            .args(["ci"])
            .current_dir("js")
            .status()
            .expect("Failed to install npm dependencies for redstone-bridge");

        std::process::Command::new("npm")
            .args(["run", "build"])
            .current_dir("js")
            .status()
            .expect("Failed to build redstone-bridge");

        std::process::Command::new("npm")
            .args(["run", "bundle"])
            .current_dir("js")
            .status()
            .expect("Failed to bundle redstone-bridge");
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let dest = std::path::Path::new(&out_dir).join("bundle.js");
    std::fs::copy("js/dist/bundle.js", &dest).expect(
        "Failed to copy js/dist/bundle.js to OUT_DIR. Run `npm run bundle` in service/redstone-bridge/js",
    );
}
