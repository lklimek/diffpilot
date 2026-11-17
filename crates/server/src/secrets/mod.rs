//! Encryption of stored tokens with AES-256-GCM under `SR_SECRET_KEY`, random tokens and SHA-256 hashes.
//!
//! Stored layout: 12-byte random nonce followed by ciphertext and tag. The associated data binds every
//! ciphertext to its table, column and row id, so a value copied to another row or column fails to decrypt.
//!
//! Key: `SR_SECRET_KEY` (ADR-001), read once by `config`, held in a `SecretBox`. No rotation in v1: a replaced
//! key makes stored values undecryptable; a user token pair then ends its sign-in on next use (the user signs in
//! again) and an OAuth login in progress fails. Random tokens (session ids, CSRF, OAuth state, PKCE verifier) come
//! from the OS generator; session ids are stored only as SHA-256; secrets are compared with [`secrets_equal`].

use std::fmt;

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use secrecy::{ExposeSecret, SecretBox, SecretString};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const NONCE_LEN: usize = 12;
const TOKEN_BYTES: usize = 32;

/// Encrypts and decrypts stored secrets; build one from the configured key where needed.
pub struct TokenCipher(Aes256Gcm);

impl fmt::Debug for TokenCipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenCipher(..)")
    }
}

/// Where a ciphertext is stored; used as associated data.
#[derive(Clone, Copy, Debug)]
pub struct Aad<'a> {
    /// Table name.
    pub table: &'static str,
    /// Column name.
    pub column: &'static str,
    /// Primary key of the row.
    pub row_id: &'a str,
}

impl Aad<'_> {
    fn bytes(&self) -> Vec<u8> {
        // Table and column are fixed identifiers without NUL; the row id comes last.
        format!(
            "smart-review\0{}\0{}\0{}",
            self.table, self.column, self.row_id
        )
        .into_bytes()
    }
}

/// A secret could not be produced, encrypted or decrypted.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// The operating system's random number generator failed.
    #[error("the system random number generator failed")]
    Random(#[source] getrandom::Error),
    /// Encryption failed.
    #[error("a secret could not be encrypted")]
    Encrypt,
    /// The stored value is malformed, was altered or moved, or `SR_SECRET_KEY` changed.
    #[error("a stored secret could not be decrypted; it was altered or SR_SECRET_KEY changed")]
    Decrypt,
}

impl TokenCipher {
    /// A cipher over the 32-byte key.
    pub fn new(key: &SecretBox<[u8; 32]>) -> Self {
        Self(Aes256Gcm::new(key.expose_secret().into()))
    }

    /// Encrypts `plaintext` for the location `aad` with a fresh random nonce.
    pub fn encrypt(&self, aad: Aad<'_>, plaintext: &str) -> Result<Vec<u8>, SecretError> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(SecretError::Random)?;
        let aad = aad.bytes();
        let sealed = self
            .0
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| SecretError::Encrypt)?;
        let mut stored = Vec::with_capacity(NONCE_LEN + sealed.len());
        stored.extend_from_slice(&nonce);
        stored.extend_from_slice(&sealed);
        Ok(stored)
    }

    /// Decrypts a value stored at `aad`.
    pub fn decrypt(&self, aad: Aad<'_>, stored: &[u8]) -> Result<SecretString, SecretError> {
        let (nonce, sealed) = stored
            .split_first_chunk::<NONCE_LEN>()
            .ok_or(SecretError::Decrypt)?;
        let aad = aad.bytes();
        let plaintext = self
            .0
            .decrypt(
                nonce.into(),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| SecretError::Decrypt)?;
        String::from_utf8(plaintext)
            .map(SecretString::from)
            .map_err(|_| SecretError::Decrypt)
    }
}

/// A fresh random token: 32 bytes from the OS generator, base64url without padding (43 characters).
pub fn random_token() -> Result<String, SecretError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(SecretError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// SHA-256 of `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Compares two secrets in constant time (over their SHA-256 digests, so lengths do not leak either).
pub fn secrets_equal(a: &str, b: &str) -> bool {
    sha256(a.as_bytes()).ct_eq(&sha256(b.as_bytes())).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher(byte: u8) -> TokenCipher {
        TokenCipher::new(&SecretBox::new(Box::new([byte; 32])))
    }

    const AAD: Aad<'static> = Aad {
        table: "user_tokens",
        column: "access_ct",
        row_id: "u-1",
    };

    #[test]
    fn round_trip_with_random_nonce() {
        let cipher = cipher(7);
        let first = cipher.encrypt(AAD, "ghu_secret").expect("encrypt");
        let second = cipher.encrypt(AAD, "ghu_secret").expect("encrypt");
        assert_ne!(first, second, "nonce must be fresh per encryption");
        assert!(!first.windows(10).any(|w| w == b"ghu_secret"));
        assert_eq!(
            cipher
                .decrypt(AAD, &first)
                .expect("decrypt")
                .expose_secret(),
            "ghu_secret"
        );
    }

    #[test]
    fn tampered_ciphertext_or_wrong_aad_rejected() {
        let cipher = cipher(7);
        let stored = cipher.encrypt(AAD, "ghu_secret").expect("encrypt");
        for index in [0, NONCE_LEN, stored.len() - 1] {
            let mut tampered = stored.clone();
            tampered[index] ^= 1;
            assert!(
                matches!(cipher.decrypt(AAD, &tampered), Err(SecretError::Decrypt)),
                "byte {index}"
            );
        }
        assert!(matches!(
            cipher.decrypt(AAD, &stored[..NONCE_LEN - 1]),
            Err(SecretError::Decrypt)
        ));
        for wrong in [
            Aad {
                row_id: "u-2",
                ..AAD
            },
            Aad {
                column: "refresh_ct",
                ..AAD
            },
            Aad {
                table: "oauth_logins",
                ..AAD
            },
        ] {
            assert!(
                matches!(cipher.decrypt(wrong, &stored), Err(SecretError::Decrypt)),
                "{wrong:?}"
            );
        }
        assert!(matches!(
            self::cipher(8).decrypt(AAD, &stored),
            Err(SecretError::Decrypt)
        ));
    }

    #[test]
    fn random_tokens_are_unique_url_safe_and_long_enough_for_pkce() {
        let a = random_token().expect("token");
        let b = random_token().expect("token");
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
    }

    #[test]
    fn secrets_compare_by_value() {
        assert!(secrets_equal("abc", "abc"));
        assert!(!secrets_equal("abc", "abd"));
        assert!(!secrets_equal("abc", ""));
    }

    #[test]
    fn debug_does_not_show_key_material() {
        assert_eq!(format!("{:?}", cipher(7)), "TokenCipher(..)");
    }
}
