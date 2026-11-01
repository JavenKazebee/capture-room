//! DIAG only: list live threads (by start module) and modules at exit.
#![allow(non_snake_case, clippy::all)]
use std::ffi::c_void;

#[repr(C)]
struct THREADENTRY32 {
    dwSize: u32,
    cntUsage: u32,
    th32ThreadID: u32,
    th32OwnerProcessID: u32,
    tpBasePri: i32,
    tpDeltaPri: i32,
    dwFlags: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
    fn Thread32First(h: *mut c_void, e: *mut THREADENTRY32) -> i32;
    fn Thread32Next(h: *mut c_void, e: *mut THREADENTRY32) -> i32;
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleExW(flags: u32, addr: *const c_void, out: *mut *mut c_void) -> i32;
    fn GetModuleFileNameW(m: *mut c_void, buf: *mut u16, n: u32) -> u32;
    fn GetThreadDescription(h: *mut c_void, out: *mut *mut u16) -> i32;
}
#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationThread(
        h: *mut c_void,
        class: u32,
        info: *mut c_void,
        len: u32,
        ret: *mut u32,
    ) -> i32;
}
extern "C" {
    fn atexit(cb: extern "C" fn()) -> i32;
}

fn module_of(addr: *const c_void) -> String {
    unsafe {
        let mut m = std::ptr::null_mut();
        // FROM_ADDRESS | UNCHANGED_REFCOUNT
        if GetModuleHandleExW(4 | 2, addr, &mut m) == 0 {
            return format!("?{addr:p}");
        }
        let mut buf = [0u16; 512];
        let n = GetModuleFileNameW(m, buf.as_mut_ptr(), 512) as usize;
        let name = String::from_utf16_lossy(&buf[..n]);
        let base = name.rsplit('\\').next().unwrap_or(&name).to_string();
        format!("{base}+{:#x}", addr as usize - m as usize)
    }
}

extern "C" fn dump() {
    unsafe {
        let snap = CreateToolhelp32Snapshot(4, 0);
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let pid = GetCurrentProcessId();
        let me = GetCurrentThreadId();
        let mut lines = Vec::new();
        if Thread32First(snap, &mut e) != 0 {
            loop {
                if e.th32OwnerProcessID == pid && e.th32ThreadID != me {
                    let h = OpenThread(0x0040 | 0x0800, 0, e.th32ThreadID);
                    let mut start: *mut c_void = std::ptr::null_mut();
                    NtQueryInformationThread(
                        h,
                        9,
                        &mut start as *mut _ as *mut c_void,
                        8,
                        std::ptr::null_mut(),
                    );
                    let mut desc: *mut u16 = std::ptr::null_mut();
                    let mut name = String::new();
                    if GetThreadDescription(h, &mut desc) >= 0 && !desc.is_null() {
                        let mut n = 0;
                        while *desc.add(n) != 0 {
                            n += 1;
                        }
                        name = String::from_utf16_lossy(std::slice::from_raw_parts(desc, n));
                    }
                    lines.push(format!(
                        "EXITDIAG thread {} start={} name={name}",
                        e.th32ThreadID,
                        module_of(start)
                    ));
                    CloseHandle(h);
                }
                if Thread32Next(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
        lines.sort();
        for l in lines {
            eprintln!("{l}");
        }
    }
}

extern "C" fn init() {
    unsafe {
        atexit(dump);
    }
}

#[used]
#[link_section = ".CRT$XCU"]
static INIT: extern "C" fn() = init;
