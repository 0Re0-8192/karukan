//! COM + TSF registration (regsvr32 entry points). Needs an elevated process.

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT, GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    GUID_TFCAT_TIPCAP_UIELEMENTENABLED, ITfCategoryMgr, ITfInputProcessorProfileMgr,
};
use windows::core::{GUID, PCWSTR, Result};

use crate::guids::{CLSID_KARUKAN, GUID_PROFILE, LANGID_JAPANESE, TIP_DESCRIPTION};

fn wz(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn clsid_string() -> String {
    format!("{{{:?}}}", CLSID_KARUKAN).to_uppercase()
}

unsafe fn set_str(key: HKEY, name: Option<&str>, value: &str) -> Result<()> {
    let v = wz(value);
    let bytes = unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2) };
    let n = name.map(wz);
    unsafe {
        RegSetValueExW(
            key,
            n.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            None,
            REG_SZ,
            Some(bytes),
        )
        .ok()
    }
}

pub fn register() -> Result<()> {
    let path = crate::dll_path()
        .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    unsafe {
        // COM server
        let base = format!("CLSID\\{}", clsid_string());
        let mut k = HKEY::default();
        RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wz(&base).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut k,
            None,
        )
        .ok()?;
        set_str(k, None, TIP_DESCRIPTION)?;
        let _ = RegCloseKey(k);
        let mut k2 = HKEY::default();
        RegCreateKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wz(&format!("{base}\\InprocServer32")).as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut k2,
            None,
        )
        .ok()?;
        set_str(k2, None, &path)?;
        set_str(k2, Some("ThreadingModel"), "Apartment")?;
        let _ = RegCloseKey(k2);

        // TSF profile + categories
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = register_tsf(&path);
        CoUninitialize();
        r
    }
}

unsafe fn register_tsf(icon_path: &str) -> Result<()> {
    let mgr: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };
    let desc: Vec<u16> = TIP_DESCRIPTION.encode_utf16().collect();
    let icon: Vec<u16> = icon_path.encode_utf16().collect();
    unsafe {
        mgr.RegisterProfile(
            &CLSID_KARUKAN,
            LANGID_JAPANESE,
            &GUID_PROFILE,
            &desc,
            &icon,
            0,
            Default::default(),
            0,
            true,
            0,
        )?;
    }
    let cat: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };
    for c in categories() {
        unsafe { cat.RegisterCategory(&CLSID_KARUKAN, &c, &CLSID_KARUKAN)? };
    }
    Ok(())
}

fn categories() -> [GUID; 5] {
    [
        GUID_TFCAT_TIP_KEYBOARD,
        GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
        GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
        GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    ]
}

pub fn unregister() -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Ok(cat) =
            CoCreateInstance::<_, ITfCategoryMgr>(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)
        {
            for c in categories() {
                let _ = cat.UnregisterCategory(&CLSID_KARUKAN, &c, &CLSID_KARUKAN);
            }
        }
        if let Ok(mgr) = CoCreateInstance::<_, ITfInputProcessorProfileMgr>(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            let _ = mgr.UnregisterProfile(&CLSID_KARUKAN, LANGID_JAPANESE, &GUID_PROFILE, 0);
        }
        CoUninitialize();
        let _ = RegDeleteTreeW(
            HKEY_CLASSES_ROOT,
            PCWSTR(wz(&format!("CLSID\\{}", clsid_string())).as_ptr()),
        );
        // UnregisterProfile / UnregisterCategory leave the TIP's key behind, empty
        let _ = RegDeleteTreeW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(
                wz(&format!(
                    "SOFTWARE\\Microsoft\\CTF\\TIP\\{}",
                    clsid_string()
                ))
                .as_ptr(),
            ),
        );
    }
    Ok(())
}
