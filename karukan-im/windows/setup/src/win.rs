use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod sys;

const MAGIC: &[u8; 16] = b"KARUKAN-SETUP-01";
const FOOTER: u64 = 16 + 8 + 32;
const VERSION: &str = env!("CARGO_PKG_VERSION");
const LOADER: &str = "karukan_tsf.dll";
const SETUP_EXE: &str = "karukan-setup.exe";
const MANIFEST: &str = "install.json";

/// Files of the payload the installer never replaces once they exist (the user may have edited them).
const KEEP_IF_PRESENT: &[&str] = &["config/config.toml"];

#[derive(Default)]
struct Opts {
    dir: Option<PathBuf>,
    no_register: bool,
    quiet: bool,
    uninstall: bool,
    delete_data: bool,
    pack: Option<(PathBuf, PathBuf)>,
}

fn parse_args() -> Result<Opts> {
    let mut o = Opts::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dir" => o.dir = Some(PathBuf::from(it.next().context("--dir needs a path")?)),
            "--no-register" => o.no_register = true,
            "--quiet" => o.quiet = true,
            "--uninstall" => o.uninstall = true,
            "--delete-data" => o.delete_data = true,
            "pack" => {
                let stage = PathBuf::from(it.next().context("pack STAGE_DIR -o OUT.exe")?);
                if it.next().as_deref() != Some("-o") {
                    bail!("pack STAGE_DIR -o OUT.exe");
                }
                o.pack = Some((
                    stage,
                    PathBuf::from(it.next().context("pack needs -o OUT.exe")?),
                ));
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(o)
}

pub fn main() -> i32 {
    let o = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            sys::message("karukan", &format!("{e:#}"), true);
            return 2;
        }
    };
    let quiet = o.quiet || o.pack.is_some();
    let r = if let Some((stage, out)) = &o.pack {
        pack(stage, out)
    } else if o.uninstall {
        uninstall(&o)
    } else {
        install(&o)
    };
    match r {
        Ok(Some(msg)) => {
            if quiet {
                println!("{msg}");
            } else {
                sys::message("karukan", &msg, false);
            }
            0
        }
        Ok(None) => 1, // cancelled
        Err(e) => {
            if quiet {
                eprintln!("{e:#}");
            } else {
                sys::message("karukan", &format!("{e:#}"), true);
            }
            2
        }
    }
}

// ---------------------------------------------------------------- payload

