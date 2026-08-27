//! NEAR network selection for off-chain gateway consumers.

use std::fmt;

use near_api::{NetworkConfig, RPCEndpoint};
use url::Url;

/// Pyth's public Hermes endpoints. A VAA is only accepted by the Pyth receiver whose
/// guardian set signed it, so the endpoint has to follow the NEAR network.
const HERMES_MAINNET_URL: &str = "https://hermes.pyth.network";
const HERMES_TESTNET_URL: &str = "https://hermes-beta.pyth.network";

/// A NEAR network, used by off-chain consumers (CLIs, bots, services) to pick
/// the default RPC endpoint when constructing a [`crate::Client`].
///
/// This is the shared home for the `Network` enum that off-chain tools/services
/// previously each defined. Under the `clap` feature it derives
/// [`clap::ValueEnum`] so binaries can accept it directly as a CLI/env argument.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum Network {
    /// NEAR mainnet.
    Mainnet,
    /// NEAR testnet.
    #[default]
    Testnet,
}

impl Network {
    /// The default public RPC URL for this network.
    ///
    /// Consumers can override this (e.g. with a `--rpc-url` flag) before
    /// building a [`near_api::NetworkConfig`].
    #[must_use]
    pub fn rpc_url(self) -> &'static str {
        match self {
            Network::Mainnet => "https://rpc.mainnet.fastnear.com",
            Network::Testnet => "https://rpc.testnet.fastnear.com",
        }
    }

    /// Pyth's public Hermes endpoint for this network, for consumers that submit or read
    /// Pyth price updates. Overridable the same way as [`rpc_url`](Self::rpc_url).
    // The endpoint constants are compile-time literals known to be valid.
    #[allow(clippy::expect_used)]
    #[must_use]
    pub fn hermes_url(self) -> Url {
        match self {
            Network::Mainnet => HERMES_MAINNET_URL,
            Network::Testnet => HERMES_TESTNET_URL,
        }
        .parse()
        .expect("Hermes endpoint constants must be valid URLs")
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Network::Mainnet => "mainnet",
            Network::Testnet => "testnet",
        })
    }
}

/// Builds a [`near_api::NetworkConfig`] for off-chain consumers, resolving the
/// RPC URL and attaching any API key as a header.
///
/// The API key must be sent as a header rather than embedded in the URL:
/// near_api's OpenAPI client builds each request path with `format!("{url}/")`,
/// which appends a slash after the base URL's query string and corrupts a
/// FastNear-style `?apiKey=...` parameter (the endpoint then answers `401`). This
/// builder routes the key through [`near_api::RPCEndpoint::with_api_key`] instead.
/// For backwards compatibility, a key still supplied as an `apiKey` query
/// parameter is extracted from the URL and moved to the header.
pub struct NetworkConfigBuilder {
    network_name: String,
    rpc_url: Url,
    archival_rpc_url: Option<Url>,
    api_key: Option<String>,
}

impl NetworkConfigBuilder {
    /// Start from a [`Network`], defaulting the RPC URL to its public endpoint.
    // The enum's RPC URLs are compile-time constants known to be valid, so the
    // parse cannot fail in practice.
    #[allow(clippy::expect_used)]
    #[must_use]
    pub fn new(network: Network) -> Self {
        Self {
            network_name: network.to_string(),
            rpc_url: network
                .rpc_url()
                .parse()
                .expect("Network::rpc_url must be a valid URL"),
            archival_rpc_url: None,
            api_key: None,
        }
    }

    /// Start from an explicit network name and pre-parsed RPC URL, for consumers
    /// that don't select via the [`Network`] enum.
    #[must_use]
    pub fn from_url(name: impl Into<String>, rpc_url: Url) -> Self {
        Self {
            network_name: name.into(),
            rpc_url,
            archival_rpc_url: None,
            api_key: None,
        }
    }

