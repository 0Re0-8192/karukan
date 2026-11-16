//! English words mixed into kana typing: `konopcnowindowsdehenkan` reads
//! このpcのwindowsでへんかん, with the kana runs between the words converted
//! by the model as usual.
//!
//! The typed keystrokes are split into English words (a bundled list of
//! common words) and runs that are complete kana. An English piece is only
//! allowed where the keystrokes are *not* readable as kana (`windows`,
//! `pc`), so ordinary Japanese typing (`ten`, `kan`, `ohayou`) is never
//! reinterpreted, and the whole thing is only attempted when the romaji
//! leaves something that cannot be kana.

use std::collections::HashSet;
use std::sync::OnceLock;

use super::*;

const MIN_WORD: usize = 2;
const MAX_KANA_RUN: usize = 12;
/// Two-letter words are mostly noise (`uh`, `ou`); only these count.
const SHORT_WORDS: [&str; 13] = [
    "pc", "tv", "os", "id", "ok", "ui", "ai", "cd", "dj", "pm", "vs", "ip", "mp",
];

fn words() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        // common words, plus product and tech names the list lacks
        include_str!("../../../data/english_words.txt")
            .lines()
            .chain(include_str!("../../../data/english_extra.txt").lines())
            .map(str::trim)
            .filter(|w| w.len() >= 3 || SHORT_WORDS.contains(w))
            .collect()
    })
}