/// Appends the files under `stage` to a copy of this exe.
fn pack(stage: &Path, out: &Path) -> Result<Option<String>> {
    let me = std::env::current_exe()?;
    // the bare program: this exe without any payload it may carry
    let exe_len = match payload_span(&me)? {
        Some((off, _, _)) => off,
        None => fs::metadata(&me)?.len(),
    };
    let mut files = Vec::new();
    collect(stage, stage, &mut files)?;
    files.sort();
    if files.is_empty() {
        bail!("{}: nothing to pack", stage.display());
    }
    let tmp = out.with_extension("tmp");
    {
        let mut w = BufWriter::new(File::create(&tmp).with_context(|| tmp.display().to_string())?);
        std::io::copy(&mut File::open(&me)?.take(exe_len), &mut w)?;
        w.flush()?;
        let mut hashed = HashWriter {
            inner: w,
            sha: Sha256::new(),
            len: 0,
        };
        {
            let mut z = DeflateEncoder::new(&mut hashed, Compression::default());
            for rel in &files {
                let path = stage.join(rel);
                let size = fs::metadata(&path)?.len();
                let name = rel.as_bytes();
                z.write_all(&(name.len() as u32).to_le_bytes())?;
                z.write_all(name)?;
                z.write_all(&size.to_le_bytes())?;
                std::io::copy(&mut File::open(&path)?, &mut z)?;
            }
            z.write_all(&0u32.to_le_bytes())?;
            z.finish()?;
        }
        let (mut w, sha, len) = (hashed.inner, hashed.sha.finalize(), hashed.len);
        w.write_all(MAGIC)?;
        w.write_all(&len.to_le_bytes())?;
        w.write_all(&sha)?;
        w.flush()?;
    }
    patient(|| out.display().to_string(), || fs::rename(&tmp, out))?;
    Ok(Some(format!(
        "{}: {} files, {} MB",
        out.display(),
        files.len(),
        fs::metadata(out)?.len() >> 20
    )))
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else {
            let rel = p.strip_prefix(root)?.to_string_lossy().replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

struct HashWriter<W: Write> {
    inner: W,
    sha: Sha256,
    len: u64,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(b)?;
        self.sha.update(&b[..n]);
        self.len += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// (offset of the payload, its length) when the exe carries one.
fn payload_span(exe: &Path) -> Result<Option<(u64, u64, [u8; 32])>> {
    let mut f = File::open(exe)?;
    let total = f.metadata()?.len();
    if total < FOOTER {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(total - FOOTER))?;
    let mut foot = [0u8; FOOTER as usize];
    f.read_exact(&mut foot)?;
    if &foot[..16] != MAGIC {
        return Ok(None);
    }
    let len = u64::from_le_bytes(foot[16..24].try_into().unwrap());
    let mut sha = [0u8; 32];
    sha.copy_from_slice(&foot[24..]);
    if len + FOOTER > total {
        bail!("インストーラーが壊れています。ダウンロードし直してください。");
    }
    Ok(Some((total - FOOTER - len, len, sha)))
}

/// The payload's entries, after its SHA-256 has been checked: path -> (offset in the stream order).
fn read_payload(
    exe: &Path,
    mut each: impl FnMut(&str, u64, &mut dyn Read) -> Result<()>,
) -> Result<()> {
    let (off, len, sha) = payload_span(exe)?.ok_or_else(|| anyhow!("this exe carries no files"))?;
    // check first, write nothing from a damaged download
    let mut f = File::open(exe)?;
    f.seek(SeekFrom::Start(off))?;
    if sha256_of(&mut (&mut f).take(len))?[..] != sha {
        bail!("インストーラーが壊れています。ダウンロードし直してください。");
    }
    f.seek(SeekFrom::Start(off))?;
    let mut z = DeflateDecoder::new(BufReader::new(f.take(len)));
    loop {
        let mut n = [0u8; 4];
        z.read_exact(&mut n)?;
        let n = u32::from_le_bytes(n) as usize;
        if n == 0 {
            return Ok(());
        }
        let mut name = vec![0u8; n];
        z.read_exact(&mut name)?;
        let name = String::from_utf8(name)?;
        let mut size = [0u8; 8];
        z.read_exact(&mut size)?;
        let size = u64::from_le_bytes(size);
        let mut body = (&mut z).take(size);
        each(&name, size, &mut body)?;
        std::io::copy(&mut body, &mut std::io::sink())?; // whatever `each` left
    }
}

// ---------------------------------------------------------------- install / update

#[derive(Serialize, Deserialize, Default)]
struct Manifest {
    version: String,
    /// path -> sha256 of every file the installer put there
    files: BTreeMap<String, String>,
}

fn load_manifest(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&fs::read(dir.join(MANIFEST)).ok()?).ok()
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Programs").join("karukan")
}

/// An existing install: the Apps entry, else the folder of the registered loader DLL.
fn existing_dir() -> Option<PathBuf> {
    if let Some(d) = sys::app_install_location()
        && d.join(LOADER).is_file()
    {
        return Some(d);
    }
    sys::com_server_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|d| d.join(LOADER).is_file())
}

fn install(o: &Opts) -> Result<Option<String>> {
    let me = std::env::current_exe()?;
    if payload_span(&me)?.is_none() {
        bail!("this exe carries no files (build one with `pack`)");
    }
    let found = if o.dir.is_some() {
        None
    } else {
        existing_dir()
    };
    let mut dir = o
        .dir
        .clone()
        .or_else(|| found.clone())
        .unwrap_or_else(default_dir);
    let old = load_manifest(&dir);
    let old_version = old
        .as_ref()
        .map(|m| m.version.clone())
        .or_else(|| found.as_ref().map(|_| "?".into()));
    if let Some(m) = &old
        && version_key(&m.version) > version_key(VERSION)
    {
        bail!(
            "karukan {} が入っています(このインストーラーは {VERSION})。",
            m.version
        );
    }
    if !o.quiet {
        let update = old_version.is_some() || dir.join(LOADER).is_file();
        match sys::confirm_install(VERSION, old_version.as_deref(), &dir, !update) {
            sys::Choice::Go => {}
            sys::Choice::Cancel => return Ok(None),
            sys::Choice::Change => match sys::pick_folder()? {
                Some(d) => {
                    dir = if d
                        .file_name()
                        .is_some_and(|n| n.eq_ignore_ascii_case("karukan"))
                    {
                        d
                    } else {
                        d.join("karukan")
                    };
                    if !sys::confirm_go(VERSION, &dir) {
                        return Ok(None);
                    }
                }
                None => return Ok(None),
            },
        }
    }
    fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let loader_before = hash_file(&dir.join(LOADER)).ok();
    let stamp = stamp();
    let mut owned = BTreeMap::new();
    let old_files: BTreeMap<String, String> = old.map(|m| m.files).unwrap_or_default();
    read_payload(&me, |rel, _size, body| {
        let target = dir.join(rel);
        if KEEP_IF_PRESENT.contains(&rel) && target.exists() {
            // still ours to list (or a later update would take it for a file that left the payload)
            owned.insert(
                rel.to_string(),
                old_files.get(rel).cloned().unwrap_or_default(),
            );
            return Ok(());
        }
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).with_context(|| p.display().to_string())?;
        }
        let tmp = target.with_file_name(format!("{}.new{stamp}", file_name(&target)));
        let sha = {
            let f = File::create(&tmp).with_context(|| tmp.display().to_string())?;
            let mut w = HashWriter {
                inner: BufWriter::new(f),
                sha: Sha256::new(),
                len: 0,
            };
            std::io::copy(body, &mut w)?;
            w.flush()?;
            hex(&w.sha.finalize())
        };
        owned.insert(rel.to_string(), sha.clone());
        if target.is_file() && hash_file(&target).ok().as_deref() == Some(sha.as_str()) {
            patient(|| tmp.display().to_string(), || fs::remove_file(&tmp))?; // unchanged
            return Ok(());
        }
        replace(&tmp, &target, &stamp)
            .with_context(|| format!("could not place {}", target.display()))
    })?;
    // what the previous version owned and this one does not (old models, old implementation DLLs)
    for rel in old_files.keys().filter(|k| !owned.contains_key(*k)) {
        let _ = remove_or_aside(&dir.join(rel), &stamp);
    }
    sweep_aside(&dir);
    // apps in an AppContainer (Start search, Settings) read the DLLs and models; nothing gets write access
    let _ = sys::run_hidden(
        "icacls",
        &[
            &dir.to_string_lossy(),
            "/grant",
            "*S-1-15-2-1:(OI)(CI)(RX)",
            "/Q",
        ],
    );
    // the uninstaller: this exe without its payload
    if let Some((off, _, _)) = payload_span(&me)? {
        let dst = dir.join(SETUP_EXE);
        if dst != me {
            let tmp = dir.join(format!("{SETUP_EXE}.new{stamp}"));
            std::io::copy(
                &mut File::open(&me)?.take(off),
                &mut File::create(&tmp).with_context(|| tmp.display().to_string())?,
            )?;
            replace(&tmp, &dst, &stamp)?;
        }
    }
    let manifest = Manifest {
        version: VERSION.into(),
        files: owned,
    };
    fs::write(dir.join(MANIFEST), serde_json::to_vec_pretty(&manifest)?)
        .with_context(|| dir.join(MANIFEST).display().to_string())?;
    let size_kb = manifest
        .files
        .keys()
        .filter_map(|r| fs::metadata(dir.join(r)).ok())
        .map(|m| m.len())
        .sum::<u64>()
        >> 10;

    let mut warning = None;
    if !o.no_register {
        let loader = dir.join(LOADER);
        let registered = sys::com_server_path();
        let same_path = registered.as_ref().is_some_and(|p| p.eq(&loader));
        if !same_path || loader_before.as_deref() != hash_file(&loader).ok().as_deref() {
            sys::regsvr32(&loader, false).context("regsvr32")?;
        }
        sys::install_layout_or_tip(true).context("入力方式の一覧")?;
        // only the Settings > Apps entry: typing works without it, so a refusal is reported, not fatal
        // (seen on one Windows 11 PC: the key denied to this exe and to processes it starts)
        if let Err(e) = sys::write_app_entry(&dir, VERSION, size_kb) {
            warning = Some(format!(
                "設定 > アプリに登録できませんでした({e})。外すときは {} --uninstall",
                dir.join(SETUP_EXE).display()
            ));
        }
    }
    // running servers swap themselves for the new one at the next composition
    let stamp_file = dir.join("data").join("user_english.txt");
    if stamp_file.is_file() {
        let _ = File::options()
            .write(true)
            .open(&stamp_file)
            .and_then(|f| f.set_modified(SystemTime::now()));
    }
    let done = match old_version {
        Some(v) if v == VERSION || v == "?" => format!("karukan {VERSION} を入れ直しました。"),
        Some(v) => format!("karukan を {v} から {VERSION} に更新しました。"),
        None => format!("karukan {VERSION} を入れました。Win+Space で選べます。"),
    };
    Ok(Some(match warning {
        Some(w) => format!(
            "{done}
{w}"
        ),
        None => done,
    }))
}

