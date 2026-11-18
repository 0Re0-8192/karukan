//! AJIMEE-Bench through karukan's own inference path (GGUF, greedy), for comparing a
//! candidate model with jinen-v2-small under one protocol.
//!
//! `KARUKAN_AJIMEE_GGUF=a.gguf;b.gguf` (tokenizer.json beside each) and
//! `KARUKAN_AJIMEE_ITEMS=evaluation_items.json`; optional `KARUKAN_AJIMEE_CTX=N` keeps only the
//! last N chars of context_text (default: all). Skipped when unset.
use std::time::Instant;

use karukan_engine::kana::normalize_nfkc;
use karukan_engine::kanji::{LlamaCppModel, build_jinen_prompt, clean_model_output};

#[test]
fn ajimee() {
    let (Ok(list), Ok(items)) = (
        std::env::var("KARUKAN_AJIMEE_GGUF"),
        std::env::var("KARUKAN_AJIMEE_ITEMS"),
    ) else {
        return;
    };
    let keep: Option<usize> = std::env::var("KARUKAN_AJIMEE_CTX").ok().and_then(|s| s.parse().ok());
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(items).unwrap()).unwrap();
    for path in list.split(';').filter(|s| !s.is_empty()) {
        let gguf = std::path::Path::new(path);
        let mut model = LlamaCppModel::from_file(gguf, gguf.with_file_name("tokenizer.json")).unwrap();
        model.set_n_threads(4);
        let eos = model.eos_token_id().0;
        let (mut ok, mut n, mut ms, mut shown) = (0, 0, 0.0, 0);
        for it in &items {
            let input = it["input"].as_str().unwrap_or("");
            if input.is_empty() {
                continue;
            }
            let ctx_full = it["context_text"].as_str().unwrap_or("");
            let ctx: String = match keep {
                Some(k) => {
                    let c: Vec<char> = ctx_full.chars().collect();
                    c[c.len().saturating_sub(k)..].iter().collect()
                }
                None => ctx_full.to_string(),
            };
            let toks = model.tokenize(&build_jinen_prompt(input, &ctx)).unwrap();
            let t = Instant::now();
            let out = model.generate(&toks, 100, Some(eos)).unwrap();
            ms += t.elapsed().as_secs_f64() * 1000.0;
            let text = clean_model_output(&model.decode(&out[toks.len()..], true).unwrap());
            let hit = it["expected_output"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| normalize_nfkc(e.as_str().unwrap()) == normalize_nfkc(&text));
            n += 1;
            ok += hit as usize;
            if !hit && shown < 12 {
                shown += 1;
                report(&format!("  MISS {input} | got {text} | want {}", it["expected_output"][0]));
            }
        }
        report(&format!(
            "AJIMEE {path}: {:.1}% ({ok}/{n}), {:.1} ms/item, ctx={keep:?}",
            100.0 * ok as f64 / n as f64,
            ms / n as f64
        ));
    }
}

/// Printed, and appended to `KARUKAN_AJIMEE_OUT` as UTF-8 when set (a Windows console
/// pipe garbles Japanese).
fn report(line: &str) {
    eprintln!("{line}");
    if let Ok(p) = std::env::var("KARUKAN_AJIMEE_OUT") {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{line}");
        }
    }
}
