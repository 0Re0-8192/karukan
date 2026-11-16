//! Client for `karukan-imserver` (newline-delimited JSON-RPC over stdio).
//!
//! The server is spawned as a hidden child process. Replies are read on a
//! helper thread so a stuck engine can never freeze the host application:
//! every call has a timeout and returns `None` on failure.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};

const CALL_TIMEOUT: Duration = Duration::from_millis(3000);

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
}

pub struct EngineClient {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    next_id: u64,
}

impl EngineClient {
    /// Spawn the server. `envs` are extra environment variables for it.
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
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
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
        });
        Ok(Self {
            child,
            stdin,
            rx,
            next_id: 0,
        })
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, method: &str, params: Value, want_reply: bool) -> Option<Value> {
        let stdin = self.stdin.as_mut()?;
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
        stdin.write_all(line.as_bytes()).ok()?;
        stdin.flush().ok()?;
        let id = id?;

        let deadline = Instant::now() + CALL_TIMEOUT;
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
                Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                    return None;
                }
            }
        }
    }

    pub fn init(&mut self) -> bool {
        self.send("init", json!({}), true).is_some()
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

    pub fn commit(&mut self) -> Option<KeyResult> {
        serde_json::from_value(self.send("commit", json!({}), true)?).ok()
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
        // Closing stdin makes the server save its learning cache and exit.
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
