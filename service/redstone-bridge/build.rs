fn main() {
    println!("cargo:rerun-if-changed=js/src/**/*.ts");
    println!("cargo:rerun-if-changed=js/package*.json");
    println!("cargo:rerun-if-changed=js/tsconfig.json");

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
    }
}
