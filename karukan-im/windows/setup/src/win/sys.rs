//! The Win32 side: dialogs, the folder picker, the registry, regsvr32 (elevated) and the input list.
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION, TaskDialogIndirect,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    SIGDN_FILESYSPATH, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW, SW_HIDE};
use windows::core::{BOOL, PCWSTR, s, w};

const CLSID: &str = "{4B9F6E5A-1D3C-4C7E-9A52-8E21F0C7A311}";
const PROFILE: &str = "{A1C2E3D4-5F60-4718-9B2A-3C4D5E6F7081}";
const APP_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\karukan";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn message(title: &str, text: &str, error: bool) {
    let (t, x) = (wide(title), wide(text));
    let icon = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    unsafe {
        MessageBoxW(None, PCWSTR(x.as_ptr()), PCWSTR(t.as_ptr()), MB_OK | icon);
    }
}

pub enum Choice {
    Go,
    Change,
    Cancel,
}

const ID_GO: i32 = 100;
const ID_CHANGE: i32 = 101;

/// Runs a task dialog with the given buttons (id, label) plus Cancel; returns the button and the checkbox.
fn task_dialog(main: &str, content: &str, buttons: &[(i32, &str)], check: Option<&str>) -> (i32, bool) {
    let (title, main, content) = (wide("karukan"), wide(main), wide(content));
    let labels: Vec<Vec<u16>> = buttons.iter().map(|(_, l)| wide(l)).collect();
    let btns: Vec<TASKDIALOG_BUTTON> = buttons
        .iter()
        .zip(&labels)
        .map(|((id, _), l)| TASKDIALOG_BUTTON { nButtonID: *id, pszButtonText: PCWSTR(l.as_ptr()) })
        .collect();
    let check_w = check.map(wide);
    let cfg = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(main.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: btns.len() as u32,
        pButtons: btns.as_ptr(),
        nDefaultButton: buttons.first().map_or(0, |b| b.0),
        pszVerificationText: check_w.as_ref().map_or(PCWSTR::null(), |c| PCWSTR(c.as_ptr())),
        ..Default::default()
    };
    let mut pressed = 0i32;
    let mut checked = BOOL(0);
    let r = unsafe { TaskDialogIndirect(&cfg, Some(&mut pressed), None, Some(&mut checked)) };
    if r.is_err() {
        return (0, false);
    }
    (pressed, checked.as_bool())
}

pub fn confirm_install(version: &str, old: Option<&str>, dir: &Path, can_change: bool) -> Choice {
    let main = match old {
        Some("?") | None if !can_change => format!("karukan を {version} に更新"),
        Some(v) if v != "?" => format!("karukan を {v} から {version} に更新"),
        Some(_) => format!("karukan を {version} に更新"),
        None => format!("karukan {version} をインストール"),
    };
    let go = if can_change { "インストール" } else { "更新" };
    let mut buttons = vec![(ID_GO, go)];
    if can_change {
        buttons.push((ID_CHANGE, "場所を変える"));
    }
    match task_dialog(&main, &dir.display().to_string(), &buttons, None).0 {
        ID_GO => Choice::Go,
        ID_CHANGE => Choice::Change,
        _ => Choice::Cancel,
    }
}

pub fn confirm_go(version: &str, dir: &Path) -> bool {
    let main = format!("karukan {version} をインストール");
    task_dialog(&main, &dir.display().to_string(), &[(ID_GO, "インストール")], None).0 == ID_GO
}

/// Some(delete learning and settings too) or None when cancelled.
pub fn confirm_uninstall(dir: &Path) -> Option<bool> {
    let (b, checked) =
        task_dialog("karukan を外す", &dir.display().to_string(), &[(ID_GO, "外す")], Some("学習と設定も消す"));
    (b == ID_GO).then_some(checked)
}

pub fn pick_folder() -> Result<Option<PathBuf>> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        dlg.SetOptions(dlg.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        if dlg.Show(None).is_err() {
            return Ok(None); // cancelled
        }
        let p = dlg.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let s = p.to_string()?;
        CoTaskMemFree(Some(p.0 as _));
        Ok(Some(PathBuf::from(s)))
    }
}

