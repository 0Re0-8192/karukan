//! karukan-setup.exe: installs, updates and uninstalls karukan for Windows.
//!
//! The files ride at the end of the exe (`pack`): a deflate stream of (path, bytes) entries, then a
//! footer of magic, length and SHA-256, checked before anything is written. Running the exe installs,
//! or updates an existing install in place: the folder is found from Settings > Apps, or else from the
//! COM registration of the text service (installs made by install.ps1 or the zip). An update keeps
//! the learning, the user dictionaries and config.toml, skips files that did not change, renames aside
//! a file an app still holds (the implementation DLL is a new name per build anyway), removes files the
//! previous version owned and this one does not, and needs no admin unless the registered loader DLL
//! changed. `install.json` in the folder lists what the installer owns.
//!
//!   karukan-setup.exe                         install or update (dialogs)
//!   karukan-setup.exe --uninstall [--delete-data]
//!   karukan-setup.exe pack STAGE_DIR -o OUT.exe
//! Test switches: --dir PATH (install there), --no-register (no regsvr32, input list or Apps entry),
//! --quiet (no dialogs).
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("karukan-setup runs on Windows only");
}
