//! Renders the candidate window offscreen to a raw file for visual checks.
//! `KARUKAN_PREVIEW_DIR` must be set; skipped otherwise.
#![cfg(windows)]

use karukan_tsf::engine::CandidateItem;

fn item(t: &str, d: Option<&str>) -> CandidateItem {
    CandidateItem {
        text: t.to_string(),
        description: d.map(str::to_string),
    }
}

#[test]
fn render_previews() {
    let Some(dir) = std::env::var_os("KARUKAN_PREVIEW_DIR") else {
        return;
    };
    let items = vec![
        item("画像", None),
        item("2", Some("全")),
        item("2", Some("半")),
        item("②", Some("環境依存")),
        item("II", Some("環境依存")),
        item("ガゾウ", None),
        item("がぞう", None),
        item("ｶﾞｿﾞｳ", Some("半角カタカナ")),
        item("画像を", None),
    ];
    for (name, dark, scale) in [
        ("dark", true, 1.0f32),
        ("light", false, 1.0),
        ("dark150", true, 1.5),
    ] {
        let (w, h, buf) = karukan_tsf::__render_candidate_preview(
            &items,
            1,
            0,
            3,
            Some("[あ] かるかん (jinen-v2-small-q8)".to_string()),
            Some("kyarameru".to_string()),
            dark,
            scale,
        );
        let mut out = Vec::new();
        out.extend_from_slice(&(w as u32).to_le_bytes());
        out.extend_from_slice(&(h as u32).to_le_bytes());
        out.extend_from_slice(&buf);
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.raw")), out).unwrap();
    }
}
