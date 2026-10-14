//! The loop that ties the pieces of automatic switching together.
//!
//! It owns the keyboard hook, feeds every character to the [`Detector`] and
//! carries out what the detector decides. Everything here runs off the hook
//! thread: a hook that takes its time is a hook Windows removes.
//!
//! Only Windows implements the watching half today. On the other platforms the
//! module still compiles and reports itself as unavailable, so nothing else in
//! the app needs a `cfg` of its own.

use super::Layout;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Whether this build can watch the keyboard at all.
pub const SUPPORTED: bool = cfg!(windows);

/// Why not, for the settings window.
pub const UNSUPPORTED_REASON: Option<&str> = if cfg!(windows) {
    None
} else {
    Some("Автоматическое переключение пока работает только на Windows")
};

/// What the supervisor needs to know before it starts.
pub struct Settings {
    /// Directory holding the `<language>.txt` word lists.
    pub lexicon_dir: std::path::PathBuf,
    /// Executable names that automatic switching never touches.
    pub excluded_apps: Vec<String>,
}

/// Runs automatic switching until it is dropped.
pub struct Supervisor {
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    /// Held because the hook stops the moment it is dropped.
    #[cfg(windows)]
    watcher: Option<super::windows::KeyboardWatcher>,
}

impl Supervisor {
    /// Starts watching, or explains why it cannot.
    ///
    /// `layouts` must be the layouts that take part in switching, each already
    /// carrying the language of its word list.
    pub fn start(settings: &Settings, layouts: &[Layout]) -> Result<Self, String> {
        // Exactly one of these blocks exists on any given platform, and each one
        // is the value of the function.
        #[cfg(not(windows))]
        {
            let _ = (settings, layouts);
            Err(UNSUPPORTED_REASON.unwrap_or_default().to_string())
        }

        #[cfg(windows)]
        {
            use super::lexicon::LexiconSet;

            if layouts.len() < 2 {
                return Err(
                    "Для автоматического переключения нужно хотя бы две раскладки.".to_string(),
                );
            }

            let handles = super::windows::handles_for(layouts)
                .ok_or_else(|| "Не удалось прочитать идентификаторы раскладок.".to_string())?;
            let keymap = super::windows::LayoutKeyMap::new(&handles);
            if keymap.usable() < 2 {
                return Err("Не удалось прочитать установленные раскладки.".to_string());
            }

            let languages: Vec<&str> = layouts
                .iter()
                .map(|layout| layout.language.as_str())
                .collect();
            let lexicons = LexiconSet::load(&settings.lexicon_dir, &languages);
            if lexicons.is_empty() {
                return Err(format!(
                    "Не найдены словари в папке {}.",
                    settings.lexicon_dir.display()
                ));
            }

            let (watcher, presses) = super::windows::KeyboardWatcher::start()?;
            let stopping = Arc::new(AtomicBool::new(false));
            let worker_stopping = Arc::clone(&stopping);

            let worker = std::thread::Builder::new()
                .name("klatz-auto-switch".to_string())
                .spawn({
                    let layouts = layouts.to_vec();
                    let excluded = settings.excluded_apps.clone();
                    let keymap = Arc::new(keymap);
                    let lexicons = Arc::new(lexicons);

                    move || {
                        run(
                            presses,
                            &layouts,
                            &lexicons,
                            keymap.as_ref(),
                            &excluded,
                            worker_stopping.as_ref(),
                        )
                    }
                })
                .map_err(|error| format!("Не удалось создать поток переключения: {error}"))?;

            Ok(Self {
                stopping,
                worker: Some(worker),
                watcher: Some(watcher),
            })
        }
    }

