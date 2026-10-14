//! Translation of a character between two keyboard layouts.
//!
//! The interesting question is never "what does this letter mean" but "what
//! would the same physical key have produced on another layout". Typing the
//! English word `To` while the Russian layout is active puts the characters
//! `ещ` on screen; those two characters sit on exactly the keys that produce
//! `t` and `o` on a US layout.
//!
//! Layouts are addressed by index because the detector works with the layouts
//! the user configured, not with platform-specific layout identifiers.

/// Something that can say which character a key position produces on a given
/// layout.
pub trait KeyMap {
    /// The character that `ch` becomes when the same physical key is pressed
    /// on layout `target`, or `None` when no key on that layout produces it.
    ///
    /// `origin` is the layout `ch` was typed on and is needed to find the key
    /// position in the first place.
    fn translate(&self, origin: usize, target: usize, ch: char) -> Option<char>;
}

/// A key map described by a translation table instead of by a live keyboard.
///
/// The table maps a character on the origin layout to the character on the
/// target layout. It exists for tests and for platforms where the layout
/// cannot be queried at runtime.
pub struct TableKeyMap {
    /// `table[origin][target][from] = to`
    table: Vec<Vec<HashMap<char, char>>>,
}

use std::collections::HashMap;

impl TableKeyMap {
    pub fn new(layouts: usize) -> Self {
        let empty: HashMap<char, char> = HashMap::new();
        Self {
            table: vec![vec![empty; layouts]; layouts],
        }
    }

    /// Fills in one direction of the mapping. The opposite direction is not
    /// filled automatically: a layout can map several characters onto the same
    /// key.
    pub fn set(&mut self, origin: usize, target: usize, from: char, to: char) {
        self.table[origin][target].insert(from, to);
    }

    pub fn fill_both_ways(&mut self, left: usize, right: usize, left_to_right: &[(char, char)]) {
        for (from, to) in left_to_right {
            self.set(left, right, *from, *to);
            self.set(right, left, *to, *from);
        }
    }
}

impl KeyMap for TableKeyMap {
    fn translate(&self, origin: usize, target: usize, ch: char) -> Option<char> {
        self.table[origin][target].get(&ch).copied()
    }
}

/// The standard Windows Russian layout, `ЙЦУКЕН`, keyed by the US layout key
/// it sits on. Used by tests and as the documented reference of what a table
/// like this looks like.
pub const YATSUKEN: [(char, char); 33] = [
    ('q', 'й'),
    ('w', 'ц'),
    ('e', 'у'),
    ('r', 'к'),
    ('t', 'е'),
    ('y', 'н'),
    ('u', 'г'),
    ('i', 'ш'),
    ('o', 'щ'),
    ('p', 'з'),
    ('[', 'х'),
    (']', 'ъ'),
    ('a', 'ф'),
    ('s', 'ы'),
    ('d', 'в'),
    ('f', 'а'),
    ('g', 'п'),
    ('h', 'р'),
    ('j', 'о'),
    ('k', 'л'),
    ('l', 'д'),
    (';', 'ж'),
    ('\'', 'э'),
    ('z', 'я'),
    ('x', 'ч'),
    ('c', 'с'),
    ('v', 'м'),
    ('b', 'и'),
    ('n', 'т'),
    ('m', 'ь'),
    (',', 'б'),
    ('.', 'ю'),
    ('/', '.'),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_translates_in_both_directions() {
        let mut map = TableKeyMap::new(2);
        map.fill_both_ways(0, 1, &[('t', 'е'), ('o', 'щ')]);

        assert_eq!(map.translate(0, 1, 't'), Some('е'));
        assert_eq!(map.translate(0, 1, 'o'), Some('щ'));
        assert_eq!(map.translate(1, 0, 'е'), Some('t'));
        assert_eq!(map.translate(1, 0, 'щ'), Some('o'));
    }

    #[test]
    fn an_unknown_character_has_no_translation() {
        let map = TableKeyMap::new(2);

        assert_eq!(map.translate(0, 1, 't'), None);
    }

    #[test]
    fn the_yatsuken_table_is_reversible() {
        let mut map = TableKeyMap::new(2);
        map.fill_both_ways(0, 1, &YATSUKEN);

        for (us, ru) in YATSUKEN {
            assert_eq!(map.translate(0, 1, us), Some(ru), "us {us} -> ru {ru}");
            assert_eq!(map.translate(1, 0, ru), Some(us), "ru {ru} -> us {us}");
        }
    }
}
