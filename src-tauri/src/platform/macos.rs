//! macOS backend built on the Carbon Text Input Sources API.
//!
//! The relevant calls live in Carbon's HIToolbox framework, so they are
//! declared here instead of pulling in the whole Objective-C runtime.

#![cfg(target_os = "macos")]

use super::Result;
use crate::layouts::LayoutInfo;
use std::ffi::{CString, c_void};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFIndex = isize;
type OSStatus = i32;
type Boolean = u8;

/// `kCFStringEncodingUTF8`.
const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Every physical keyboard layout uses this prefix; input methods do not.
const KEYBOARD_LAYOUT_PREFIX: &str = "com.apple.keylayout";

#[link(name = "Carbon", kind = "framework")]
extern "C" {
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyLocalizedName: CFStringRef;

    fn TISCreateInputSourceList(
        filter: CFDictionaryRef,
        include_all_installed: Boolean,
    ) -> CFArrayRef;
    fn TISSelectInputSource(input_source: CFTypeRef) -> OSStatus;
    fn TISCopyCurrentKeyboardInputSource() -> CFTypeRef;
    /// Returns a `Get`-rule value: it must **not** be released by the caller.
    fn TISGetInputSourceProperty(input_source: CFTypeRef, property_key: CFStringRef) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, index: CFIndex) -> CFTypeRef;
    fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
    fn CFStringGetCString(
        the_string: CFStringRef,
        buffer: *mut u8,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> Boolean;
    fn CFStringCreateWithCString(alloc: CFTypeRef, c_str: *const i8, encoding: u32) -> CFStringRef;
    fn CFRelease(cf: CFTypeRef);
}

/// Copies a `CFStringRef` into an owned Rust `String`.
fn to_string(cf: CFTypeRef) -> Option<String> {
    if cf.is_null() {
        return None;
    }

    unsafe {
        // One extra byte for the null terminator that `CFStringGetCString` writes.
        let mut buf = vec![0u8; (CFStringGetLength(cf) as usize + 1) * std::mem::size_of::<char>()];

        if CFStringGetCString(
            cf,
            buf.as_mut_ptr(),
            buf.len() as CFIndex,
            CF_STRING_ENCODING_UTF8,
        ) == 0
        {
            return None;
        }

        let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
        buf.truncate(end);
        String::from_utf8(buf).ok()
    }
}

/// Calls `body` with the list of all installed input sources.
///
/// The array is a `Create`-rule object, so it is always released, including on
/// the error paths.
fn with_sources<T>(body: impl FnOnce(&[CFTypeRef]) -> T) -> Option<T> {
    unsafe {
        let list = TISCreateInputSourceList(std::ptr::null(), 1);
        if list.is_null() {
            return None;
        }

        let count = CFArrayGetCount(list).max(0) as usize;
        let mut items = Vec::with_capacity(count);
        for index in 0..count {
            let source = CFArrayGetValueAtIndex(list, index as CFIndex);
            if !source.is_null() {
                items.push(source);
            }
        }

        let result = body(&items);
        CFRelease(list);
        Some(result)
    }
}

fn id_of(source: CFTypeRef) -> Option<String> {
    unsafe { to_string(TISGetInputSourceProperty(source, kTISPropertyInputSourceID)) }
}

fn name_of(source: CFTypeRef, id: &str) -> String {
    let localized =
        unsafe { to_string(TISGetInputSourceProperty(source, kTISPropertyLocalizedName)) };
    // `com.apple.keylayout.US` -> `US` as a last resort.
    let fallback = id.strip_prefix(KEYBOARD_LAYOUT_PREFIX).unwrap_or(id);
    localized.unwrap_or_else(|| fallback.to_string())
}

pub fn supported() -> (bool, Option<String>) {
    (true, None)
}

pub fn available() -> Result<Vec<LayoutInfo>> {
    with_sources(|sources| {
        sources
            .iter()
            .filter_map(|source| {
                let id = id_of(*source)?;
                if !id.starts_with(KEYBOARD_LAYOUT_PREFIX) {
                    return None;
                }
                let name = name_of(*source, &id);
                Some(LayoutInfo { id, name })
            })
            .collect::<Vec<LayoutInfo>>()
    })
    .ok_or_else(|| "Не удалось получить список раскладок (TISCreateInputSourceList)".to_string())
}

pub fn current() -> Result<Option<String>> {
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();
        if source.is_null() {
            return Ok(None);
        }

        // This one is a `Copy`-rule reference, so it does need releasing.
        let id = id_of(source);
        CFRelease(source);
        Ok(id)
    }
}

pub fn switch_to(id: &str) -> Result<()> {
    if let Some(source) = with_sources(|sources| {
        sources
            .iter()
            .copied()
            .find(|source| id_of(*source).as_deref() == Some(id))
    }) {
        let status = unsafe { TISSelectInputSource(source) };
        return check_status(status, id);
    }

    // Not in the installed list: try selecting by identifier directly.
    let c_string =
        CString::new(id).map_err(|_| format!("Некорректный идентификатор раскладки: {id}"))?;
    unsafe {
        let source =
            CFStringCreateWithCString(std::ptr::null(), c_string.as_ptr(), CF_STRING_ENCODING_UTF8);
        if source.is_null() {
            return Err(format!("Не удалось создать ссылку на раскладку {id}"));
        }

        let status = TISSelectInputSource(source);
        CFRelease(source);
        check_status(status, id)
    }
}

fn check_status(status: OSStatus, id: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "Система отклонила переключение на {id} (OSStatus {status}). \
             Проверьте, что в «Системные настройки → Клавиатура» раскладка {id} включена."
        ))
    }
}
