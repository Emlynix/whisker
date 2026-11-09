//! ECDSA P-256 signature check through CNG (bcrypt.dll), for releases.

use std::ptr::null;
use windows_sys::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_ECCKEY_BLOB, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM, BCRYPT_ECDSA_PUBLIC_P256_MAGIC, BCRYPT_KEY_HANDLE,
    BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptImportKeyPair, BCryptOpenAlgorithmProvider, BCryptVerifySignature,
};

const COORD_LEN: usize = 32;
const STATUS_SUCCESS: i32 = 0;

/// True when `sig` (r || s) signs `digest` (SHA-256) for `key` (x || y).
pub fn verify_p256(key: &[u8; 64], digest: &[u8; 32], sig: &[u8; 64]) -> bool {
    // Key blob: header, then x and y.
    let header = BCRYPT_ECCKEY_BLOB { dwMagic: BCRYPT_ECDSA_PUBLIC_P256_MAGIC, cbKey: COORD_LEN as u32 };
    let mut blob = Vec::with_capacity(size_of::<BCRYPT_ECCKEY_BLOB>() + key.len());
    blob.extend_from_slice(&header.dwMagic.to_le_bytes());
    blob.extend_from_slice(&header.cbKey.to_le_bytes());
    blob.extend_from_slice(key);

    let mut alg: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
    if unsafe { BCryptOpenAlgorithmProvider(&mut alg, BCRYPT_ECDSA_P256_ALGORITHM, null(), 0) } != STATUS_SUCCESS {
        return false;
    }
    let mut hkey: BCRYPT_KEY_HANDLE = std::ptr::null_mut();
    let imported = unsafe { BCryptImportKeyPair(alg, std::ptr::null_mut(), BCRYPT_ECCPUBLIC_BLOB, &mut hkey, blob.as_ptr(), blob.len() as u32, 0) };
    let ok = imported == STATUS_SUCCESS
        && unsafe { BCryptVerifySignature(hkey, null(), digest.as_ptr(), digest.len() as u32, sig.as_ptr(), sig.len() as u32, 0) } == STATUS_SUCCESS;

    unsafe {
        if !hkey.is_null() {
            BCryptDestroyKey(hkey);
        }
        BCryptCloseAlgorithmProvider(alg, 0);
    }
    ok
}
