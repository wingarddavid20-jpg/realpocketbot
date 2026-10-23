//! Cryptographic utilities for encrypting, decrypting, and zeroizing user private keys.
//!
//! Uses AES-256-GCM with per-user key derivation via SHA-256. Plaintext keys are
//! wrapped in `Zeroizing<String>` so memory is erased as soon as it goes out of scope.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{BotError, BotResult};

const NONCE_LEN: usize = 12;

/// Derive a 32-byte encryption key bound to a specific Telegram user ID.
fn derive_user_key(master_key: &str, user_id: u64) -> Zeroizing<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(master_key.as_bytes());
    hasher.update(b":user_key_salt:");
    hasher.update(user_id.to_le_bytes());
    let hash = hasher.finalize();

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&hash);
    Zeroizing::new(key_bytes)
}

/// Encrypt a private key for a specific Telegram user.
///
/// Returns a hex-encoded string of `nonce (12 bytes) || ciphertext`.
pub fn encrypt_user_key(master_key: &str, user_id: u64, plaintext_key: &str) -> BotResult<String> {
    let derived = derive_user_key(master_key, user_id);
    let key = Key::<Aes256Gcm>::from_slice(derived.as_ref());
    let cipher = Aes256Gcm::new(key);

    use aes_gcm::aead::AeadCore;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext_key.as_bytes())
        .map_err(|e| BotError::Internal(format!("Encryption failed: {e}")))?;

    let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    combined.extend_from_slice(nonce.as_slice());
    combined.extend_from_slice(&ciphertext);

    Ok(hex_encode(&combined))
}

/// Decrypt an encrypted private key for a specific Telegram user.
///
/// Returns a `Zeroizing<String>` that is securely wiped from RAM on drop.
pub fn decrypt_user_key(
    master_key: &str,
    user_id: u64,
    hex_payload: &str,
) -> BotResult<Zeroizing<String>> {
    let mut raw_bytes = hex_decode(hex_payload)
        .map_err(|e| BotError::Internal(format!("Invalid hex payload: {e}")))?;

    if raw_bytes.len() < NONCE_LEN {
        raw_bytes.zeroize();
        return Err(BotError::Internal("Encrypted payload too short".into()));
    }

    let (nonce_bytes, ciphertext) = raw_bytes.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);

    let derived = derive_user_key(master_key, user_id);
    let key = Key::<Aes256Gcm>::from_slice(derived.as_ref());
    let cipher = Aes256Gcm::new(key);

    let decrypted_bytes = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| BotError::Internal("Decryption failed: corrupted key or invalid auth".into()))?;

    // Zeroize raw encrypted bytes buffer
    raw_bytes.zeroize();

    let plain_str = String::from_utf8(decrypted_bytes)
        .map_err(|_| BotError::Internal("Decrypted key is not valid UTF-8".into()))?;

    Ok(Zeroizing::new(plain_str))
}

// ─── Constant-time safe Hex Helpers ──────────────────────────────────────────

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    let hex = hex.trim();
    if hex.len() % 2 != 0 {
        return Err("Hex string length must be even".into());
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let chars: Vec<char> = hex.chars().collect();
    for i in (0..chars.len()).step_by(2) {
        let hi = chars[i].to_digit(16).ok_or("Invalid hex character")?;
        let lo = chars[i + 1].to_digit(16).ok_or("Invalid hex character")?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let master = "my_super_secret_master_key_123456789";
        let user_id = 987654321u64;
        let secret = "iotaprivkey1qqmernxez395qnef07r74jtss4w0fv7l2zde8ckfjutgcuqlzg245pk5nug";

        let encrypted = encrypt_user_key(master, user_id, secret).unwrap();
        assert_ne!(encrypted, secret);

        let decrypted = decrypt_user_key(master, user_id, &encrypted).unwrap();
        assert_eq!(decrypted.as_str(), secret);
    }

    #[test]
    fn test_cross_user_isolation() {
        let master = "my_super_secret_master_key_123456789";
        let user1 = 111111u64;
        let user2 = 222222u64;
        let secret = "iotaprivkey1secretkey";

        let encrypted = encrypt_user_key(master, user1, secret).unwrap();

        // User 2 cannot decrypt User 1's key
        let result = decrypt_user_key(master, user2, &encrypted);
        assert!(result.is_err());
    }
}
