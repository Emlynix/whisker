//! Release signatures: Linux updates open the release page, nothing is
//! installed in place, so nothing is verified here yet.

/// Always false: no in-place install on Linux.
pub fn verify_p256(_key: &[u8; 64], _digest: &[u8; 32], _sig: &[u8; 64]) -> bool {
    false
}
