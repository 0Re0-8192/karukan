//! Which part of a reading each part of its conversion spells, so the caret
//! can step into a live conversion and turn back to kana only the word it
//! is in (今日は|雨 → 今日はあ|め) instead of the whole composition.
//!
//! The model says nothing about that, but its answer keeps the reading's
//! kana where it did not convert them (the は of 今日は雨), and those pin the
//! rest down: the converted runs between them (今日, 雨) take whatever
//! reading lies between the same kana there (きょう, あめ).

use karukan_engine::{katakana_to_hiragana, normalize_nfkc};

/// One piece of a conversion: `reading` chars of the reading, spelled as
/// `surface`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Segment {
    pub reading: usize,
    pub surface: String,
}

/// A surface char that stands for itself in the reading: the same char, its
/// katakana or width twin (キ for き, ，for 、 after the width rules).
fn same(surface: char, reading: char) -> bool {
    if surface == reading || surface.eq_ignore_ascii_case(&reading) {
        return true;
    }
    let s = surface.to_string();
    let r = reading.to_string();
    katakana_to_hiragana(&s) == r || normalize_nfkc(&s) == normalize_nfkc(&r)
}

fn is_hiragana(c: char) -> bool {
    ('\u{3041}'..='\u{309f}').contains(&c)
}

/// Whether `c` can be part of a converted run: kanji, katakana (ー and 々
/// included), letters and digits. Hiragana are the reading's own, and
/// punctuation and spaces are typed as they are, so both end a run — the 、
/// in 中国、韓国 keeps them two words.
fn in_run(c: char) -> bool {
    c.is_alphanumeric() && !is_hiragana(c)
}

/// Split `surface` (the conversion of `reading`) into segments, each a char
/// standing for itself or a run of converted chars with the reading it
/// spells. A hiragana char or a mark in the surface is never part of a
/// converted run: those are the reading's own, and they pin where a run ends. `None` when
/// no such split exists (the model rewrote kana, or answered in another
/// script altogether).
pub(super) fn align(reading: &str, surface: &str) -> Option<Vec<Segment>> {
    let r: Vec<char> = reading.chars().collect();
    let s: Vec<char> = surface.chars().collect();
    let (n, m) = (r.len(), s.len());
    // the converted run starting at each surface position: up to the next
    // hiragana char or mark
    let run_end: Vec<usize> = (0..m)
        .map(|j| {
            let mut e = j;
            while e < m && in_run(s[e]) {
                e += 1;
            }
            e
        })
        .collect();
    // best[i][j]: fewest converted runs aligning r[i..] with s[j..]
    const NONE: usize = usize::MAX;
    let mut best = vec![vec![NONE; m + 1]; n + 1];
    // how: 0 = a char standing for itself, k > 0 = a run taking k reading chars
    let mut how = vec![vec![0usize; m + 1]; n + 1];
    best[n][m] = 0;
    for i in (0..=n).rev() {
        for j in (0..=m).rev() {
            if i == n && j == m {
                continue;
            }
            let mut b = NONE;
            let mut h = 0;
            if i < n && j < m && same(s[j], r[i]) && best[i + 1][j + 1] != NONE {
                b = best[i + 1][j + 1];
            }
            if j < m && run_end[j] > j {
                let e = run_end[j];
                // the fewest reading chars first: a later, equal split would
                // only move the boundary into the next run
                for k in 1..=(n - i) {
                    let rest = best[i + k][e];
                    if rest != NONE && rest + 1 < b {
                        b = rest + 1;
                        h = k;
                    }
                }
            }
            best[i][j] = b;
            how[i][j] = h;
        }
    }
    if best[0][0] == NONE {
        return None;
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        match how[i][j] {
            0 => {
                out.push(Segment {
                    reading: 1,
                    surface: s[j].to_string(),
                });
                i += 1;
                j += 1;
            }
            k => {
                let e = run_end[j];
                out.push(Segment {
                    reading: k,
                    surface: s[j..e].iter().collect(),
                });
                i += k;
                j = e;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(reading: &str, surface: &str) -> Option<String> {
        let segs = align(reading, surface)?;
        let r: Vec<char> = reading.chars().collect();
        let mut at = 0;
        Some(
            segs.iter()
                .map(|s| {
                    let piece: String = r[at..at + s.reading].iter().collect();
                    at += s.reading;
                    let one = s.reading == 1
                        && s.surface.chars().count() == 1
                        && same(s.surface.chars().next().unwrap(), r[at - 1]);
                    if piece == s.surface || one {
                        s.surface.clone()
                    } else {
                        format!("[{}={}]", s.surface, piece)
                    }
                })
                .collect(),
        )
    }

    #[test]
    fn kana_left_as_they_were_pin_the_words_between() {
        assert_eq!(
            show("きょうはあめ", "今日は雨").as_deref(),
            Some("[今日=きょう]は[雨=あめ]")
        );
        assert_eq!(
            show(
                "きしゃのきしゃがきしゃできしゃした",
                "貴社の記者が汽車で帰社した"
            )
            .as_deref(),
            Some("[貴社=きしゃ]の[記者=きしゃ]が[汽車=きしゃ]で[帰社=きしゃ]した")
        );
        assert_eq!(
            show("とうきょうえきまで", "東京駅まで").as_deref(),
            Some("[東京駅=とうきょうえき]まで")
        );
    }

    #[test]
    fn marks_part_words_too() {
        assert_eq!(
            show("ちゅうごく、かんこくがある", "中国、韓国がある").as_deref(),
            Some("[中国=ちゅうごく]、[韓国=かんこく]がある")
        );
        assert_eq!(
            show("あめ。ゆき", "雨。雪").as_deref(),
            Some("[雨=あめ]。[雪=ゆき]")
        );
    }

    #[test]
    fn katakana_and_width_twins_stand_for_themselves() {
        assert_eq!(
            show("こーひーを", "コーヒーを").as_deref(),
            Some("コーヒーを")
        );
        assert_eq!(show("pcの", "PCの").as_deref(), Some("PCの"));
    }

    #[test]
    fn a_rewritten_kana_has_no_alignment() {
        // the model dropped the は the reading has
        assert_eq!(
            show("きょうはあめ", "今日雨"),
            Some("[今日雨=きょうはあめ]".into())
        );
        // and answered kana the reading lacks
        assert_eq!(show("あめ", "あまい"), None);
    }
}
