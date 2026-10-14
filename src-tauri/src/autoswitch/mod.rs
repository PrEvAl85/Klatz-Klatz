//! Deciding when the layout has to change on its own.
//!
//! The problem sounds trivial — "switch when the user starts typing another
//! language" — and the difficulty is entirely in the word "another": text is
//! typed one character at a time, so the detector has to answer after every
//! keystroke whether the fragment on screen so far already rules out the
//! language it was typed in.
//!
//! Say the Russian layout is active and the English word `To` is typed. The
//! screen shows `ещ`, which is a perfectly good beginning of a Russian word
//! (`ещё`), so no switch happens yet. The next key makes it `ещв`, which no
//! Russian word starts with, while the same three keys read `tod` in English.
//! That is the moment to switch.
//!
//! The detector never touches the keyboard itself: it returns a [`Decision`]
//! describing what should change, and the platform layer performs it.

pub mod keymap;
pub mod lexicon;
pub mod supervisor;
pub mod windows;

use keymap::KeyMap;
use lexicon::{LexiconSet, Support};

/// The shortest fragment the dictionary rule is allowed to act on.
///
/// A single character is too weak a signal: `a` is an English word and `ф` is
/// a fine Russian word too.
const MIN_FRAGMENT: usize = 2;

/// How common a word has to be before it may justify a layout switch on a
/// fragment that is only a prefix of it.
const RANK_LIMIT: u32 = 5_000;

/// What the platform layer should do with the keyboard after a keystroke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Type the character as it is.
    Keep,
    /// The word ended; nothing needs to happen beyond the buffer reset.
    Commit,
    /// Switch to `layout` and repair the fragment that was typed in the old
    /// one: erase `erase` characters and type `retype` in their place.
    Switch {
        layout: usize,
        erase: usize,
        retype: String,
    },
}

/// A layout taking part in automatic switching.
#[derive(Debug, Clone)]
pub struct Layout {
    /// The platform layout identifier, for example `04090419`.
    pub id: String,
    /// The language whose word list applies, for example `en`.
    pub language: String,
}

impl Layout {
    pub fn new(id: &str, language: &str) -> Self {
        Self {
            id: id.to_string(),
            language: language.to_string(),
        }
    }
}

/// The layout-independent half of automatic switching.
pub struct Detector<'a> {
    layouts: &'a [Layout],
    lexicons: &'a LexiconSet,
    keymap: &'a dyn KeyMap,
    /// Characters of the word being typed, as the current layout produced them.
    fragment: String,
    current: usize,
    /// How many characters have been seen since the last switch, to keep the
    /// detector from flipping between layouts on one ambiguous fragment.
    since_switch: usize,
}

impl<'a> Detector<'a> {
    pub fn new(
        layouts: &'a [Layout],
        lexicons: &'a LexiconSet,
        keymap: &'a dyn KeyMap,
        current: usize,
    ) -> Self {
        Self {
            layouts,
            lexicons,
            keymap,
            fragment: String::new(),
            current,
            since_switch: usize::MAX,
        }
    }

    pub fn current(&self) -> usize {
        self.current
    }

    /// The switch the current fragment calls for, without changing anything.
    ///
    /// This is what to ask once typing has gone quiet: the fragment may have grown
    /// since the last decision, and the characters to erase have to match what is
    /// on screen at the moment of the repair.
    pub fn pending_switch(&self) -> Option<(usize, usize, String)> {
        match self.evaluate()? {
            Decision::Switch {
                layout,
                erase,
                retype,
            } => Some((layout, erase, retype)),
            _ => None,
        }
    }

    /// Records that a switch was carried out, so that the fragment matches the
    /// text now on screen.
    pub fn applied(&mut self, layout: usize, retype: &str) {
        self.current = layout;
        self.fragment = retype.to_string();
        self.since_switch = 0;
    }

    /// Forgets the word being typed, because something that is not a character
    /// ended it: a space, a comma, an arrow key, or the focus moving away.
    pub fn reset(&mut self) {
        self.fragment.clear();
        self.since_switch = usize::MAX;
    }

    /// Takes back one character, because the user erased it.
    pub fn backspace(&mut self) {
        self.fragment.pop();
        self.since_switch = self.since_switch.saturating_add(1);
    }

    /// Feeds one typed character to the detector and returns what to do.
    ///
    /// The detector does not act on its own decision: the characters to erase have
    /// to match what is on screen at the moment of the repair, which may be a few
    /// keystrokes later. The caller calls [`Detector::applied`] once it has done
    /// it, and can ask for a fresh decision with [`Detector::pending_switch`].
    pub fn on_char(&mut self, ch: char) -> Decision {
        if !ch.is_alphabetic() {
            let had_fragment = !self.fragment.is_empty();
            self.fragment.clear();
            self.since_switch = usize::MAX;

            return if had_fragment {
                Decision::Commit
            } else {
                Decision::Keep
            };
        }

        self.fragment.extend(ch.to_lowercase());
        // Saturating: the initial value means "nothing typed since the last
        // switch" and must not wrap around on the first keystroke.
        self.since_switch = self.since_switch.saturating_add(1);

        self.evaluate().unwrap_or(Decision::Keep)
    }

