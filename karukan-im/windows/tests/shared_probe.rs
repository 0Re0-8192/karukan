//! The shared-server path an installed text service takes, minus TSF: decide by
//! integrity level, start the server, connect, check who serves the pipe, init,
//! one key; twice, the second time as a second "app" finding the server running.
//! Runs where a thread manager cannot (an SSH session), so it can be run at medium
//! integrity on a PC with UAC on (`icacls <this exe> /setintegritylevel medium`).
//!
//! Needs `KARUKAN_IMSERVER` and `KARUKAN_SHARED=auto`; skipped otherwise.
#![cfg(windows)]

#[test]
fn an_app_reaches_the_shared_server() {
    if std::env::var_os("KARUKAN_IMSERVER").is_none()
        || std::env::var("KARUKAN_SHARED").as_deref() != Ok("auto")
    {
        eprintln!("KARUKAN_IMSERVER / KARUKAN_SHARED=auto not set; skipped");
        return;
    }
    let (name, first) = karukan_tsf::__shared_probe().expect("first app");
    let (again, second) = karukan_tsf::__shared_probe().expect("second app");
    eprintln!("pipe {name}: first app {first:?}, second app {second:?}");
    assert_eq!(name, again);
    assert!(second < std::time::Duration::from_millis(150), "{second:?}");
}
