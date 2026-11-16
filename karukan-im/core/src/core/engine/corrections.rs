//! What the user corrected, kept locally for a later pass.
//!
//! Two things come out of a commit:
//! - `user_english.txt` (data dir): English words the user spelled out on
//!   purpose (Shift-started words, F9/F10). They are picked up by the English
//!   mix from the next keystroke on, so nothing has to be registered by hand.
//! - `corrections.tsv` (data dir): one line per commit that differs from what
//!   was offered first — the typed keys, the reading, the first candidate and
//!   the text that was committed. A separate batch job (a local model, run
//!   when the PC is idle) reads it and proposes dictionary entries. Lines
//!   older than [`KEEP_DAYS`] are dropped at startup; nothing is ever sent
//!   anywhere.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

const KEEP_DAYS: u64 = 30;
const MAX_RAW: usize = 120;

#[derive(Default)]
pub(super) struct Corrections {
    dir: Option<PathBuf>,
    pub(super) english: HashSet<String>,
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
    /// Load the learned words and drop log lines past the retention window.
    pub(super) fn open(dir: PathBuf) -> Self {
        let english = std::fs::read_to_string(dir.join("user_english.txt"))
            .map(|t| t.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|w| is_word(w)).collect())
            .unwrap_or_default();
        let log = dir.join("corrections.tsv");
        if let Ok(text) = std::fs::read_to_string(&log) {
            let cutoff = now().saturating_sub(KEEP_DAYS * 86_400);
            let kept: Vec<&str> = text
                .lines()
                .filter(|l| {
                    l.split('\t')
                        .next()
                        .and_then(|t| t.parse::<u64>().ok())
                        .is_some_and(|t| t >= cutoff)
                })
                .collect();
            if kept.len() != text.lines().count() {
                let mut out = kept.join("\n");
                if !out.is_empty() {
                    out.push('\n');
                }
                let _ = std::fs::write(&log, out);
            }
        }
        Self {
            dir: Some(dir),
            english,
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
            None if first.is_some_and(|f| f != committed) => "pick",
            None => return,
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
        if committed.chars().any(|c| c.is_ascii_alphabetic()) && committed != reading {
            self.corrections.log("live", &raw, reading, "", committed);
        }
    }
}
