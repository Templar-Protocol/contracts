use near_sdk::serde_json;
use templar_common::market::MarketConfiguration;

/// Every checked-in market config under `examples/config/` must still
/// deserialize into the current [`MarketConfiguration`], so a field rename or a
/// type change cannot silently invalidate the deployment configs.
///
/// The invariant this rests on: under `examples/config/`, a `*.near.json` file
/// *is* a market configuration. Args for the other contracts of a deployment
/// (proxy oracle, governance, the Pyth Lazer adapter) live in that deployment's
/// subdirectory as `*-args.json` — name one `*.near.json` and this test will
/// fail trying to parse it as a market.
///
/// Lives in the lib rather than `tests/` because this crate's integration tests
/// are node-backed and excluded from the fast gate wholesale, by package.
#[test]
fn parse_configurations() {
    let mut read = std::fs::read_dir("./examples/config/")
        .unwrap()
        .collect::<Vec<_>>();
    let mut total = 0;

    while let Some(Ok(entry)) = read.pop() {
        let t = entry.file_type().unwrap();
        if t.is_dir() {
            // recurse directories
            read.extend(std::fs::read_dir(entry.path()).unwrap());
        } else if t.is_file() {
            let path = entry.path();
            let display = path.display();
            if display.to_string().ends_with(".near.json") {
                eprint!("Parsing {display}: ");
                let file = std::fs::File::open(&path).unwrap();
                // Attempt to parse:
                serde_json::from_reader::<_, MarketConfiguration>(file)
                    .unwrap_or_else(|e| panic!("Failed: {e}"));
                eprintln!("Success!");
                total += 1;
            }
        }
    }

    assert!(total > 0, "No configurations parsed");
}
