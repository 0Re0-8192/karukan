//! MS-IME style script conversion keys: F6 (ひらがな), F7 (全角カタカナ),
//! F8 (半角カタカナ), F9 (全角英数), F10 (半角英数) and 無変換 (cycle
//! カタカナ → 半角カタカナ → ひらがな).
//!
//! Each opens a Conversion state whose candidates are the forms of that
//! script; pressing the same key again steps through them (F9/F10:
//! lower → UPPER → Capitalized), Enter commits, Escape returns to the
//! composition. Nothing is recorded in the learning cache (candidates carry
//! no reading), so a half-width alphanumeric never pollutes later
//! conversions.

use karukan_engine::kana::{hiragana_to_katakana, katakana_to_half_width};
use karukan_engine::width::to_full_width;

use super::*;

/// F6-F10 and 無変換: the keys that take part in the script cycle.
pub(super) fn is_script_key(sym: Keysym) -> bool {
    matches!(
        sym,
        Keysym::F6 | Keysym::F7 | Keysym::F8 | Keysym::F9 | Keysym::F10 | Keysym::MUHENKAN
    )
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// The three casings F9/F10 cycle through.
fn case_cycle(raw: &str) -> Vec<String> {
    let lower = raw.to_lowercase();
    let upper = raw.to_uppercase();
    let cap = capitalize(raw);
    let mut forms = vec![lower, upper, cap];
    forms.dedup();
    forms
}

impl InputMethodEngine {
    /// What was typed, or the reading when the keystrokes are unknown
    /// (e.g. the buffer was edited around them).
    fn raw_or_reading(&self, reading: &str) -> String {
        if self.raw_keys.is_empty() {
            reading.to_string()
        } else {
            self.raw_keys.clone()
        }
    }

    /// `key`: 6..=10 for F6..F10, 0 for 無変換.
    pub(super) fn script_convert(&mut self, key: u8) -> EngineResult {
        let reading = self.input_buf.settled_reading(&self.converters.romaji);
        if reading.is_empty() {
            return EngineResult::consumed();
        }
        let hira = reading.clone();
        let kata = hiragana_to_katakana(&hira);
        let half_kata = katakana_to_half_width(&kata);
        let forms: Vec<String> = match key {
            6 => vec![hira],
            7 => vec![kata],
            8 => vec![half_kata],
            9 => case_cycle(&self.raw_or_reading(&reading))
                .iter()
                .map(|s| to_full_width(s))
                .collect(),
            10 => case_cycle(&self.raw_or_reading(&reading)),
            _ => vec![kata, half_kata, hira],
        };

        let idx = match (self.fkey_cycle, &self.state) {
            (Some((k, i)), InputState::Conversion { .. }) if k == key => (i + 1) % forms.len(),
            _ => 0,
        };
        self.fkey_cycle = Some((key, idx));

        let single = forms.len() == 1;
        let list = CandidateList::new(forms.into_iter().map(Candidate::new).collect());
        let mut list = list;
        list.set_cursor(idx);
        let mut result = self.enter_conversion_state(&reading, list);
        if single {
            result
                .actions
                .retain(|a| !matches!(a, EngineAction::ShowCandidates(_)));
            result.actions.push(EngineAction::HideCandidates);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casings() {
        assert_eq!(case_cycle("kyou"), vec!["kyou", "KYOU", "Kyou"]);
        assert_eq!(case_cycle("a"), vec!["a", "A"]);
    }

    #[test]
    fn script_keys() {
        assert!(is_script_key(Keysym::F7));
        assert!(is_script_key(Keysym::MUHENKAN));
        assert!(!is_script_key(Keysym::SPACE));
    }
}
