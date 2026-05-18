use near_sdk::{near, BorshStorageKey};

#[derive(BorshStorageKey, Debug, Clone, PartialEq, Eq)]
#[near(serializers = [json, borsh])]
pub enum Role {
    OfflineManualTrip,
    OfflineManualUntrip,
}
