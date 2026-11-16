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
    assert!(list.iter().all(|t| t.chars().all(|c| !c.is_ascii_alphabetic())), "{list:?}");
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
            list.iter().all(|t| t.chars().all(|c| !c.is_ascii_alphabetic())),
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
