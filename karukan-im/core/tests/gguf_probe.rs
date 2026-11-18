//! Does karukan's own llama.cpp load a candidate GGUF and decode with it?
//! `KARUKAN_PROBE_GGUF=a.gguf;b.gguf` (tokenizer.json beside each, or
//! `KARUKAN_PROBE_TOKENIZER`); skipped when unset. Prints load time and the
//! per-token cost of greedy generation on the CPU, jinen-v2-small as reference.
use std::time::Instant;

use karukan_engine::kanji::LlamaCppModel;

#[test]
fn probe() {
    let Ok(list) = std::env::var("KARUKAN_PROBE_GGUF") else {
        return;
    };
    for path in list.split(';').filter(|s| !s.is_empty()) {
        let gguf = std::path::Path::new(path);
        let tok = std::env::var("KARUKAN_PROBE_TOKENIZER")
            .map(Into::into)
            .unwrap_or_else(|_| gguf.with_file_name("tokenizer.json"));
        let t = Instant::now();
        let mut model = match LlamaCppModel::from_file(gguf, &tok) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("PROBE {path}: LOAD FAILED: {e}");
                continue;
            }
        };
        model.set_n_threads(4);
        let load = t.elapsed();
        let prompt = "今日は天気がいいので、公園で散歩をしてから図書館で本を借りて、夕方には家に帰って夕飯を作る予定です。";
        let tokens = model.tokenize(prompt).expect("tokenize");
        // warm up, then time several decodes of 16 new tokens
        let _ = model.generate(&tokens, 16, None);
        let t = Instant::now();
        let runs = 5;
        let mut produced = 0;
        for i in 0..runs {
            // a different tail each time so the persistent session cannot replay it
            let mut tk = tokens.clone();
            tk.truncate(tk.len() - (i % 3));
            produced += model.generate(&tk, 16, None).map(|o| o.len()).unwrap_or(0);
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / produced.max(1) as f64;
        eprintln!(
            "PROBE {path}: load {:.2}s, prompt {} tokens, {:.1} ms/new token ({produced} tokens)",
            load.as_secs_f64(),
            tokens.len(),
            per
        );
    }
}
