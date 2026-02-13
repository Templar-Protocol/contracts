use market_config_cli::ContractReader;
use templar_common::utils::Network;

#[test]
fn contract_reader_builds_from_network() {
    let _ = ContractReader::new(Network::Testnet).expect("reader should construct");
    let _ = ContractReader::new(Network::Mainnet).expect("reader should construct");
}
