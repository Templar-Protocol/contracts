use std::{
    num::{NonZeroU64, NonZeroUsize},
    time::Duration,
};

use clap::Parser;
use getset::{CopyGetters, Getters};

use crate::hot_relayer::{HotRelayerError, HotRelayerRouting, HOT_STELLAR_CHAIN_ID};

const DEFAULT_MPC_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MAX_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Parser)]
#[command(name = "hot-relayer")]
#[command(about = "Narrow HOT bridge completion relayer")]
#[command(version)]
pub struct Config {
    #[arg(long, env = "PORT", default_value_t = 3001)]
    pub port: u16,

    #[arg(long, env = "HOT_MPC_API_URL")]
    pub hot_mpc_api_url: String,

    #[arg(long, env = "HOT_RELAYER_NEAR_RECEIVER")]
    pub near_receiver: String,

    #[arg(long, env = "HOT_RELAYER_STELLAR_RECEIVER")]
    pub stellar_receiver: String,

    #[arg(long, env = "HOT_RELAYER_TOKEN_ID")]
    pub token_id: String,

    #[arg(long, env = "HOT_RELAYER_CHAIN_ID", default_value_t = HOT_STELLAR_CHAIN_ID)]
    pub chain_id: u64,

    #[arg(long, env = "HOT_RELAYER_AUTH_TOKEN")]
    pub auth_token: String,

    #[arg(
        long,
        env = "HOT_RELAYER_MPC_TIMEOUT_SECS",
        default_value_t = DEFAULT_MPC_TIMEOUT_SECS
    )]
    pub mpc_timeout_secs: u64,

    #[arg(
        long,
        env = "HOT_RELAYER_MAX_REQUEST_BYTES",
        default_value_t = DEFAULT_MAX_REQUEST_BYTES
    )]
    pub max_request_bytes: usize,
}

