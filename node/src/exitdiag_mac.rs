//! DIAG only: name the threads alive at exit.
#![allow(clippy::all, deprecated)]

extern "C" fn dump() {
    unsafe {
        let mut threads: libc::thread_act_array_t = std::ptr::null_mut();
        let mut count: libc::mach_msg_type_number_t = 0;
        if libc::task_threads(libc::mach_task_self(), &mut threads, &mut count) != 0 {
            return;
        }
        let me = libc::pthread_self();
        let mut names = Vec::new();
        for i in 0..count as usize {
            let t = libc::pthread_from_mach_thread_np(*threads.add(i));
            if t == 0 as libc::pthread_t || t == me {
                continue;
            }
            let mut buf = [0 as libc::c_char; 64];
            libc::pthread_getname_np(t, buf.as_mut_ptr(), 64);
            let name = std::ffi::CStr::from_ptr(buf.as_ptr())
                .to_string_lossy()
                .into_owned();
            names.push(name);
        }
        names.sort();
        eprintln!("EXITDIAG {}", names.join(" | "));
    }
}

extern "C" fn init() {
    unsafe {
        libc::atexit(dump);
    }
}

#[used]
#[link_section = "__DATA,__mod_init_func"]
static INIT: extern "C" fn() = init;
