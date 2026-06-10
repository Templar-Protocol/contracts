use near_sdk::serde_json;
use templar_common::market::MarketConfiguration;

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