impl Config {
    pub fn validate(&self) -> Result<ValidatedConfig, ConfigError> {
        let hot_mpc_api_url = HotMpcApiUrl::parse(&self.hot_mpc_api_url)?;
        let auth_token = AuthToken::new(&self.auth_token)?;
        let mpc_timeout = MpcTimeout::new(self.mpc_timeout_secs)?;
        let max_request_bytes = MaxRequestBytes::new(self.max_request_bytes)?;

        let routing = HotRelayerRouting::new(
            self.near_receiver.clone(),
            self.stellar_receiver.clone(),
            self.chain_id,
            self.token_id.clone(),
        )
        .map_err(ConfigError::Relayer)?;

        Ok(ValidatedConfig {
            port: self.port,
            hot_mpc_api_url,
            routing,
            auth_token,
            mpc_timeout,
            max_request_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MpcTimeout(NonZeroU64);

impl MpcTimeout {
    fn new(value: u64) -> Result<Self, ConfigError> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(ConfigError::ZeroMpcTimeout)
    }

    #[must_use]
    pub fn duration(self) -> Duration {
        Duration::from_secs(self.0.get())
    }

    #[must_use]
    pub fn seconds(self) -> u64 {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxRequestBytes(NonZeroUsize);

impl MaxRequestBytes {
    fn new(value: usize) -> Result<Self, ConfigError> {
        NonZeroUsize::new(value)
            .map(Self)
            .ok_or(ConfigError::ZeroMaxRequestBytes)
    }

    #[must_use]
    pub fn get(self) -> usize {
        self.0.get()
    }
}

#[derive(Debug, Clone, Getters, CopyGetters)]
pub struct ValidatedConfig {
    #[get_copy = "pub"]
    port: u16,
    #[get = "pub"]
    hot_mpc_api_url: HotMpcApiUrl,
    #[get = "pub"]
    routing: HotRelayerRouting,
    #[get = "pub"]
    auth_token: AuthToken,
    #[get_copy = "pub"]
    mpc_timeout: MpcTimeout,
    #[get_copy = "pub"]
    max_request_bytes: MaxRequestBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotMpcApiUrl(reqwest::Url);

impl HotMpcApiUrl {
    pub(crate) fn parse(value: &str) -> Result<Self, ConfigError> {
        let trimmed = require_non_empty("HOT_MPC_API_URL", value)?;
        let mut url = reqwest::Url::parse(trimmed).map_err(|error| ConfigError::InvalidUrl {
            name: "HOT_MPC_API_URL",
            reason: error.to_string(),
        })?;

        match url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(ConfigError::InvalidUrl {
                    name: "HOT_MPC_API_URL",
                    reason: format!("unsupported scheme {scheme}"),
                });
            }
        }

        if !url.path().ends_with('/') {
            let path = format!("{}/", url.path());
            url.set_path(&path);
        }

        Ok(Self(url))
    }

    #[must_use]
    pub fn join(&self, path: &str) -> reqwest::Url {
        self.0
            .join(path.trim_start_matches('/'))
            .unwrap_or_else(|error| panic!("validated HOT MPC URL must join path: {error}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    #[must_use]
    pub fn redacted(&self) -> reqwest::Url {
        let mut url = self.0.clone();
        if !url.username().is_empty() {
            let _ = url.set_username("redacted");
        }
        if url.password().is_some() {
            let _ = url.set_password(Some("redacted"));
        }
        if url.query().is_some() {
            url.set_query(Some("redacted"));
        }
        if url.path() != "/" {
            url.set_path("/redacted");
        }
        url.set_fragment(None);
        url
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthToken(String);

impl AuthToken {
    pub(crate) fn new(value: &str) -> Result<Self, ConfigError> {
        let trimmed = require_non_empty("HOT_RELAYER_AUTH_TOKEN", value)?;
        Ok(Self(trimmed.to_string()))
    }

    #[must_use]
    pub fn bearer_header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

fn require_non_empty<'a>(name: &'static str, value: &'a str) -> Result<&'a str, ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(ConfigError::Missing(name))
    } else {
        Ok(trimmed)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{name} is invalid: {reason}")]
    InvalidUrl { name: &'static str, reason: String },
    #[error("HOT_RELAYER_MPC_TIMEOUT_SECS must be non-zero")]
    ZeroMpcTimeout,
    #[error("HOT_RELAYER_MAX_REQUEST_BYTES must be non-zero")]
    ZeroMaxRequestBytes,
    #[error("{0}")]
    Relayer(#[from] HotRelayerError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_config() -> Config {
        Config {
            port: 3001,
            hot_mpc_api_url: "https://rpc1.hotdao.ai".to_string(),
            near_receiver: "vault-counterparty.near".to_string(),
            stellar_receiver: "GCMVV45LOZUYYVXOQJ626VXGL3KFXY73DHFBT4EDPDBE2LN4USRQDYVV"
                .to_string(),
            token_id: "1100_CUSDC".to_string(),
            chain_id: HOT_STELLAR_CHAIN_ID,
            auth_token: "relay-secret".to_string(),
            mpc_timeout_secs: DEFAULT_MPC_TIMEOUT_SECS,
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
        }
    }

    #[test]
    fn validate_builds_typed_config() {
        let config = valid_config();
        let validated = config.validate().unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(validated.port(), 3001);
        assert_eq!(
            validated.hot_mpc_api_url().join("/withdraw/sign").as_str(),
            "https://rpc1.hotdao.ai/withdraw/sign"
        );
        assert_eq!(
            validated.auth_token().bearer_header(),
            "Bearer relay-secret"
        );
        assert_eq!(validated.routing().chain_id(), HOT_STELLAR_CHAIN_ID);
        assert_eq!(
            validated.mpc_timeout().duration(),
            Duration::from_secs(DEFAULT_MPC_TIMEOUT_SECS)
        );
        assert_eq!(
            validated.max_request_bytes().get(),
            DEFAULT_MAX_REQUEST_BYTES
        );
    }

    #[test]
    fn validate_redacts_mpc_url_for_logs() {
        let mut config = valid_config();
        config.hot_mpc_api_url = "https://user:pass@example.com/private?token=secret".to_string();
        let validated = config.validate().unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            validated.hot_mpc_api_url().redacted().as_str(),
            "https://redacted:redacted@example.com/redacted?redacted"
        );
    }

    #[test]
    fn validate_rejects_missing_auth() {
        let mut config = valid_config();
        config.auth_token = " ".to_string();

        assert!(matches!(
            config.validate(),
            Err(ConfigError::Missing("HOT_RELAYER_AUTH_TOKEN"))
        ));
    }

    #[test]
    fn validate_rejects_invalid_mpc_url() {
        let mut config = valid_config();
        config.hot_mpc_api_url = "not a url".to_string();

        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidUrl {
                name: "HOT_MPC_API_URL",
                ..
            })
        ));
    }

    #[test]
    fn validate_rejects_invalid_routing() {
        let mut config = valid_config();
        config.chain_id = 1101;

        assert!(matches!(
            config.validate(),
            Err(ConfigError::Relayer(HotRelayerError::InvalidRouting {
                field: "chain_id",
                ..
            }))
        ));
    }

    #[test]
    fn validate_rejects_zero_operational_limits() {
        let mut config = valid_config();
        config.mpc_timeout_secs = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::ZeroMpcTimeout)
        ));

        let mut config = valid_config();
        config.max_request_bytes = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::ZeroMaxRequestBytes)
        ));
    }
}
