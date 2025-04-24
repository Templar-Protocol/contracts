use std::{fs, path::Path};

pub fn main() {
    let name = "templar_market_contract";

    let path = Path::new(env!("CARGO_WORKSPACE_DIR"))
        .join("target/near/")
        .join(name)
        .join(name.to_owned() + ".wasm");

    let wasm = fs::read(path).unwrap();

    let version_key = std::env::args().collect::<Vec<_>>()[1].clone();

    let args = (version_key, wasm);
    near_sdk::borsh::to_writer(std::io::stdout(), &args).unwrap();
}