    /// Stops the loop and removes the hook.
    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);

        // The worker waits for keys with a timeout, so it notices the flag on
        // its own; the hook goes away with the watcher below.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }

        #[cfg(windows)]
        {
            self.watcher = None;
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Picks the layouts that take part and the language each one belongs to.
///
/// The language comes from the operating system, because the layout id already
/// carries it; layouts whose language is unknown are left out, because a layout
/// without a word list can never be detected anyway.
pub fn layouts_for(ids: &[String], language_of: impl Fn(&str) -> Option<String>) -> Vec<Layout> {
    ids.iter()
        .filter_map(|id| {
            Some(Layout {
                id: id.clone(),
                language: language_of(id)?,
            })
        })
        .collect()
}

#[cfg(windows)]
fn run(
    presses: std::sync::mpsc::Receiver<super::windows::KeyPress>,
    layouts: &[Layout],
    lexicons: &super::lexicon::LexiconSet,
    keymap: &super::windows::LayoutKeyMap,
    excluded: &[String],
    stopping: &AtomicBool,
) {
    use super::windows::{
        Corrector, KeyboardState, PasswordGuard, character_of, foreground_window,
    };
    use super::{Decision, Detector};
    use std::time::Duration;

    /// Virtual key of Backspace: it removes a character, so the word being
    /// typed is one shorter afterwards.
    const VK_BACK: u32 = 0x08;

    /// How long typing has to pause before a switch is carried out, in
    /// milliseconds.
    ///
    /// Short enough to feel instant, long enough for a burst of fast typing to
    /// finish. Without it the repair works from a character count that is stale
    /// by the time the keystrokes are sent.
    const SETTLE_MS: u64 = 60;

    // Set `KLATZ_VERBOSE=1` to log every character the detector sees.
    let verbose = std::env::var_os("KLATZ_VERBOSE").is_some();

    let mut state = KeyboardState::new();
    let mut guard = PasswordGuard::new(excluded);
    let mut detector = Detector::new(layouts, lexicons, keymap, 0);

    // Feeds one keystroke to the detector and returns what it decided.
    //
    // Nothing is applied here: a decision is only acted on once typing has
    // gone quiet, because by then the characters to erase have to be looked
    // at again.
    fn handle(
        press: super::windows::KeyPress,
        state: &mut super::windows::KeyboardState,
        detector: &mut Detector,
        verbose: bool,
    ) -> Decision {
        state.update(press);
        if press.released {
            return Decision::Keep;
        }

        if press.vk_code == VK_BACK {
            detector.backspace();
            return Decision::Keep;
        }

        match character_of(
            press,
            state,
            super::windows::LayoutKeyMap::foreground_layout(),
        ) {
            Some(ch) => {
                if verbose {
                    eprintln!("klatz: '{}'", ch);
                }
                detector.on_char(ch)
            }
            // Arrows, function keys and the like type nothing, but they do
            // end the word as far as the detector is concerned.
            None => {
                detector.reset();
                Decision::Keep
            }
        }
    }

    while !stopping.load(Ordering::SeqCst) {
        let press = match presses.recv_timeout(Duration::from_millis(200)) {
            Ok(press) => press,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        };

        // A password field is none of our business: no switch, and not even a
        // look at what is being typed there.
        if guard.blocks_switching(foreground_window()) {
            state.update(press);
            detector.reset();
            continue;
        }

        if !matches!(
            handle(press, &mut state, &mut detector, verbose),
            Decision::Switch { .. }
        ) {
            continue;
        }

        // The decision was made from the characters typed so far, and the user
        // is very likely still typing. Acting now would erase characters that
        // have since been added and are no longer the ones that are wrong, so
        // the burst is let to finish first.
        std::thread::sleep(Duration::from_millis(SETTLE_MS));

        while let Ok(press) = presses.try_recv() {
            if guard.blocks_switching(foreground_window()) {
                detector.reset();
                continue;
            }
            let _ = handle(press, &mut state, &mut detector, verbose);
        }

        let Some((layout, erase, retype)) = detector.pending_switch() else {
            continue;
        };
        let Some(target) = layouts.get(layout) else {
            continue;
        };

        if let Err(error) = crate::platform::imp::switch_to(&target.id) {
            eprintln!("klatz: не удалось переключить раскладку: {error}");
            continue;
        }

        eprintln!(
            "klatz: язык сменился на {} — «{retype}» вместо {} символ.",
            target.language, erase
        );

        // The repair is typed as unicode, which no layout interferes with, so
        // it lands correctly whichever order the layout change finishes in.
        Corrector::send(erase, &retype);
        detector.applied(layout, &retype);
    }
}

#[cfg(windows)]
#[test]
fn the_real_layouts_and_word_lists_switch_russian_to_english() {
    use crate::autoswitch::windows::{LayoutKeyMap, layout_handles};
    use crate::autoswitch::{Decision, Detector};

    let handles = layout_handles();
    let layouts: Vec<Layout> = handles
        .iter()
        .filter_map(|hkl| {
            let id = format!("{:08x}", *hkl as u32);
            let language = crate::platform::imp::language_of(&id)?;
            (language == "ru" || language == "en").then(|| Layout::new(&id, &language))
        })
        .collect();

    if layouts.len() < 2 {
        // One layout on the machine: nothing to compare against.
        return;
    }

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/lexicon");
    let lexicons = crate::autoswitch::lexicon::LexiconSet::load(&dir, &["ru", "en"]);
    assert!(
        lexicons.contains("ru"),
        "the Russian word list must be there"
    );

    let keymap = LayoutKeyMap::new(&handles);
    let russian = layouts
        .iter()
        .position(|layout| layout.language == "ru")
        .expect("this test needs a Russian layout");
    let english = layouts
        .iter()
        .position(|layout| layout.language == "en")
        .expect("this test needs an English layout");

    let mut detector = Detector::new(&layouts, &lexicons, &keymap, russian);

    // What the Russian layout really puts on screen while the English word
    // `hello` is being typed.
    for ch in ['р', 'у', 'д', 'д', 'щ'] {
        if let Decision::Switch { layout, retype, .. } = detector.on_char(ch) {
            detector.applied(layout, &retype);
        }
    }

    assert_eq!(
        detector.current(),
        english,
        "typing an English word on the Russian layout must switch to English"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(id: &str, language: &str) -> Layout {
        Layout::new(id, language)
    }

    #[test]
    fn layouts_without_a_language_are_left_out() {
        let ids = vec!["04090419".to_string(), "04190419".to_string()];
        let chosen = layouts_for(&ids, |id| match id {
            "04090419" => Some("en".to_string()),
            _ => None,
        });

        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].language, "en");
    }

    #[test]
    fn a_single_layout_is_not_enough_to_switch() {
        let layouts = vec![layout("04090419", "en")];
        let settings = Settings {
            lexicon_dir: std::env::temp_dir(),
            excluded_apps: Vec::new(),
        };

        assert!(Supervisor::start(&settings, &layouts).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn the_languages_are_read_out_of_the_layout_ids() {
        let ids = vec![
            "04090419".to_string(), // Russian locale, US keyboard
            "04190419".to_string(), // Russian locale, Russian keyboard
            "04070407".to_string(), // German
            "00000409".to_string(), // English
            "00000809".to_string(), // English in another region
        ];
        let chosen = layouts_for(&ids, crate::platform::imp::language_of);

        let languages: Vec<&str> = chosen.iter().map(|l| l.language.as_str()).collect();
        assert_eq!(languages, vec!["ru", "ru", "de", "en", "en"]);
    }
}
