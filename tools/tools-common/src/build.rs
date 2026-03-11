use std::path::Path;

use anyhow::Context;

/// Run `cargo near build reproducible-wasm` in `dir`.
///
/// Used by CLI tools that need to build a contract before uploading it.
pub fn build_contract(dir: &Path) -> anyhow::Result<()> {
    let status = std::process::Command::new("cargo")
        .args(["near", "build", "reproducible-wasm"])
        .current_dir(dir)
        .status()
        .with_context(|| format!("run cargo near build in {}", dir.display()))?;

    anyhow::ensure!(
        status.success(),
        "cargo near build failed in {}",
        dir.display()
    );
    Ok(())
}
