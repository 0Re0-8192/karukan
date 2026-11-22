//! What the user typed, kept locally for a later pass.
//!
//! Two things come out of a commit:
//! - `user_english.txt` (data dir): English words the user spelled out on
//!   purpose (Shift-started words, F9/F10, the typed keys picked from the
//!   candidate list). They are picked up by the English
//!   mix from the next keystroke on, so nothing has to be registered by hand.
//! - `corrections.tsv` (data dir): one line per committed input — the typed
//!   keys, the reading, the first candidate and the text that was committed,
//!   with a kind: `conv` (taken as offered), `pick` (another candidate),
//!   `fkey` (F9/F10), `live` (live-conversion text). A separate batch job (a
//!   local model, run when the PC is idle) reads it and proposes dictionary
//!   entries. The log is kept for good (the batch job remembers how far it
//!   has read); nothing is ever sent anywhere.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::*;

const MAX_RAW: usize = 120;

/// How often the word list is looked at for changes another process made.
const RELOAD_CHECK: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct Corrections {
    dir: Option<PathBuf>,
    pub(super) english: HashSet<String>,
    /// The word list as last read (mtime, length): another engine appending a
    /// word changes it, and the next word typed here reloads it.
    stamp: Option<(SystemTime, u64)>,
    checked: Option<Instant>,
}

fn file_stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

fn read_words(p: &Path) -> HashSet<String> {
    std::fs::read_to_string(p)
        .map(|t| {
            t.lines()
                .map(|l| l.trim().to_ascii_lowercase())
                .filter(|w| is_word(w))
                .collect()
        })
        .unwrap_or_default()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A word worth remembering: 3+ ASCII letters, nothing else.
fn is_word(s: &str) -> bool {
    s.len() >= 3 && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphabetic())
}

impl Corrections {
    /// Load the learned words.
    pub(super) fn open(dir: PathBuf) -> Self {
        let path = dir.join("user_english.txt");
        Self {
            english: read_words(&path),
            stamp: file_stamp(&path),
            checked: Some(Instant::now()),
            dir: Some(dir),
        }
    }

    /// Pick up words another process (another app's server, the curate job) added,
    /// without restarting anything. Cheap: one stat at most every `RELOAD_CHECK`.
    pub(super) fn reload_if_changed(&mut self) {
        let Some(dir) = &self.dir else { return };
        if self.checked.is_some_and(|t| t.elapsed() < RELOAD_CHECK) {
            return;
        }
        self.checked = Some(Instant::now());
        let path = dir.join("user_english.txt");
        let stamp = file_stamp(&path);
        if stamp != self.stamp {
            self.english = read_words(&path);
            self.stamp = stamp;
        }
    }

    pub(super) fn is_known(&self, word: &str) -> bool {
        self.english.contains(word)
    }

    /// Remember an English word the user spelled on purpose.
    pub(super) fn learn_english(&mut self, word: &str) {
        let word = word.to_ascii_lowercase();
        if !is_word(&word) || !self.english.insert(word.clone()) {
            return;
        }
        if let Some(dir) = &self.dir
            && let Ok(mut f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("user_english.txt"))
        {
            let _ = writeln!(f, "{word}");
            drop(f);
            // our own append is already in `english`: no reload for it
            self.stamp = file_stamp(&dir.join("user_english.txt"));
        }
    }

    pub(super) fn log(&self, kind: &str, raw: &str, reading: &str, first: &str, committed: &str) {
        let Some(dir) = &self.dir else { return };
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("corrections.tsv"))
        else {
            return;
        };
        let _ = writeln!(
            f,
            "{}\t{kind}\t{}\t{}\t{}\t{}",
            now(),
            clean(raw),
            clean(reading),
            clean(first),
            clean(committed)
        );
    }
}

impl InputMethodEngine {
    /// Load the learned English words and open the corrections log.
    pub(super) fn init_corrections(&mut self) {
        if self.corrections.dir.is_none()
            && let Some(dir) = Settings::data_dir()
        {
            let _ = std::fs::create_dir_all(&dir);
            self.corrections = Corrections::open(dir);
        }
    }

    /// Called as a conversion is committed (before the composition ends):
    /// `committed` is the text, `first` what the list offered first.
    pub(super) fn note_conversion_commit(&mut self, committed: &str, first: Option<&str>) {
        if self.mode.current() == InputMode::Emoji || self.raw_keys.is_empty() {
            return;
        }
        let raw = self.raw_keys.clone();
        if raw.chars().count() > MAX_RAW {
            return;
        }
        let reading = self.input_buf.reading();
        let kind = match self.fkey_committed.map(|k| (k, 0)) {
            Some((9 | 10, _)) => {
                // F9/F10: the typed letters are an English word
                if is_word(&raw) {
                    self.corrections.learn_english(&raw);
                }
                "fkey"
            }
            Some(_) => "fkey",
            None if first.is_some_and(|f| f != committed) => {
                // picking the keys as typed (`repo`) spells the word on purpose
                if committed == raw && is_word(&raw) {
                    self.corrections.learn_english(&raw);
                }
                "pick"
            }
            None => "conv",
        };
        self.corrections
            .log(kind, &raw, &reading, first.unwrap_or(""), committed);
    }

    /// Called as the live composition is committed with Enter.
    pub(super) fn note_live_commit(&mut self, reading: &str, committed: &str) {
        if self.mode.current() == InputMode::Emoji || self.raw_keys.is_empty() {
            return;
        }
        let raw = self.raw_keys.clone();
        if raw.chars().count() > MAX_RAW {
            return;
        }
        // Shift-started English word committed as typed
        if self.mode.current() == InputMode::Alphabet && is_word(&committed.replace(' ', "")) {
            self.corrections.learn_english(committed);
        }
        self.corrections.log("live", &raw, reading, "", committed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_another_engine_learned_is_picked_up_without_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Corrections::open(dir.path().to_path_buf());
        let mut b = Corrections::open(dir.path().to_path_buf());
        a.learn_english("repo");
        assert!(!b.is_known("repo"));
        b.reload_if_changed(); // within RELOAD_CHECK: not looked at yet
        assert!(!b.is_known("repo"));
        b.checked = None;
        b.reload_if_changed();
        assert!(b.is_known("repo"));
        // the learner's own append does not count as a change
        a.checked = None;
        let before = a.english.len();
        a.reload_if_changed();
        assert_eq!(a.english.len(), before);
    }
}