/// Puts `tmp` at `target`; a target an app holds is renamed aside first.
fn replace(tmp: &Path, target: &Path, stamp: &str) -> Result<()> {
    if target.exists() && fs::remove_file(target).is_err() {
        let aside = target.with_file_name(format!("{}.old{stamp}", file_name(target)));
        patient(
            || target.display().to_string(),
            || fs::rename(target, &aside),
        )?;
    }
    patient(|| target.display().to_string(), || fs::rename(tmp, target))
}

/// A file operation that a virus scanner or the indexer may hold for a moment (access denied, sharing
/// violation, lock violation): tried again for up to 3 s.
fn patient<T>(what: impl Fn() -> String, mut f: impl FnMut() -> std::io::Result<T>) -> Result<T> {
    let mut last = None;
    for _ in 0..30 {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(anyhow::Error::new(e).context(what())),
        }
    }
    Err(anyhow::Error::new(last.expect("tried")).context(what()))
}

fn remove_or_aside(p: &Path, stamp: &str) -> Result<()> {
    if !p.exists() || fs::remove_file(p).is_ok() {
        return Ok(());
    }
    let aside = p.with_file_name(format!("{}.old{stamp}", file_name(p)));
    patient(|| p.display().to_string(), || fs::rename(p, &aside))
}

