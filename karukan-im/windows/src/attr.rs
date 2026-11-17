//! Display attribute provider: tells the host how to draw composition text
//! (dotted underline while typing, thick underline for the selected segment).

use std::cell::Cell;

use windows::Win32::Foundation::{E_INVALIDARG, S_FALSE};
use windows::Win32::UI::TextServices::{
    IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
    ITfDisplayAttributeInfo_Impl, TF_ATTR_INPUT, TF_ATTR_TARGET_CONVERTED, TF_CT_NONE,
    TF_DA_COLOR, TF_DISPLAYATTRIBUTE, TF_LS_DOT, TF_LS_SOLID,
};
use windows::core::{BOOL, BSTR, GUID, Ref, Result, implement};

use crate::guids::{GUID_ATTR_CONVERTED, GUID_ATTR_INPUT};

fn no_color() -> TF_DA_COLOR {
    let mut c = TF_DA_COLOR::default();
    c.r#type = TF_CT_NONE;
    c
}

pub fn input_attr() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        crText: no_color(),
        crBk: no_color(),
        lsStyle: TF_LS_DOT,
        fBoldLine: BOOL(0),
        crLine: no_color(),
        bAttr: TF_ATTR_INPUT,
    }
}

pub fn converted_attr() -> TF_DISPLAYATTRIBUTE {
    TF_DISPLAYATTRIBUTE {
        crText: no_color(),
        crBk: no_color(),
        lsStyle: TF_LS_SOLID,
        fBoldLine: BOOL(1),
        crLine: no_color(),
        bAttr: TF_ATTR_TARGET_CONVERTED,
    }
}

#[implement(ITfDisplayAttributeInfo)]
pub struct AttrInfo {
    guid: GUID,
    desc: &'static str,
    attr: TF_DISPLAYATTRIBUTE,
}

impl AttrInfo {
    pub fn input() -> ITfDisplayAttributeInfo {
        AttrInfo {
            guid: GUID_ATTR_INPUT,
            desc: "Karukan Input",
            attr: input_attr(),
        }
        .into()
    }
    pub fn converted() -> ITfDisplayAttributeInfo {
        AttrInfo {
            guid: GUID_ATTR_CONVERTED,
            desc: "Karukan Converted",
            attr: converted_attr(),
        }
        .into()
    }
    pub fn lookup(guid: &GUID) -> Option<ITfDisplayAttributeInfo> {
        if *guid == GUID_ATTR_INPUT {
            Some(Self::input())
        } else if *guid == GUID_ATTR_CONVERTED {
            Some(Self::converted())
        } else {
            None
        }
    }
}

impl ITfDisplayAttributeInfo_Impl for AttrInfo_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(self.guid)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.desc))
    }
    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if pda.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe { *pda = self.attr };
        Ok(())
    }
    fn SetAttributeInfo(&self, _pda: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        Ok(())
    }
    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}

#[implement(IEnumTfDisplayAttributeInfo)]
pub struct EnumAttr {
    pos: Cell<usize>,
}

impl EnumAttr {
    pub fn new() -> IEnumTfDisplayAttributeInfo {
        EnumAttr { pos: Cell::new(0) }.into()
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumAttr_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        let e = EnumAttr {
            pos: Cell::new(self.pos.get()),
        };
        Ok(e.into())
    }
    fn Next(
        &self,
        ulcount: u32,
        rginfo: *mut Option<ITfDisplayAttributeInfo>,
        pcfetched: *mut u32,
    ) -> Result<()> {
        let mut fetched = 0u32;
        while fetched < ulcount && self.pos.get() < 2 {
            let info = if self.pos.get() == 0 {
                AttrInfo::input()
            } else {
                AttrInfo::converted()
            };
            // the caller's array is uninitialised: write, never assign (which would drop garbage)
            unsafe { rginfo.add(fetched as usize).write(Some(info)) };
            self.pos.set(self.pos.get() + 1);
            fetched += 1;
        }
        if !pcfetched.is_null() {
            unsafe { *pcfetched = fetched };
        }
        if fetched == ulcount {
            Ok(())
        } else {
            Err(S_FALSE.into())
        }
    }
    fn Reset(&self) -> Result<()> {
        self.pos.set(0);
        Ok(())
    }
    fn Skip(&self, ulcount: u32) -> Result<()> {
        self.pos.set((self.pos.get() + ulcount as usize).min(2));
        Ok(())
    }
}

#[allow(dead_code)]
fn _unused(_: Ref<'_, ITfDisplayAttributeInfo>) {}
