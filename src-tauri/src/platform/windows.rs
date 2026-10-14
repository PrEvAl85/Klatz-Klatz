//! Windows backend built on the classic Win32 input-language API.
//!
//! The handful of Win32 entry points this file needs are declared directly
//! instead of pulling in the `windows` crate: the crate renames its feature
//! flags between releases, and six `extern` declarations are cheaper to own
//! than a dependency that keeps breaking the build.

use super::Result;
use crate::layouts::LayoutInfo;

/// `WM_INPUTLANGCHANGEREQUEST`, sent to a window to change its input language.
const WM_INPUTLANGCHANGEREQUEST: u32 = 0x0050;

/// `LOCALE_SNAME`, asks `GetLocaleInfoW` for the short ISO name (`en-US`).
const LOCALE_SNAME: u32 = 0x0000_0002;

/// `LOCALE_NAME_MAX_LENGTH` from winnls.h; the API needs a caller buffer.
const LOCALE_NAME_MAX_LENGTH: usize = 85;

// The aliases mirror the Win32 ABI names on purpose, so the FFI block below
// reads like the header it replaces.
#[allow(clippy::upper_case_acronyms)]
mod ffi {
    pub type HWND = *mut core::ffi::c_void;
    pub type HKL = isize;
    pub type WPARAM = usize;
    pub type LPARAM = isize;
}

use ffi::{HKL, HWND, LPARAM, WPARAM};

#[link(name = "user32")]
unsafe extern "system" {
    /// Fills `lp_buff` with the loaded layout handles, returning the count.
    fn GetKeyboardLayoutList(n_buff: i32, lp_buff: *mut HKL) -> i32;
    /// Returns the layout of a thread; the layout is a *thread* property.
    fn GetKeyboardLayout(idd_thread: u32) -> HKL;
    fn GetForegroundWindow() -> HWND;
    fn GetWindowThreadProcessId(hwnd: HWND, lpdw_process_id: *mut u32) -> u32;
    fn PostMessageW(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    /// Writes the ISO name of a locale (`en-US`, `ru-RU`) into the buffer and
    /// returns its length, excluding the terminating null.
    ///
    /// `LCIDToLocaleName` would also do, but it reports failure for LCIDs the
    /// machine does not have, which is common for a HKL whose locale half has
    /// no matching system locale. `LOCALE_SNAME` only needs the language id.
    fn GetLocaleInfoW(
        locale: u32,
        locale_type: u32,
        lp_locale_info: *mut u16,
        cch_locale_info: i32,
    ) -> i32;
}

/// A keyboard layout handle (`HKL`) packs the locale id into the low word and
/// the keyboard layout id into the high word.
///
/// Both halves matter: `0x04090419` and `0x04190419` share the `en-US` locale
/// but are different layouts (US vs. Dvorak). Using the locale alone would make
/// the two indistinguishable, so the whole 32-bit value becomes our id.
fn hkl_to_id(hkl: HKL) -> String {
    format!("{:08x}", hkl as u32)
}

fn name_for(hkl: HKL) -> String {
    let locale = (hkl as u32) & 0xFFFF;
    if locale == 0 {
        return "Unknown layout".to_string();
    }

    let mut buf = [0u16; LOCALE_NAME_MAX_LENGTH];
    let written =
        unsafe { GetLocaleInfoW(locale, LOCALE_SNAME, buf.as_mut_ptr(), buf.len() as i32) };

    if written <= 0 {
        return format!("Layout 0x{locale:04x}");
    }

    String::from_utf16_lossy(&buf[..written as usize])
}

pub fn supported() -> (bool, Option<String>) {
    (true, None)
}

/// The language a layout belongs to, as a two-letter tag, read from the locale
/// half of the layout handle.
///
/// Automatic switching needs to know which word list applies to a layout, and
/// the answer is already written into the layout id: the low word of an `HKL`
/// is a `LANGID`, whose primary part identifies the language. Guessing from the
/// keyboard alone would be a guess.
pub fn language_of(id: &str) -> Option<String> {
    let hkl = u32::from_str_radix(id, 16).ok()?;
    // The low word of an `HKL` is the locale and the high word is the keyboard
    // layout; `04090419` is the Russian locale on a US keyboard, which is why
    // the two halves both have to be read to tell layouts apart.
    let locale = hkl & 0xFFFF;
    let primary = locale & 0x03ff;

    LANGID_TO_TAG
        .iter()
        .find(|(langid, _)| *langid == primary)
        .map(|(_, tag)| (*tag).to_string())
}

/// Primary `LANGID` values for the languages that ship a word list. Only the
/// primary part matters: `de-AT` and `de-CH` are German for this purpose.
const LANGID_TO_TAG: &[(u32, &str)] = &[
    (0x009, "en"),
    (0x019, "ru"),
    (0x007, "de"),
    (0x00c, "fr"),
    (0x00a, "es"),
    (0x010, "it"),
    (0x016, "pt"),
    (0x015, "pl"),
    (0x005, "cs"),
    (0x01b, "sk"),
    (0x022, "uk"),
    (0x002, "bg"),
    // Croatian and Serbian share the primary language id 0x1a; one row can
    // only win, and the two are interchangeable for language detection.
    (0x01a, "sr"),
    (0x024, "sl"),
    (0x01f, "tr"),
    (0x018, "ro"),
    (0x00e, "hu"),
    (0x01d, "sv"),
    (0x006, "da"),
    (0x014, "no"),
    (0x00b, "fi"),
    (0x00f, "is"),
    (0x013, "nl"),
    (0x008, "el"),
    (0x003, "ca"),
    (0x021, "id"),
    (0x02a, "vi"),
    (0x056, "gl"),
];

pub fn available() -> Result<Vec<LayoutInfo>> {
    unsafe {
        let count = GetKeyboardLayoutList(0, core::ptr::null_mut());
        if count <= 0 {
            return Err("Не удалось получить список раскладок (GetKeyboardLayoutList)".into());
        }

        let mut buf = vec![0 as HKL; count as usize];
        let filled = GetKeyboardLayoutList(count, buf.as_mut_ptr());
        buf.truncate(filled.max(0) as usize);

        Ok(buf
            .into_iter()
            .filter(|hkl| *hkl != 0)
            .map(|hkl| LayoutInfo {
                id: hkl_to_id(hkl),
                name: name_for(hkl),
            })
            .collect())
    }
}

pub fn current() -> Result<Option<String>> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return Ok(None);
        }

        // The active layout belongs to the foreground thread, not to ours.
        let thread_id = GetWindowThreadProcessId(hwnd, core::ptr::null_mut());
        Ok(Some(hkl_to_id(GetKeyboardLayout(thread_id))))
    }
}

pub fn switch_to(id: &str) -> Result<()> {
    let hkl = i64::from_str_radix(id.trim_start_matches("0x"), 16)
        .map_err(|_| format!("Некорректный идентификатор раскладки: {id}"))?;

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return Err("Нет активного окна для переключения раскладки".into());
        }

        // Posting to the foreground window is what makes the change visible to
        // the focused application. `ActivateKeyboardLayout` would only affect
        // the layout list of our own thread, which is useless from a tray app.
        PostMessageW(hwnd, WM_INPUTLANGCHANGEREQUEST, 0, hkl as LPARAM);
    }

    Ok(())
}
