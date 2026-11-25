//! Follow a flick composition with the real engine: `flick <reading>...` hands each reading to the
//! bridge as if Mozc had composed it, prints the candidates, then taps the first (KARUKAN_* env as usual).
use karukan_mozc::bridge::{Bridge, ImBackend};
use karukan_mozc::proto::{self, W};
use std::time::{Duration, Instant};

fn input(f: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, f);
    w.0
}

fn mozc_reply(reading: &str) -> Vec<u8> {
    let mut o = W::new();
    o.uint(1, 1);
    o.bool(3, true);
    o.message(5, |p| {
        p.uint(1, reading.chars().count() as u64);
        p.group(2, |g| {
            g.uint(3, 1);
            g.string(4, reading);
            g.uint(5, reading.chars().count() as u64);
        });
    });
    o.message(13, |s| {
        s.bool(1, true);
        s.uint(2, 1);
    });
    let mut w = W::new();
    w.bytes(1, &[0x08, 0x03]);
    w.bytes(2, &o.0);
    w.0
}

/// (id, value) of a reply's `all_candidate_words`, and its result.
fn show(reply: &[u8]) -> (Vec<(i64, String)>, Option<String>) {
    let top = proto::parse(reply).unwrap();
    let o = proto::parse(proto::get(&top, 2).unwrap().as_bytes().unwrap()).unwrap();
    let result = proto::get(&o, 4)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
        .and_then(|r| proto::get(&r, 2).and_then(|v| v.as_str()));
    let mut cands = Vec::new();
    if let Some(l) = proto::get(&o, 14)
        .and_then(|v| v.as_bytes())
        .and_then(proto::parse)
    {
        for (f, v) in &l {
            if *f == 2 {
                let c = proto::parse(v.as_bytes().unwrap()).unwrap();
                let id = proto::get(&c, 1).and_then(|v| v.as_u64()).unwrap_or(0) as i32 as i64;
                cands.push((id, proto::get(&c, 4).unwrap().as_str().unwrap()));
            }
        }
    }
    (cands, result)
}

fn main() {
    let readings: Vec<String> = std::env::args().skip(1).collect();
    let mut b = Bridge::new(ImBackend::new());
    b.init();
    let t = Instant::now();
    while !b.is_ready() {
        if t.elapsed() > Duration::from_secs(60) {
            eprintln!("the model did not load");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // The toggle-flick keyboard.
    b.handle(&input(|i| {
        i.uint(1, 17);
        i.message(9, |r| r.int(4, 16));
    }));
    let mut last = Vec::new();
    for r in &readings {
        let key = input(|i| {
            i.uint(1, 3);
            i.uint(2, 1);
            i.message(3, |k| {
                k.string(5, r.chars().last().unwrap().to_string().as_str())
            });
            i.message(6, |c| c.string(1, "明日の"));
        });
        assert!(b.handle(&key).is_none());
        let t = Instant::now();
        let reply = b.observe(&key, &mozc_reply(r));
        let ms = t.elapsed().as_millis();
        let (cands, _) = reply.as_deref().map(show).unwrap_or_default();
        let texts: Vec<&str> = cands.iter().map(|c| c.1.as_str()).collect();
        println!("{r} ({ms}ms): {}", texts.join(" / "));
        last = cands;
    }
    if let Some((id, _)) = last.first() {
        let tap = input(|i| {
            i.uint(1, 5);
            i.uint(2, 1);
            i.message(4, |c| {
                c.uint(1, 7);
                c.int(2, *id);
            });
        });
        let out = b.handle(&tap).expect("ours");
        println!("tap: commit {:?}", show(&out).1);
        println!("revert to Mozc: {}", b.shadow_command(&tap).is_some());
    }
}