    /// The switch this fragment calls for, if any.
    fn evaluate(&self) -> Option<Decision> {
        if self.fragment.chars().count() < MIN_FRAGMENT {
            return None;
        }
        // Right after a switch the word is still incomplete, so the remaining
        // characters of that same word must not trigger another one.
        if self.since_switch < MIN_FRAGMENT {
            return None;
        }

        let origin = self.layouts.get(self.current)?;
        // A fragment that could still grow into a word of the language it was
        // typed in is never a reason to switch: that is the normal case.
        if !self
            .lexicons
            .support(&origin.language, &self.fragment)
            .is_none()
        {
            return None;
        }

        let mut best: Option<(u32, usize, String)> = None;

        for (index, layout) in self.layouts.iter().enumerate() {
            if index == self.current || layout.language == origin.language {
                continue;
            }

            let Some(retype) = self.reread(index) else {
                continue;
            };

            let rank = match self.lexicons.support(&layout.language, &retype) {
                Support::Exact { rank } => rank,
                Support::Prefix { best_rank } if best_rank <= RANK_LIMIT => best_rank,
                _ => continue,
            };

            if best
                .as_ref()
                .is_none_or(|(best_rank, _, _)| rank < *best_rank)
            {
                best = Some((rank, index, retype));
            }
        }

        let (_, layout, retype) = best?;
        Some(Decision::Switch {
            layout,
            erase: self.fragment.chars().count(),
            retype,
        })
    }

