//! Secrets kept in the user's files (a host's token in `client-app.json`):
//! encrypted with DPAPI under the Windows account (CurrentUser scope).
//!
//! What it gives: the file alone is useless: copied to another computer or
//! account, read from a backup or by another user of this PC, it does not
//! open. What it does not: a program running under the same account can ask
//! Windows to decrypt it just as Lumen does (the entropy below is in the open
//! source): against that nothing kept on this computer helps.

use std::io;

use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::PCWSTR;

/// Ties the blobs to Lumen: another program's DPAPI blob does not open as ours.
const ENTROPY: &[u8] = b"Lumen secret v1";

/// How an encrypted string starts in a file; a string without it is plain
/// text (written before 0.6.0).
const PREFIX: &str = "dpapi:";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Takes the output of DPAPI and frees it.
fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if out.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: DPAPI allocated `cbData` bytes at `pbData` with LocalAlloc.
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    // SAFETY: freed once, as DPAPI asks.
    unsafe { LocalFree(Some(HLOCAL(out.pbData.cast()))) };
    v
}

/// Encrypts `data` for this Windows account.
fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point to live slices; `out` is ours to free.
    unsafe {
        CryptProtectData(
            &blob(data),
            PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    }
    .map_err(io::Error::other)?;
    Ok(take(out))
}

/// Decrypts what [`protect`] gave under this Windows account.
fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    unsafe {
        CryptUnprotectData(
            &blob(data),
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    }
    .map_err(io::Error::other)?;
    Ok(take(out))
}

/// `dpapi:<hex>` for a file.
pub fn seal(text: &str) -> io::Result<String> {
    use std::fmt::Write;
    let sealed = protect(text.as_bytes())?;
    let mut s = String::with_capacity(PREFIX.len() + sealed.len() * 2);
    s.push_str(PREFIX);
    for b in sealed {
        let _ = write!(s, "{b:02x}");
    }
    Ok(s)
}

/// What [`seal`] wrote; a string without the prefix is taken as plain text.
pub fn open(stored: &str) -> io::Result<String> {
    let Some(hex) = stored.strip_prefix(PREFIX) else {
        return Ok(stored.to_string());
    };
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "not a sealed secret");
    if hex.len() % 2 != 0 || !hex.is_ascii() {
        return Err(bad());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| bad())?;
    String::from_utf8(unprotect(&bytes)?).map_err(|_| bad())
}

/// Whether the stored string is still plain text (a file to write again).
pub fn is_plain(stored: &str) -> bool {
    !stored.is_empty() && !stored.starts_with(PREFIX)
}

/// `#[serde(with = "desktop::secret::field")]`: a string field kept sealed.
/// One that does not open (the file came from another account or computer)
/// reads as empty, so the rest of the file still loads.
pub mod field {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
        if v.is_empty() {
            return s.serialize_str("");
        }
        let sealed = super::seal(v).map_err(serde::ser::Error::custom)?;
        s.serialize_str(&sealed)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        let stored = String::deserialize(d)?;
        Ok(super::open(&stored).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_opens_and_hides_the_text() {
        let token = "AAAA-BBBB-CCCC-DDDD-EEEE-FFFF";
        let sealed = seal(token).unwrap();
        assert!(sealed.starts_with(PREFIX));
        assert!(!sealed.contains("AAAA"));
        assert!(!is_plain(&sealed));
        assert_eq!(open(&sealed).unwrap(), token);
        // Each seal is different (DPAPI salts it), both open.
        assert_ne!(seal(token).unwrap(), sealed);
    }

    #[test]
    fn plain_text_reads_as_it_is() {
        assert!(is_plain("AAAA-BBBB"));
        assert!(!is_plain(""));
        assert_eq!(open("AAAA-BBBB").unwrap(), "AAAA-BBBB");
    }

    #[test]
    fn broken_or_foreign_does_not_open() {
        assert!(open("dpapi:zz").is_err());
        assert!(open("dpapi:abc").is_err());
        let mut sealed = seal("x").unwrap();
        let last = sealed.pop().unwrap();
        sealed.push(if last == '0' { '1' } else { '0' });
        assert!(open(&sealed).is_err(), "a changed blob");
        // Another program's blob (no entropy) is not ours.
        let other = {
            let mut out = CRYPT_INTEGER_BLOB::default();
            unsafe {
                CryptProtectData(
                    &blob(b"x"),
                    PCWSTR::null(),
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out,
                )
            }
            .unwrap();
            take(out)
        };
        assert!(unprotect(&other).is_err());
    }
}