fn reg_string(root: HKEY, sub: &str, value: Option<&str>) -> Option<String> {
    let (sub, value) = (wide(sub), value.map(wide));
    let vp = value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(root, PCWSTR(sub.as_ptr()), vp, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut len))
    };
    if r.is_err() {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The DLL the text service's class is registered to (HKCR\CLSID\{...}\InprocServer32).
pub fn com_server_path() -> Option<PathBuf> {
    reg_string(HKEY_CLASSES_ROOT, &format!(r"CLSID\{CLSID}\InprocServer32"), None).map(PathBuf::from)
}

pub fn app_install_location() -> Option<PathBuf> {
    reg_string(HKEY_CURRENT_USER, APP_KEY, Some("InstallLocation")).map(PathBuf::from)
}

pub fn write_app_entry(dir: &Path, version: &str, size_kb: u64) -> Result<()> {
    let sub = wide(APP_KEY);
    let mut key = HKEY::default();
    unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut key, None)
            .ok()?;
    }
    let exe = dir.join("karukan-setup.exe");
    let strings = [
        ("DisplayName", "karukan".to_string()),
        ("DisplayVersion", version.to_string()),
        ("Publisher", "karukan".to_string()),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", format!("\"{}\" --uninstall", exe.display())),
        ("DisplayIcon", exe.display().to_string()),
    ];
    let result = (|| -> Result<()> {
        for (name, v) in &strings {
            let (n, v) = (wide(name), wide(v));
            let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
            unsafe { RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_SZ, Some(bytes)).ok()? };
        }
        for (name, v) in [("NoModify", 1u32), ("NoRepair", 1), ("EstimatedSize", size_kb.min(u32::MAX as u64) as u32)] {
            let n = wide(name);
            unsafe { RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&v.to_le_bytes())).ok()? };
        }
        Ok(())
    })();
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

pub fn delete_app_entry() {
    let sub = wide(APP_KEY);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(sub.as_ptr()));
    }
}

/// regsvr32 [/u] /s DLL with the admin prompt (the only step that needs admin).
pub fn regsvr32(dll: &Path, unregister: bool) -> Result<()> {
    if unsafe { IsUserAnAdmin() }.as_bool() {
        // already elevated (an admin shell, ssh): no prompt to show
        let mut args = vec![];
        if unregister {
            args.push("/u".to_string());
        }
        args.push("/s".into());
        args.push(dll.display().to_string());
        let st = Command::new("regsvr32.exe").args(&args).creation_flags(CREATE_NO_WINDOW).status()?;
        if !st.success() {
            bail!("regsvr32 が失敗しました({st})");
        }
        return Ok(());
    }
    let params = wide(&format!("{}/s \"{}\"", if unregister { "/u " } else { "" }, dll.display()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("regsvr32.exe"),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        if e.code() == ERROR_CANCELLED.to_hresult() {
            bail!("管理者の確認が取り消されました。");
        }
        return Err(e.into());
    }
    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    if code != 0 {
        bail!("regsvr32 が失敗しました(終了コード {code})");
    }
    Ok(())
}

/// Adds the input method to (or removes it from) the current user's list.
pub fn install_layout_or_tip(add: bool) -> Result<()> {
    type F = unsafe extern "system" fn(PCWSTR, u32) -> BOOL;
    let spec = wide(&format!("0411:{CLSID}{PROFILE}"));
    unsafe {
        let lib = LoadLibraryW(w!("input.dll"))?;
        let Some(f) = GetProcAddress(lib, s!("InstallLayoutOrTip")) else { bail!("InstallLayoutOrTip が見つかりません") };
        let f: F = std::mem::transmute(f);
        // flags: 0 = add, 1 (ILOT_UNINSTALL) = remove
        if !f(PCWSTR(spec.as_ptr()), if add { 0 } else { 1 }).as_bool() && add {
            bail!("入力方式の一覧に入れられませんでした");
        }
    }
    Ok(())
}

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DETACHED_PROCESS: u32 = 0x0000_0008;

pub fn run_hidden(exe: &str, args: &[&str]) -> Result<()> {
    let st = Command::new(exe)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !st.success() {
        bail!("{exe} failed: {st}");
    }
    Ok(())
}

pub fn spawn_detached(exe: &str, args: &[&str]) -> Result<()> {
    Command::new(exe).args(args).creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS).spawn()?;
    Ok(())
}

#[allow(dead_code)]
fn _unused(_: HWND) {}
