//! Candidates that mix English words into the kana: `windowsdeha` (typed
//! as plain romaji, so it reads うぃんどwsでは) also offers `windowsでは`.
//!
//! The typed keystrokes are split into English words (from a bundled list)
//! and runs that are complete kana on their own; the split with the most
//! English letters wins. Only offered when English makes up at least half of
//! the keystrokes, and words need 3+ letters, so ordinary Japanese typing
//! almost never produces one.

use std::collections::HashSet;
use std::sync::OnceLock;

use super::*;

const MIN_WORD: usize = 3;
const MAX_KANA_RUN: usize = 12;

fn words() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        include_str!("../../../data/english_words.txt")
            .lines()
            .map(str::trim)
            .filter(|w| w.len() >= MIN_WORD)
            .collect()
    })
}

/// Best split of `raw` (ASCII letters): English words verbatim, the rest
/// through `kana` (None when a run is not complete kana).
pub(super) fn mix(raw: &str, kana: impl Fn(&str) -> Option<String>) -> Option<String> {
    if raw.len() < MIN_WORD || !raw.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let n = raw.len();
    let words = words();

    // best[i]: (english letters, -pieces) for the suffix starting at i,
    // with the piece that achieves it.
    #[derive(Clone)]
    struct Step {
        score: (usize, i64),
        end: usize,
        english: bool,
    }
    let mut best: Vec<Option<Step>> = vec![None; n + 1];
    best[n] = Some(Step {
        score: (0, 0),
        end: n,
        english: false,
    });
    for i in (0..n).rev() {
        let mut here: Option<Step> = None;
        let consider = |end: usize, english: bool, here: &mut Option<Step>| {
            let Some(next) = &best[end] else { return };
            let gain = if english { end - i } else { 0 };
            let score = (next.score.0 + gain, next.score.1 - 1);
            if here.as_ref().is_none_or(|h| score > h.score) {
                *here = Some(Step { score, end, english });
            }
        };
        for end in (i + MIN_WORD)..=n {
            if words.contains(&lower[i..end]) {
                consider(end, true, &mut here);
            }
        }
        for end in (i + 1)..=n.min(i + MAX_KANA_RUN) {
            if kana(&lower[i..end]).is_some() {
                consider(end, false, &mut here);
            }
        }
        best[i] = here;
    }

    let start = best[0].as_ref()?;
    let letters = start.score.0;
    if letters < MIN_WORD || letters * 2 < n {
        return None;
    }
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let step = best[i].as_ref()?;
        if step.english {
            out.push_str(&raw[i..step.end]);
        } else {
            out.push_str(&kana(&lower[i..step.end])?);
        }
        i = step.end;
    }
    Some(out)
}

impl InputMethodEngine {
    /// Whether the typed romaji left something that cannot be kana: an ASCII
    /// letter already settled into the reading (`ウィンドws`) or a pending
    /// tail no kana starts with (`ws`). Plain Japanese typing never does.
    fn has_romaji_leftover(&self, converted: &str) -> bool {
        let pending = self.input_buf.pending();
        converted.chars().any(|c| c.is_ascii_alphabetic())
            || self
                .input_buf
                .reading()
                .chars()
                .any(|c| c.is_ascii_alphabetic())
            || (!pending.is_empty() && self.converters.romaji.pending_expansions(&pending).is_empty())
    }

    /// The English-mixed reading of what was typed, when the keystrokes are
    /// known and consistent with the reading.
    pub(super) fn english_mix_candidate(&self, reading: &str) -> Option<String> {
        if self.mode.current() != InputMode::Hiragana || self.raw_keys.is_empty() {
            return None;
        }
        let romaji = &self.converters.romaji;
        // Plain Japanese typing (ohayou, genkidesuka) must not sprout
        // English readings; a lone English word (tokyo) may.
        let leftover = self.has_romaji_leftover("");
        // Edits around the keystrokes (cursor moves, mid-buffer inserts)
        // leave raw_keys stale; only trust it when it reproduces the reading.
        if romaji.convert_flush(&self.raw_keys) != reading {
            return None;
        }
        let text = mix(&self.raw_keys, |run| {
            let k = romaji.convert_flush(run);
            (!k.is_empty() && k.chars().all(|c| !c.is_ascii())).then_some(k)
        })?;
        let whole_word = text.eq_ignore_ascii_case(&self.raw_keys) && text.len() >= 4;
        ((leftover || whole_word) && text != reading).then_some(text)
    }
}

impl InputMethodEngine {
    /// Live conversion text with English words restored, for a model answer
    /// that still carries unresolved romaji (`ウィンドws`): the keystrokes
    /// before the pending tail, mixed. `None` when the answer is clean or
    /// the keystrokes do not make an English-mixed reading.
    pub(super) fn live_english(&self, converted: &str) -> Option<String> {
        if self.mode.current() != InputMode::Hiragana || self.raw_keys.is_empty() {
            return None;
        }
        let romaji = &self.converters.romaji;
        let pending = self.input_buf.pending();
        let tail = romaji.convert_flush(&pending);
        if !self.has_romaji_leftover(converted) {
            return None;
        }
        if romaji.convert_flush(&self.raw_keys) != self.input_buf.settled_reading(romaji) {
            return None;
        }
        let kana = |run: &str| {
            let k = romaji.convert_flush(run);
            (!k.is_empty() && k.chars().all(|c| !c.is_ascii())).then_some(k)
        };
        // The caller appends `tail` (the pending keystrokes as typed) after
        // this text, so the mixed text must end with it. A partial last
        // syllable (`windowsd`) is carried over verbatim.
        let raw = &self.raw_keys;
        let n = raw.len();
        let text = (0..=3.min(n))
            .filter(|k| raw.is_char_boundary(n - k))
            .find_map(|k| {
                let (head, rest) = raw.split_at(n - k);
                (rest.chars().all(|c| c.is_ascii_alphabetic()))
                    .then(|| mix(head, kana).map(|t| format!("{t}{rest}")))
                    .flatten()
            })?;
        text.strip_suffix(tail.as_str()).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_kana(run: &str) -> Option<String> {
        // enough romaji for the tests: de, ha, wo, ga, da
        let table = [("de", "で"), ("ha", "は"), ("wo", "を"), ("ga", "が"), ("da", "だ")];
        let mut rest = run;
        let mut out = String::new();
        'outer: while !rest.is_empty() {
            for (r, k) in table {
                if let Some(t) = rest.strip_prefix(r) {
                    out.push_str(k);
                    rest = t;
                    continue 'outer;
                }
            }
            return None;
        }
        Some(out)
    }

    #[test]
    fn mixes_word_and_kana() {
        assert_eq!(mix("windowsdeha", fake_kana).as_deref(), Some("windowsでは"));
        assert_eq!(mix("goodda", fake_kana).as_deref(), Some("goodだ"));
    }

    #[test]
    fn plain_japanese_is_left_alone() {
        assert_eq!(mix("dehaga", fake_kana), None);
        assert_eq!(mix("ab", fake_kana), None);
    }

    #[test]
    fn too_little_english_is_left_alone() {
        // "the" is a word but covers well under half of the keystrokes
        assert_eq!(mix("thedehagadawo", fake_kana), None);
    }

    #[test]
    fn keeps_typed_case() {
        assert_eq!(mix("WindowsDeha", fake_kana).as_deref(), Some("Windowsでは"));
    }
}
