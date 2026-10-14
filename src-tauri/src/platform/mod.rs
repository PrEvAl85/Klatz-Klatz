//! Platform backends.
//!
//! Every backend exposes the same five functions so the rest of the app never
//! needs `cfg` checks:
//!
//! - `supported() -> (bool, Option<String>)`
//! - `available() -> Result<Vec<LayoutInfo>>`
//! - `current() -> Result<Option<String>>`
//! - `switch_to(id: &str) -> Result<()>`
//! - `language_of(id: &str) -> Option<String>`

/// Backend results are plain strings: they end up in the UI status bar, and a
/// `thiserror` dependency would buy nothing for an app with no error taxonomy.
pub type Result<T> = std::result::Result<T, String>;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use self::windows as imp;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use self::linux as imp;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use self::macos as imp;

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
compile_error!("Klatz-Klatz supports Windows, Linux and macOS only.");
