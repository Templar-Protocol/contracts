use dialoguer::theme::ColorfulTheme;
use market_config_cli::ui::prompt::wizard::MarketPrompter;
use templar_common::utils::Network;

#[test]
fn constructors_require_network_and_theme() {
    let theme = ColorfulTheme::default();
    let _ = MarketPrompter::new(&theme, Network::Testnet);
    let _ = MarketPrompter::new(&theme, Network::Mainnet);
}
