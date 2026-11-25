//! How many threads a conversion can use, given what the rest of the machine
//! is doing.
//!
//! A conversion's threads meet at a barrier after every step of the graph, so
//! each step waits on the slowest thread. When something else (a render, a
//! game) holds every core, one of them is always waiting for a core, and a
//! conversion that takes 20 ms on an idle machine takes seconds — on a 35 W
//! 13900T with every core busy, 1.2–1.7 s per keystroke at 2, 4 or 8
//! threads, even at high priority. One thread never waits on another: on the
//! same machine it took 76 ms (121 ms at worst). So while the other processes
//! leave fewer free cores than the configured threads, a
//! conversion runs on one thread, and goes back to the configured count once
//! the machine has been calm for [`CALM_FOR`].
//!
//! The load is read from the system's CPU times at each conversion, over the
//! time since the previous reading, minus this process's own share. After a
//! pause in typing that window is long, which averages a sustained load (what
//! a render or a game is) correctly. Only Windows is probed; elsewhere the
//! configured count is used as it is.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A shorter window than this says more about scheduling noise than load:
/// the previous verdict stands.
const MIN_WINDOW: Duration = Duration::from_millis(100);

/// How long the machine must stay calm before conversions spread out again.
/// Switching rebuilds the greedy session (one prompt decode), so it must not
/// flicker with every burst of load.
const CALM_FOR: Duration = Duration::from_secs(2);

/// llama.cpp's own thread count when none is configured.
const LLAMA_DEFAULT_THREADS: u32 = 4;

/// CPU times in 100 ns units, summed over every logical processor.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Times {
    /// Busy time of the whole system.
    busy: u64,
    /// Busy and idle time of the whole system.
    total: u64,
    /// This process's busy time.
    own: u64,
}

#[derive(Debug)]
struct Probe {
    last: Option<(Instant, Times)>,
    /// Conversions stay on one thread until this instant.
    crowded_until: Option<Instant>,
    /// The threads the last conversion asked for: what "crowded" is judged
    /// against when the engine asks without converting.
    wanted: u32,
}

impl Default for Probe {
    fn default() -> Self {
        PROBE_START
    }
}

const PROBE_START: Probe = Probe {
    last: None,
    crowded_until: None,
    wanted: LLAMA_DEFAULT_THREADS,
};

static PROBE: Mutex<Probe> = Mutex::new(PROBE_START);

/// Free logical processors the other processes left over a window, out of
/// `cpus`. `None` when the window holds no time.
fn free_cpus(before: Times, after: Times, cpus: u32) -> Option<f64> {
    let total = after.total.saturating_sub(before.total);
    if total == 0 {
        return None;
    }
    let busy = after.busy.saturating_sub(before.busy);
    let own = after.own.saturating_sub(before.own);
    let others = busy.saturating_sub(own).min(total) as f64 / total as f64;
    Some(cpus as f64 * (1.0 - others))
}

impl Probe {
    /// Read the times taken at `now`, unless the window since the last
    /// reading is too short to say anything.
    fn sample(&mut self, now: Instant, times: Times, cpus: u32) {
        let window_ok = self
            .last
            .is_none_or(|(at, _)| now.duration_since(at) >= MIN_WINDOW);
        if !window_ok {
            return;
        }
        if let Some((_, before)) = self.last
            && let Some(free) = free_cpus(before, times, cpus)
            && free < self.wanted as f64
        {
            self.crowded_until = Some(now + CALM_FOR);
        }
        self.last = Some((now, times));
    }

    fn crowded(&self, now: Instant) -> bool {
        self.crowded_until.is_some_and(|until| now < until)
    }

    /// Threads for a conversion starting at `now`, given the times read then.
    fn threads(&mut self, configured: u32, now: Instant, times: Times, cpus: u32) -> u32 {
        let wanted = if configured == 0 {
            LLAMA_DEFAULT_THREADS
        } else {
            configured
        };
        if wanted <= 1 {
            return configured;
        }
        self.wanted = wanted;
        self.sample(now, times, cpus);
        if self.crowded(now) { 1 } else { configured }
    }
}

fn cpus() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// The thread count for a conversion starting now: `configured` (0 =
/// llama.cpp's default), or 1 while the machine is crowded.
pub(super) fn threads_for(configured: u32) -> u32 {
    let Some(times) = read_times() else {
        return configured;
    };
    PROBE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .threads(configured, Instant::now(), times, cpus())
}

