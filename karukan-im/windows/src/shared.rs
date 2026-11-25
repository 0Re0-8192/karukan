//! What the text service needs to share one server among all apps: who the user
//! is, whether this app may use the shared server, and starting it detached.

use std::path::Path;
use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenIntegrityLevel, TokenIsAppContainer,
    TokenUser,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    GetCurrentProcess, OpenProcessToken, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

fn token_info(class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u8>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, class, None, 0, &mut len);
        let mut buf = vec![0u8; len.max(4) as usize];
        let ok = GetTokenInformation(token, class, Some(buf.as_mut_ptr().cast()), len, &mut len);
        let _ = CloseHandle(token);
        ok.ok()?;
        Some(buf)
    }
}

/// The current user's SID as a string, e.g. `S-1-5-21-…`.
pub fn user_sid() -> Option<String> {
    static SID: OnceLock<Option<String>> = OnceLock::new();
    SID.get_or_init(|| unsafe {
        let buf = token_info(TokenUser)?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let sid = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        sid
    })
    .clone()
}

/// This process's integrity level (the label's RID: 0x2000 medium, 0x3000 high).
pub fn integrity() -> Option<u32> {
    static RID: OnceLock<Option<u32>> = OnceLock::new();
    *RID.get_or_init(|| unsafe {
        let buf = token_info(TokenIntegrityLevel)?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let n = *GetSidSubAuthorityCount(sid);
        if n == 0 {
            return None;
        }
        Some(*GetSidSubAuthority(sid, (n - 1) as u32))
    })
}

/// An ordinary desktop app: not an app container, medium integrity or above. A
/// low-integrity or sandboxed app cannot open the pipe and keeps a private server.
pub fn eligible() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        let container = token_info(TokenIsAppContainer)
            .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) != 0)
            .unwrap_or(true);
        !container && integrity().is_some_and(|r| r >= SECURITY_MANDATORY_MEDIUM_RID)
    })
}

/// The full path of process `pid`'s image, or None when it cannot be read (it
/// belongs to someone else, or is gone).
pub fn process_image(pid: u32) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(std::path::PathBuf::from(String::from_utf16_lossy(
            &buf[..n as usize],
        )))
    }
}

/// A server already listens on the name (checked without connecting to it).
pub fn pipe_exists(name: &str) -> bool {
    let path = HSTRING::from(format!(r"\\.\pipe\{name}"));
    // returns at once with "file not found" when no instance exists; succeeds
    // or times out after 1 ms when one does
    let ok = unsafe { WaitNamedPipeW(&path, 1).as_bool() };
    ok || windows::core::Error::from_thread().code().0 & 0xffff != 2
}

/// Start `exe --pipe name` on its own: no console, no handles of the app
/// inherited (a long-lived server holding an app's pipe open could keep that app
/// from ever seeing EOF), and out of the app's job when the job allows it, so the
/// server outlives the app that happened to start it.
pub fn spawn_detached(exe: &Path, name: &str, envs: &[(String, String)]) -> std::io::Result<()> {
    spawn_with_args(exe, &format!("--pipe {name}"), envs)
}

/// One argument quoted for a Windows command line (the rules
/// `CommandLineToArgvW` reads it back with).
pub fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
            }
        }
        if c != '\\' {
            out.push(c);
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// Start `exe args` on its own: no handles inherited, out of the app's job,
/// run from the exe's folder.
pub fn spawn_with_args(exe: &Path, args: &str, envs: &[(String, String)]) -> std::io::Result<()> {
    let mut cmd: Vec<u16> = format!("\"{}\" {args}", exe.display())
        .encode_utf16()
        .chain([0])
        .collect();
    // the environment block: this process's variables with `envs` on top
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !envs.iter().any(|(e, _)| e.eq_ignore_ascii_case(k)))
        .collect();
    vars.extend(envs.iter().cloned());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in &vars {
        block.extend(format!("{k}={v}").encode_utf16());
        block.push(0);
    }
    block.push(0);
    let exe_w = HSTRING::from(exe.as_os_str());
    // run from the install folder: a server living for hours must not hold the
    // folder of whichever app started it open
    let cwd = HSTRING::from(exe.parent().unwrap_or(exe).as_os_str());
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let run = |flags: PROCESS_CREATION_FLAGS, cmd: &mut Vec<u16>| unsafe {
        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessW(
            PCWSTR(exe_w.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            flags | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            Some(block.as_ptr().cast()),
            PCWSTR(cwd.as_ptr()),
            &si,
            &mut pi,
        )
        .map(|_| {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        })
    };
    run(CREATE_BREAKAWAY_FROM_JOB, &mut cmd)
        .or_else(|_| run(PROCESS_CREATION_FLAGS(0), &mut cmd))
        .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xffff))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_survive_the_command_line() {
        assert_eq!(quote_arg("かるかん"), "\"かるかん\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(quote_arg("C:\\dir\\"), "\"C:\\dir\\\\\"");
        assert_eq!(quote_arg(""), "\"\"");
    }

    /// `eligible` agrees with the integrity label `whoami` reports for this process.
    #[test]
    fn eligible_follows_the_integrity_level() {
        let out = std::process::Command::new("whoami")
            .args(["/groups", "/fo", "csv"])
            .output()
            .unwrap();
        let groups = String::from_utf8_lossy(&out.stdout);
        let level = [
            ("S-1-16-4096", 0x1000),
            ("S-1-16-8192", 0x2000),
            ("S-1-16-12288", 0x3000),
            ("S-1-16-16384", 0x4000),
        ]
        .iter()
        .find(|(sid, _)| groups.contains(sid))
        .map(|(_, rid)| *rid);
        assert_eq!(integrity(), level, "{groups}");
        assert_eq!(eligible(), level.is_some_and(|r| r >= 0x2000));
        assert!(user_sid().is_some_and(|s| s.starts_with("S-1-5-")));
    }
}
