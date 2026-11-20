//! Drive the bridge with the real engine: `sim <romaji> [space] [enter]` (KARUKAN_* env as usual).
use karukan_mozc::bridge::{Bridge, ImBackend};
use karukan_mozc::proto::{self, Val, W};
use std::time::{Duration, Instant};

fn key(f: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, 3);
        i.uint(2, 1);
        i.message(3, f);
        i.message(6, |c| c.string(1, "今日は天気が"));
    });
    w.0
}

fn show(reply: &[u8]) {
    let top = proto::parse(reply).unwrap();
    let o = proto::parse(proto::get(&top, 2).unwrap().as_bytes().unwrap()).unwrap();
    let mut line = String::new();
    if let Some(r) = proto::get(&o, 4).and_then(|v| v.as_bytes()).and_then(proto::parse) {
        line += &format!("[commit {}] ", proto::get(&r, 2).unwrap().as_str().unwrap());
    }
    if let Some(p) = proto::get(&o, 5).and_then(|v| v.as_bytes()).and_then(proto::parse) {
        let segs: Vec<String> = p
            .iter()
            .filter_map(|(f, v)| match (f, v) {
                (2, Val::Group(g)) => {
                    let g = proto::parse(g).unwrap();
                    Some(format!(
                        "{}{}",
                        if proto::get(&g, 3) == Some(Val::Varint(2)) { "*" } else { "_" },
                        proto::get(&g, 4).unwrap().as_str().unwrap()
                    ))
                }
                _ => None,
            })
            .collect();
        line += &format!("preedit {} ", segs.join("|"));
    }
    if let Some(l) = proto::get(&o, 14).and_then(|v| v.as_bytes()).and_then(proto::parse) {
        let c: Vec<String> = l
            .iter()
            .filter(|(f, _)| *f == 2)
            .map(|(_, v)| {
                let c = proto::parse(v.as_bytes().unwrap()).unwrap();
                proto::get(&c, 4).unwrap().as_str().unwrap()
            })
            .collect();
        line += &format!("cands {}", c.join(" / "));
    }
    println!("{line}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut b = Bridge::new(ImBackend::new());
    b.init();
    let t = Instant::now();
    while !b.is_ready() {
        if t.elapsed() > Duration::from_secs(60) {
            println!("model not ready");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("ready after {:?}", t.elapsed());
    for a in &args {
        let cmd = match a.as_str() {
            "space" => key(|k| k.uint(3, 4)),
            "enter" => key(|k| k.uint(3, 5)),
            "bs" => key(|k| k.uint(3, 12)),
            s => {
                for ch in s.chars() {
                    let t = Instant::now();
                    match b.handle(&key(|k| k.uint(1, ch as u64))) {
                        Some(r) => {
                            print!("{ch} ({:?}) ", t.elapsed());
                            show(&r)
                        }
                        None => println!("{ch}: -> Mozc"),
                    }
                }
                continue;
            }
        };
        let t = Instant::now();
        match b.handle(&cmd) {
            Some(r) => {
                print!("{a} ({:?}) ", t.elapsed());
                show(&r)
            }
            None => println!("{a}: -> Mozc"),
        }
    }
}
