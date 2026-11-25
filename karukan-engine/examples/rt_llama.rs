//! llama.cpp's side of the runtime comparison (rtbench): one cached token, a
//! 32-token prefill on it, then 8 single-token steps with the KV cache.
//!
//!   rt_llama MODEL.gguf THREADS [--stress N]
//!
//! Runs at high priority; `--stress N` spins N threads at normal priority
//! in a second process (which exits by itself after 180 s).

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::token::LlamaToken;

const PREFILL: usize = 32;
const STEPS: usize = 8;
const ROUNDS: usize = 30;

#[cfg(windows)]
fn set_high() {
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn SetPriorityClass(h: isize, class: u32) -> i32;
    }
    unsafe {
        SetPriorityClass(GetCurrentProcess(), 0x80);
    }
}
#[cfg(not(windows))]
fn set_high() {}

fn spin(n: usize) -> ! {
    for _ in 0..n {
        std::thread::spawn(|| {
            let mut x = 0u64;
            loop {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
    }
    std::thread::sleep(Duration::from_secs(180));
    std::process::exit(0);
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn argmax(logits: &[f32]) -> i32 {
    logits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap()
        .0 as i32
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "spin" {
        spin(args[2].parse().unwrap());
    }
    let threads: i32 = args[2].parse().unwrap();
    let stress: usize = args
        .iter()
        .position(|a| a == "--stress")
        .map_or(0, |i| args[i + 1].parse().unwrap());
    let mut backend = LlamaBackend::init().unwrap();
    backend.void_logs();
    let params = LlamaModelParams::default()
        .with_n_gpu_layers(0)
        .with_devices(&[])
        .unwrap();
    let model = LlamaModel::load_from_file(&backend, &args[1], &params).unwrap();
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(128))
        .with_n_batch(128)
        .with_n_ubatch(128)
        .with_n_threads(threads)
        .with_n_threads_batch(threads);
    let mut ctx = model.new_context(&backend, ctx_params).unwrap();
    set_high();
    let mut child = (stress > 0).then(|| {
        let c = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["spin", &stress.to_string()])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        c
    });

    let mut batch = LlamaBatch::new(128, 1);
    let mut feed = |ctx: &mut llama_cpp_2::context::LlamaContext, ids: &[i32], pos: usize| -> i32 {
        batch.clear();
        for (i, &t) in ids.iter().enumerate() {
            batch
                .add(LlamaToken(t), (pos + i) as i32, &[0], i == ids.len() - 1)
                .unwrap();
        }
        ctx.decode(&mut batch).unwrap();
        argmax(ctx.get_logits_ith(ids.len() as i32 - 1))
    };
    let (mut keys, mut prefills, mut steps) = (Vec::new(), Vec::new(), Vec::new());
    for round in 0..ROUNDS + 2 {
        ctx.clear_kv_cache();
        feed(&mut ctx, &[2], 0);
        let prompt: Vec<i32> = (0..PREFILL)
            .map(|i| (100 + (i * 7919 + round * 104729) % 30000) as i32)
            .collect();
        let t0 = Instant::now();
        let mut next = feed(&mut ctx, &prompt, 1);
        let t1 = Instant::now();
        for s in 0..STEPS {
            next = feed(&mut ctx, &[next.max(4) % 30000], 1 + PREFILL + s);
        }
        let t2 = Instant::now();
        if round >= 2 {
            keys.push((t2 - t0).as_secs_f64() * 1e3);
            prefills.push((t1 - t0).as_secs_f64() * 1e3);
            steps.push((t2 - t1).as_secs_f64() * 1e3 / STEPS as f64);
        }
    }
    if let Some(c) = child.as_mut() {
        let _ = c.kill();
    }
    let name = std::path::Path::new(&args[1])
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    println!(
        "llama {name} t={threads} stress={stress}: key p50={:.1} p90={:.1} max={:.1}ms | prefill32 p50={:.1}ms | step p50={:.2}ms",
        pct(&mut keys.clone(), 0.5),
        pct(&mut keys.clone(), 0.9),
        pct(&mut keys, 1.0),
        pct(&mut prefills, 0.5),
        pct(&mut steps, 0.5),
    );
}
