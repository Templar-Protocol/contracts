use std::{env, path::PathBuf};

#[test]
fn run_bridge_jest_tests() {
    let crate_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bridge_dir = crate_root.join("js");

    if env::var("CI").is_ok_and(|v| !v.is_empty()) {
        let status = std::process::Command::new("npm")
            .arg("ci")
            .current_dir(&bridge_dir)
            .status()
            .expect("failed to run npm ci");
        assert!(status.success(), "npm ci failed");
    } else {
        // Do nothing: let developers manage their own node_modules
    }

    // check exit code to ensure tests pass
    let status = std::process::Command::new("npm")
        .arg("test")
        .current_dir(&bridge_dir)
        .status()
        .expect("failed to run npm test");
    assert!(status.success(), "npm test failed");
}
