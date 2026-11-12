//! Decode stored `CircuitBreakerSet` bytes with both production codecs: Borsh (NEAR state) and
//! postcard (Soroban persistent storage). Inputs are either raw bytes or an arbitrary,
//! possibly structurally invalid set encoded by the codec and then corrupted, so decoding gets
//! past the first length prefix and into the set's own validation.
//!
//! * Decoding never panics.
//! * An uncorrupted encoding decodes exactly when its structure is valid (every breaker id
//!   below `next_id`, both histories of equal capacity), and Borsh reproduces it bit-exactly.
//! * Any decoded set keeps those invariants, round-trips through both codecs, and survives
//!   `validate` and a price update without panicking.
//!
//! MUTATION-CHECK (P5): in `TryFrom<UncheckedCircuitBreakerSet<R>> for CircuitBreakerSet<R>`
//! (contract/proxy-oracle/kernel/src/proxy/circuit_breaker/set.rs), skip
//! `validate_structure`. An uncorrupted set whose breaker id is at or above `next_id` then
//! decodes, and the decode-iff-valid assertion must fire.

#![no_main]

use std::collections::BTreeMap;

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use templar_fuzz::oracle::{self, input::RuleInput};
use templar_primitives::Nanoseconds;
use templar_proxy_oracle_kernel::{
    proxy::circuit_breaker::{
        CircuitBreakerSet, CircuitBreakerState, CircuitBreakerStatus, Observation, RingBuffer,
        UncheckedCircuitBreakerSet,
    },
    Price,
};

const MAX_HISTORY_ENTRIES: usize = 64;
const MAX_BREAKERS: usize = 16;
const MAX_CORRUPTIONS: usize = 8;

#[derive(Arbitrary, Debug, Clone, Copy)]
struct PriceInput {
    mantissa: i64,
    confidence: u64,
    exponent: i32,
    publish_time_ns: u64,
}

impl PriceInput {
    fn price(self) -> Price {
        Price {
            price: self.mantissa,
            conf: self.confidence,
            expo: self.exponent,
            publish_time_ns: Nanoseconds::from_ns(self.publish_time_ns),
        }
    }
}

#[derive(Arbitrary, Debug, Clone, Copy)]
struct ObservationInput {
    price: PriceInput,
    observed_at_ns: u64,
}

impl ObservationInput {
    fn observation(self) -> Observation {
        Observation {
            price: self.price.price(),
            observed_at_ns: Nanoseconds::from_ns(self.observed_at_ns),
        }
    }
}

#[derive(Arbitrary, Debug)]
enum StatusInput {
    ArmedAfter(u64),
    Tripped {
        tripped_at_ns: u64,
        price_update: ObservationInput,
    },
}

#[derive(Arbitrary, Debug)]
struct BreakerInput {
    breaker_id: u32,
    rule: RuleInput<PriceInput>,
    is_enforced: bool,
    status: StatusInput,
}

#[derive(Arbitrary, Debug)]
struct SetInput {
    sample_interval_ns: u64,
    accepted_capacity: u8,
    observed_capacity: u8,
    accepted: Vec<ObservationInput>,
    observed: Vec<ObservationInput>,
    next_id: u32,
    is_manually_tripped: bool,
    breakers: Vec<BreakerInput>,
}

fn ring(capacity: u8, entries: &[ObservationInput]) -> RingBuffer<Observation> {
    let mut ring = RingBuffer::new(capacity.into());
    for entry in entries.iter().take(MAX_HISTORY_ENTRIES) {
        ring.push(entry.observation());
    }
    ring
}

