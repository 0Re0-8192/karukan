use super::*;

fn key(keysym: Keysym) -> KeyEvent {
    KeyEvent {
        keysym,
        modifiers: KeyModifiers::default(),
        is_press: true,
    }
}

fn type_str(engine: &mut InputMethodEngine, s: &str) {
    for c in s.chars() {
        engine.process_key(&press(c));
    }
}

fn commit_text(result: &EngineResult) -> Option<String> {
    result.actions.iter().find_map(|a| match a {
        EngineAction::Commit(t) => Some(t.clone()),
        _ => None,
    })
}

#[test]
fn f7_converts_to_full_katakana_and_commits() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    let r = engine.process_key(&key(Keysym::F7));
    assert!(r.consumed);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    assert_eq!(engine.preedit().unwrap().text(), "キョウ");
    let r = engine.process_key(&key(Keysym::RETURN));
    assert_eq!(commit_text(&r).as_deref(), Some("キョウ"));
}

#[test]
fn f6_returns_hiragana() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::F7));
    engine.process_key(&key(Keysym::F6));
    assert_eq!(engine.preedit().unwrap().text(), "きょう");
}

#[test]
fn f8_half_width_katakana() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::F8));
    assert_eq!(engine.preedit().unwrap().text(), "ｷｮｳ");
}

#[test]
fn f9_cycles_full_width_alnum() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::F9));
    assert_eq!(engine.preedit().unwrap().text(), "ｋｙｏｕ");
    engine.process_key(&key(Keysym::F9));
    assert_eq!(engine.preedit().unwrap().text(), "ＫＹＯＵ");
    engine.process_key(&key(Keysym::F9));
    assert_eq!(engine.preedit().unwrap().text(), "Ｋｙｏｕ");
    engine.process_key(&key(Keysym::F9));
    assert_eq!(engine.preedit().unwrap().text(), "ｋｙｏｕ");
}

#[test]
fn f10_half_width_alnum_commits_without_learning_key() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::F10));
    assert_eq!(engine.preedit().unwrap().text(), "kyou");
    let r = engine.process_key(&key(Keysym::RETURN));
    assert_eq!(commit_text(&r).as_deref(), Some("kyou"));
}

#[test]
fn muhenkan_cycles_katakana_halfwidth_hiragana() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::MUHENKAN));
    assert_eq!(engine.preedit().unwrap().text(), "キョウ");
    engine.process_key(&key(Keysym::MUHENKAN));
    assert_eq!(engine.preedit().unwrap().text(), "ｷｮｳ");
    engine.process_key(&key(Keysym::MUHENKAN));
    assert_eq!(engine.preedit().unwrap().text(), "きょう");
}

#[test]
fn escape_returns_to_composition() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kyou");
    engine.process_key(&key(Keysym::F7));
    engine.process_key(&key(Keysym::ESCAPE));
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(engine.preedit().unwrap().text(), "きょう");
}

#[test]
fn function_keys_ignored_when_empty() {
    let mut engine = InputMethodEngine::new();
    let r = engine.process_key(&key(Keysym::F7));
    assert!(!r.consumed);
}

#[test]
fn a_last_n_settles_into_n_kana_for_the_script_keys() {
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "kan");
    engine.process_key(&key(Keysym::F7));
    assert_eq!(engine.preedit().unwrap().text(), "カン");
    engine.process_key(&key(Keysym::F6));
    assert_eq!(engine.preedit().unwrap().text(), "かん");
    // F10 is the keys as typed
    engine.process_key(&key(Keysym::F10));
    assert_eq!(engine.preedit().unwrap().text(), "kan");
}

#[test]
fn f10_never_offers_the_same_form_twice() {
    // `/e` capitalized is `/e` again
    let mut engine = InputMethodEngine::new();
    type_str(&mut engine, "/e");
    engine.process_key(&key(Keysym::F10));
    let texts: Vec<String> = engine
        .state()
        .candidates()
        .unwrap()
        .candidates()
        .iter()
        .map(|c| c.text.clone())
        .collect();
    let mut unique = texts.clone();
    unique.dedup();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), texts.len(), "{texts:?}");
}
