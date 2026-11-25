//! A word registered while the server runs is read in place.

use super::*;

fn surfaces(engine: &InputMethodEngine, reading: &str) -> Vec<String> {
    engine
        .dicts
        .user
        .as_ref()
        .and_then(|d| d.exact_match_search(reading))
        .map(|r| r.candidates.iter().map(|c| c.surface.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn a_word_added_to_the_directory_is_read_without_a_restart() {
    let dir = std::env::temp_dir().join(format!("karukan-udict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // no directory yet: the first registration creates it
    let mut engine = InputMethodEngine::new();
    engine.load_user_dictionaries_from(&dir);
    assert!(surfaces(&engine, "かるかん").is_empty());

    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registered.txt"), "かるかん\tkarukan\t名詞\t\n").unwrap();
    engine.reload_user_dictionaries_in(&dir);
    assert_eq!(surfaces(&engine, "かるかん"), vec!["karukan"]);

    // another word: looked at again only after a pause
    std::fs::write(
        dir.join("registered.txt"),
        "かるかん\tkarukan\t名詞\t\nてすと\tテスト語\t名詞\t\n",
    )
    .unwrap();
    engine.reload_user_dictionaries_in(&dir);
    assert!(surfaces(&engine, "てすと").is_empty());
    engine.dicts.user_checked = None;
    engine.reload_user_dictionaries_in(&dir);
    assert_eq!(surfaces(&engine, "てすと"), vec!["テスト語"]);

    // another engine (another app) finds it in the cache
    let mut other = InputMethodEngine::new();
    other.load_user_dictionaries_from(&dir);
    assert_eq!(surfaces(&other, "てすと"), vec!["テスト語"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_engine_that_never_read_them_ignores_the_directory() {
    let dir = std::env::temp_dir().join(format!("karukan-udict-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.txt"), "かるかん\tkarukan\t名詞\t\n").unwrap();
    let mut engine = InputMethodEngine::new();
    engine.reload_user_dictionaries_in(&dir);
    assert!(engine.dicts.user.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_word_ending_in_n_is_an_exact_match_when_typed_with_one_n() {
    // karukan + Space: the lone n settles into ん, and the registered
    // かるかん leads like any exact match, not as a prediction from かるか + n
    let dir = std::env::temp_dir().join(format!("karukan-udict-n-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registered.txt"), "かるかん\tKarukanReg\t名詞\t\n").unwrap();
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.load_user_dictionaries_from(&dir);
    for c in "karukan".chars() {
        engine.process_key(&press(c));
    }
    engine.process_key(&press_key(Keysym::SPACE));
    let list: Vec<String> = engine
        .state()
        .candidates()
        .unwrap()
        .candidates()
        .iter()
        .map(|c| format!("{}:{:?}", c.text, c.source))
        .collect();
    assert!(list[0].starts_with("KarukanReg"), "{list:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