/// Not read in this crate's own tests: they run side by side, and a thread
/// count that follows their load would make the session tests flaky.
#[cfg(all(windows, not(test)))]
fn read_times() -> Option<Times> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime(u32, u32);
    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
    }
    let v = |f: &FileTime| (f.1 as u64) << 32 | f.0 as u64;
    let (mut idle, mut kernel, mut user) = Default::default();
    let (mut created, mut exited, mut own_kernel, mut own_user) = Default::default();
    // SAFETY: plain out-parameters
    let ok = unsafe {
        GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0
            && GetProcessTimes(
                GetCurrentProcess(),
                &mut created,
                &mut exited,
                &mut own_kernel,
                &mut own_user,
            ) != 0
    };
    if !ok {
        return None;
    }
    // the system's kernel time includes its idle time
    let total = v(&kernel) + v(&user);
    Some(Times {
        busy: total.saturating_sub(v(&idle)),
        total,
        own: v(&own_kernel) + v(&own_user),
    })
}

#[cfg(any(not(windows), test))]
fn read_times() -> Option<Times> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Times after `ms` of wall time on `cpus` processors, `others` and `own`
    /// of them busy.
    fn after(before: Times, ms: u64, cpus: u64, others: f64, own: f64) -> Times {
        let span = ms * 10_000;
        Times {
            busy: before.busy + ((others + own) * span as f64) as u64,
            total: before.total + cpus * span,
            own: before.own + (own * span as f64) as u64,
        }
    }

    #[test]
    fn a_busy_machine_converts_on_one_thread_until_it_calms_down() {
        let mut p = Probe::default();
        let t0 = Instant::now();
        let mut times = Times::default();
        // the first reading has nothing to compare with
        assert_eq!(p.threads(4, t0, times, 32), 4);
        // 30 of 32 processors busy elsewhere: 2 free, fewer than 4
        times = after(times, 500, 32, 30.0, 1.0);
        let t1 = t0 + Duration::from_millis(500);
        assert_eq!(p.threads(4, t1, times, 32), 1);
        // calm again, but not for long enough
        times = after(times, 500, 32, 2.0, 1.0);
        let t2 = t1 + Duration::from_millis(500);
        assert_eq!(p.threads(4, t2, times, 32), 1);
        // calm since the last crowded reading for CALM_FOR
        times = after(times, 2000, 32, 2.0, 1.0);
        let t3 = t1 + CALM_FOR + Duration::from_millis(1);
        assert_eq!(p.threads(4, t3, times, 32), 4);
    }

    #[test]
    fn its_own_work_does_not_count_as_load() {
        let mut p = Probe::default();
        let t0 = Instant::now();
        let mut times = Times::default();
        p.threads(4, t0, times, 8);
        // all 8 busy, 4 of them this process's own conversion
        times = after(times, 300, 8, 4.0, 4.0);
        assert_eq!(p.threads(4, t0 + Duration::from_millis(300), times, 8), 4);
    }

    #[test]
    fn half_a_machine_busy_leaves_room() {
        let mut p = Probe::default();
        let t0 = Instant::now();
        let mut times = Times::default();
        p.threads(4, t0, times, 32);
        times = after(times, 300, 32, 16.0, 0.0);
        assert_eq!(p.threads(4, t0 + Duration::from_millis(300), times, 32), 4);
    }

    #[test]
    fn a_short_window_keeps_the_verdict() {
        let mut p = Probe::default();
        let t0 = Instant::now();
        let mut times = Times::default();
        p.threads(4, t0, times, 4);
        // a 20 ms window that looks crowded is not read
        times = after(times, 20, 4, 4.0, 0.0);
        assert_eq!(p.threads(4, t0 + Duration::from_millis(20), times, 4), 4);
    }

    #[test]
    fn one_configured_thread_is_left_alone() {
        let mut p = Probe::default();
        let t0 = Instant::now();
        assert_eq!(p.threads(1, t0, Times::default(), 4), 1);
        // 0 = llama.cpp's default, which is several threads: probed
        let mut times = Times::default();
        p.threads(0, t0, times, 4);
        times = after(times, 300, 4, 4.0, 0.0);
        assert_eq!(p.threads(0, t0 + Duration::from_millis(300), times, 4), 1);
    }
}
