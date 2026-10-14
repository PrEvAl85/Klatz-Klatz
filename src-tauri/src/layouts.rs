//! Data model shared by every platform backend.

use serde::Serialize;

/// A keyboard layout reported by the operating system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayoutInfo {
    /// Stable platform-specific identifier, e.g. `04090419` on Windows.
    pub id: String,
    /// Human readable name shown in the UI.
    pub name: String,
}

/// Everything the frontend needs to render one consistent snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub available: Vec<LayoutInfo>,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    /// Why the global hotkey is not registered, if registration failed.
    pub hotkey_error: Option<String>,
    pub current: Option<String>,
    pub config: crate::config::Config,
}

/// Picks the layout that follows `current` in a cycle.
///
/// The list is the user's cycle order and is expected to be non-empty.
/// When `current` is missing or unknown the first entry wins, so a single
/// stray unknown id can never leave the user without a working hotkey.
pub fn next_in_cycle<'a>(cycle: &'a [String], current: Option<&str>) -> Option<&'a String> {
    let first = cycle.first()?;

    let Some(current) = current else {
        return Some(first);
    };

    cycle
        .iter()
        .position(|id| id == current)
        .map(|index| &cycle[(index + 1) % cycle.len()])
        .or(Some(first))
}

/// Reorders `available` so that `target` comes first, keeping the rest in place.
///
/// Used by backends (X11 today) where the active layout is expressed as the
/// first entry of the keyboard layout group rather than as a separate flag.
/// It lives here, with its tests, so that the cycle logic is covered on every
/// platform instead of only where the backend that calls it is compiled.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn move_to_front(ids: &mut Vec<String>, target: &str) {
    if let Some(index) = ids.iter().position(|id| id == target) {
        let id = ids.remove(index);
        ids.insert(0, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn cycles_forward_and_wraps_around() {
        let cycle = ids(&["en", "ru", "de"]);
        assert_eq!(next_in_cycle(&cycle, Some("en")), Some(&ids(&["ru"])[0]));
        assert_eq!(next_in_cycle(&cycle, Some("ru")), Some(&ids(&["de"])[0]));
        assert_eq!(next_in_cycle(&cycle, Some("de")), Some(&ids(&["en"])[0]));
    }

    #[test]
    fn unknown_current_falls_back_to_first() {
        let cycle = ids(&["en", "ru"]);
        assert_eq!(next_in_cycle(&cycle, Some("zz")), Some(&ids(&["en"])[0]));
        assert_eq!(next_in_cycle(&cycle, None), Some(&ids(&["en"])[0]));
    }

    #[test]
    fn single_element_cycle_is_stable() {
        let cycle = ids(&["en"]);
        assert_eq!(next_in_cycle(&cycle, Some("en")), Some(&ids(&["en"])[0]));
    }

    #[test]
    fn empty_cycle_yields_nothing() {
        assert_eq!(next_in_cycle(&[], Some("en")), None);
    }

    #[test]
    fn move_to_front_preserves_others() {
        let mut list = ids(&["en", "ru", "de"]);
        move_to_front(&mut list, "de");
        assert_eq!(list, ids(&["de", "en", "ru"]));
    }

    #[test]
    fn move_to_front_ignores_unknown_target() {
        let mut list = ids(&["en", "ru"]);
        move_to_front(&mut list, "zz");
        assert_eq!(list, ids(&["en", "ru"]));
    }
}