    /// What the current fragment looks like if it is read on layout `target`
    /// instead, that is, the same physical keys on another layout.
    ///
    /// `None` when the fragment cannot be read there at all, which happens when
    /// the layouts share too few keys to be worth switching between.
    fn reread(&self, target: usize) -> Option<String> {
        let mut out = String::with_capacity(self.fragment.len());

        for ch in self.fragment.chars() {
            out.push(self.keymap.translate(self.current, target, ch)?);
        }

        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::keymap::{TableKeyMap, YATSUKEN};
    use super::*;
    use std::path::Path;

    const EN: &str = "the\nof\nand\nto\nin\nyou\nit\nthat\nis\nwas\ntoday\nhello\nas\nat\n";
    const RU: &str = "и\nв\nне\nна\nя\nбыть\nпривет\nсегодня\nещё\nдом\nэто\nкак\n";

    /// Layout 0 is US English, layout 1 is Russian.
    fn english_layout() -> Layout {
        Layout::new("04090419", "en")
    }

    fn russian_layout() -> Layout {
        Layout::new("04190419", "ru")
    }

    fn detector<'a>(
        layouts: &'a [Layout],
        lexicons: &'a LexiconSet,
        keymap: &'a dyn KeyMap,
        current: usize,
    ) -> Detector<'a> {
        Detector::new(layouts, lexicons, keymap, current)
    }

    fn fixtures() -> (LexiconSet, TableKeyMap) {
        let lexicons = LexiconSet::from_files(&[("en", EN), ("ru", RU)]);
        let mut keymap = TableKeyMap::new(2);
        keymap.fill_both_ways(0, 1, &YATSUKEN);
        (lexicons, keymap)
    }

    /// Types `word` the way a person would.
    ///
    /// `intended` is the layout the word belongs to and therefore the layout
    /// whose letters the fingers go for; `current` is what is actually active,
    /// so the characters that reach the screen are the ones the other layout
    /// puts on the same keys. That mismatch is the whole problem.
    fn type_word(
        detector: &mut Detector,
        keymap: &TableKeyMap,
        intended: usize,
        current: usize,
        word: &str,
    ) -> Vec<Decision> {
        word.chars()
            .map(|ch| {
                let produced = keymap.translate(intended, current, ch).unwrap_or(ch);
                detector.on_char(produced)
            })
            .collect()
    }

    #[test]
    fn typing_in_the_active_language_never_switches() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        // Russian keys on a Russian layout: the screen shows what was meant.
        let decisions = type_word(&mut detector, &keymap, 1, 1, "привет");

        assert_eq!(decisions.last(), Some(&Decision::Keep));
        assert_eq!(detector.current(), 1);
    }

    #[test]
    fn a_word_that_is_not_russian_switches_to_english() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        // "Today" typed on the Russian layout puts `ещвфн` on screen. The first
        // two characters still start the Russian word `ещё`, so the switch has
        // to wait for the third one to rule Russian out.
        let decisions = type_word(&mut detector, &keymap, 0, 1, "today");

        assert_eq!(
            decisions,
            vec![
                Decision::Keep,
                Decision::Keep,
                Decision::Switch {
                    layout: 0,
                    erase: 3,
                    retype: "tod".to_string(),
                },
                Decision::Switch {
                    layout: 0,
                    erase: 4,
                    retype: "toda".to_string(),
                },
                Decision::Switch {
                    layout: 0,
                    erase: 5,
                    retype: "today".to_string(),
                },
            ],
            "each keystroke re-reports the switch with what is on screen now"
        );

        // The detector does not move on its own: the caller carries the repair
        // out and says so.
        detector.applied(0, "today");
        assert_eq!(detector.current(), 0);
    }

    #[test]
    fn a_word_that_is_not_english_switches_to_russian() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 0);

        // Russian keys on an English layout put "dom" on screen, which is not
        // an English word, while `дом` is.
        let decisions = type_word(&mut detector, &keymap, 1, 0, "дом");

        assert_eq!(
            decisions,
            vec![
                // One character is never enough.
                Decision::Keep,
                Decision::Switch {
                    layout: 1,
                    erase: 2,
                    retype: "до".to_string(),
                },
                Decision::Switch {
                    layout: 1,
                    erase: 3,
                    retype: "дом".to_string(),
                },
            ]
        );
        detector.applied(1, "дом");
        assert_eq!(detector.current(), 1);
    }

    #[test]
    fn a_fragment_that_fits_both_languages_is_left_alone() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        // `т` and `о` on a Russian layout produce `ещ`, a valid start of `ещё`,
        // so the switch is deferred rather than guessed.
        let decisions = type_word(&mut detector, &keymap, 0, 1, "то");

        assert!(
            !decisions
                .iter()
                .any(|d| matches!(d, Decision::Switch { .. }))
        );
        assert_eq!(detector.current(), 1);
    }

    #[test]
    fn one_character_never_triggers_a_switch() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 0);

        // "a" is an English word, so the Russian `ф` it produces deserves a
        // look — but one character never decides anything.
        let decisions = type_word(&mut detector, &keymap, 1, 0, "а");

        assert_eq!(decisions, vec![Decision::Keep]);
        assert_eq!(detector.current(), 0);
    }

    #[test]
    fn separators_finish_the_word() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        type_word(&mut detector, &keymap, 1, 1, "привет");
        assert_eq!(detector.on_char(' '), Decision::Commit);
        assert_eq!(detector.on_char(' '), Decision::Keep);
    }

    #[test]
    fn a_layout_without_a_word_list_is_never_selected() {
        let layouts = vec![english_layout(), Layout::new("0409040f", "fr")];
        let lexicons = LexiconSet::from_files(&[("en", EN)]);
        let mut keymap = TableKeyMap::new(2);
        keymap.fill_both_ways(0, 1, &[('d', 'в'), ('o', 'щ')]);
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        // "vo" on the French layout reads as `щщ` in Russian letters, which no
        // French word list could ever confirm.
        let decisions: Vec<Decision> = "vo"
            .chars()
            .map(|ch| {
                let produced = keymap.translate(1, 1, ch).unwrap_or(ch);
                detector.on_char(produced)
            })
            .collect();

        assert!(
            !decisions
                .iter()
                .any(|d| matches!(d, Decision::Switch { .. }))
        );
    }

    #[test]
    fn a_pending_switch_follows_the_text_that_is_on_screen() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        // "Today" on a Russian layout puts `ещвдщ` on screen; the first three
        // characters already rule Russian out, so a switch is decided while the
        // word is still being typed.
        let decisions: Vec<Decision> = ['е', 'щ', 'в']
            .into_iter()
            .map(|ch| detector.on_char(ch))
            .collect();

        assert!(
            decisions
                .iter()
                .any(|decision| matches!(decision, Decision::Switch { .. }))
        );

        // But the user keeps typing before anyone acts on it, and now there are
        // five wrong characters on screen rather than three: `ещвфн`.
        let _ = detector.on_char('ф');
        let _ = detector.on_char('н');

        let (layout, erase, retype) = detector
            .pending_switch()
            .expect("the switch is still warranted");

        assert_eq!(layout, 0);
        assert_eq!(
            erase, 5,
            "every wrong character has to go, not the three seen earlier"
        );
        assert_eq!(retype, "today");

        detector.applied(layout, &retype);
        assert_eq!(detector.current(), 0);
    }

    #[test]
    fn the_switcher_keeps_its_place_after_a_switch() {
        let layouts = vec![english_layout(), russian_layout()];
        let (lexicons, keymap) = fixtures();
        let mut detector = detector(&layouts, &lexicons, &keymap, 1);

        type_word(&mut detector, &keymap, 0, 1, "today");

        detector.applied(0, "today");
        assert_eq!(detector.current(), 0);
        // The rest of the same word must not send it straight back.
        let decisions = type_word(&mut detector, &keymap, 0, 0, "ay");
        assert!(
            !decisions
                .iter()
                .any(|d| matches!(d, Decision::Switch { .. }))
        );
        assert_eq!(detector.current(), 0);
    }

    #[test]
    fn the_real_word_lists_are_usable() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/lexicon");
        if !dir.join("ru.txt").exists() {
            return;
        }

        let lexicons = LexiconSet::load(&dir, &["en", "ru"]);

        assert!(lexicons.contains("en") && lexicons.contains("ru"));
        assert!(matches!(
            lexicons.support("en", "the"),
            Support::Exact { .. }
        ));
        // `qwerty` on a Russian layout is `цщцопй`, and no Russian word starts
        // that way.
        assert!(lexicons.support("ru", "цщцопй").is_none());
        assert!(!lexicons.support("en", "the").is_none());
    }
}
