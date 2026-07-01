//! NEAR network selection for off-chain gateway consumers.

use std::fmt;

use near_api::NetworkConfig;

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
    rpc_url: url::Url,
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
            api_key: None,
        }
    }

    /// Start from an explicit network name and pre-parsed RPC URL, for consumers
    /// that don't select via the [`Network`] enum.
    #[must_use]
    pub fn from_url(name: impl Into<String>, rpc_url: url::Url) -> Self {
        Self {
            network_name: name.into(),
            rpc_url,
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

    /// Set the RPC API key, sent as an `Authorization` header. Takes precedence
    /// over a key embedded in the URL; `None` falls back to that embedded key.
    #[must_use]
    pub fn api_key(mut self, api_key: Option<String>) -> Self {
        self.api_key = api_key;
        self
    }

    /// Resolve the configuration, moving any API key onto the endpoint header.
    #[must_use]
    pub fn build(mut self) -> NetworkConfig {
        let embedded_key = self
            .rpc_url
            .query_pairs()
            .find(|(key, _)| key == "apiKey")
            .map(|(_, value)| value.into_owned());
        if embedded_key.is_some() {
            self.rpc_url.set_query(None);
        }

        let mut network = NetworkConfig::from_rpc_url(&self.network_name, self.rpc_url);
        if let Some(api_key) = self.api_key.or(embedded_key) {
            network.rpc_endpoints = network
                .rpc_endpoints
                .into_iter()
                .map(|endpoint| endpoint.with_api_key(api_key.clone()))
                .collect();
        }

        network
    }
}

#[cfg(test)]
mod tests {
    use super::NetworkConfigBuilder;

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
