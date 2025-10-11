use std::{fs, path::Path};

use templar_common::registry::DeployMode;

pub fn main() {
    let name = "templar_market_contract";

    let path = Path::new(env!("CARGO_WORKSPACE_DIR"))
        .join("target/near/")
        .join(name)
        .join(name.to_owned() + ".wasm");

    let wasm = fs::read(path).unwrap();

    let args = std::env::args().collect::<Vec<_>>();
    let version_key = args[1].clone();
    let mode = match args[2].as_str() {
        "normal" => DeployMode::Normal,
        "global_hash" => DeployMode::GlobalHash,
        _ => panic!("Must specify mode: (normal|global_hash)"),
    };

    let args = (version_key, mode, wasm);
    near_sdk::borsh::to_writer(std::io::stdout(), &args).unwrap();
}
