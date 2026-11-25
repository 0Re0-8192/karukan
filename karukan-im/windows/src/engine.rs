//! Client for `karukan-imserver` (newline-delimited JSON-RPC over stdio).
//!
//! The server is spawned as a hidden child process. Replies are read on a
//! helper thread so a stuck engine can never freeze the host application:
//! every call has a timeout and returns `None` on failure.
//!
//! Two kinds of server: the one every app of this user shares, reached over a
//! named pipe (`connect`), and a private child process (`spawn`), used where the
//! shared one cannot be reached (an elevated or sandboxed app) and by the tests.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

const CALL_TIMEOUT: Duration = Duration::from_millis(3000);
const INIT_TIMEOUT: Duration = Duration::from_millis(8000);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreeditAttr {
    pub start: usize,
    pub end: usize,
    pub style: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CandidateItem {
    pub text: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    UpdatePreedit {
        text: String,
        caret: usize,
        #[serde(default)]
        attributes: Vec<PreeditAttr>,
    },
    ShowCandidates {
        candidates: Vec<CandidateItem>,
        cursor: usize,
        #[serde(default)]
        page: usize,
        #[serde(default)]
        total_pages: usize,
    },
    HideCandidates,
    Commit {
        text: String,
    },
    UpdateAux {
        text: String,
    },
    HideAux,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct KeyResult {
    pub consumed: bool,
    #[serde(default)]
    pub actions: Vec<Action>,
    /// The composition's keys as typed (`kaigi`); empty when the engine does not know.
    #[serde(default)]
    pub typed: String,
    /// The same keys as kana (`かいぎ`); older servers leave it out.
    #[serde(default)]
    pub typed_kana: String,
}

/// How the client reaches its server.
enum Link {
    /// A private server, the app's own child process.
    Child(Child),
    /// The server every app of this user shares (see `karukan-imserver --pipe`).
    #[cfg(windows)]
    Pipe(std::sync::Arc<pipe::Pipe>),
}

pub struct EngineClient {
    link: Option<Link>,
    writer: Option<Box<dyn Write + Send>>,
    rx: Receiver<String>,
    /// Set by the reader thread when the server side closed.
    closed: Arc<AtomicBool>,
    next_id: u64,
    /// A call ran out of time: the server is wedged, and asking it again would
    /// freeze the host application for another `CALL_TIMEOUT` on every key.
    stuck: bool,
    /// The pipe name when shared: a different name means a different server.
    pub pipe_name: Option<String>,
}

/// Read lines from `r` on a helper thread into a channel.
fn spawn_reader(r: impl std::io::Read + Send + 'static) -> (Receiver<String>, Arc<AtomicBool>) {
    let (tx, rx) = channel();
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(r);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.trim_end().to_string()).is_err() {
                        break;
                    }
                }
            }
        }
        flag.store(true, Ordering::SeqCst);
    });
    (rx, closed)
}

