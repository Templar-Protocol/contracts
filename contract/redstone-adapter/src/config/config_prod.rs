use hex_literal::hex;

pub type SignerAddressBs = [u8; 20];

pub const SIGNER_COUNT: usize = 5 /*+ 5*/;
pub const UPDATER_COUNT: usize = 1;

pub const MAX_TIMESTAMP_AHEAD_MS: u64 = 3 * 60 * 1_000;
pub const MAX_TIMESTAMP_DELAY_MS: u64 = 3 * 60 * 1_000;

pub const REDSTONE_PRIMARY_PROD_ALLOWED_SIGNERS: [SignerAddressBs; SIGNER_COUNT] = [
    hex!("8bb8f32df04c8b654987daaed53d6b6091e3b774"),
    hex!("deb22f54738d54976c4c0fe5ce6d408e40d88499"),
    hex!("51ce04be4b3e32572c4ec9135221d0691ba7d202"),
    hex!("dd682daec5a90dd295d14da4b0bec9281017b5be"),
    hex!("9c5ae89c4af6aa32ce58588dbaf90d18a855b6de"),
    // new
    // hex!("7e41cba46d3dcfcabaadcbaec39fd9be49637366"),
    // [
    //     189, 224, 123, 252, 151, 70, 6, 229, 123, 207, 139, 30, 67, 16, 24, 113, 138, 1, 252, 220,
    // ],
    // [
    //     89, 114, 16, 135, 99, 87, 215, 165, 44, 7, 13, 15, 48, 0, 119, 22, 98, 91, 211, 213,
    // ],
    // [
    //     53, 2, 246, 217, 254, 45, 199, 60, 238, 103, 89, 164, 115, 4, 189, 77, 185, 196, 102, 36,
    // ],
    // [
    //     183, 184, 246, 151, 77, 156, 175, 254, 157, 130, 138, 162, 57, 87, 123, 116, 170, 100, 104,
    //     36,
    // ],
];

#[test]
fn prod_signers() {
    for signer in REDSTONE_PRIMARY_PROD_ALLOWED_SIGNERS {
        eprintln!("{:?}", signer);
        eprintln!("{}", hex::encode(signer));
    }
}

pub const TRUSTED_UPDATERS: [&str; UPDATER_COUNT] = ["trusted_updater.near"];
