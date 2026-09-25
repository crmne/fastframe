//! Publisher authorization, separate from GitHub's build attestations.
//!
//! native-packages' `sign-release` action signs the exact bytes of
//! `checksums.txt` with Ed25519 (not Ed25519ph) and publishes the raw
//! 64-byte signature as `checksums.txt.sig`.

use anyhow::{Context, Result, ensure};
use ring::signature::{ED25519, UnparsedPublicKey};

/// The 32-byte key from its 64 hexadecimal digits.
pub(crate) fn decode_key(hex: &str) -> Result<[u8; 32]> {
    let hex = hex.trim();
    ensure!(
        hex.len() == 64 && hex.is_ascii(),
        "Invalid embedded update public key"
    );
    let mut key = [0; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16)
            .context("Invalid embedded update public key")?;
    }
    Ok(key)
}

pub(crate) fn verify(manifest: &[u8], signature: &[u8], key: &[u8]) -> Result<()> {
    ensure!(
        key.len() == 32 && signature.len() == 64,
        "Invalid update publisher signature"
    );
    UnparsedPublicKey::new(&ED25519, key)
        .verify(manifest, signature)
        .map_err(|_| anyhow::anyhow!("The update publisher signature could not be verified"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixture_key;
    use ring::signature::KeyPair;

    fn unhex(encoded: &str) -> Vec<u8> {
        (0..encoded.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&encoded[at..at + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn a_published_zapfast_release_verifies_against_its_embedded_key() {
        // checksums.txt and checksums.txt.sig from ZapFast v0.16.3 on GitHub,
        // and the key ZapFast 0.16.3 embeds.
        let manifest = include_bytes!("../tests/fixtures/zapfast-0.16.3-checksums.txt");
        let signature = include_bytes!("../tests/fixtures/zapfast-0.16.3-checksums.txt.sig");
        let key = decode_key(include_str!("../tests/fixtures/zapfast-public-key.hex")).unwrap();
        verify(manifest, signature, &key).unwrap();
        let mut changed = manifest.to_vec();
        changed[0] ^= 1;
        assert!(verify(&changed, signature, &key).is_err());
    }

    #[test]
    fn native_packages_signature_is_accepted_without_format_conversion() {
        // Shared with native-packages' Ruby/OpenSSL release signer test.
        // The disposable seed is [42; 32], never the publisher's real key.
        let manifest =
            b"f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d  app-v1.2.3.zip\n";
        let signature = unhex(
            "562fce6b4dc6e8d30a907f104a3b1c5fbf777c3b849ee23181de624c1398f6af3d1ee6b4a272f87c7100b6f58d34854c1d2285c854335f23edf1c0fa5a9e2a0f",
        );
        assert!(verify(manifest, &signature, fixture_key().public_key().as_ref()).is_ok());
        let production =
            decode_key(include_str!("../tests/fixtures/zapfast-public-key.hex")).unwrap();
        assert!(verify(manifest, &signature, &production).is_err());
    }

    #[test]
    fn publisher_verification_rejects_changed_manifests_keys_and_signatures() {
        let key = fixture_key();
        let manifest = b"fixture checksums\n";
        let signature = key.sign(manifest);
        let public = key.public_key().as_ref();
        assert!(verify(manifest, signature.as_ref(), public).is_ok());
        for bad in [b"fixture checksums".as_slice(), b"forged checksums\n", b""] {
            assert!(verify(bad, signature.as_ref(), public).is_err());
        }
        assert!(verify(manifest, &[], public).is_err());
        assert!(verify(manifest, &signature.as_ref()[..63], public).is_err());
        let mut altered = signature.as_ref().to_vec();
        altered[0] ^= 1;
        assert!(verify(manifest, &altered, public).is_err());
        let other = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[43; 32]).unwrap();
        assert!(verify(manifest, signature.as_ref(), other.public_key().as_ref()).is_err());
        assert!(verify(manifest, signature.as_ref(), &public[..31]).is_err());
    }

    #[test]
    fn keys_are_64_hexadecimal_digits() {
        assert_eq!(decode_key(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert_eq!(
            decode_key(&format!("{}\n", "00".repeat(32))).unwrap(),
            [0; 32]
        );
        for bad in [
            "",
            "ab",
            &"zz".repeat(32),
            &"ab".repeat(33),
            &"é".repeat(32),
        ] {
            assert!(decode_key(bad).is_err(), "{bad}");
        }
    }
}