    /// Override the RPC URL (e.g. from a `--rpc-url` flag). `None` keeps the
    /// current value, so a bare `Some`/`None` CLI argument can be passed through.
    ///
    /// The URL is parsed here so an invalid value fails at the call site rather
    /// than in [`build`](Self::build).
    ///
    /// # Errors
    /// Returns an error if `rpc_url` is `Some` and fails to parse.
    pub fn rpc_url(mut self, rpc_url: Option<&str>) -> Result<Self, url::ParseError> {
        if let Some(rpc_url) = rpc_url {
            self.rpc_url = rpc_url.parse()?;
        }
        Ok(self)
    }

    /// Add an archival RPC endpoint, tried after the primary. Reconciling a
    /// long-submitted transaction needs one: a regular node garbage collects the
    /// outcome and then answers `UNKNOWN_TRANSACTION` for a transaction that did
    /// execute.
    #[must_use]
    pub fn archival_rpc_url(mut self, archival_rpc_url: Option<Url>) -> Self {
        self.archival_rpc_url = archival_rpc_url;
        self
    }

    /// Set the RPC API key, sent as an `Authorization` header. Takes precedence
    /// over a key embedded in the URL; `None` falls back to that embedded key.
    ///
    /// A blank or whitespace-only value (e.g. an empty env var parsed as
    /// `Some("")`) is treated as unset, so it doesn't suppress an embedded key.
    #[must_use]
    pub fn api_key(mut self, api_key: Option<String>) -> Self {
        self.api_key = api_key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());
        self
    }

    /// Resolve the configuration, moving any API key onto the endpoint headers.
    #[must_use]
    pub fn build(self) -> NetworkConfig {
        let mut network = NetworkConfig::from_rpc_url(&self.network_name, self.rpc_url.clone());
        // Primary first: near_api and the gateway's own reconciliation both walk
        // this list in order, so the archival node only answers what the primary
        // could not.
        network.rpc_endpoints = std::iter::once(self.rpc_url)
            .chain(self.archival_rpc_url)
            .map(|url| endpoint(url, self.api_key.as_deref()))
            .collect();
        network
    }
}

/// Build an endpoint, moving any API key onto its header. An explicitly
/// configured key takes precedence over one embedded in this URL.
fn endpoint(mut url: Url, explicit_api_key: Option<&str>) -> RPCEndpoint {
    let embedded_api_key = take_embedded_api_key(&mut url);
    let endpoint = RPCEndpoint::new(url);
    match explicit_api_key.map(str::to_owned).or(embedded_api_key) {
        Some(api_key) => endpoint.with_api_key(api_key),
        None => endpoint,
    }
}

/// Remove and return an `apiKey` query parameter from `rpc_url`, leaving any
/// other query parameters intact.
fn take_embedded_api_key(rpc_url: &mut Url) -> Option<String> {
    let mut api_key = None;
    let remaining: Vec<(String, String)> = rpc_url
        .query_pairs()
        .filter_map(|(key, value)| {
            if key == "apiKey" {
                api_key = Some(value.into_owned());
                None
            } else {
                Some((key.into_owned(), value.into_owned()))
            }
        })
        .collect();

    if api_key.is_some() {
        if remaining.is_empty() {
            rpc_url.set_query(None);
        } else {
            rpc_url.query_pairs_mut().clear().extend_pairs(remaining);
        }
    }

    api_key
}

#[cfg(test)]
mod tests {
    use super::{Network, NetworkConfigBuilder};

    /// A mainnet Pyth receiver rejects a VAA signed by the testnet guardian set.
    #[test]
    fn each_network_gets_its_own_hermes_endpoint() {
        assert_eq!(
            Network::Mainnet.hermes_url().as_str(),
            "https://hermes.pyth.network/"
        );
        assert_eq!(
            Network::Testnet.hermes_url().as_str(),
            "https://hermes-beta.pyth.network/"
        );
    }

