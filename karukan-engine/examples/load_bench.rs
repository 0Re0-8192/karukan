//! Typing latency while the rest of the PC is busy (a render, a game):
//! converts a sentence one keystroke at a time, as live conversion does,
//! with every core kept busy by another process at normal priority.
//!
//! Usage (Windows):
//!   cargo run --release -p karukan-engine --example load_bench -- \
//!     [--stress N] [--stress-prio P] [--prio normal|above|high] [--threads T]
//!     [--gap MS] [--model REPO FILE]
//!
//! `--stress N` starts a second copy of this program that spins N threads
//! (0 = none) at priority class `--stress-prio` (a game that raises its own
//! priority: `above`); `--prio` is the priority class this process converts at.
//! `--gap` waits between keystrokes; `cpu` against `busy` (the time spent in
//! conversions) then shows what the threads burn while nothing is converted.

use karukan_engine::{KanaKanjiConverter, ModelSource};
use std::time::{Duration, Instant};

#[cfg(windows)]
mod prio {
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn SetPriorityClass(h: isize, class: u32) -> i32;
    }
    #[repr(C)]
    #[derive(Default)]
    struct FileTime(u32, u32);
    unsafe extern "system" {
        fn GetProcessTimes(
            h: isize,
            c: *mut FileTime,
            e: *mut FileTime,
            k: *mut FileTime,
            u: *mut FileTime,
        ) -> i32;
    }
    /// CPU time this process has used, in ms.
    pub fn cpu_ms() -> u64 {
        let (mut c, mut e, mut k, mut u) = Default::default();
        unsafe {
            GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        }
        let t = |f: &FileTime| ((f.1 as u64) << 32 | f.0 as u64) / 10_000;
        t(&k) + t(&u)
    }
    pub fn set(name: &str) {
        let class = match name {
            "above" => 0x8000,
            "high" => 0x80,
            "below" => 0x4000,
            _ => 0x20,
        };
        unsafe {
            SetPriorityClass(GetCurrentProcess(), class);
        }
    }
}
#[cfg(not(windows))]
mod prio {
    pub fn cpu_ms() -> u64 {
        0
    }
    pub fn set(_: &str) {}
}

fn spin(n: usize) -> ! {
    for _ in 0..n {
        std::thread::spawn(|| {
            let mut x = 0u64;
            loop {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
    }
    // never outlives a run that lost track of it: a machine whose every
    // core spins at a raised priority stops answering even ssh
    std::thread::sleep(Duration::from_secs(180));
    std::process::exit(0);
}

fn pct(v: &mut [u128], p: f64) -> u128 {
    v.sort_unstable();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("spin") {
        prio::set(args.get(3).map_or("normal", String::as_str));
        spin(args[2].parse().unwrap());
    }
    let mut stress = 0usize;
    let mut prio_name = "normal".to_string();
    let mut stress_prio = "normal".to_string();
    let mut threads = 4u32;
    let mut gap = 0u64;
    let mut repo = "OReOOreoaaaa/jinen-v2-small-karukan.gguf".to_string();
    let mut file = "jinen-v2-small-karukan-r9b-Q8_0.gguf".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--stress" => {
                stress = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--prio" => {
                prio_name = args[i + 1].clone();
                i += 1;
            }
            "--stress-prio" => {
                stress_prio = args[i + 1].clone();
                i += 1;
            }
            "--gap" => {
                gap = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--threads" => {
                threads = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--model" => {
                repo = args[i + 1].clone();
                file = args[i + 2].clone();
                i += 2;
            }
            _ => {}
        }
        i += 1;
    }
    let source = ModelSource::HuggingFace {
        repo,
        filename: file,
    };
    let mut conv = KanaKanjiConverter::from_source(&source, "bench").expect("model load");
    conv.set_n_threads(threads);
    prio::set(&prio_name);

    let sentences = [
        ("", "きょうはてんきがいいのでこうえんをさんぽした"),
        (
            "今日は",
            "あしたのかいぎのしりょうをじゅんびしておいてください",
        ),
        ("", "このぷろぐらむはにほんごをへんかんします"),
    ];
    // warm up without load
    let _ = conv.convert("きょうは", "", 1);

    let mut child = (stress > 0).then(|| {
        let c = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["spin", &stress.to_string(), &stress_prio])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        c
    });

    let mut times = Vec::new();
    let mut cpu0 = 0;
    for round in 0..3 {
        if round == 1 {
            cpu0 = prio::cpu_ms();
        }
        for (lctx, s) in sentences {
            let chars: Vec<char> = s.chars().collect();
            for n in 1..=chars.len() {
                let reading: String = chars[..n].iter().collect();
                let t = Instant::now();
                let _ = std::hint::black_box(conv.convert(&reading, lctx, 1));
                if round > 0 {
                    times.push(t.elapsed().as_millis());
                }
                std::thread::sleep(Duration::from_millis(gap));
            }
        }
    }
    if let Some(c) = child.as_mut() {
        let _ = c.kill();
    }
    let cpu = prio::cpu_ms() - cpu0;
    let busy: u128 = times.iter().sum();
    let n = times.len();
    let over = times.iter().filter(|&&t| t > 100).count();
    println!(
        "stress={stress:>2}@{stress_prio:<6} prio={prio_name:<6} threads={threads}: keys={n} p50={}ms p90={}ms p99={}ms max={}ms >100ms={over} cpu={cpu}ms busy={busy}ms",
        pct(&mut times, 0.5),
        pct(&mut times, 0.9),
        pct(&mut times, 0.99),
        pct(&mut times, 1.0),
    );
}
