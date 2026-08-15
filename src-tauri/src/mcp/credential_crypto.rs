//! At-rest encryption for the MCP credential store.
//!
//! On Windows the store file is encrypted with DPAPI
//! (`CryptProtectData`) — the ciphertext is tied to the current Windows
//! user account, so a copied/backed-up file leaks nothing to another
//! account or machine. On other platforms (where no OS keychain is wired
//! yet) encryption is unavailable and the caller falls back to plaintext
//! with a warning.

/// Encrypt a plaintext blob, or `None` when at-rest encryption is not
/// available on this platform.
pub fn encrypt(plain: &[u8]) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        encrypt_windows(plain)
    }
    #[cfg(not(windows))]
    {
        let _ = plain;
        None
    }
}

/// Decrypt a blob produced by [`encrypt`]. Returns `None` on any failure
/// (wrong platform, corrupted data, different user account).
pub fn decrypt(cipher: &[u8]) -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        decrypt_windows(cipher)
    }
    #[cfg(not(windows))]
    {
        let _ = cipher;
        None
    }
}

#[cfg(windows)]
fn encrypt_windows(plain: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HLOCAL;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plain.len().try_into().ok()?,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok.is_err() {
            return None;
        }
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(
            output.pbData as *mut core::ffi::c_void,
        )));
        Some(out)
    }
}

#[cfg(windows)]
fn decrypt_windows(cipher: &[u8]) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HLOCAL;
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: cipher.len().try_into().ok()?,
            pbData: cipher.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let ok = CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok.is_err() {
            return None;
        }
        let out = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(
            output.pbData as *mut core::ffi::c_void,
        )));
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_roundtrip_when_available() {
        let secret = b"access_token_12345";
        match encrypt(secret) {
            Some(cipher) => {
                // Ciphertext must not contain the plaintext.
                assert!(!cipher.windows(secret.len()).any(|w| w == secret));
                let plain = decrypt(&cipher).expect("decrypt must succeed");
                assert_eq!(plain, secret);
            }
            None => {
                // Non-Windows: encryption unavailable — nothing to assert.
                assert!(decrypt(b"x").is_none());
            }
        }
    }

    #[test]
    fn decrypt_garbage_returns_none_when_supported() {
        if encrypt(b"x").is_some() {
            assert!(decrypt(b"not-dpapi-data").is_none());
        }
    }
}
