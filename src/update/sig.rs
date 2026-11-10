//! Release signatures: ECDSA P-256 (DER) over a statement naming the
//! platform, version and file hash, so an old signed file can't be passed
//! off as a newer version:
//!
//! ```text
//! printf 'whisker|windows-x86_64|0.9.1|<sha256 hex>' |
//!     openssl dgst -sha256 -sign whisker-release-private.pem -out f.sig
//! ```
//!
//! latest.json carries it as base64, a GitHub release as the `<asset>.sig`
//! file. Only the private key's owner can make one, so a swapped file or
//! version on the web or on GitHub is refused.

use crate::platform;

/// Public half of the release key (x || y, P-256). The private key is kept
/// outside the repository.
const RELEASE_KEY: [u8; 64] = [
    0x88, 0xee, 0x18, 0x58, 0xb1, 0x67, 0x48, 0xcd, 0x58, 0xe0, 0xd8, 0x38, 0x06, 0xc2, 0xe5, 0x75,
    0x52, 0x2a, 0xbc, 0x0b, 0x27, 0x76, 0x68, 0x0e, 0x71, 0x7b, 0x40, 0x3d, 0x17, 0xa4, 0x68, 0x27,
    0x77, 0xb2, 0x91, 0xf7, 0x3e, 0x7a, 0x43, 0x5b, 0x6a, 0xeb, 0xa2, 0x9c, 0xc2, 0xf3, 0x39, 0x79,
    0x20, 0xc4, 0xbd, 0xb6, 0x80, 0xf7, 0x52, 0xc8, 0x64, 0x87, 0x9a, 0xcf, 0xbb, 0x55, 0x67, 0x2b,
];

/// A DER P-256 signature is at most 72 bytes; more is not a signature.
pub(super) const SIG_MAX: usize = 128;
const INT_LEN: usize = 32;
const DER_SEQUENCE: u8 = 0x30;
const DER_INTEGER: u8 = 0x02;

/// What a release signature covers.
pub(super) fn statement(target: &str, version: &str, sha: &[u8; 32]) -> String {
    let hex: String = sha.iter().map(|b| format!("{b:02x}")).collect();
    format!("whisker|{target}|{version}|{hex}")
}

/// True when `der` is the release key's signature of `statement`.
pub(super) fn verify(statement: &str, der: &[u8]) -> bool {
    verify_with(&RELEASE_KEY, &super::sha256::digest(statement.as_bytes()), der)
}

fn verify_with(key: &[u8; 64], digest: &[u8; 32], der: &[u8]) -> bool {
    let Some(raw) = der_to_raw(der) else {
        return false;
    };
    platform::sign::verify_p256(key, digest, &raw)
}

/// DER `SEQUENCE { INTEGER r, INTEGER s }` -> r || s, each 32 bytes.
fn der_to_raw(der: &[u8]) -> Option<[u8; 64]> {
    let (&tag, rest) = der.split_first()?;
    let (&len, body) = rest.split_first()?;
    if tag != DER_SEQUENCE || len as usize != body.len() || der.len() > SIG_MAX {
        return None;
    }
    let (r, body) = der_int(body)?;
    let (s, body) = der_int(body)?;
    if !body.is_empty() {
        return None;
    }
    let mut out = [0u8; 64];
    out[INT_LEN - r.len()..INT_LEN].copy_from_slice(r);
    out[2 * INT_LEN - s.len()..].copy_from_slice(s);
    Some(out)
}

/// One positive INTEGER, its leading zero dropped; rest of the input.
fn der_int(b: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&tag, rest) = b.split_first()?;
    let (&len, rest) = rest.split_first()?;
    let len = len as usize;
    if tag != DER_INTEGER || len == 0 || len > rest.len() {
        return None;
    }
    let (int, rest) = rest.split_at(len);
    // A set top bit means negative: never a signature value.
    if int[0] & 0x80 != 0 {
        return None;
    }
    let int = match int {
        [0, tail @ ..] if !tail.is_empty() => tail,
        _ => int,
    };
    if int.len() > INT_LEN {
        return None;
    }
    Some((int, rest))
}

