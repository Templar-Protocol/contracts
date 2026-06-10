#![allow(clippy::unwrap_used, clippy::expect_used)]

fn main() {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let js_dir = manifest_dir.join("js");
    let js_dir_display = js_dir.display();

    let mut stack = std::fs::read_dir(js_dir.join("src"))
        .unwrap()
        .filter_map(|entry| entry.ok())
        .collect::<Vec<_>>();
    while let Some(entry) = stack.pop() {
        let metadata = entry.metadata().unwrap();
        if metadata.is_dir() {
            stack.extend(
                std::fs::read_dir(entry.path())
                    .unwrap()
                    .filter_map(|entry| entry.ok()),
            );
        } else if metadata.is_file() {
            let is_ts = entry
                .path()
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ts"));
            let is_test = entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(".test.");
            if is_ts && !is_test {
                println!("cargo::rerun-if-changed={}", entry.path().display());
            }
        }
    }
    println!("cargo::rerun-if-changed={js_dir_display}/package.json");
    println!("cargo::rerun-if-changed={js_dir_display}/tsconfig.json");

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
