use windows::core::GUID;

/// CLSID of the karukan text service.
pub const CLSID_KARUKAN: GUID = GUID::from_u128(0x4b9f6e5a_1d3c_4c7e_9a52_8e21f0c7a311);
/// Language profile GUID (Japanese).
pub const GUID_PROFILE: GUID = GUID::from_u128(0xa1c2e3d4_5f60_4718_9b2a_3c4d5e6f7081);
/// Display attribute: composing (not yet converted) text.
pub const GUID_ATTR_INPUT: GUID = GUID::from_u128(0x6d0a1b2c_3e4f_4a5b_8c6d_7e8f90a1b2c3);
/// Display attribute: the selected/converted segment.
pub const GUID_ATTR_CONVERTED: GUID = GUID::from_u128(0x7e1b2c3d_4f50_4b6c_9d7e_8f90a1b2c3d4);

pub const LANGID_JAPANESE: u16 = 0x0411;
pub const TIP_DESCRIPTION: &str = "Karukan";