/// Standard base64 (with padding), whitespace ignored.
pub(super) fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    let s = s.trim_end_matches(|c: char| c == '=' || c.is_ascii_whitespace());
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test key and a signature of "whisker test payload" made with openssl.
    const TEST_KEY: [u8; 64] = [
        0x32, 0xa6, 0x24, 0x67, 0x94, 0x17, 0x5e, 0xdf, 0x0e, 0x25, 0x6f, 0xd2, 0x2a, 0x29, 0x03, 0x0c,
        0x03, 0xf1, 0x0c, 0x48, 0x5a, 0xd0, 0xb9, 0xd6, 0xa2, 0xf3, 0x06, 0x5d, 0xcf, 0x8a, 0xa9, 0x10,
        0x18, 0x6a, 0xd9, 0xba, 0x7f, 0x8d, 0xe0, 0xab, 0x2a, 0xd8, 0x72, 0xe5, 0x31, 0x5b, 0x48, 0xb2,
        0x50, 0x27, 0x26, 0x2d, 0xdd, 0x71, 0xf9, 0x63, 0x7a, 0xd9, 0x2f, 0xbb, 0x2e, 0x5e, 0x30, 0x21,
    ];
    const TEST_SIG: &str = "MEUCIGdZlc+Vw9U8CYV4/3hjjgjs5ZyDTpMtFGMWtYZ+XY5cAiEAsYeX9TzIO4PDYWbH7sDUlycvZpAXstm83VeFtMy+HjU=";
    const PAYLOAD: &[u8] = b"whisker test payload";
    /// The test key's signature of the payload's release statement (0.9.1).
    const TEST_STATEMENT_SIG: &str = "MEUCIQCQbb9aS1bSy6DJIpt6D+LS+jnnPswWQiyLItt0ie3RcgIgKEj6s/eqgUZnp6v4uqcXTH1F+vfpqamaL4c5VNO9Ohc=";

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVs\nbG8").unwrap(), b"hello");
        assert_eq!(base64("").unwrap(), b"");
        assert!(base64("a-b").is_none());
    }

    #[test]
    fn der_signature_becomes_raw() {
        let der = base64(TEST_SIG).unwrap();
        let raw = der_to_raw(&der).unwrap();
        // s had a leading zero byte (0x21 = 33 long): dropped.
        assert_eq!(raw[32], 0xb1);
        assert!(der_to_raw(&der[..der.len() - 1]).is_none());
        assert!(der_to_raw(&[]).is_none());
        assert!(der_to_raw(&[DER_SEQUENCE, 0]).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn signatures_verify_only_for_their_key_and_data() {
        let der = base64(TEST_SIG).unwrap();
        let digest = super::super::sha256::digest(PAYLOAD);
        assert!(verify_with(&TEST_KEY, &digest, &der));

        let other = super::super::sha256::digest(b"whisker test payloaD");
        assert!(!verify_with(&TEST_KEY, &other, &der));
        // Signed by the test key, not the release key.
        assert!(!verify(&statement("windows-x86_64", "0.9.1", &digest), &der));
        let mut bad = der.clone();
        bad[10] ^= 1;
        assert!(!verify_with(&TEST_KEY, &digest, &bad));
    }

    #[cfg(windows)]
    #[test]
    fn signature_binds_version_and_platform() {
        let der = base64(TEST_STATEMENT_SIG).unwrap();
        let sha = super::super::sha256::digest(PAYLOAD);
        let check = |target, version| verify_with(&TEST_KEY, &super::super::sha256::digest(statement(target, version, &sha).as_bytes()), &der);
        assert!(check("windows-x86_64", "0.9.1"));
        // The same file claimed as another version or platform: refused.
        assert!(!check("windows-x86_64", "99.0.0"));
        assert!(!check("linux-x86_64", "0.9.1"));
    }
}
