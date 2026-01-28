fn main() {
    // Set CARGO_WORKSPACE_DIR at compile time for path resolution
    let workspace_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map(|d| {
            std::path::Path::new(&d)
                .parent()
                .expect("Failed to get parent directory")
                .to_string_lossy()
                .to_string()
        })
        .expect("CARGO_MANIFEST_DIR not set");

    println!("cargo:rustc-env=CARGO_WORKSPACE_DIR={workspace_dir}");
}
