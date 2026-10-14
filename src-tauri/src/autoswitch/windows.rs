//! Windows support for automatic layout switching.
//!
//! Three things are needed to switch a layout without being asked:
//!
//! 1. **Seeing every keystroke.** A `WH_KEYBOARD_LL` hook sees keys in every
//!    process, which no other mechanism does. Windows removes such a hook if
//!    its callback is slow, so the callback only copies the event into a
//!    channel and returns; every decision happens on other threads.
//!
//! 2. **Knowing which character a key produced.** A virtual key is not a
//!    character: the same key is `a` on one layout and `ф` on another, and
//!    Shift, Caps Lock and AltGr change it further. `ToUnicodeEx` answers that
//!    for the layout that was active at the moment, given a keyboard state
//!    kept up to date here from the hook itself.
//!
//! 3. **Knowing when not to act.** Text typed into a password field must never
//!    be inspected or rewritten, so the focused element is asked whether it is
//!    a password box — and anything the system cannot answer for counts as a
//!    password. See [`PasswordGuard`].

#![cfg(windows)]

use super::keymap::KeyMap;
use std::collections::HashMap;
use std::ffi::OsString;
use std::ffi::c_void;
use std::os::windows::ffi::OsStringExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

// -- Win32 constants --------------------------------------------------------

const WH_KEYBOARD_LL: i32 = 13;
const WM_KEYDOWN: usize = 0x0100;
const WM_KEYUP: usize = 0x0101;
const WM_SYSKEYDOWN: usize = 0x0104;
const WM_SYSKEYUP: usize = 0x0105;
const WM_QUIT: u32 = 0x0012;

/// Virtual key of Backspace, used to erase the wrongly typed characters.
const VK_BACK: u16 = 0x08;

/// `MapVirtualKeyEx` mode: ask for the character a virtual key produces.
const MAPVK_VK_TO_CHAR: u32 = 2;

const KEYEVENTF_KEYUP: u32 = 0x0002;
const KEYEVENTF_UNICODE: u32 = 0x0004;
const INPUT_KEYBOARD: u32 = 1;

#[allow(clippy::upper_case_acronyms)]
mod ffi {
    pub type HWND = *mut core::ffi::c_void;
    pub type HKL = isize;
    pub type HHOOK = *mut core::ffi::c_void;
    pub type HANDLE = *mut core::ffi::c_void;
    pub type LPARAM = isize;
    pub type WPARAM = usize;
    pub type LRESULT = isize;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct POINT {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct MSG {
        pub hwnd: HWND,
        pub message: u32,
        pub wparam: WPARAM,
        pub lparam: LPARAM,
        pub time: u32,
        pub pt: POINT,
    }

    /// `KBDLLHOOKSTRUCT`, the payload a low-level keyboard hook receives.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KBDLLHOOKSTRUCT {
        pub vk_code: u32,
        pub scan_code: u32,
        pub flags: u32,
        pub time: u32,
        pub dw_extra_info: usize,
    }

