use std::fs;

use templar_common::registry::DeployMode;

pub fn main() {
    let cliargs = std::env::args().collect::<Vec<_>>();
    let name = &cliargs[1];
    let workspace_root = test_utils::workspace_root();

    let path = workspace_root
        .join("target/near/")
        .join(name)
        .join(name.to_owned() + ".wasm");

    let wasm = fs::read(path).unwrap();

    match &cliargs[2..] {
        [version_key] => {
            let args = (version_key, wasm);
            near_sdk::borsh::to_writer(std::io::stdout(), &args).unwrap();
        }
        [version_key, mode] => {
            let mode = match mode.as_str() {
                "normal" => DeployMode::Normal,
                "global_hash" => DeployMode::GlobalHash,
                _ => panic!("Must specify mode: (normal|global_hash)"),
            };

            let args = (version_key, mode, wasm);
            near_sdk::borsh::to_writer(std::io::stdout(), &args).unwrap();
        }
        _ => {
            panic!("Expects 2 or 3 arguments");
        }
    }
}
