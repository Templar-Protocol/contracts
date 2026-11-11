//! Backward-compatible, versioned breaker storage bytes.

#[cfg(feature = "borsh")]
use alloc::string::ToString;
#[cfg(feature = "postcard")]
use alloc::vec::Vec;

use super::CircuitBreakerSet;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "borsh", derive(::borsh::BorshSchema))]
pub struct VersionedCircuitBreakerSet {
    magic: [u8; 16],
    version: u8,
    set: CircuitBreakerSet,
}

impl VersionedCircuitBreakerSet {
    // Borsh sees accepted_history { capacity: 0, entries: 1 }; postcard sees
    // capacity 66 with 82 entries. Neither can be a valid legacy RingBuffer.
    pub const MAGIC: [u8; 16] = *b"CBREAKER\x00\x00\x00\x00\x01\x00\x00\x00";
    pub const VERSION: u8 = 1;

    #[must_use]
    pub fn new(set: CircuitBreakerSet) -> Self {
        Self {
            magic: Self::MAGIC,
            version: Self::VERSION,
            set,
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::new(CircuitBreakerSet::empty())
    }

    #[must_use]
    pub fn into_inner(self) -> CircuitBreakerSet {
        self.set
    }

    #[must_use]
    pub fn as_inner(&self) -> &CircuitBreakerSet {
        &self.set
    }

    #[cfg(feature = "postcard")]
    pub fn to_postcard_bytes(&self) -> Result<Vec<u8>, postcard::Error> {
        let mut bytes = Vec::from(Self::MAGIC);
        bytes.push(Self::VERSION);
        bytes.extend(postcard::to_allocvec(&self.set)?);
        Ok(bytes)
    }

    #[cfg(feature = "postcard")]
    pub fn from_postcard_bytes(bytes: &[u8]) -> Result<Self, postcard::Error> {
        fn decode_set(bytes: &[u8]) -> Result<CircuitBreakerSet, postcard::Error> {
            let (set, remaining) = postcard::take_from_bytes(bytes)?;
            if remaining.is_empty() {
                Ok(set)
            } else {
                Err(postcard::Error::DeserializeBadEncoding)
            }
        }

        if !bytes.starts_with(&Self::MAGIC) {
            return decode_set(bytes).map(Self::new);
        }
        let Some(&version) = bytes.get(Self::MAGIC.len()) else {
            return Err(postcard::Error::DeserializeBadEncoding);
        };
        if version != Self::VERSION {
            return Err(postcard::Error::DeserializeBadEncoding);
        }
        Ok(Self {
            magic: Self::MAGIC,
            version,
            set: decode_set(&bytes[Self::MAGIC.len() + 1..])?,
        })
    }
}

#[cfg(feature = "borsh")]
struct PrefixReader<'a, R> {
    prefix: &'a [u8],
    reader: &'a mut R,
}

#[cfg(feature = "borsh")]
impl<R: ::borsh::io::Read> ::borsh::io::Read for PrefixReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> ::borsh::io::Result<usize> {
        if self.prefix.is_empty() {
            return self.reader.read(buf);
        }
        let count = buf.len().min(self.prefix.len());
        buf[..count].copy_from_slice(&self.prefix[..count]);
        self.prefix = &self.prefix[count..];
        Ok(count)
    }
}

impl From<CircuitBreakerSet> for VersionedCircuitBreakerSet {
    fn from(set: CircuitBreakerSet) -> Self {
        Self::new(set)
    }
}

impl core::ops::Deref for VersionedCircuitBreakerSet {
    type Target = CircuitBreakerSet;
    fn deref(&self) -> &Self::Target {
        self.as_inner()
    }
}

impl core::ops::DerefMut for VersionedCircuitBreakerSet {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.set
    }
}

#[cfg(feature = "borsh")]
impl ::borsh::BorshSerialize for VersionedCircuitBreakerSet {
    fn serialize<W: ::borsh::io::Write>(&self, writer: &mut W) -> ::borsh::io::Result<()> {
        writer.write_all(&Self::MAGIC)?;
        writer.write_all(&[Self::VERSION])?;
        self.set.serialize(writer)
    }
}

#[cfg(feature = "borsh")]
impl ::borsh::BorshDeserialize for VersionedCircuitBreakerSet {
    fn deserialize_reader<R: ::borsh::io::Read>(reader: &mut R) -> ::borsh::io::Result<Self> {
        let mut prefix = [0_u8; Self::MAGIC.len()];
        reader.read_exact(&mut prefix)?;
        if prefix != Self::MAGIC {
            let mut reader = PrefixReader {
                prefix: &prefix,
                reader,
            };
            return CircuitBreakerSet::deserialize_reader(&mut reader).map(Self::new);
        }
        let version = u8::deserialize_reader(reader)?;
        if version != Self::VERSION {
            return Err(::borsh::io::Error::new(
                ::borsh::io::ErrorKind::InvalidData,
                "unsupported circuit breaker storage version",
            ));
        }
        Ok(Self {
            magic: Self::MAGIC,
            version,
            set: CircuitBreakerSet::deserialize_reader(reader)?,
        })
    }
}

