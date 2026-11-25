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

/// Top-level domains that mark `a.b` as a host name (`example.com`).
const LONG_TLDS: [&str; 9] = [
    "com", "net", "org", "dev", "app", "info", "biz", "edu", "gov",
];
/// Two-letter ones: also ordinary kana typing (`asu.ne` is あす。ね), so a host
/// name needs a label before them that cannot be kana (`example.jp`).
const SHORT_TLDS: [&str; 12] = [
    "jp", "io", "ai", "co", "tv", "us", "uk", "de", "fr", "cn", "kr", "me",
];

/// Byte ranges of `raw` that are host names: letters joined by single dots,
/// ending in a known top-level domain. `kana` says whether a run of keys
/// reads as kana on its own. A trailing dot or a half-typed domain
/// (`example.j`) is not one yet, so the word flips to English when the
/// domain is complete.
pub(super) fn host_names(raw: &str, kana: &impl Fn(&str) -> bool) -> Vec<(usize, usize)> {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        // a run of letters and dots, ending on a letter
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'.') {
            i += 1;
        }
        let mut end = i;
        while end > start && bytes[end - 1] == b'.' {
            end -= 1;
        }
        let run = raw[start..end].to_ascii_lowercase();
        let labels: Vec<&str> = run.split('.').collect();
        if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
            continue;
        }
        let tld = labels[labels.len() - 1];
        let before = &labels[..labels.len() - 1];
        let host = LONG_TLDS.contains(&tld)
            || (SHORT_TLDS.contains(&tld) && before.iter().any(|l| !kana(l)));
        if host {
            out.push((start, end));
        }
    }
    out
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

    // Score, compared in order: fewest common words, then the English gain,
    // then the fewest pieces. A common word is only there because the keys
    // around it cannot be kana, so the split needing the fewest of them wins;
    // without that a word would grab letters of the syllables beside it
    // (`ko|no` read as `k|ok|`, `a|ri|ma|su` as `a|rim|ass|u`), each such
    // fragment being a word that is not kana on its own — and always one word
    // more. Counting words, not letters, keeps a whole word whole where its
    // start also reads as kana (`return`, not `re|turn`). Product names and
    // learned words are free: they are meant to win where the keys also read
    // as kana. What follows a word in Japanese is a particle or する — が, を,
    // に, で, する — not a bare vowel, so a word leaving the kana a vowel start
    // gains less: 3·len² less, which is about what one more letter is worth
    // (len³ − (len−1)³), so `return|suru` beats `returns|uru` while
    // `international|ization` still beats `inter|nationalization`.
    #[derive(Clone)]
    struct Step {
        score: (i64, usize, i64),
        end: usize,
        english: bool,
    }
    let mut best: Vec<Option<Step>> = vec![None; n + 1];
    best[n] = Some(Step {
        score: (0, 0, 0),
        end: n,
        english: false,
    });
    for i in (0..n).rev() {
        let mut here: Option<Step> = None;
        let consider = |end: usize, english: bool, cost: i64, here: &mut Option<Step>| {
            let Some(next) = &best[end] else { return };
            // cubed, so one long word beats several short ones that merely
            // fit (windows, not now+in+dow+sd)
            let len = end - i;
            let vowel_after = english
                && end < n
                && !next.english
                && matches!(lower.as_bytes()[end], b'a' | b'i' | b'u' | b'e' | b'o');
            let gain = match (english, vowel_after) {
                (false, _) => 0,
                (true, false) => len.pow(3),
                (true, true) => len.pow(3).saturating_sub(3 * len.pow(2)).max(1),
            };
            let score = (next.score.0 - cost, next.score.1 + gain, next.score.2 - 1);
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
            let named = names().contains(run);
            if (words.contains(run) || learned) && (!kana(run) || named || learned) {
                let cost = if named || learned { 0 } else { 1 };
                consider(end, true, cost, &mut here);
            }
        }
        for end in (i + 1)..=n.min(i + MAX_KANA_RUN) {
            if kana(&lower[i..end]) {
                consider(end, false, 0, &mut here);
            }
        }
        best[i] = here;
    }

    if best[0].as_ref()?.score.1 == 0 {
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
        let k = self.converters.romaji.convert_settle(run);
        (!k.is_empty() && k.chars().all(|c| !c.is_ascii())).then_some(k)
    }

    /// Pieces for the keystrokes in `raw` (see [`split`]); host names such as
    /// `example.jp` are carried whole, dots included (a `.` would otherwise
    /// settle into `。` and cut the word in two).
    fn english_pieces(&self, raw: &str) -> Option<Vec<Piece>> {
        let kana = |run: &str| self.kana_of(run).is_some();
        let learned = |w: &str| self.corrections.is_known(w);
        let hosts = host_names(raw, &kana);
        if hosts.is_empty() {
            return split(raw, kana, learned);
        }
        let mut pieces: Vec<Piece> = Vec::new();
        let mut at = 0;
        let push_gap = |gap: &str, pieces: &mut Vec<Piece>| -> Option<()> {
            if gap.is_empty() {
                return Some(());
            }
            match split(gap, kana, learned) {
                Some(ps) => pieces.extend(ps),
                None if kana(gap) => pieces.push(Piece::Kana(gap.to_string())),
                None => return None,
            }
            Some(())
        };
        for (s, e) in hosts {
            push_gap(&raw[at..s], &mut pieces)?;
            pieces.push(Piece::English(raw[s..e].to_string()));
            at = e;
        }
        push_gap(&raw[at..], &mut pieces)?;
        // neighbouring kana runs are one run, as `split` leaves them
        let mut merged: Vec<Piece> = Vec::new();
        for p in pieces {
            match (merged.last_mut(), p) {
                (Some(Piece::Kana(a)), Piece::Kana(b)) => a.push_str(&b),
                (_, p) => merged.push(p),
            }
        }
        Some(merged)
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
                    let after_word = out.ends_with(|c: char| c.is_ascii_alphabetic());
                    if after_word {
                        out.push_str(&keep_typed_spelling(run, &converted));
                    } else {
                        out.push_str(&converted);
                    }
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
        if !with_decimal_points(self.converters.romaji.convert_settle(&raw))
            .eq_ignore_ascii_case(reading)
        {
            return None;
        }
        // A lone English word (tokyo) is offered even when it reads as kana,
        // and so is one the user spelled out before (repo).
        let lower = raw.to_ascii_lowercase();
        if (raw.len() >= 4 && words().contains(lower.as_str())) || self.corrections.is_known(&lower)
        {
            return (raw != reading).then_some(raw);
        }
        if !leftover {
            return None;
        }
        let pieces = self.english_pieces(&raw)?;
        let text = self.assemble_pieces(&pieces);
        (text != reading).then_some(text)
    }

    /// The keys as typed (`kaigi` for かいぎ), offered at the tail of the
    /// Space list like MS-IME's raw-input candidate. Only when the
    /// keystrokes are known and reproduce the reading.
    pub(super) fn typed_keys_candidate(&self, reading: &str) -> Option<String> {
        if self.mode.current() != InputMode::Hiragana
            || self.raw_keys.is_empty()
            || !self.raw_keys.chars().all(|c| c.is_ascii_graphic())
        {
            return None;
        }
        if !with_decimal_points(self.converters.romaji.convert_settle(&self.raw_keys))
            .eq_ignore_ascii_case(reading)
        {
            return None;
        }
        (self.raw_keys != reading).then(|| self.raw_keys.clone())
    }

    /// The keystrokes of the current composition as typed, for a frontend to
    /// show beside the candidates. Empty when unknown, or when edits around
    /// them (cursor moves, mid-buffer inserts) mean they no longer spell the
    /// composition. Keeps following Backspace-out-of-conversion and the like,
    /// which a frontend counting keys itself cannot see.
    pub fn typed_keys(&self) -> String {
        if self.raw_keys.is_empty() || !self.raw_keys.chars().all(|c| c.is_ascii_graphic()) {
            return String::new();
        }
        if self.mode.current() == InputMode::Hiragana {
            let romaji = &self.converters.romaji;
            if !with_decimal_points(romaji.convert_settle(&self.raw_keys))
                .eq_ignore_ascii_case(&self.input_buf.settled_reading(romaji))
            {
                return String::new();
            }
        }
        self.raw_keys.clone()
    }

    /// The typed keys as kana (`かいぎ` for `kaigi`), for the frontend's
    /// typed-keys row: the reading of what was typed, while the preedit shows its
    /// conversion. Direct input (alphabet, emoji) shows the keys themselves.
    pub fn typed_kana(&self) -> String {
        let keys = self.typed_keys();
        if keys.is_empty() || matches!(self.mode.current(), InputMode::Alphabet | InputMode::Emoji)
        {
            return keys;
        }
        // While typing a last `n` may still become な; once the composition
        // is converted it has settled into ん, like the reading behind the list.
        let romaji = &self.converters.romaji;
        let kana = if matches!(self.state, InputState::Composing { .. }) {
            romaji.convert_flush(&keys)
        } else {
            romaji.convert_settle(&keys)
        };
        with_decimal_points(kana)
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
            return self.learned_word_as_typed(&pending);
        }
        let romaji = &self.converters.romaji;
        if !with_decimal_points(romaji.convert_settle(&self.raw_keys))
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
                shape
                    .ends_with(tail.as_str())
                    .then(|| (pieces, rest.to_string()))
            })?;
        let text = format!("{}{}", self.assemble_pieces(&pieces), rest);
        text.strip_suffix(tail.as_str()).map(str::to_string)
    }

    /// A word the user spelled out before (picked as typed from the list,
    /// F9/F10) shows as typed while it is typed, even where it reads as
    /// kana (`repo`, not レポ), so Enter commits it as it is. Only the whole
    /// composition, with nothing pending.
    fn learned_word_as_typed(&self, pending: &str) -> Option<String> {
        let raw = &self.raw_keys;
        let word = raw.to_ascii_lowercase();
        if !pending.is_empty() || !self.corrections.is_known(&word) {
            return None;
        }
        let romaji = &self.converters.romaji;
        with_decimal_points(romaji.convert_settle(raw))
            .eq_ignore_ascii_case(&self.input_buf.settled_reading(romaji))
            .then(|| raw.clone())
    }
}