    /// Order is the contract: reconciliation walks endpoints in sequence, so the
    /// archival node must only answer what the primary could not.
    #[test]
    fn archival_endpoint_is_appended_after_the_primary() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/".parse().unwrap(),
        )
        .archival_rpc_url(Some(
            "https://archival.mainnet.fastnear.com/?apiKey=ARCHIVAL"
                .parse()
                .unwrap(),
        ))
        .build();

        let urls: Vec<&str> = network
            .rpc_endpoints
            .iter()
            .map(|endpoint| endpoint.url.as_str())
            .collect();
        assert_eq!(
            urls,
            [
                "https://rpc.mainnet.fastnear.com/",
                "https://archival.mainnet.fastnear.com/"
            ]
        );
        assert_eq!(
            network.rpc_endpoints[1].bearer_header.as_deref(),
            Some("Bearer ARCHIVAL")
        );
    }

    /// A key configured for the deployment has to reach the archival endpoint
    /// too, or a keyed provider answers 401 exactly when reconciliation needs it.
    #[test]
    fn explicit_api_key_reaches_the_archival_endpoint() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/".parse().unwrap(),
        )
        .archival_rpc_url(Some(
            "https://archival.mainnet.fastnear.com/".parse().unwrap(),
        ))
        .api_key(Some("SECRET".to_owned()))
        .build();

        for endpoint in &network.rpc_endpoints {
            assert_eq!(endpoint.bearer_header.as_deref(), Some("Bearer SECRET"));
        }
    }

    #[test]
    fn no_archival_endpoint_leaves_a_single_endpoint() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/".parse().unwrap(),
        )
        .build();

        assert_eq!(network.rpc_endpoints.len(), 1);
    }

    #[test]
    fn embedded_api_key_moves_to_header() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/?apiKey=SECRET"
                .parse()
                .unwrap(),
        )
        .build();

        let endpoint = &network.rpc_endpoints[0];
        assert_eq!(endpoint.url.as_str(), "https://rpc.mainnet.fastnear.com/");
        assert_eq!(endpoint.bearer_header.as_deref(), Some("Bearer SECRET"));
    }

    #[test]
    fn embedded_api_key_extraction_preserves_other_query_params() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/?apiKey=SECRET&foo=bar"
                .parse()
                .unwrap(),
        )
        .build();

        let endpoint = &network.rpc_endpoints[0];
        assert_eq!(
            endpoint.url.as_str(),
            "https://rpc.mainnet.fastnear.com/?foo=bar"
        );
        assert_eq!(endpoint.bearer_header.as_deref(), Some("Bearer SECRET"));
    }

    #[test]
    fn blank_api_key_falls_back_to_embedded() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/?apiKey=EMBEDDED"
                .parse()
                .unwrap(),
        )
        .api_key(Some(String::new()))
        .build();

        assert_eq!(
            network.rpc_endpoints[0].bearer_header.as_deref(),
            Some("Bearer EMBEDDED")
        );
    }

    #[test]
    fn blank_api_key_without_embedded_leaves_bare() {
        let network = NetworkConfigBuilder::new(super::Network::Testnet)
            .api_key(Some("   ".to_owned()))
            .build();

        assert!(network.rpc_endpoints[0].bearer_header.is_none());
    }

    #[test]
    fn explicit_api_key_takes_precedence_over_embedded() {
        let network = NetworkConfigBuilder::from_url(
            "mainnet",
            "https://rpc.mainnet.fastnear.com/?apiKey=FROM_URL"
                .parse()
                .unwrap(),
        )
        .api_key(Some("EXPLICIT".to_owned()))
        .build();

        let endpoint = &network.rpc_endpoints[0];
        assert!(endpoint.url.query().is_none());
        assert_eq!(endpoint.bearer_header.as_deref(), Some("Bearer EXPLICIT"));
    }

    #[test]
    fn no_api_key_leaves_endpoint_bare() {
        let network = NetworkConfigBuilder::new(super::Network::Testnet).build();

        assert!(network.rpc_endpoints[0].bearer_header.is_none());
    }

    #[test]
    fn rpc_url_override_replaces_default() {
        let network = NetworkConfigBuilder::new(super::Network::Testnet)
            .rpc_url(Some("https://example.invalid/"))
            .unwrap()
            .build();

        assert_eq!(
            network.rpc_endpoints[0].url.as_str(),
            "https://example.invalid/"
        );
    }

    #[test]
    fn invalid_rpc_url_override_errors_early() {
        let result = NetworkConfigBuilder::new(super::Network::Testnet).rpc_url(Some("not a url"));

        assert!(result.is_err());
    }
}
