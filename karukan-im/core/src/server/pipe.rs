//! Named-pipe mode (Windows): one server shared by every app.
//!
//! The Windows text service runs inside each app, and a private server per app
//! cost ~570 MB and half a second of the first key's time for every app. Here one
//! process serves them all: each connection gets an engine of its own (state,
//! learning cache), while the system dictionary, the user dictionaries and the
//! models are the process-global caches every engine shares. Requests from
//! different apps are serialized — one human types into one app at a time — so
//! the engines never contend for a model.
//!
//! The pipe accepts only the current user (and SYSTEM), and only local clients.
//! Its name is chosen by the client and carries what the server was started
//! with (install, server build, dictionaries): when any of those changes,
//! clients move to a new name, a new server starts for it, and the old one
//! exits once its last client is gone.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PWSTR};

use super::ImServer;

/// How often a connection's learning cache is saved while it stays open.
const SAVE_EVERY: Duration = Duration::from_secs(60);

/// One request at a time across all connections.
static DISPATCH: Mutex<()> = Mutex::new(());

/// The current user's SID as a string (`S-1-5-21-…`).
pub fn current_user_sid() -> anyhow::Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let got = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        );
        let _ = CloseHandle(token);
        got?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s)?;
        let sid = s.to_string()?;
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        Ok(sid)
    }
}

/// A security descriptor granting the pipe to this user and SYSTEM only.
struct Security {
    sd: PSECURITY_DESCRIPTOR,
}

impl Security {
    fn for_current_user() -> anyhow::Result<Self> {
        let sddl = format!("D:P(A;;GA;;;{})(A;;GA;;;SY)", current_user_sid()?);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )?;
        }
        Ok(Self { sd })
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.sd.0,
            bInheritHandle: false.into(),
        }
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.sd.0)));
        }
    }
}

enum Instance {
    Created(HANDLE),
    /// Another server already owns the name.
    Taken,
}

fn create_instance(path: &HSTRING, sec: &Security, first: bool) -> anyhow::Result<Instance> {
    let sa = sec.attributes();
    let mut mode = PIPE_ACCESS_DUPLEX;
    if first {
        mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            path,
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            Some(&sa),
        )
    };
    if h.is_invalid() {
        let e = windows::core::Error::from_thread();
        if first && e.code() == ERROR_ACCESS_DENIED.to_hresult() {
            return Ok(Instance::Taken);
        }
        bail!("CreateNamedPipeW: {e}");
    }
    Ok(Instance::Created(h))
}

/// Serve `\\.\pipe\<name>` until no client has been connected for `idle_exit`.
/// Returns at once when another server already serves the name (two apps
/// started one at the same moment).
pub fn serve(name: &str, idle_exit: Duration) -> anyhow::Result<()> {
    let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
    let sec = Security::for_current_user().context("pipe security")?;
    let first = match create_instance(&path, &sec, true)? {
        Instance::Created(h) => h,
        Instance::Taken => {
            tracing::info!("{name} is already served; exiting");
            return Ok(());
        }
    };
    tracing::info!("serving {name} (pid={})", std::process::id());

    // Read the dictionaries and load the models now, before anyone types: the
    // engines the connections make find them in the process-global caches.
    std::thread::spawn(|| {
        let mut warm = ImServer::new();
        let _ = warm.handle_line(r#"{"jsonrpc":"2.0","id":0,"method":"init","params":{}}"#);
        // keep the engine until its models arrive, so the loader is not orphaned early
        for _ in 0..600 {
            let s = warm.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"status","params":{}}"#);
            if s.is_some_and(|s| !s.contains("\"loading\"")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
            let _ = warm.handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"process_key","params":{"keysym":65505}}"#,
            );
        }
    });

    let active = Arc::new(AtomicUsize::new(0));
    let idle_since = Arc::new(Mutex::new(Some(Instant::now())));
    {
        let active = active.clone();
        let idle_since = idle_since.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let idle = *idle_since.lock().unwrap_or_else(|e| e.into_inner());
                if active.load(Ordering::SeqCst) == 0
                    && idle.is_some_and(|t| t.elapsed() >= idle_exit)
                {
                    tracing::info!("no client for {idle_exit:?}; exiting");
                    std::process::exit(0);
                }
            }
        });
    }

    let mut next = Some(first);
    loop {
        let h = match next.take() {
            Some(h) => h,
            None => match create_instance(&path, &sec, false)? {
                Instance::Created(h) => h,
                Instance::Taken => unreachable!("only the first instance can be taken"),
            },
        };
        if let Err(e) = unsafe { ConnectNamedPipe(h, None) }
            && e.code() != ERROR_PIPE_CONNECTED.to_hresult()
        {
            unsafe {
                let _ = CloseHandle(h);
            }
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        *idle_since.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let file = unsafe { File::from_raw_handle(h.0) };
        let active = active.clone();
        let idle_since = idle_since.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve_one(file) {
                tracing::warn!("connection ended: {e:#}");
            }
            if active.fetch_sub(1, Ordering::SeqCst) == 1 {
                *idle_since.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
            }
        });
    }
}

/// One app's connection: its own engine, for as long as the app keeps the pipe open.
fn serve_one(file: File) -> anyhow::Result<()> {
    let mut writer = file.try_clone()?;
    let reader = BufReader::new(file);
    let mut server = ImServer::new();
    let mut last_save = Instant::now();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = {
            let _one_at_a_time = DISPATCH.lock().unwrap_or_else(|e| e.into_inner());
            server.handle_line(&line)
        };
        if let Some(response) = response
            && writeln!(writer, "{response}").is_err()
        {
            break;
        }
        // An app can stay connected for days, and a server killed at log-off or by
        // the uninstaller says no goodbye: save what was learned now and then
        // (after the reply, so the key is not kept waiting; a no-op when clean).
        if last_save.elapsed() >= SAVE_EVERY {
            last_save = Instant::now();
            let _one_at_a_time = DISPATCH.lock().unwrap_or_else(|e| e.into_inner());
            server.save_learning();
        }
    }
    // the app is gone: what it learned is merged into the file
    let _one_at_a_time = DISPATCH.lock().unwrap_or_else(|e| e.into_inner());
    server.save_learning();
    Ok(())
}