/// A word the list lacks (`internationalization`) splits into a word it has
/// and a kana run (`international` + いざちおん), which the model, reading the
/// word as context, may answer in English — spelled its own way
/// (`isation`). Where the answer opens with letters that are the typed ones
/// give or take two, the typed spelling is kept; the rest of the answer
/// (`は難しい`) stays the model's.
fn keep_typed_spelling(run: &str, converted: &str) -> String {
    let letters = converted
        .bytes()
        .take_while(|b| b.is_ascii_alphabetic())
        .count();
    if letters < 3 || run.len() < letters || !run.is_char_boundary(letters) {
        return converted.to_string();
    }
    let typed = &run[..letters];
    let differ = typed
        .bytes()
        .zip(converted.bytes())
        .filter(|(a, b)| !a.eq_ignore_ascii_case(b))
        .count();
    if differ > 2 {
        return converted.to_string();
    }
    format!("{typed}{}", &converted[letters..])
}

/// What the keys read as, with a `.` between digits left a point (`1.5`) —
/// the romaji rules alone read it as `。`, which the composition no longer
/// holds once the digit after it has been typed.
pub(super) fn with_decimal_points(reading: String) -> String {
    let chars: Vec<char> = reading.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let between = i > 0
                && i + 1 < chars.len()
                && chars[i - 1].is_ascii_digit()
                && chars[i + 1].is_ascii_digit();
            if c == '。' && between { '.' } else { c }
        })
        .collect()
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
    fn a_word_never_takes_letters_from_the_syllables_beside_it() {
        // `ok` fits k|ok|... and `rim`/`ass` fit a|rim|ass|u, and none of
        // them is kana on its own; the kana around `null` must still win
        let is_kana = |run: &str| {
            let table = [
                "ko", "no", "wo", "a", "ri", "ma", "su", "ha", "ka", "n", "u",
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
        };
        let show = |raw: &str| {
            split(raw, is_kana, |_| false).map(|ps| {
                ps.iter()
                    .map(|p| match p {
                        Piece::English(w) => format!("[{w}]"),
                        Piece::Kana(k) => format!("<{k}>"),
                    })
                    .collect::<String>()
            })
        };
        assert_eq!(
            show("kononullwoarimasu").as_deref(),
            Some("<kono>[null]<woarimasu>")
        );
        assert_eq!(show("konokanhanull").as_deref(), Some("<konokanha>[null]"));
    }

    #[test]
    fn a_whole_word_stays_whole_where_its_start_reads_as_kana() {
        // `re`/`to`/`u` are kana, `turn`/`day`/`pdate` are not: still one word
        let is_kana = |run: &str| {
            let table = ["re", "to", "u", "su", "ru", "ha", "wo"];
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
        };
        let show = |raw: &str| {
            split(raw, is_kana, |_| false).map(|ps| {
                ps.iter()
                    .map(|p| match p {
                        Piece::English(w) => format!("[{w}]"),
                        Piece::Kana(k) => format!("<{k}>"),
                    })
                    .collect::<String>()
            })
        };
        assert_eq!(show("returnha").as_deref(), Some("[return]<ha>"));
        assert_eq!(show("todayha").as_deref(), Some("[today]<ha>"));
        assert_eq!(show("updatewo").as_deref(), Some("[update]<wo>"));
    }

    #[test]
    fn a_word_leaves_the_kana_a_consonant_start() {
        // returns|uru and return|suru both fit; Japanese goes on with する
        let is_kana = |run: &str| {
            let table = ["su", "ru", "u", "ha", "wo"];
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
        };
        let pieces = split("returnsuru", is_kana, |_| false).unwrap();
        assert_eq!(
            pieces,
            vec![Piece::English("return".into()), Piece::Kana("suru".into())]
        );
    }

    #[test]
    fn the_typed_spelling_survives_the_model() {
        assert_eq!(
            keep_typed_spelling("izationhamuzukashii", "isationは難しい"),
            "izationは難しい"
        );
        // a real conversion is left alone
        assert_eq!(keep_typed_spelling("dehenkan", "で変換"), "で変換");
        // letters that are not the typed ones are not a respelling
        assert_eq!(keep_typed_spelling("wosuru", "update"), "update");
    }

    #[test]
    fn keeps_typed_case() {
        assert_eq!(show("WindowsDeha").as_deref(), Some("[Windows]<Deha>"));
    }
}