/// Files renamed aside by earlier updates that nothing holds any more.
fn sweep_aside(dir: &Path) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_aside(&file_name(&p)) {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn is_aside(name: &str) -> bool {
    [".old", ".new"].iter().any(|t| {
        name.rfind(t).is_some_and(|i| {
            name[i + t.len()..].chars().all(|c| c.is_ascii_digit()) && name.len() > i + t.len()
        })
    })
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stamp() -> String {
    let s = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    s.to_string()
}

fn sha256_of(r: &mut impl Read) -> Result<[u8; 32]> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

fn hash_file(p: &Path) -> Result<String> {
    Ok(hex(&sha256_of(&mut File::open(p)?)?))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------- uninstall

fn uninstall(o: &Opts) -> Result<Option<String>> {
    let me = std::env::current_exe()?;
    let dir = match &o.dir {
        Some(d) => d.clone(),
        None => existing_dir()
            .or_else(|| me.parent().map(Path::to_path_buf))
            .context("karukan は入っていません。")?,
    };
    let manifest = load_manifest(&dir)
        .context("このフォルダはインストーラーで入れたものではありません(install.json がない)。")?;
    let mut delete_data = o.delete_data;
    if !o.quiet {
        match sys::confirm_uninstall(&dir) {
            None => return Ok(None),
            Some(d) => delete_data = d,
        }
    }
    if !o.no_register {
        sys::install_layout_or_tip(false)?;
        let loader = dir.join(LOADER);
        if sys::com_server_path().is_some_and(|p| p == loader) {
            sys::regsvr32(&loader, true)?;
        }
        sys::delete_app_entry();
    }
    // the shared server outlives the apps: stop it so its files can go
    sys::stop_servers(&dir);
    let stamp = stamp();
    let mut left = BTreeSet::new();
    for rel in manifest.files.keys() {
        if !delete_data && KEEP_IF_PRESENT.contains(&rel.as_str()) {
            continue; // the settings stay unless asked
        }
        let p = dir.join(rel);
        if remove_or_aside(&p, &stamp).is_err() || p.exists() {
            left.insert(rel.clone());
        }
    }
    let _ = fs::remove_file(dir.join(MANIFEST));
    if delete_data {
        for d in ["data", "config"] {
            let _ = patient(|| d.to_string(), || fs::remove_dir_all(dir.join(d)));
        }
    }
    sweep_aside(&dir);
    remove_empty_dirs(&dir);
    // this exe cannot delete itself: a detached cmd does it once we have exited
    if me.parent() == Some(dir.as_path()) {
        // also what this exe held while it ran (its runtime DLLs, renamed aside above)
        let cmd = format!(
            "ping -n 3 127.0.0.1 >nul & del /f /q \"{exe}\" \"{d}\\*.old*\" 2>nul & rmdir \"{d}\" 2>nul",
            exe = me.display(),
            d = dir.display()
        );
        let _ = sys::spawn_cmd_detached(&cmd);
    }
    let kept = if delete_data {
        ""
    } else {
        "学習と設定は残しました。"
    };
    Ok(Some(if left.is_empty() {
        format!("karukan を外しました。{kept}")
    } else {
        format!(
            "karukan を外しました。{kept}使用中のファイルはアプリを閉じたあと消せます({})。",
            dir.display()
        )
    }))
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                remove_empty_dirs(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aside_names() {
        assert!(is_aside("karukan-imserver.exe.old1795259494"));
        assert!(is_aside("dict.bin.new1795259494"));
        assert!(!is_aside("karukan-imserver.exe"));
        assert!(!is_aside("old.txt"));
        assert!(!is_aside("x.old"));
    }

    #[test]
    fn versions_order() {
        assert!(version_key("0.10.0") > version_key("0.9.1"));
        assert!(version_key("0.2.0") == version_key("0.2.0"));
    }
}