/// Long product and tech names that win even where the keystrokes also
/// read as kana (`youtube` is also ようつべ).
fn names() -> &'static HashSet<&'static str> {
    static NAMES: OnceLock<HashSet<&'static str>> = OnceLock::new();
    NAMES.get_or_init(|| {
        include_str!("../../../data/english_extra.txt")
            .lines()
            .map(str::trim)
            .filter(|w| w.len() >= 6)
            .collect()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Piece {
    /// An English word, as typed.
    English(String),
    /// Keystrokes that read as kana.
    Kana(String),
}

/// Best split of `raw` (ASCII letters) into pieces, or `None` when it has no
/// English word or does not split at all. `kana` says whether a run of
/// keystrokes is complete kana on its own.
pub(super) fn split(
    raw: &str,
    kana: impl Fn(&str) -> bool,
    learned: impl Fn(&str) -> bool,
) -> Option<Vec<Piece>> {
    if raw.len() < MIN_WORD || !raw.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    // all kana already: nothing to restore
    if kana(&lower) {
        return None;
    }
    let n = raw.len();
    let words = words();

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
            // cubed, so one long word beats several short ones that merely
            // fit (windows, not now+in+dow+sd)
            let gain = if english { (end - i).pow(3) } else { 0 };
            let score = (next.score.0 + gain, next.score.1 - 1);
            if here.as_ref().is_none_or(|h| score > h.score) {
                *here = Some(Step {
                    score,
                    end,
                    english,
                });
            }
        };
        for end in (i + MIN_WORD)..=n {
            let run = &lower[i..end];
            let learned = learned(run);
            if (words.contains(run) || learned)
                && (!kana(run) || names().contains(run) || learned)
            {
                consider(end, true, &mut here);
            }
        }
        for end in (i + 1)..=n.min(i + MAX_KANA_RUN) {
            if kana(&lower[i..end]) {
                consider(end, false, &mut here);
            }
        }
        best[i] = here;
    }

    if best[0].as_ref()?.score.0 == 0 {
        return None;
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut i = 0;
    while i < n {
        let step = best[i].as_ref()?;
        if step.english {
            pieces.push(Piece::English(raw[i..step.end].to_string()));
        } else {
            match pieces.last_mut() {
                Some(Piece::Kana(run)) => run.push_str(&raw[i..step.end]),
                _ => pieces.push(Piece::Kana(raw[i..step.end].to_string())),
            }
        }
        i = step.end;
    }
    Some(pieces)
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
            || (!pending.is_empty()
                && self
                    .converters
                    .romaji
                    .pending_expansions(&pending)
                    .is_empty())
    }

    /// The kana a run of keystrokes spells, `None` when part of it cannot
    /// be kana. A last lone `n` counts as ん here: inside a longer word
    /// (`kensaku`) it is one, so `ken` must not pass for an English word.
    fn kana_of(&self, run: &str) -> Option<String> {
        let k = self.converters.romaji.convert_flush(run);
        let k = match k.strip_suffix('n') {
            Some(head) => format!("{head}ん"),
            None => k,
        };
        (!k.is_empty() && k.chars().all(|c| !c.is_ascii())).then_some(k)
    }

    /// Pieces for the keystrokes in `raw` (see [`split`]).
    fn english_pieces(&self, raw: &str) -> Option<Vec<Piece>> {
        split(
            raw,
            |run| self.kana_of(run).is_some(),
            |w| self.corrections.is_known(w),
        )
    }

    /// English words verbatim; kana runs through the model, each with the
    /// text before it as left context.
    fn assemble_pieces(&mut self, pieces: &[Piece]) -> String {
        let base = self.truncate_context_for_api();
        let mut out = String::new();
        for piece in pieces {
            match piece {
                Piece::English(w) => out.push_str(w),
                Piece::Kana(run) => {
                    let reading = self
                        .kana_of(run)
                        .unwrap_or_else(|| self.converters.romaji.convert_flush(run));
                    let lctx = self.lctx_for(&base, &out);
                    let converted = self.convert_chunk(&reading, &lctx);
                    out.push_str(&converted);
                }
            }
        }
        out
    }

    /// The English-mixed reading of what was typed, for the Space
    /// conversion, when the keystrokes are known and consistent with the
    /// reading.
    pub(super) fn english_mix_candidate(&mut self, reading: &str) -> Option<String> {
        if !self.config.english_mix
            || self.mode.current() != InputMode::Hiragana
            || self.raw_keys.is_empty()
        {
            return None;
        }
        let leftover = self.has_romaji_leftover("");
        let raw = self.raw_keys.clone();
        // Edits around the keystrokes (cursor moves, mid-buffer inserts)
        // leave raw_keys stale; only trust it when it reproduces the reading.
        if !self
            .converters
            .romaji
            .convert_flush(&raw)
            .eq_ignore_ascii_case(reading)
        {
            return None;
        }
        // A lone English word (tokyo) is offered even when it reads as kana.
        if raw.len() >= 4 && words().contains(raw.to_ascii_lowercase().as_str()) {
            return (raw != reading).then_some(raw);
        }
        if !leftover {
            return None;
        }
        let pieces = self.english_pieces(&raw)?;
        let text = self.assemble_pieces(&pieces);
        (text != reading).then_some(text)
    }

    /// Live conversion text with English words restored, for a model answer
    /// that still carries unresolved romaji (`ウィンドws`): the keystrokes
    /// before the pending tail, mixed. `None` when the answer is clean or
    /// the keystrokes do not make an English-mixed reading.
    pub(super) fn live_english(&mut self, converted: &str) -> Option<String> {
        if !self.config.english_mix
            || self.mode.current() != InputMode::Hiragana
            || self.raw_keys.is_empty()
        {
            return None;
        }
        let pending = self.input_buf.pending();
        let tail = self.converters.romaji.convert_flush(&pending);
        if !self.has_romaji_leftover(converted) {
            return None;
        }
        let romaji = &self.converters.romaji;
        if !romaji
            .convert_flush(&self.raw_keys)
            .eq_ignore_ascii_case(&self.input_buf.settled_reading(romaji))
        {
            return None;
        }
        // The caller appends `tail` (the pending keystrokes as typed) after
        // this text, so the mixed text must end with it. A partial last
        // syllable (`windowsd`) is carried over verbatim.
        let raw = self.raw_keys.clone();
        let n = raw.len();
        let (pieces, rest) = (0..=3.min(n))
            .filter(|k| raw.is_char_boundary(n - k))
            .find_map(|k| {
                let (head, rest) = raw.split_at(n - k);
                // What is carried over verbatim must be the pending keystrokes
                // themselves, never settled letters.
                if !rest.chars().all(|c| c.is_ascii_alphabetic()) || !tail.ends_with(rest) {
                    return None;
                }
                let pieces = self.english_pieces(head)?;
                // Cheap shape check before any model call: the text has to
                // end with the pending tail the caller will append (a kana
                // run counts as one non-ASCII char).
                let shape: String = pieces
                    .iter()
                    .map(|p| match p {
                        Piece::English(w) => w.as_str(),
                        Piece::Kana(_) => "\u{3042}",
                    })
                    .chain(std::iter::once(rest))
                    .collect();
                shape.ends_with(tail.as_str()).then(|| (pieces, rest.to_string()))
            })?;
        let text = format!("{}{}", self.assemble_pieces(&pieces), rest);
        text.strip_suffix(tail.as_str()).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_kana(run: &str) -> bool {
        let table = [
            "de", "ha", "wo", "ga", "da", "no", "ko", "he", "ni", "n", "u", "ho", "ka", "te",
        ];
        let mut rest = run;
        'outer: while !rest.is_empty() {
            for r in table {
                if let Some(t) = rest.strip_prefix(r) {
                    rest = t;
                    continue 'outer;
                }
            }
            return false;
        }
        !run.is_empty()
    }

    fn show(raw: &str) -> Option<String> {
        split(raw, is_kana, |_| false).map(|ps| {
            ps.iter()
                .map(|p| match p {
                    Piece::English(w) => format!("[{w}]"),
                    Piece::Kana(k) => format!("<{k}>"),
                })
                .collect()
        })
    }

    #[test]
    fn splits_word_and_kana() {
        assert_eq!(show("windowsdeha").as_deref(), Some("[windows]<deha>"));
        assert_eq!(
            show("konopcnowindowsdehenkan").as_deref(),
            Some("<kono>[pc]<no>[windows]<dehenkan>")
        );
    }

    #[test]
    fn kana_readable_runs_stay_kana() {
        // "ten" and "no" are English words but read as kana
        assert_eq!(show("konotenno"), None);
        assert_eq!(show("dehaga"), None);
    }

    #[test]
    fn keeps_typed_case() {
        assert_eq!(show("WindowsDeha").as_deref(), Some("[Windows]<Deha>"));
    }
}
