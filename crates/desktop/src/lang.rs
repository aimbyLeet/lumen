//! The language of the apps' windows (web/client/src/i18n.ts): the core has no
//! words, it only keeps the user's choice in the app's settings.

/// What the interfaces speak: English first, the default.
pub const LANGS: [&str; 2] = ["en", "ru"];

/// A language the interfaces know, or the code `err_lang`.
pub fn check(lang: &str) -> Result<(), String> {
    if LANGS.contains(&lang) {
        Ok(())
    } else {
        Err("err_lang".into())
    }
}

/// Until the user chooses one with the globe: the installer's language
/// (`SOFTWARE\Lumen\Language`, deploy/installer/lumen.iss: in HKCU for the
/// client, installed for the user; in HKLM for the host), else English.
pub fn default() -> String {
    pick(installer())
}

/// The installer's choice if it is a language the interfaces know.
fn pick(installer: Option<String>) -> String {
    installer
        .filter(|l| check(l).is_ok())
        .unwrap_or_else(|| LANGS[0].into())
}

#[cfg(any(windows, lumen_check))]
fn installer() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use windows::core::w;
    read_sz(HKEY_CURRENT_USER, w!(r"SOFTWARE\Lumen"), w!("Language"))
        .or_else(|| read_sz(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\Lumen"), w!("Language")))
}

/// A short REG_SZ value (a language code); None: no such value, or a longer one.
#[cfg(any(windows, lumen_check))]
fn read_sz(
    root: windows::Win32::System::Registry::HKEY,
    key: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Option<String> {
    use windows::Win32::System::Registry::{RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = [0u16; 16];
    let mut len = std::mem::size_of_val(&buf) as u32;
    // SAFETY: the buffer and its size in bytes are ours; Windows writes at most `len` bytes, ending with a zero.
    let r = unsafe {
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if r.is_err() {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..chars]))
}

#[cfg(not(any(windows, lumen_check)))]
fn installer() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_known_languages() {
        assert!(check("en").is_ok());
        assert!(check("ru").is_ok());
        assert_eq!(check("de"), Err("err_lang".into()));
        assert_eq!(check(""), Err("err_lang".into()));
    }

    #[test]
    fn the_installers_language_first() {
        assert_eq!(pick(None), "en");
        assert_eq!(pick(Some("ru".into())), "ru");
        assert_eq!(pick(Some("en".into())), "en");
        // Something else written there: English.
        assert_eq!(pick(Some("de".into())), "en");
        assert_eq!(pick(Some(String::new())), "en");
    }

    #[cfg(windows)]
    #[test]
    fn a_value_written_like_the_installers_is_read() {
        use windows::Win32::System::Registry::HKEY_CURRENT_USER;
        use windows::core::w;
        // HKCU: HKLM needs an administrator. A key of this test's own, removed after.
        let key = r"HKCU\Software\LumenLangTest";
        let reg = |args: &[&str]| {
            std::process::Command::new("reg.exe")
                .args(args)
                .output()
                .unwrap()
        };
        assert!(
            reg(&["add", key, "/v", "Language", "/d", "ru", "/f"])
                .status
                .success()
        );
        let read = read_sz(
            HKEY_CURRENT_USER,
            w!(r"Software\LumenLangTest"),
            w!("Language"),
        );
        let missing = read_sz(
            HKEY_CURRENT_USER,
            w!(r"Software\LumenLangTest"),
            w!("Other"),
        );
        reg(&["delete", key, "/f"]);
        assert_eq!(read.as_deref(), Some("ru"));
        assert_eq!(missing, None);
    }

    #[cfg(windows)]
    #[test]
    fn reading_the_registry_does_not_fail() {
        // Here it may be there or not, by what was installed; whatever it holds, the result is a language.
        assert!(check(&default()).is_ok());
    }
}