impl EngineClient {
    /// Spawn a private server. `envs` are extra environment variables for it.
    pub fn spawn(exe: &Path, envs: &[(String, String)]) -> std::io::Result<Self> {
        let mut cmd = Command::new(exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (rx, closed) = spawn_reader(stdout);
        Ok(Self {
            link: Some(Link::Child(child)),
            writer: Some(Box::new(stdin)),
            rx,
            closed,
            next_id: 0,
            stuck: false,
            pipe_name: None,
        })
    }

    /// Connect to the shared server on `\\.\pipe\<name>`.
    #[cfg(windows)]
    pub fn connect(name: &str) -> std::io::Result<Self> {
        let p = Arc::new(pipe::Pipe::open(name)?);
        let (rx, closed) = spawn_reader(pipe::Reader(p.clone()));
        Ok(Self {
            writer: Some(Box::new(pipe::Writer(p.clone()))),
            link: Some(Link::Pipe(p)),
            rx,
            closed,
            next_id: 0,
            stuck: false,
            pipe_name: Some(name.to_string()),
        })
    }

    /// The process on the other end of the shared server's pipe.
    #[cfg(windows)]
    pub fn server_pid(&self) -> Option<u32> {
        match self.link.as_ref()? {
            Link::Pipe(p) => p.server_pid(),
            Link::Child(c) => Some(c.id()),
        }
    }

    /// Running and answering: a server that timed out counts as dead, so the
    /// caller replaces it instead of waiting on it again.
    pub fn is_alive(&mut self) -> bool {
        if self.stuck || self.closed.load(Ordering::SeqCst) {
            return false;
        }
        match self.link.as_mut() {
            Some(Link::Child(c)) => matches!(c.try_wait(), Ok(None)),
            #[cfg(windows)]
            Some(Link::Pipe(_)) => true,
            None => false,
        }
    }

    fn send(&mut self, method: &str, params: Value, want_reply: bool) -> Option<Value> {
        self.send_within(method, params, want_reply, CALL_TIMEOUT)
    }

    fn send_within(
        &mut self,
        method: &str,
        params: Value,
        want_reply: bool,
        timeout: Duration,
    ) -> Option<Value> {
        let writer = self.writer.as_mut()?;
        let mut req = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let id = if want_reply {
            self.next_id += 1;
            req["id"] = json!(self.next_id);
            Some(self.next_id)
        } else {
            None
        };
        let mut line = serde_json::to_string(&req).ok()?;
        line.push('\n');
        writer.write_all(line.as_bytes()).ok()?;
        writer.flush().ok()?;
        let id = id?;

        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(text) => {
                    let v: Value = serde_json::from_str(&text).ok()?;
                    // Replies to earlier (timed-out) calls are dropped.
                    if v.get("id").and_then(Value::as_u64) == Some(id) {
                        return v.get("result").cloned();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.stuck = true;
                    return None;
                }
                Err(RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    /// `init` reads the dictionaries the first time: a cold 200 MB read on a slow
    /// disk must not count as a wedged server.
    pub fn init(&mut self) -> bool {
        self.send_within("init", json!({}), true, INIT_TIMEOUT)
            .is_some()
    }

    pub fn process_key(&mut self, keysym: u32, mods: Mods) -> Option<KeyResult> {
        let v = self.send(
            "process_key",
            json!({
                "keysym": keysym,
                "modifiers": {"shift": mods.shift, "control": mods.control, "alt": mods.alt},
                "is_release": false,
            }),
            true,
        )?;
        serde_json::from_value(v).ok()
    }

    /// Commit the candidate at `page_index` on the page on screen (a click).
    pub fn select_candidate(&mut self, page_index: usize) -> Option<KeyResult> {
        let v = self.send("select_candidate", json!({"page_index": page_index}), true)?;
        serde_json::from_value(v).ok()
    }

    pub fn commit(&mut self) -> Option<KeyResult> {
        serde_json::from_value(self.send("commit", json!({}), true)?).ok()
    }

    /// Reset without waiting for the answer.
    pub fn reset_nowait(&mut self) {
        let _ = self.send("reset", json!({}), false);
    }

    pub fn reset(&mut self) {
        let _ = self.send("reset", json!({}), true);
    }

    pub fn set_surrounding_text(&mut self, text: &str, cursor_pos: usize) {
        let _ = self.send(
            "set_surrounding_text",
            json!({"text": text, "cursor_pos": cursor_pos}),
            false,
        );
    }

    pub fn save_learning(&mut self) {
        let _ = self.send("save_learning", json!({}), false);
    }
}

impl Drop for EngineClient {
    fn drop(&mut self) {
        // Closing our end makes the server save the learning cache. Nothing here
        // waits on it: this runs on the host app's UI thread.
        self.writer.take();
        match self.link.take() {
            Some(Link::Child(mut child)) => {
                std::thread::spawn(move || {
                    let deadline = Instant::now() + Duration::from_millis(1500);
                    while Instant::now() < deadline {
                        if let Ok(Some(_)) = child.try_wait() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let _ = child.kill();
                });
            }
            #[cfg(windows)]
            Some(Link::Pipe(p)) => p.close(),
            None => {}
        }
    }
}

#[cfg(windows)]
pub mod pipe {
    //! A named-pipe client handle opened for overlapped I/O, so the reader
    //! thread's pending read never blocks a write (synchronous I/O on one
    //! handle is serialized by the system).

    use std::io;

    use windows::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, WriteFile,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Threading::CreateEventW;
    use windows::core::HSTRING;

    fn to_io(e: windows::core::Error) -> io::Error {
        io::Error::from_raw_os_error(e.code().0 & 0xffff)
    }

    pub struct Pipe(HANDLE);
    // the handle is only used through overlapped calls, each with its own event
    unsafe impl Send for Pipe {}
    unsafe impl Sync for Pipe {}

    impl Pipe {
        pub fn open(name: &str) -> io::Result<Self> {
            let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
            let h = unsafe {
                CreateFileW(
                    &path,
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .map_err(to_io)?;
            Ok(Self(h))
        }

        /// The server's process id.
        pub fn server_pid(&self) -> Option<u32> {
            let mut pid = 0u32;
            unsafe {
                windows::Win32::System::Pipes::GetNamedPipeServerProcessId(self.0, &mut pid)
                    .ok()?;
            }
            Some(pid)
        }

        /// Make the pending read return, so the reader thread lets go of the handle.
        pub fn close(&self) {
            unsafe {
                let _ = CancelIoEx(self.0, None);
            }
        }

        fn io(&self, read: bool, buf: &mut [u8]) -> io::Result<usize> {
            unsafe {
                let ev = CreateEventW(None, true, false, None).map_err(to_io)?;
                let mut ov = OVERLAPPED {
                    hEvent: ev,
                    ..Default::default()
                };
                let started = if read {
                    ReadFile(self.0, Some(buf), None, Some(&mut ov))
                } else {
                    WriteFile(self.0, Some(buf), None, Some(&mut ov))
                };
                let result = match started {
                    Err(e) if e.code() != ERROR_IO_PENDING.to_hresult() => Err(e),
                    _ => {
                        let mut n = 0u32;
                        GetOverlappedResult(self.0, &ov, &mut n, true).map(|_| n as usize)
                    }
                };
                let _ = CloseHandle(ev);
                match result {
                    Ok(n) => Ok(n),
                    Err(e) if read && e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
                    Err(e) => Err(to_io(e)),
                }
            }
        }
    }

    impl Drop for Pipe {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub struct Reader(pub std::sync::Arc<Pipe>);
    impl io::Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.io(true, buf)
        }
    }

    pub struct Writer(pub std::sync::Arc<Pipe>);
    impl io::Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let mut copy = buf.to_vec();
            self.0.io(false, &mut copy)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sort` reads its input to the end and answers nothing: a wedged server.
    #[test]
    #[cfg(windows)]
    fn a_wedged_server_is_given_up_after_one_timeout() {
        let mut e = EngineClient::spawn(Path::new("sort.exe"), &[]).unwrap();
        assert!(e.is_alive());
        let t = Instant::now();
        assert!(e.process_key(0x61, Mods::default()).is_none());
        assert!(t.elapsed() >= CALL_TIMEOUT);
        // the next key must not wait again: the caller sees a dead server and replaces it
        assert!(!e.is_alive());
    }

    #[test]
    fn parse_actions() {
        let r: KeyResult = serde_json::from_str(
            r#"{"consumed":true,"actions":[
                {"type":"update_preedit","text":"きょう","caret":3,"attributes":[{"start":0,"end":3,"style":"underline"}]},
                {"type":"show_candidates","candidates":[{"text":"今日","description":"d"},{"text":"京"}],"cursor":0,"page":0,"total_pages":2},
                {"type":"hide_candidates"},{"type":"commit","text":"x"},
                {"type":"update_aux","text":"a"},{"type":"hide_aux"}]}"#,
        )
        .unwrap();
        assert!(r.consumed);
        assert_eq!(r.actions.len(), 6);
    }
}