impl SetInput {
    fn unchecked(&self) -> UncheckedCircuitBreakerSet {
        UncheckedCircuitBreakerSet {
            sample_interval_ns: Nanoseconds::from_ns(self.sample_interval_ns),
            accepted_history: ring(self.accepted_capacity, &self.accepted),
            observed_history: ring(self.observed_capacity, &self.observed),
            next_id: self.next_id,
            is_manually_tripped: self.is_manually_tripped,
            breakers: self
                .breakers
                .iter()
                .take(MAX_BREAKERS)
                .map(|input| {
                    let status = match input.status {
                        StatusInput::ArmedAfter(timestamp_ns) => CircuitBreakerStatus::ArmedAfter {
                            timestamp_ns: Nanoseconds::from_ns(timestamp_ns),
                        },
                        StatusInput::Tripped {
                            tripped_at_ns,
                            price_update,
                        } => CircuitBreakerStatus::Tripped {
                            tripped_at_ns: Nanoseconds::from_ns(tripped_at_ns),
                            price_update: price_update.observation(),
                        },
                    };
                    let state = CircuitBreakerState {
                        breaker: input.rule.breaker(|baseline| baseline.price()),
                        is_enforced: input.is_enforced,
                        status,
                    };
                    (input.breaker_id, state)
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }
}

fn is_structurally_valid(set: &UncheckedCircuitBreakerSet) -> bool {
    set.breakers
        .keys()
        .all(|breaker_id| *breaker_id < set.next_id)
        && set.accepted_history.capacity() == set.observed_history.capacity()
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum Codec {
    Borsh,
    Postcard,
}

impl Codec {
    fn encode(self, set: &UncheckedCircuitBreakerSet) -> Vec<u8> {
        match self {
            Self::Borsh => borsh::to_vec(set).ok(),
            Self::Postcard => postcard::to_allocvec(set).ok(),
        }
        .unwrap_or_else(|| panic!("{self:?} failed to encode {set:?}"))
    }

    fn decode(self, bytes: &[u8]) -> Option<CircuitBreakerSet> {
        match self {
            Self::Borsh => borsh::from_slice(bytes).ok(),
            Self::Postcard => postcard::from_bytes(bytes).ok(),
        }
    }
}

#[derive(Arbitrary, Debug)]
enum Stored<'a> {
    Raw(&'a [u8]),
    Encoded {
        set: SetInput,
        codec: Codec,
        corruptions: Vec<(u16, u8)>,
    },
}

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    stored: Stored<'a>,
    update: PriceInput,
    now_ns: u64,
}

fn assert_structure(set: &CircuitBreakerSet) {
    let capacity = set.accepted_history().capacity();
    assert_eq!(set.observed_history().capacity(), capacity);
    for history in [set.accepted_history(), set.observed_history()] {
        assert!(u32::try_from(history.len()).is_ok_and(|len| len <= capacity));
    }
    assert!(set
        .breakers()
        .keys()
        .all(|breaker_id| *breaker_id < set.next_id()));
}

fn exercise(mut set: CircuitBreakerSet, update: PriceInput, now_ns: u64) {
    assert_structure(&set);
    oracle::assert_storage_round_trips(&set);
    let _validation = set.validate();
    let _acceptance = set.try_accept_price(update.price(), Nanoseconds::from_ns(now_ns));
    assert_structure(&set);
}

fuzz_target!(|input: Input<'_>| {
    match &input.stored {
        Stored::Raw(bytes) => {
            for codec in [Codec::Borsh, Codec::Postcard] {
                if let Some(set) = codec.decode(bytes) {
                    exercise(set, input.update, input.now_ns);
                }
            }
        }
        Stored::Encoded {
            set,
            codec,
            corruptions,
        } => {
            let unchecked = set.unchecked();
            let mut bytes = codec.encode(&unchecked);
            for (offset, mask) in corruptions.iter().take(MAX_CORRUPTIONS) {
                let len = bytes.len();
                if let Some(byte) = bytes.get_mut(usize::from(*offset) % len.max(1)) {
                    *byte ^= mask;
                }
            }
            let decoded = codec.decode(&bytes);
            if corruptions.is_empty() {
                assert_eq!(
                    decoded.is_some(),
                    is_structurally_valid(&unchecked),
                    "{codec:?} decode of {unchecked:?}"
                );
                if let (Codec::Borsh, Some(decoded)) = (codec, &decoded) {
                    assert_eq!(
                        &UncheckedCircuitBreakerSet::from(decoded.clone()),
                        &unchecked
                    );
                }
            }
            if let Some(decoded) = decoded {
                exercise(decoded, input.update, input.now_ns);
            }
        }
    }
});
