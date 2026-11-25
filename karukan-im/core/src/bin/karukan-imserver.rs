//! karukan-imserver: stdio JSON-RPC engine server for the macOS frontend.
//!
//! Reads newline-delimited JSON-RPC 2.0 requests from stdin and writes one
//! response per line to stdout. Logs go to stderr (`RUST_LOG` controls the
//! filter; defaults to `info`). The learning cache is saved on EOF, so the
//! frontend should close the child's stdin (or send `save_learning`) before
//! terminating it.
//!
//! `--prefetch-models` downloads every conversion model defined in the
//! config's `[models]` table into the HuggingFace cache and exits (used by
//! `make install` to avoid a multi-minute download on first launch).

use std::io::{BufRead, Write};

use anyhow::Context;
use karukan_im::config::Settings;
use karukan_im::server::ImServer;

/// Warm the HF cache for every `[models]` entry; local-path entries are
/// only checked for existence. Stops at the first failure: the next one
/// would only repeat the cause after another network timeout.
fn prefetch_models() -> anyhow::Result<()> {
    let settings = Settings::load()?;
    for key in settings.models.keys() {
        let (gguf, tokenizer) = settings
            .model_source(key)?
            .resolve()
            .with_context(|| format!("model '{key}'"))?;
        tracing::info!(
            "Model '{}' ready: {} (tokenizer: {})",
            key,
            gguf.display(),
            tokenizer.display()
        );
    }
    Ok(())
}

/// Keeps typing responsive while something else holds every core (a render,
/// a game), and keeps the server off the cores between keystrokes.
///
/// - High priority: at normal priority a conversion's threads are preempted
///   by the busy ones and each waits on the slowest of them; above normal is
///   not enough once the other work raises its own priority, as games do.
///   Measured with all 32 threads of a 16-core CPU spinning (`load_bench`):
///   single keys stalled 0.4–2.9 s at normal, and up to 3.4 s above normal
///   against spinners at above normal; at high, none past 125 ms either way.
/// - The conversion threads must sleep between keystrokes, or at high
///   priority they would take the cores from the game: MSVC's OpenMP spun
///   there, keeping about 3 cores busy while one typed (50 s of CPU over 18 s
///   of typing). On Windows llama.cpp is built on ggml's own thread pool
///   instead (karukan-engine's Cargo.toml), which sleeps by itself.
/// - Opts out of power throttling, which on CPUs with efficiency cores would
///   put a windowless background process on them.
///
/// Priority alone does not save a multi-threaded conversion on a machine
/// with every core busy; the engine drops to one thread then
/// (`karukan_engine`'s `kanji::load`).
#[cfg(windows)]
fn keep_up_under_load() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, HIGH_PRIORITY_CLASS, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
    };
    unsafe {
        let me = GetCurrentProcess();
        if let Err(e) = SetPriorityClass(me, HIGH_PRIORITY_CLASS) {
            tracing::warn!("priority not raised: {e}");
        }
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: 0,
        };
        let _ = SetProcessInformation(
            me,
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

fn main() {
    #[cfg(windows)]
    keep_up_under_load();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if std::env::args().any(|arg| arg == "--prefetch-models") {
        if let Err(e) = prefetch_models() {
            tracing::error!("model prefetch failed: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    // Windows: `--pipe NAME [--idle-exit SECS]` serves every app over one named pipe
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--pipe") {
            let Some(name) = args.get(i + 1) else {
                tracing::error!("--pipe needs a name");
                std::process::exit(2);
            };
            let idle = args
                .iter()
                .position(|a| a == "--idle-exit")
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(300u64);
            if let Err(e) =
                karukan_im::server::pipe::serve(name, std::time::Duration::from_secs(idle))
            {
                tracing::error!("pipe server failed: {e:#}");
                std::process::exit(1);
            }
            return;
        }
    }

    let mut server = ImServer::new();
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    tracing::info!("karukan-imserver started (pid={})", std::process::id());

    for line in stdin.lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                tracing::error!("stdin read error: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_line(&line)
            && writeln!(stdout, "{response}")
                .and_then(|_| stdout.flush())
                .is_err()
        {
            // stdout closed: frontend is gone
            break;
        }
    }

    tracing::info!("stdin closed, saving learning cache and exiting");
    server.save_learning();
}
