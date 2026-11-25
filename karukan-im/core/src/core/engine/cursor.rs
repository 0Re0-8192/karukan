//! Cursor movement and character deletion

use super::*;

impl InputMethodEngine {
    /// Common helper for cursor movement: clear live conversion and set the
    /// new display position. Nothing settles — unevaluated romaji stays
    /// live, so typing after a move can still combine with it. Moving does
    /// end a temporary alphabet word (the user left the word they were
    /// typing), so the next key is evaluated as romaji again.
    fn move_caret(&mut self, new_pos: impl FnOnce(&InputBuffer) -> usize) -> EngineResult {
        if self.mode.current() == InputMode::Alphabet {
            self.mode.exit_temporary();
        }
        self.live.shown = false;
        self.input_buf.set_cursor(new_pos(&self.input_buf));
        self.log_chunk_state("cursor");
        let preedit = self.set_composing_state();
        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::UpdateAuxText(self.format_aux_composing()))
    }

    /// Handle backspace in composing mode
    pub(super) fn backspace_composing(&mut self) -> EngineResult {
        // Remove one display character before the cursor: a single-character
        // element vanishes whole, re-exposing the live element before it
        // (`ytko` → BS → `o` → 「yと」); きょ is truncated per character.
        let reading_before = self.input_buf.reading();
        if !self.edit_with_chunk_breaks(|e| e.input_buf.backspace(&e.converters.romaji)) {
            // Nothing to delete
            return EngineResult::consumed();
        }
        self.raw_keys.pop();
        self.realign_raw_keys();

        if let Some(result) = self.try_reset_if_empty() {
            return result;
        }

        // Reading unchanged (a live keystroke was popped): keep the
        // candidate window as-is
        if self.input_buf.reading() == reading_before {
            let preedit = self.set_composing_state();
            return EngineResult::consumed()
                .with_action(EngineAction::UpdatePreedit(preedit))
                .with_action(EngineAction::UpdateAuxText(self.format_aux_composing()));
        }
        self.refresh_input_state()
    }

    /// After a character-wise Backspace the typed keys may no longer spell the reading
    /// (`kyo` → き leaves `ky`, which spells ky), and the typed-keys row would vanish.
    /// Keep the longest run of typed keys that still agrees with the reading and add the
    /// shortest keys that spell the rest (き → `ki`), so the row follows the Backspace.
    /// Only for the plain case (kana input, caret at the end, no chunk break, lowercase keys).
    fn realign_raw_keys(&mut self) {
        if self.mode.current() != InputMode::Hiragana
            || !self.chunk_breaks.is_empty()
            || self.raw_keys.is_empty()
            || self.input_buf.cursor() != self.input_buf.char_count()
            || !self
                .raw_keys
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-')
        {
            return;
        }
        let romaji = &self.converters.romaji;
        let target = self.input_buf.settled_reading(romaji);
        let spell = |k: &str| super::english_mix::with_decimal_points(romaji.convert_settle(k));
        if spell(&self.raw_keys).eq_ignore_ascii_case(&target) {
            return;
        }
        const KEYS: &[u8] = b"abcdefghijklmnopqrstuvwxyz-";
        let raw = self.raw_keys.clone();
        for k in (0..raw.len()).rev() {
            let head = &raw[..k];
            if !target.starts_with(&spell(head)) {
                continue;
            }
            if spell(head).eq_ignore_ascii_case(&target) {
                self.raw_keys = head.to_string();
                return;
            }
            // shortest tail (up to 3 keys) that completes the reading
            for len in 1..=3usize {
                for n in 0..KEYS.len().pow(len as u32) {
                    let tail: String = (0..len)
                        .map(|d| {
                            KEYS[(n / KEYS.len().pow((len - 1 - d) as u32)) % KEYS.len()] as char
                        })
                        .collect();
                    let cand = format!("{head}{tail}");
                    if spell(&cand).eq_ignore_ascii_case(&target) {
                        self.raw_keys = cand;
                        return;
                    }
                }
            }
        }
    }

    /// Move caret left within the composition
    pub(super) fn move_caret_left(&mut self) -> EngineResult {
        self.move_caret(|buf| buf.cursor().saturating_sub(1))
    }

    /// Move caret right within the composition
    pub(super) fn move_caret_right(&mut self) -> EngineResult {
        self.move_caret(|buf| buf.cursor() + 1)
    }

    /// Handle delete key in composing mode
    pub(super) fn delete_composing(&mut self) -> EngineResult {
        if !self.edit_with_chunk_breaks(|e| e.input_buf.delete_at_cursor(&e.converters.romaji)) {
            return EngineResult::consumed();
        }

        if let Some(result) = self.try_reset_if_empty() {
            return result;
        }

        self.refresh_input_state()
    }

    /// Move caret to start of input
    pub(super) fn move_caret_home(&mut self) -> EngineResult {
        self.move_caret(|_| 0)
    }

    /// Move caret to end of input
    pub(super) fn move_caret_end(&mut self) -> EngineResult {
        self.move_caret(|buf| buf.char_count())
    }
}
