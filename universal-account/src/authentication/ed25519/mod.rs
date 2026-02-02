use std::marker::PhantomData;

use near_sdk::near;
use near_sdk::serde::de::DeserializeOwned;

use crate::encoding;

use super::with_raw_string::WithRawString;
use super::{
    CheckSignatureError, ExecutionContextProvider, HashForSigning, Key, MessageWithSignature,
    MessageWithValidSignature, Payload,
};

pub mod eip191;
pub mod raw;
pub mod sep53;

pub trait Ed25519Variant {
    const PREFIX: &'static [u8];
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[near(serializers = [json])]
#[serde(transparent, bound = "T: DeserializeOwned")]
pub struct Message<K: Ed25519Variant, T>(
    pub WithRawString<Payload<T>>,
    #[serde(skip)] pub PhantomData<K>,
);

impl<K: Ed25519Variant, T> HashForSigning for Message<K, T> {
    const MAGIC_NUMBER: &'static [u8] = K::PREFIX;

    fn content_bytes(&self) -> Vec<u8> {
        self.0.raw.as_bytes().to_vec()
    }
}

impl<K: Ed25519Variant + Key<Self>, T> super::SignableMessage for Message<K, T> {
    type Key = K;
    type Signature = encoding::ed25519::Signature;
    type Auxiliary = ();
}

impl<K: Ed25519Variant + Key<Self>, T: near_sdk::serde::Serialize> Message<K, T> {
    pub fn new(inner: WithRawString<Payload<T>>) -> Self {
        Self(inner, PhantomData)
    }

    pub fn from_parsed(payload: Payload<T>) -> Self {
        Self(WithRawString::from_parsed(payload), PhantomData)
    }

    pub fn with_signature(
        self,
        signature: encoding::ed25519::Signature,
    ) -> MessageWithSignature<Self> {
        MessageWithSignature {
            message: self,
            signature,
            auxiliary: (),
        }
    }
}

impl<K: Ed25519Variant, T> From<WithRawString<Payload<T>>> for Message<K, T> {
    fn from(value: WithRawString<Payload<T>>) -> Self {
        Self(value, PhantomData)
    }
}

impl<K: Ed25519Variant + Key<Message<K, T>>, T> ExecutionContextProvider
    for MessageWithValidSignature<Message<K, T>>
{
    type Payload = T;

    fn payload(self) -> Payload<Self::Payload> {
        self.0.message.0.parsed
    }

    fn origin(&self) -> Option<&str> {
        None
    }
}
