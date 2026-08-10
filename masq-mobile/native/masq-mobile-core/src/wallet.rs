use bip39::{Language, Mnemonic, Seed};
use libsecp256k1::{PublicKey, SecretKey};
use tiny_hderive::bip32::ExtendedPrivKey;
use tiny_keccak::{Hasher, Keccak};
use zeroize::Zeroizing;

const CONSUMER_DERIVATION_PATH: &str = "m/44'/60'/0'/0/0";

pub struct WalletMaterial {
    #[allow(dead_code)]
    secret: Zeroizing<Vec<u8>>,
    address: String,
}

impl WalletMaterial {
    pub fn import(value: &str) -> Result<Self, String> {
        if value.split_whitespace().count() > 1 {
            return Self::import_seed_phrase(value);
        }
        Self::import_private_key(value)
    }

    fn import_seed_phrase(value: &str) -> Result<Self, String> {
        let phrase = Zeroizing::new(value.split_whitespace().collect::<Vec<_>>().join(" "));
        if phrase.split_whitespace().count() != 12 {
            return Err("Enter exactly 12 recovery words.".to_owned());
        }
        let mnemonic = Mnemonic::from_phrase(phrase.as_str(), Language::English)
            .map_err(|_| "Enter a valid English BIP-39 recovery phrase.".to_owned())?;
        let seed = Zeroizing::new(Seed::new(&mnemonic, "").as_bytes().to_vec());
        let derived = ExtendedPrivKey::derive(seed.as_slice(), CONSUMER_DERIVATION_PATH)
            .map_err(|_| "The MASQ consumer wallet could not be derived.".to_owned())?;
        Self::from_secret(Zeroizing::new(derived.secret().to_vec()))
    }

    fn import_private_key(value: &str) -> Result<Self, String> {
        let value = value.strip_prefix("0x").unwrap_or(value);
        if value.len() != 64 {
            return Err("A private key must be 32 bytes long.".to_owned());
        }
        let mut secret = Zeroizing::new(vec![0u8; 32]);
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            secret[index] = decode_pair(pair)?;
        }

        Self::from_secret(secret)
    }

    fn from_secret(secret: Zeroizing<Vec<u8>>) -> Result<Self, String> {
        let parsed = SecretKey::parse_slice(secret.as_slice())
            .map_err(|_| "The private key is outside the secp256k1 range.".to_owned())?;
        let public = PublicKey::from_secret_key(&parsed).serialize();
        let mut hash = [0u8; 32];
        let mut keccak = Keccak::v256();
        keccak.update(&public[1..]);
        keccak.finalize(&mut hash);
        let address = format!("0x{}", encode_hex(&hash[12..]));

        Ok(Self { secret, address })
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    #[cfg(feature = "node-engine")]
    pub fn private_key_hex(&self) -> Zeroizing<String> {
        Zeroizing::new(encode_hex(self.secret.as_slice()))
    }

    #[cfg(feature = "node-engine")]
    pub fn private_key_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.secret.to_vec())
    }

    #[cfg(test)]
    pub fn secret_bytes(&self) -> &[u8] {
        self.secret.as_slice()
    }
}

fn decode_pair(pair: &[u8]) -> Result<u8, String> {
    Ok((decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?)
}

fn decode_nibble(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("A private key may only contain hexadecimal characters.".to_owned()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_the_expected_ethereum_address() {
        let wallet = WalletMaterial::import(
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        assert_eq!(
            wallet.address(),
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf"
        );
        assert_eq!(wallet.secret_bytes().len(), 32);
    }

    #[test]
    fn rejects_zero_and_non_hex_keys() {
        assert!(WalletMaterial::import(&"0".repeat(64)).is_err());
        assert!(WalletMaterial::import(&"z".repeat(64)).is_err());
    }

    #[test]
    fn derives_the_masq_consumer_wallet_from_twelve_words() {
        let wallet =
            WalletMaterial::import("test test test test test test test test test test test junk")
                .unwrap();
        assert_eq!(
            wallet.address(),
            "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
        );
    }

    #[test]
    fn rejects_an_invalid_recovery_phrase() {
        assert!(WalletMaterial::import(&vec!["notaword"; 12].join(" ")).is_err());
    }
}
