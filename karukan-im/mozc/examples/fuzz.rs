//! Random key sequences through the bridge with the real engine; every reply must decode.
//! `fuzz [steps] [seed]`
use karukan_mozc::bridge::{Bridge, ImBackend};
use karukan_mozc::proto::{self, W};
use std::time::{Duration, Instant};

fn cmd(ty: u32, f: impl FnOnce(&mut W)) -> Vec<u8> {
    let mut w = W::new();
    w.message(1, |i| {
        i.uint(1, ty as u64);
        i.uint(2, 1);
        f(i);
        i.message(6, |c| c.string(1, "今日は"));
    });
    w.0
}

fn main() {
    let steps: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);
    let mut seed: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let mut rnd = move |n: u64| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    let mut b = Bridge::new(ImBackend::new());
    let t = Instant::now();
    while !b.is_ready() {
        assert!(t.elapsed() < Duration::from_secs(60), "model not ready");
        std::thread::sleep(Duration::from_millis(200));
    }
    let letters = b"abcdefghijklmnopqrstuvwxyz1234567890,.-!?ABCDEFG";
    let (mut handled, mut passed) = (0, 0);
    let mut slowest = Duration::ZERO;
    for step in 0..steps {
        let kind = rnd(100);
        let command = if kind < 62 {
            let c = letters[rnd(letters.len() as u64) as usize] as u64;
            cmd(3, |i| i.message(3, |k| k.uint(1, c)))
        } else if kind < 72 {
            cmd(3, |i| i.message(3, |k| k.uint(3, 4))) // space
        } else if kind < 80 {
            cmd(3, |i| i.message(3, |k| k.uint(3, 12))) // backspace
        } else if kind < 85 {
            cmd(3, |i| i.message(3, |k| k.uint(3, 5))) // enter
        } else if kind < 88 {
            cmd(3, |i| i.message(3, |k| k.uint(3, 10))) // escape
        } else if kind < 92 {
            let s = [6u64, 7, 8, 9][rnd(4) as usize];
            cmd(3, |i| i.message(3, |k| k.uint(3, s)))
        } else if kind < 95 {
            let id = rnd(10) as i64;
            cmd(5, |i| {
                i.message(4, |c| {
                    c.uint(1, 7);
                    c.int(2, id)
                })
            })
        } else if kind < 97 {
            let ty = [1u64, 2, 10, 11, 4, 5, 22, 23][rnd(8) as usize];
            let pos = rnd(6);
            cmd(5, |i| {
                i.message(4, |c| {
                    c.uint(1, ty);
                    c.uint(5, pos);
                    c.uint(3, 1)
                })
            })
        } else {
            cmd(4, |i| i.message(3, |k| k.uint(1, 'a' as u64))) // TEST_SEND_KEY
        };
        let t = Instant::now();
        match b.handle(&command) {
            Some(r) => {
                handled += 1;
                let top =
                    proto::parse(&r).unwrap_or_else(|| panic!("step {step}: reply does not parse"));
                let out = proto::get(&top, 2)
                    .and_then(|v| v.as_bytes())
                    .expect("output");
                proto::parse(out).unwrap_or_else(|| panic!("step {step}: output does not parse"));
            }
            None => passed += 1,
        }
        slowest = slowest.max(t.elapsed());
    }
    println!("steps {steps}: handled {handled}, passed to Mozc {passed}, slowest {slowest:?}");
}
