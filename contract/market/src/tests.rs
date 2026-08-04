use near_sdk::serde_json;
use templar_common::market::MarketConfiguration;

/// Every deployed market configuration in the regression corpus must still
/// deserialize into the current [`MarketConfiguration`], so a field rename or a
/// type change cannot silently invalidate what is running.
///
/// The corpus moved out of this crate with the declarative-deployment migration
/// (ENG-537); the files are the same `market-args.json` each market was
/// deployed with. Read by relative path rather than by depending on
/// `templar-manager`, which is a tool and must not become a contract dependency.
///
/// Lives in the lib rather than `tests/` because this crate's integration tests
/// are node-backed and excluded from the fast gate wholesale, by package.
#[test]
fn parse_configurations() {
    let mut read = std::fs::read_dir("../../tools/manager/fixtures/deployed")
        .unwrap()
        .collect::<Vec<_>>();
    let mut total = 0;

    while let Some(Ok(entry)) = read.pop() {
        let t = entry.file_type().unwrap();
        if t.is_dir() {
            read.extend(std::fs::read_dir(entry.path()).unwrap());
        } else if t.is_file() {
            let path = entry.path();
            let display = path.display();
            if display.to_string().ends_with(".near.json") {
                let raw = std::fs::read_to_string(&path).unwrap();
                let value: serde_json::Value = serde_json::from_str(&raw)
                    .unwrap_or_else(|e| panic!("{display} is not JSON: {e}"));
                // Proxy-mode deployments wrap the configuration in the
                // `registry.deploy` init-args envelope; flat ones do not.
                let configuration = value.get("configuration").cloned().unwrap_or(value);
                serde_json::from_value::<MarketConfiguration>(configuration)
                    .unwrap_or_else(|e| panic!("{display} is not a market configuration: {e}"));
                total += 1;
            }
        }
    }

    assert_eq!(total, 45, "every deployed configuration must be covered");
}