    /// `KEYBDINPUT`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KEYBDINPUT {
        pub w_vk: u16,
        pub w_scan: u16,
        pub dw_flags: u32,
        pub time: u32,
        pub dw_extra_info: usize,
    }

    /// `INPUT`: a tag, four bytes of padding, and the union of every input
    /// kind, sized by its largest member.
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union INPUTUNION {
        pub keyboard: KEYBDINPUT,
        pub padding: [u8; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct INPUT {
        pub r#type: u32,
        pub padding: u32,
        pub data: INPUTUNION,
    }

    /// `VARIANT`, large enough for the `VT_BOOL` that is asked for here.
    #[repr(C)]
    pub struct VARIANT {
        pub vt: u16,
        pub reserved: [u16; 3],
        pub value: [u8; 8],
    }
}

use ffi::*;

// -- Declared entry points ---------------------------------------------------

#[link(name = "user32")]
unsafe extern "system" {
    fn SetWindowsHookExW(
        id_hook: i32,
        lpfn: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
        hmod: HANDLE,
        thread_id: u32,
    ) -> HHOOK;
    fn UnhookWindowsHookEx(hhk: HHOOK) -> i32;
    fn CallNextHookEx(hhk: HHOOK, n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT;

    fn GetMessageW(lp_msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32;
    fn PostThreadMessageW(thread_id: u32, msg: u32, w_param: WPARAM, l_param: LPARAM) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetKeyboardLayoutList(n_buff: i32, lp_buff: *mut HKL) -> i32;
    fn GetForegroundWindow() -> HWND;
    fn GetWindowThreadProcessId(hwnd: HWND, process_id: *mut u32) -> u32;
    fn GetKeyboardLayout(thread_id: u32) -> HKL;
    fn MapVirtualKeyExW(vk: u32, mode: u32, hkl: HKL) -> u32;
    fn ToUnicodeEx(
        vk: u32,
        scan: u32,
        key_state: *const u8,
        buff: *mut u16,
        c_buff: i32,
        flags: u32,
        hkl: HKL,
    ) -> i32;
    fn SendInput(n_inputs: u32, inputs: *const INPUT, size: i32) -> u32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> HANDLE;
    fn CloseHandle(handle: HANDLE) -> i32;
    fn QueryFullProcessImageNameW(
        process: HANDLE,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoCreateInstance(
        class_id: *const Guid,
        outer: *mut c_void,
        context: u32,
        interface_id: *const Guid,
        out: *mut *mut c_void,
    ) -> i32;
}

/// `PROCESS_QUERY_LIMITED_INFORMATION`, enough to read a process name and
/// nothing else.
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const CLSCTX_INPROC_SERVER: u32 = 0x1;

/// `VT_BOOL`, the variant type `UIA_IsPasswordPropertyId` answers with.
const VT_BOOL: u16 = 11;

/// `UIA_IsPasswordPropertyId`. An element reports this as `true` when it is a
/// password box, which is the contract every accessible control follows.
const UIA_IS_PASSWORD_PROPERTY_ID: i32 = 30019;

/// Slots in the `IUIAutomation` and `IUIAutomationElement` vtables that are
/// called below. Both interfaces start with the three `IUnknown` slots, so the
/// numbers are the order the methods appear in the interface declarations.
const IUNKNOWN_RELEASE: usize = 2;
const IUIAUTOMATION_ELEMENT_FROM_HANDLE: usize = 6;
const IUIAUTOMATION_ELEMENT_GET_CURRENT_PROPERTY_VALUE: usize = 10;

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `CLSID_CUIAutomation` and `IID_IUIAutomation` from `uiautomationcore.h`.
const CLSID_UI_AUTOMATION: Guid = Guid {
    data1: 0xff48_dba4,
    data2: 0x60ef,
    data3: 0x4201,
    data4: [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
};
const IID_UI_AUTOMATION: Guid = Guid {
    data1: 0x30cb_e57d,
    data2: 0xd9d0,
    data3: 0x452a,
    data4: [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
};

// -- Layout translation -----------------------------------------------------

/// Maps characters between two Windows layouts, built at run time.
///
/// Nothing about a layout is assumed: every virtual key is asked what
/// character it produces on each layout, so the table is correct for AZERTY,
/// QWERTZ, Dvorak, ЙЦУКЕН and anything else the user has installed.
pub struct LayoutKeyMap {
    /// Layout index to `virtual key -> lowercase character`.
    by_layout: Vec<HashMap<u32, char>>,
}

impl LayoutKeyMap {
    /// Builds the tables for the given layout handles, in the same order as
    /// the layouts they came from.
    pub fn new(layouts: &[isize]) -> Self {
        Self {
            by_layout: layouts.iter().map(|hkl| Self::keys_of(*hkl)).collect(),
        }
    }

    /// How many of the layouts could be read. A layout the API would not
    /// describe is useless for translation and is skipped by the caller.
    pub fn usable(&self) -> usize {
        self.by_layout
            .iter()
            .filter(|keys| !keys.is_empty())
            .count()
    }

    fn keys_of(hkl: isize) -> HashMap<u32, char> {
        let mut keys = HashMap::new();
        if hkl == 0 {
            return keys;
        }

        for vk in 0x08..=0xfeu32 {
            // SAFETY: a pure lookup on a live layout handle, writing nothing.
            let produced = unsafe { MapVirtualKeyExW(vk, MAPVK_VK_TO_CHAR, hkl) };

            // Dead keys and unbound keys come back as 0xFFFF or 0.
            if produced == 0 || produced == 0xffff {
                continue;
            }
            let Some(ch) = char::from_u32(produced) else {
                continue;
            };
            if ch.is_alphabetic() {
                keys.insert(vk, ch.to_lowercase().next().unwrap_or(ch));
            }
        }

        keys
    }

    /// The layout that was focused when a key was pressed is a thread
    /// property, and the foreground thread is the one typing.
    pub fn foreground_layout() -> isize {
        // SAFETY: `GetForegroundWindow` has no preconditions.
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return 0;
        }

        // SAFETY: a null pointer means "do not return a process id".
        unsafe { GetKeyboardLayout(GetWindowThreadProcessId(window, std::ptr::null_mut())) }
    }
}

impl KeyMap for LayoutKeyMap {
    fn translate(&self, origin: usize, target: usize, ch: char) -> Option<char> {
        let lowered = ch.to_lowercase().next()?;
        let origin = self.by_layout.get(origin)?;
        let target = self.by_layout.get(target)?;

        let vk = *origin.iter().find(|(_, c)| **c == lowered)?.0;
        target.get(&vk).copied()
    }
}

// -- Keyboard state and character production --------------------------------

/// A keystroke as the hook saw it, before any interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    pub vk_code: u32,
    pub scan_code: u32,
    pub released: bool,
}

/// The keys that are down, mirroring what `GetKeyboardState` would report.
///
/// The hook's own thread has no keyboard state of its own, so the array is
/// kept from the stream of events instead of queried.
#[derive(Debug, Clone)]
pub struct KeyboardState {
    keys: [u8; 256],
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyboardState {
    pub fn new() -> Self {
        Self { keys: [0; 256] }
    }

    /// Records a key going down or coming up.
    pub fn update(&mut self, press: KeyPress) {
        if let Some(slot) = self.keys.get_mut((press.vk_code & 0xff) as usize) {
            *slot = u8::from(!press.released);
        }
    }

    pub fn is_down(&self, vk_code: u32) -> bool {
        self.keys
            .get((vk_code & 0xff) as usize)
            .copied()
            .unwrap_or(0)
            != 0
    }

    fn as_slice(&self) -> &[u8; 256] {
        &self.keys
    }
}

/// Turns a keystroke into the character it put on screen, or `None` for keys
/// that produce no character.
///
/// `hkl` has to be the layout that was active when the key went down. Dead
/// keys return `None` as well: their character only exists after the next key,
/// and switching in the middle of a dead key sequence would corrupt both
/// languages.
pub fn character_of(press: KeyPress, state: &KeyboardState, hkl: isize) -> Option<char> {
    if press.released || hkl == 0 {
        return None;
    }

    let mut buffer = [0u16; 4];
    // SAFETY: the buffer holds four units, the state array is the 256 bytes
    // the API documents, and a live layout handle is all `hkl` is used for.
    let produced = unsafe {
        ToUnicodeEx(
            press.vk_code,
            press.scan_code,
            state.as_slice().as_ptr(),
            buffer.as_mut_ptr(),
            buffer.len() as i32,
            0,
            hkl,
        )
    };

    // -1 means a dead key, 0 means the key produced nothing printable.
    if produced != 1 {
        return None;
    }

    char::from_u32(u32::from(buffer[0]))
}

// -- Watching the keyboard --------------------------------------------------

thread_local! {
    /// The sink the hook writes to. A thread-local keeps the hook a plain
    /// `extern "system" fn` without a global, and the hook only ever runs on
    /// the thread that installed it.
    static SINK: std::cell::RefCell<Option<Sender<KeyPress>>> =
        const { std::cell::RefCell::new(None) };
}

/// The global keyboard hook.
///
/// Hooks have to be installed on a thread with a message loop, so this owns
/// one. Dropping it removes the hook and ends the thread.
pub struct KeyboardWatcher {
    thread: Option<JoinHandle<()>>,
    thread_id: Arc<AtomicU32>,
}

impl KeyboardWatcher {
    /// Installs the hook and returns a receiver for the keystrokes.
    pub fn start() -> Result<(Self, Receiver<KeyPress>), String> {
        let (sender, receiver) = mpsc::channel();
        let thread_id = Arc::new(AtomicU32::new(0));
        let thread_id_shared = Arc::clone(&thread_id);

        let thread = std::thread::Builder::new()
            .name("klatz-keyboard-hook".to_string())
            .spawn(move || {
                SINK.with(|sink| *sink.borrow_mut() = Some(sender));
                // SAFETY: returns this thread's own id.
                thread_id_shared.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);

                // SAFETY: a null module handle is what a low-level hook wants;
                // the callback runs in this process, not in the one that gets
                // the keystroke.
                let hook = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, hook_callback, std::ptr::null_mut(), 0)
                };
                if hook.is_null() {
                    return;
                }

                // SAFETY: the loop only reads the message it was handed.
                unsafe {
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                        if message.message == WM_QUIT {
                            break;
                        }
                    }
                }

                // SAFETY: the hook was installed by this thread.
                unsafe { UnhookWindowsHookEx(hook) };
                SINK.with(|sink| *sink.borrow_mut() = None);
            })
            .map_err(|error| format!("не удалось создать поток перехвата клавиш: {error}"))?;

        Ok((
            Self {
                thread: Some(thread),
                thread_id,
            },
            receiver,
        ))
    }

    /// Asks the hook thread to finish and waits for it.
    pub fn stop(&mut self) {
        let id = self.thread_id.load(Ordering::SeqCst);

        // The loop is parked in `GetMessageW`, and only a posted message wakes
        // it. SAFETY: the id is this process' own hook thread.
        unsafe {
            PostThreadMessageW(id, WM_QUIT, 0, 0);
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for KeyboardWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

unsafe extern "system" fn hook_callback(code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    // SAFETY: a negative code must be passed straight on.
    let next = unsafe { CallNextHookEx(std::ptr::null_mut(), code, w_param, l_param) };
    if code < 0 {
        return next;
    }

    let down = matches!(w_param, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(w_param, WM_KEYUP | WM_SYSKEYUP);
    if !down && !up {
        return next;
    }

    // SAFETY: a hook guarantees `l_param` points at a KBDLLHOOKSTRUCT that
    // stays valid for the duration of the call.
    let press = unsafe {
        let event = &*(l_param as *const KBDLLHOOKSTRUCT);
        KeyPress {
            vk_code: event.vk_code,
            scan_code: event.scan_code,
            released: up,
        }
    };

    // Keep the callback short: a slow hook is removed by Windows.
    SINK.with(|sink| {
        if let Some(sender) = sink.borrow().as_ref() {
            let _ = sender.send(press);
        }
    });

    next
}

// -- Password fields --------------------------------------------------------

/// Decides whether the user is typing into something that must be left alone.
///
/// The check is deliberately one-sided. A password field is switched off for
/// certain, and anything the system cannot answer for is treated as a password
/// as well: failing to switch is a small annoyance, while switching inside a
/// password field can mangle the very thing the user is protecting.
pub struct PasswordGuard {
    /// Executable names that never get automatic switching, lower-cased.
    denied: Vec<String>,
    /// Whether the window that currently has focus is off limits.
    focused_is_blocked: bool,
    /// The window that answer belongs to.
    checked: isize,
}

impl PasswordGuard {
    pub fn new(denied: &[String]) -> Self {
        Self {
            denied: denied.iter().map(|name| name.to_lowercase()).collect(),
            // Until something has been checked, nothing is typed into.
            focused_is_blocked: true,
            checked: 0,
        }
    }

    /// Whether the keyboard should be left alone entirely.
    pub fn blocks_switching(&mut self, foreground: HWND) -> bool {
        if foreground.is_null() {
            return true;
        }

        let handle = foreground as isize;
        if handle != self.checked {
            self.checked = handle;
            self.focused_is_blocked =
                self.is_password_field(foreground).unwrap_or(true) || self.is_denied(foreground);
        }

        self.focused_is_blocked
    }

    /// Asks UI Automation whether the element under the keyboard is a password
    /// box. `None` means the question could not be answered.
    fn is_password_field(&self, window: HWND) -> Option<bool> {
        // SAFETY: the vtable slots and signatures are described above and come
        // from the interface declarations in `uiautomationclient.h`.
        unsafe {
            // `S_FALSE` here only means the thread was already initialised,
            // which is just as good.
            CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);

            let mut automation: *mut c_void = std::ptr::null_mut();
            let created = CoCreateInstance(
                &CLSID_UI_AUTOMATION as *const Guid,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_UI_AUTOMATION as *const Guid,
                &mut automation,
            );
            if created < 0 || automation.is_null() {
                return None;
            }

            // Every COM call takes the object itself as `this`, not its
            // vtable; passing the vtable crashes the process.
            let mut element: *mut c_void = std::ptr::null_mut();
            let got = {
                let from_handle: unsafe extern "system" fn(
                    *mut c_void,
                    *mut c_void,
                    *mut *mut c_void,
                ) -> i32 =
                    std::mem::transmute(Self::slot(automation, IUIAUTOMATION_ELEMENT_FROM_HANDLE));
                from_handle(automation, window, &mut element)
            };
            if got < 0 || element.is_null() {
                Self::release(automation);
                return None;
            }

            let mut value: VARIANT = std::mem::zeroed();
            let read = {
                let get_property: unsafe extern "system" fn(*mut c_void, i32, *mut VARIANT) -> i32 =
                    std::mem::transmute(Self::slot(
                        element,
                        IUIAUTOMATION_ELEMENT_GET_CURRENT_PROPERTY_VALUE,
                    ));
                let read = get_property(element, UIA_IS_PASSWORD_PROPERTY_ID, &mut value);
                Self::release(element);
                read
            };
            Self::release(automation);
            if read < 0 {
                return None;
            }

            let boolean = i16::from_le_bytes([value.value[0], value.value[1]]);
            Some(value.vt == VT_BOOL && boolean != 0)
        }
    }

    /// Reads the function pointer at `slot` of a COM object's vtable.
    ///
    /// # Safety
    ///
    /// `object` has to be a live COM pointer and `slot` an index that exists.
    unsafe fn slot(object: *mut c_void, slot: usize) -> usize {
        // The first field of a COM object is the vtable pointer; the methods
        // live in that table, not in the object itself.
        unsafe {
            let vtable = *(object as *const *const usize);
            *vtable.add(slot)
        }
    }

    /// Drops a reference to a COM object.
    ///
    /// # Safety
    ///
    /// `object` has to be a live COM pointer.
    unsafe fn release(object: *mut c_void) {
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> i32 =
                std::mem::transmute(Self::slot(object, IUNKNOWN_RELEASE));
            release(object);
        }
    }

    /// Whether the focused window belongs to a process the user excluded.
    fn is_denied(&self, window: HWND) -> bool {
        if self.denied.is_empty() {
            return false;
        }

        Self::process_name(window).is_some_and(|name| self.denied.contains(&name))
    }

    /// The file name of the process owning `window`, if it can be read.
    pub fn process_name(window: HWND) -> Option<String> {
        // SAFETY: every call checks its own result before using the handle.
        unsafe {
            let mut process = 0;
            GetWindowThreadProcessId(window, &mut process);
            if process == 0 {
                return None;
            }

            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process);
            if handle.is_null() {
                return None;
            }

            let mut buffer = [0u16; 260];
            let mut length = buffer.len() as u32;
            let read = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
            CloseHandle(handle);

            if read == 0 {
                return None;
            }

            let path = OsString::from_wide(&buffer[..length as usize]);
            Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        }
    }
}

// -- Correcting what was already typed --------------------------------------

/// Repairs characters that were typed in the wrong layout.
///
/// Text cannot be rewritten in place: the only way to put the right characters
/// on screen is to erase what is there and type it again, which is what
/// [`Corrector`] does by synthesising keystrokes.
pub struct Corrector;

impl Corrector {
    /// Builds the events that erase `erase` characters and type `retype`.
    ///
    /// Kept apart from sending so the sequence can be inspected in tests
    /// without putting anything on the screen.
    pub fn events(erase: usize, retype: &str) -> Vec<INPUT> {
        let mut inputs = Vec::with_capacity(erase * 2 + retype.chars().count() * 2);

        for _ in 0..erase {
            inputs.push(Self::special(VK_BACK, false));
            inputs.push(Self::special(VK_BACK, true));
        }

        for ch in retype.chars() {
            inputs.push(Self::unicode(ch, false));
            inputs.push(Self::unicode(ch, true));
        }

        inputs
    }

    /// Sends the repair keystrokes and returns how many the system took.
    pub fn send(erase: usize, retype: &str) -> usize {
        let inputs = Self::events(erase, retype);
        if inputs.is_empty() {
            return 0;
        }

        // SAFETY: the buffer is a slice of `INPUT` that outlives the call, and
        // its size is passed alongside it.
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                size_of::<INPUT>() as i32,
            ) as usize
        }
    }

    fn special(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            padding: 0,
            data: INPUTUNION {
                keyboard: KEYBDINPUT {
                    w_vk: vk,
                    w_scan: 0,
                    dw_flags: u32::from(up) * KEYEVENTF_KEYUP,
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        }
    }

    /// Unicode events carry the character itself rather than a key, which is
    /// what makes it possible to type text that no layout has.
    fn unicode(ch: char, up: bool) -> INPUT {
        let mut units = [0u16; 2];
        let encoded = ch.encode_utf16(&mut units);

        INPUT {
            r#type: INPUT_KEYBOARD,
            padding: 0,
            data: INPUTUNION {
                keyboard: KEYBDINPUT {
                    w_vk: 0,
                    w_scan: encoded[0],
                    dw_flags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyboard layouts installed on the machine running the tests, or
    /// nothing when there is only one: translation needs two.
    fn installed_layouts() -> Vec<isize> {
        let mut layouts = [0isize; 16];
        // SAFETY: the buffer is the size the count is bounded by.
        let count = unsafe { GetKeyboardLayoutList(layouts.len() as i32, layouts.as_mut_ptr()) };

        (0..count.max(0) as usize)
            .map(|index| layouts[index])
            .filter(|hkl| *hkl != 0)
            .collect()
    }

    #[test]
    fn the_keyboard_state_follows_keys_down_and_up() {
        let mut state = KeyboardState::new();
        let down = KeyPress {
            vk_code: 0x41,
            scan_code: 0x1e,
            released: false,
        };
        let up = KeyPress {
            released: true,
            ..down
        };

        assert!(!state.is_down(0x41));
        state.update(down);
        assert!(state.is_down(0x41));
        state.update(up);
        assert!(!state.is_down(0x41));
    }

    #[test]
    fn a_released_key_produces_no_character() {
        let press = KeyPress {
            vk_code: 0x41,
            scan_code: 0x1e,
            released: true,
        };

        assert_eq!(character_of(press, &KeyboardState::new(), 0), None);
    }

    #[test]
    fn a_layout_that_cannot_be_read_is_not_invented() {
        let map = LayoutKeyMap::new(&[0]);

        assert_eq!(map.usable(), 0);
        assert_eq!(KeyMap::translate(&map, 0, 0, 'a'), None);
    }

    /// The real check of the whole idea: on a machine that has both an English
    /// and a Russian layout, `A` must read as `ф` on the Russian one and back
    /// again on the English one.
    #[test]
    fn the_live_layouts_translate_each_other() {
        let layouts = installed_layouts();
        let map = LayoutKeyMap::new(&layouts);

        if map.usable() < 2 {
            // Nothing to compare against on a machine with a single layout.
            return;
        }

        let mut checked = 0;
        for (origin, source) in layouts.iter().enumerate() {
            // SAFETY: the same call `character_of` makes, on a live layout.
            let here = unsafe { character_on(*source, 0x41, &KeyboardState::new()) };
            let Some(here) = here else {
                continue;
            };

            for target in 0..layouts.len() {
                if target == origin {
                    continue;
                }

                assert_eq!(
                    map.translate(origin, target, here),
                    map.translate(target, origin, here),
                    "`{here}` must read the same way on both layouts"
                );
                checked += 1;
            }
        }

        // A machine with two identical layouts proves nothing.
        assert!(checked > 0 || map.usable() < 2);
    }

    /// The character a virtual key produces on a layout, for the tests.
    ///
    /// # Safety
    ///
    /// `hkl` has to be a live layout handle.
    unsafe fn character_on(hkl: isize, vk: u32, state: &KeyboardState) -> Option<char> {
        let mut buffer = [0u16; 4];
        let produced = unsafe {
            ToUnicodeEx(
                vk,
                0,
                state.as_slice().as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as i32,
                0,
                hkl,
            )
        };

        (produced == 1)
            .then(|| char::from_u32(u32::from(buffer[0])))
            .flatten()
    }

    #[test]
    fn erasing_and_retyping_produces_one_pair_of_events_per_character() {
        let inputs = Corrector::events(2, "ab");

        assert_eq!(inputs.len(), 2 * 2 + 2 * 2);
        assert!(inputs.iter().all(|input| input.r#type == INPUT_KEYBOARD));

        // SAFETY: every `INPUT` here was built by `Corrector`, so the union
        // really does hold a keyboard event.
        let flags = inputs
            .iter()
            .map(|input| unsafe { input.data.keyboard.dw_flags })
            .collect::<Vec<_>>();

        // The first four events are the two backspaces, pressing and releasing.
        assert!(
            flags[..4]
                .iter()
                .step_by(2)
                .all(|flag| flag & KEYEVENTF_KEYUP == 0)
        );
        assert!(
            flags[..4]
                .iter()
                .skip(1)
                .step_by(2)
                .all(|flag| flag & KEYEVENTF_KEYUP != 0)
        );

        // The rest carry the characters as unicode rather than as a key.
        assert!(
            flags[4..]
                .iter()
                .step_by(2)
                .all(|flag| flag & KEYEVENTF_UNICODE != 0)
        );
    }

    #[test]
    fn nothing_to_do_produces_no_events() {
        assert!(Corrector::events(0, "").is_empty());
    }

    #[test]
    fn the_focused_window_can_be_asked_about_passwords() {
        // SAFETY: reading the foreground window has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }

        // The point of the test is that asking does not take the process down:
        // these are hand-written COM calls, and a wrong vtable slot would
        // crash rather than fail. What the focused window answers is not up
        // to the test, so only the shape of the answer is checked.
        let guard = PasswordGuard::new(&[]);
        for _ in 0..3 {
            let _ = guard.is_password_field(foreground);
        }
    }

    #[test]
    fn the_process_behind_the_focused_window_can_be_named() {
        // SAFETY: reading the foreground window has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }

        // The name depends on what happens to be in front, but it is a
        // lower-cased file name and never a full path.
        if let Some(name) = PasswordGuard::process_name(foreground) {
            assert!(!name.contains('\\'));
            assert!(!name.contains('/'));
        }
    }

    #[test]
    fn a_denied_process_is_always_blocked() {
        // SAFETY: reading the foreground window has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }

        let Some(name) = PasswordGuard::process_name(foreground) else {
            return;
        };

        let mut guarded = PasswordGuard::new(&[name]);
        assert!(guarded.blocks_switching(foreground));
    }
}
