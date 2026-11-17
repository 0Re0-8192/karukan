//! English words typed as plain romaji ride along as a mixed candidate.

use super::*;

fn type_str(engine: &mut InputMethodEngine, s: &str) {
    for c in s.chars() {
        engine.process_key(&press(c));
    }
}

fn candidates_after_space(engine: &mut InputMethodEngine) -> Vec<String> {
    engine.process_key(&press(' '));
    match engine.state() {
        InputState::Conversion { candidates, .. } => {
            candidates.candidates().iter().map(|c| c.text.clone()).collect()
        }
        other => panic!("not converting: {other:?}"),
    }
}

#[test]
fn word_plus_kana_is_offered() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "windowsdeha");
    let list = candidates_after_space(&mut engine);
    assert!(list.iter().any(|t| t == "windowsでは"), "{list:?}");
}

#[test]
fn plain_japanese_gets_no_mixed_candidate() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "kyouhatenkiga");
    let list = candidates_after_space(&mut engine);
    // the typed keys themselves are the one ASCII candidate
    assert!(
        list.iter().all(|t| t == "kyouhatenkiga" || t.chars().all(|c| !c.is_ascii_alphabetic())),
        "{list:?}"
    );
}

#[test]
fn typed_keys_sit_behind_the_kana_pair() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "kaigi");
    let list = candidates_after_space(&mut engine);
    let pos = |s: &str| list.iter().position(|t| t == s);
    let (kata, typed) = (pos("カイギ"), pos("kaigi"));
    assert!(typed.is_some(), "{list:?}");
    assert!(kata < typed, "{list:?}");
}

#[test]
fn typed_keys_are_not_offered_after_the_buffer_is_edited_around_them() {
    // a cursor move leaves raw_keys stale; it is only trusted when it
    // reproduces the reading
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "kaigi");
    engine.process_key(&press_key(Keysym::LEFT));
    type_str(&mut engine, "ka");
    let list = candidates_after_space(&mut engine);
    assert!(!list.iter().any(|t| t.starts_with("kai")), "{list:?}");
}

#[test]
fn typed_keys_survive_backing_out_of_conversion() {
    // a frontend counting keys would pop one letter here: Backspace in the
    // candidate list only cancels the conversion
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "kaigi");
    assert_eq!(engine.typed_keys(), "kaigi");
    engine.process_key(&press(' '));
    assert_eq!(engine.typed_keys(), "kaigi");
    engine.process_key(&press_key(Keysym::BACKSPACE));
    assert_eq!(engine.preedit().unwrap().text(), "かいぎ");
    assert_eq!(engine.typed_keys(), "kaigi");
}

#[test]
fn typed_keys_are_dropped_once_edits_leave_them_stale() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "kaigi");
    engine.process_key(&press_key(Keysym::LEFT));
    type_str(&mut engine, "x");
    assert_eq!(engine.typed_keys(), "");
    engine.process_key(&press_key(Keysym::ESCAPE));
    assert_eq!(engine.typed_keys(), "");
}

#[test]
fn valid_romaji_that_spells_english_words_is_not_hijacked() {
    // ohayou / genkidesuka contain English words (you, gen…) but are
    // perfectly good kana typing.
    for typed in ["ohayou", "genkidesuka", "tabemashita"] {
        let mut engine = InputMethodEngine::new();
        engine.converters.kanji = None;
        type_str(&mut engine, typed);
        let list = candidates_after_space(&mut engine);
        assert!(
            list.iter().all(|t| t == typed || t.chars().all(|c| !c.is_ascii_alphabetic())),
            "{typed}: {list:?}"
        );
    }
}

#[test]
fn kana_continue_after_an_all_caps_word() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "PCno");
    assert_eq!(engine.preedit().unwrap().text(), "PCの");
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
}

#[test]
fn a_single_capital_word_stays_english() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    type_str(&mut engine, "Linuxno");
    assert_eq!(engine.preedit().unwrap().text(), "Linuxno");
}

#[test]
fn can_be_switched_off() {
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.config.english_mix = false;
    type_str(&mut engine, "windowsdeha");
    let list = candidates_after_space(&mut engine);
    assert!(list.iter().all(|t| t != "windowsでは"), "{list:?}");
}