#[cfg(all(test, feature = "borsh", feature = "postcard"))]
mod tests {
    use super::*;
    use crate::proxy::circuit_breaker::CircuitBreakerSetConfig;
    use templar_primitives::Nanoseconds;

    fn envelope(version: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::from(VersionedCircuitBreakerSet::MAGIC);
        bytes.push(version);
        bytes.extend_from_slice(payload);
        bytes
    }

    const ARMED_SET: &[u8] = &[
        0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 1,
        0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 7, 0, 0, 0, 0, 0, 0, 0,
    ];
    const TRIPPED_SET: &[u8] = &[
        0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 1,
        0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 9, 0, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0, 0, 0, 0, 0,
        1, 0, 0, 0, 0, 0, 0, 0, 254, 255, 255, 255, 8, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0,
    ];
    const ARMED_POSTCARD: &[u8] = b"\x010\x02\x00\x02\x00\x04\x00\x01\x03\x00\x011\x01\x00\x017";
    const TRIPPED_POSTCARD: &[u8] =
        b"\x010\x02\x00\x02\x00\x04\x00\x01\x03\x00\x011\x01\x01\x019\xc8\x01\x01\x03\x018\x019";

    #[test]
    fn legacy_bytes_roundtrip_and_new_bytes_are_versioned() {
        for (borsh_bytes, postcard_bytes) in
            [(ARMED_SET, ARMED_POSTCARD), (TRIPPED_SET, TRIPPED_POSTCARD)]
        {
            let from_borsh =
                <VersionedCircuitBreakerSet as ::borsh::BorshDeserialize>::try_from_slice(
                    borsh_bytes,
                )
                .unwrap();
            let from_postcard =
                VersionedCircuitBreakerSet::from_postcard_bytes(postcard_bytes).unwrap();
            assert_eq!(from_borsh, from_postcard);
            let encoded = from_borsh.to_postcard_bytes().unwrap();
            assert!(encoded.starts_with(&envelope(VersionedCircuitBreakerSet::VERSION, &[])));
            assert!(postcard::from_bytes::<CircuitBreakerSet>(&encoded).is_err());
            assert_eq!(
                VersionedCircuitBreakerSet::from_postcard_bytes(&encoded).unwrap(),
                from_borsh
            );
            let borsh_encoded = ::borsh::to_vec(&from_borsh).unwrap();
            assert!(borsh_encoded.starts_with(&envelope(VersionedCircuitBreakerSet::VERSION, &[])));
            assert!(::borsh::from_slice::<CircuitBreakerSet>(&borsh_encoded).is_err());
            assert_eq!(
                <VersionedCircuitBreakerSet as ::borsh::BorshDeserialize>::try_from_slice(
                    &borsh_encoded
                )
                .unwrap(),
                from_borsh
            );
        }
    }

    #[test]
    fn unknown_versions_and_truncated_envelopes_fail_closed() {
        for bytes in [
            Vec::from(VersionedCircuitBreakerSet::MAGIC),
            envelope(2, &[]),
            envelope(1, &[]),
        ] {
            assert!(VersionedCircuitBreakerSet::from_postcard_bytes(&bytes).is_err());
            assert!(
                <VersionedCircuitBreakerSet as ::borsh::BorshDeserialize>::try_from_slice(&bytes)
                    .is_err()
            );
        }
    }

    #[test]
    fn trailing_bytes_are_rejected_for_legacy_and_versioned_payloads() {
        for legacy in [ARMED_SET, TRIPPED_SET] {
            let mut bytes = legacy.to_vec();
            bytes.push(0);
            assert!(
                <VersionedCircuitBreakerSet as ::borsh::BorshDeserialize>::try_from_slice(&bytes)
                    .is_err()
            );
        }
        for legacy in [ARMED_POSTCARD, TRIPPED_POSTCARD] {
            let mut bytes = legacy.to_vec();
            bytes.push(0);
            assert!(VersionedCircuitBreakerSet::from_postcard_bytes(&bytes).is_err());
        }
        let mut encoded = VersionedCircuitBreakerSet::empty()
            .to_postcard_bytes()
            .unwrap();
        encoded.push(0);
        assert!(VersionedCircuitBreakerSet::from_postcard_bytes(&encoded).is_err());
    }

    #[test]
    fn legacy_borsh_prefix_collision_and_enclosing_fields_are_preserved() {
        let set = CircuitBreakerSet::new(CircuitBreakerSetConfig {
            sample_interval_ns: Nanoseconds::from_ns(u64::from_le_bytes(
                VersionedCircuitBreakerSet::MAGIC[..8].try_into().unwrap(),
            )),
            history_len: 0,
        });
        let mut bytes = ::borsh::to_vec(&set).unwrap();
        assert_eq!(bytes[..8], VersionedCircuitBreakerSet::MAGIC[..8]);
        bytes.push(42);
        let (decoded, trailing) =
            <(VersionedCircuitBreakerSet, u8) as ::borsh::BorshDeserialize>::try_from_slice(&bytes)
                .unwrap();
        assert_eq!(decoded.into_inner(), set);
        assert_eq!(trailing, 42);
    }
}
