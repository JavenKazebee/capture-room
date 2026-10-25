//! Reading values from the Windows registry (privacy settings, the taskbar's
//! theme). Read only, and only what's needed, so no Windows API crate.

use std::ffi::c_void;

#[derive(Debug, Clone, Copy)]
pub enum Root {
    CurrentUser,
    LocalMachine,
}

impl Root {
    /// `HKEY_CURRENT_USER` and `HKEY_LOCAL_MACHINE`: 32-bit constants that
    /// the headers sign-extend to pointer width.
    fn handle(self) -> isize {
        match self {
            Self::CurrentUser => 0x8000_0001_u32 as i32 as isize,
            Self::LocalMachine => 0x8000_0002_u32 as i32 as isize,
        }
    }
}

const RRF_RT_REG_SZ: u32 = 0x0000_0002;
const RRF_RT_REG_DWORD: u32 = 0x0000_0010;
const ERROR_SUCCESS: i32 = 0;

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        key: isize,
        sub_key: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A short `REG_SZ` value (under 256 characters), if it's set.
pub fn string(root: Root, key: &str, value: &str) -> Option<String> {
    let (key, value) = (wide(key), wide(value));
    let mut data = [0u16; 256];
    let mut size = std::mem::size_of_val(&data) as u32;
    // SAFETY: null-terminated names; `size` is the buffer's size in bytes,
    // and the call writes no more than that.
    let status = unsafe {
        RegGetValueW(
            root.handle(),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let len = data.iter().position(|c| *c == 0).unwrap_or(data.len());
    Some(String::from_utf16_lossy(&data[..len]))
}

/// A `REG_DWORD` value, if it's set.
pub fn dword(root: Root, key: &str, value: &str) -> Option<u32> {
    let (key, value) = (wide(key), wide(value));
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: as in `string`, into a single DWORD.
    let status = unsafe {
        RegGetValueW(
            root.handle(),
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}
