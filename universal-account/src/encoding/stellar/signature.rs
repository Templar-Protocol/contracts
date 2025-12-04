use near_sdk::near;

type ByteEncoding = [u8; 64];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[near(serializers = [borsh])]
pub struct Signature(pub ByteEncoding);

#[cfg(test)]
mod tests {
    use near_sdk::{
        json_types::Base64VecU8,
        serde_json::{self, json},
    };

    #[test]
    fn verify() {
        let signature_b64 = "Lu5CNbdyO/ZMPIdGNFB43WY3JgQ39FTDXjtbLP6Hxz4stAAFwad1GJviErrmBIMSpEqcgv01d3PRbDZNJTE+Cw==";
        let pubkey_str = "GBPNJTA5DARWSGLGPAGUHZBE44IOCFSKCL525WK7VZK6BEX4DPLIJXZ7";
        let message = "my message";

        let b: Base64VecU8 = serde_json::from_value(json!(signature_b64)).unwrap();

        eprintln!("{b:?}");
        eprintln!("{}", b.0.len());
    }
}
