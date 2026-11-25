//! Renders candidate windows for recorded states (a JSON array written by an
//! input sweep) offscreen, one raw RGBA-ish buffer per state, for visual checks.
//! `KARUKAN_PREVIEW_STATES` (the JSON file) and `KARUKAN_PREVIEW_DIR` must be
//! set; skipped otherwise.
//!
//! Each state: `{ "name", "items": [{ "text", "description"? }], "cursor",
//! "page", "total_pages", "aux"?, "typed"?, "dark"?, "scale"? }`.
#![cfg(windows)]

use karukan_tsf::engine::CandidateItem;
use serde::Deserialize;

#[derive(Deserialize)]
struct Item {
    text: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct State {
    name: String,
    items: Vec<Item>,
    #[serde(default)]
    cursor: usize,
    #[serde(default)]
    page: usize,
    #[serde(default = "one")]
    total_pages: usize,
    aux: Option<String>,
    typed: Option<String>,
    #[serde(default)]
    dark: bool,
    #[serde(default = "unit")]
    scale: f32,
}

fn one() -> usize {
    1
}
fn unit() -> f32 {
    1.0
}

#[test]
fn render_recorded_states() {
    let (Some(file), Some(dir)) = (
        std::env::var_os("KARUKAN_PREVIEW_STATES"),
        std::env::var_os("KARUKAN_PREVIEW_DIR"),
    ) else {
        return;
    };
    let states: Vec<State> = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    for s in states {
        let items: Vec<CandidateItem> = s
            .items
            .into_iter()
            .map(|i| CandidateItem {
                text: i.text,
                description: i.description,
            })
            .collect();
        let (w, h, buf) = karukan_tsf::__render_candidate_preview(
            &items,
            s.cursor,
            s.page,
            s.total_pages,
            s.aux.filter(|a| !a.is_empty()),
            s.typed.filter(|t| !t.is_empty()),
            s.dark,
            s.scale,
        );
        let mut out = Vec::new();
        out.extend_from_slice(&(w as u32).to_le_bytes());
        out.extend_from_slice(&(h as u32).to_le_bytes());
        out.extend_from_slice(&buf);
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{}.raw", s.name)),
            out,
        )
        .unwrap();
    }
}
