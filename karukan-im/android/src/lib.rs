//! JNI surface for the Gboard patch (`app.re0.extension.gboard.KarukanNative`).
//!
//! * `init(dataDir, configDir)` points the engine at the app's files (config.toml, models,
//!   dict.bin, learning cache) and starts loading the model in the background.
//! * `handle(command)` takes the serialized Mozc `Command` Gboard is about to send to Mozc and
//!   returns the reply when karukan owns the input, or `null` to let Mozc run it.
//! * `observe(command, mozcReply)` reports what Mozc answered; returns an amended reply or `null`.
//! * `shadowCommand` / `shadowReply` / `shadowRevert` / `setHybrid`: the Mozc candidates added to karukan's (hybrid).
#![cfg(target_os = "android")]

use std::sync::Mutex;

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::jbyteArray;
use karukan_mozc::{Bridge, ImBackend};

static BRIDGE: Mutex<Option<Bridge<ImBackend>>> = Mutex::new(None);

fn s(env: &mut JNIEnv, j: &JString) -> String {
    env.get_string(j).map(|s| s.into()).unwrap_or_default()
}

fn bytes(env: &JNIEnv, a: &JByteArray) -> Vec<u8> {
    env.convert_byte_array(a).unwrap_or_default()
}

fn ret(env: &JNIEnv, v: Option<Vec<u8>>) -> jbyteArray {
    match v.and_then(|b| env.byte_array_from_slice(&b).ok()) {
        Some(a) => a.into_raw(),
        None => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_init(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    config_dir: JString,
) {
    let (d, c) = (s(&mut env, &data_dir), s(&mut env, &config_dir));
    // SAFETY: called once on the IME thread before the engine is used.
    unsafe {
        std::env::set_var("KARUKAN_DATA_DIR", d);
        std::env::set_var("KARUKAN_CONFIG_DIR", c);
    }
    let mut g = BRIDGE.lock().unwrap();
    if g.is_none() {
        let mut b = Bridge::new(ImBackend::new());
        b.init();
        *g = Some(b);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_handle(
    env: JNIEnv,
    _class: JClass,
    command: JByteArray,
) -> jbyteArray {
    let cmd = bytes(&env, &command);
    let out =
        std::panic::catch_unwind(|| BRIDGE.lock().ok()?.as_mut().and_then(|b| b.handle(&cmd)))
            .ok()
            .flatten();
    ret(&env, out)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_observe(
    env: JNIEnv,
    _class: JClass,
    command: JByteArray,
    reply: JByteArray,
) -> jbyteArray {
    let (cmd, rep) = (bytes(&env, &command), bytes(&env, &reply));
    let out = std::panic::catch_unwind(|| {
        BRIDGE
            .lock()
            .ok()?
            .as_mut()
            .and_then(|b| b.observe(&cmd, &rep))
    })
    .ok()
    .flatten();
    ret(&env, out)
}

/// After `handle` answered `command`: the command to give Mozc in the background (a mirrored key, or
/// a revert when the karukan composition has ended), or `null`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_shadowCommand(
    env: JNIEnv,
    _class: JClass,
    command: JByteArray,
) -> jbyteArray {
    let cmd = bytes(&env, &command);
    let out = std::panic::catch_unwind(|| {
        BRIDGE
            .lock()
            .ok()?
            .as_mut()
            .and_then(|b| b.shadow_command(&cmd))
    })
    .ok()
    .flatten();
    ret(&env, out)
}

/// Mozc's answer to the background command: the karukan reply with Mozc's candidates added, or `null`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_shadowReply(
    env: JNIEnv,
    _class: JClass,
    mozc_reply: JByteArray,
) -> jbyteArray {
    let rep = bytes(&env, &mozc_reply);
    let out = std::panic::catch_unwind(|| {
        BRIDGE
            .lock()
            .ok()?
            .as_mut()
            .and_then(|b| b.shadow_reply(&rep))
    })
    .ok()
    .flatten();
    ret(&env, out)
}

/// Before `command` goes to Mozc for real: the command that clears the background composition first, or `null`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_shadowRevert(
    env: JNIEnv,
    _class: JClass,
    command: JByteArray,
) -> jbyteArray {
    let cmd = bytes(&env, &command);
    let out = std::panic::catch_unwind(|| {
        BRIDGE
            .lock()
            .ok()?
            .as_mut()
            .and_then(|b| b.shadow_revert(&cmd))
    })
    .ok()
    .flatten();
    ret(&env, out)
}

/// Turns the Mozc candidates on (`true`, the default) or off.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_re0_extension_gboard_KarukanNative_setHybrid(
    _env: JNIEnv,
    _class: JClass,
    on: jni::sys::jboolean,
) {
    if let Ok(mut g) = BRIDGE.lock() {
        if let Some(b) = g.as_mut() {
            b.set_hybrid(on != 0);
        }
    }
}
