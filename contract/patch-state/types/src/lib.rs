#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use borsh::{BorshDeserialize, BorshSerialize};

/// Tags are written out because [`Patch::from_borsh_slice`] matches on them:
/// renumbering a variant here silently redirects a decode.
#[derive(Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Op {
    Set {
        key: Vec<u8>,
        value: Vec<u8>,
    } = 0,
    Remove {
        key: Vec<u8>,
    } = 1,
    Expect {
        key: Vec<u8>,
        value: Option<Vec<u8>>,
    } = 2,
    ExpectHash {
        key: Vec<u8>,
        sha256: [u8; 32],
    } = 3,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct Patch {
    pub account_id: String,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchDecodeError {
    InvalidLength,
    InvalidOption,
    InvalidString,
    InvalidVariant,
    TrailingBytes,
    UnexpectedEnd,
}

struct Decoder<'a> {
    remaining: &'a [u8],
}

impl<'a> Decoder<'a> {
    const fn new(input: &'a [u8]) -> Self {
        Self { remaining: input }
    }

    fn byte(&mut self) -> Result<u8, PatchDecodeError> {
        Ok(self.take(1)?[0])
    }

    fn bytes(&mut self) -> Result<Vec<u8>, PatchDecodeError> {
        let length = self.length()?;
        Ok(self.take(length)?.to_vec())
    }

    fn length(&mut self) -> Result<usize, PatchDecodeError> {
        let length = self.take(4)?;
        let length = u32::from_le_bytes(
            length
                .try_into()
                .map_err(|_| PatchDecodeError::InvalidLength)?,
        );
        usize::try_from(length).map_err(|_| PatchDecodeError::InvalidLength)
    }

    fn string(&mut self) -> Result<String, PatchDecodeError> {
        let bytes = self.bytes()?;
        String::from_utf8(bytes).map_err(|_| PatchDecodeError::InvalidString)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], PatchDecodeError> {
        let value = self
            .remaining
            .get(..length)
            .ok_or(PatchDecodeError::UnexpectedEnd)?;
        self.remaining = &self.remaining[length..];
        Ok(value)
    }
}

impl Patch {
    pub fn from_borsh_slice(input: &[u8]) -> Result<Self, PatchDecodeError> {
        let mut decoder = Decoder::new(input);
        let account_id = decoder.string()?;
        let operations = decoder.length()?;
        if operations > decoder.remaining.len() / 5 {
            return Err(PatchDecodeError::InvalidLength);
        }
        let mut ops = Vec::with_capacity(operations);

        for _ in 0..operations {
            let operation = match decoder.byte()? {
                0 => Op::Set {
                    key: decoder.bytes()?,
                    value: decoder.bytes()?,
                },
                1 => Op::Remove {
                    key: decoder.bytes()?,
                },
                2 => {
                    let key = decoder.bytes()?;
                    let value = match decoder.byte()? {
                        0 => None,
                        1 => Some(decoder.bytes()?),
                        _ => return Err(PatchDecodeError::InvalidOption),
                    };
                    Op::Expect { key, value }
                }
                3 => Op::ExpectHash {
                    key: decoder.bytes()?,
                    sha256: decoder
                        .take(32)?
                        .try_into()
                        .map_err(|_| PatchDecodeError::InvalidLength)?,
                },
                _ => return Err(PatchDecodeError::InvalidVariant),
            };
            ops.push(operation);
        }

        if !decoder.remaining.is_empty() {
            return Err(PatchDecodeError::TrailingBytes);
        }

        Ok(Self { account_id, ops })
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::String, vec};

    #[test]
    fn round_trips_binary_patch_operations() {
        let patch = Patch {
            account_id: String::from("target.near"),
            ops: vec![
                Op::Set {
                    key: vec![0, 255],
                    value: vec![1, 2, 3],
                },
                Op::Remove { key: vec![4, 5] },
                Op::Expect {
                    key: vec![6],
                    value: None,
                },
                Op::ExpectHash {
                    key: vec![7],
                    sha256: [8; 32],
                },
            ],
        };

        let encoded = match borsh::to_vec(&patch) {
            Ok(encoded) => encoded,
            Err(error) => panic!("serialize patch: {error}"),
        };
        let decoded = match Patch::from_borsh_slice(&encoded) {
            Ok(decoded) => decoded,
            Err(error) => panic!("decode patch: {error:?}"),
        };
        assert_eq!(decoded, patch);
    }

    #[test]
    fn rejects_operation_count_larger_than_the_input() {
        assert!(matches!(
            Patch::from_borsh_slice(&[0, 0, 0, 0, 1, 0, 0, 0]),
            Err(PatchDecodeError::InvalidLength)
        ));
    }
}
