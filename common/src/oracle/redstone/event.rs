use near_sdk::{
    near,
    serde::{
        ser::{SerializeSeq, SerializeStruct},
        Serialize, Serializer,
    },
    AccountId,
};

use super::{config::SignerAddressBs, feed_data::FeedData, Config, FeedId};

#[near(event_json(standard = "redstone-adapter"))]
pub enum RedStoneEvent<'a> {
    #[event_version("1.0.0")]
    WritePrices {
        updater: AccountId,
        updated_feeds: Vec<(FeedId, FeedData)>,
    },

    /// The effective policy after a successful configuration replacement. Signers are
    /// lowercase hex strings in this event only, bounding even 255 signers below
    /// NEAR's 16 KiB receipt-log limit without copying the stored configuration.
    #[event_version("1.0.0")]
    ConfigUpdated {
        updater: AccountId,
        #[serde(serialize_with = "serialize_config")]
        config: &'a Config,
    },
}

fn serialize_config<S: Serializer>(config: &Config, serializer: S) -> Result<S::Ok, S::Error> {
    // Keep exhaustive so new config fields require an event-schema decision.
    let Config {
        signer_count_threshold,
        signers,
        max_timestamp_delay_ms,
        max_timestamp_ahead_ms,
        min_interval_between_updates_ms,
    } = config;
    let mut data = serializer.serialize_struct("Config", 5)?;
    data.serialize_field("signer_count_threshold", signer_count_threshold)?;
    data.serialize_field("signers", &HexSigners(signers))?;
    data.serialize_field("max_timestamp_delay_ms", max_timestamp_delay_ms)?;
    data.serialize_field("max_timestamp_ahead_ms", max_timestamp_ahead_ms)?;
    data.serialize_field(
        "min_interval_between_updates_ms",
        min_interval_between_updates_ms,
    )?;
    data.end()
}

struct HexSigners<'a>(&'a [SignerAddressBs]);

impl Serialize for HexSigners<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut signers = serializer.serialize_seq(Some(self.0.len()))?;
        for signer in self.0 {
            signers.serialize_element(&HexSigner(signer))?;
        }
        signers.end()
    }
}

struct HexSigner<'a>(&'a SignerAddressBs);

impl Serialize for HexSigner<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut encoded = [0; 40];
        hex::encode_to_slice(self.0, &mut encoded).map_err(near_sdk::serde::ser::Error::custom)?;
        let address = std::str::from_utf8(&encoded).map_err(near_sdk::serde::ser::Error::custom)?;
        serializer.serialize_str(address)
    }
}
