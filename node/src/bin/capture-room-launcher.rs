//! Windows: starts `capture-room.exe` beside it with the same arguments and
//! no console window, then exits. The logon task runs the service through it
//! (`service run`), and the Start menu shortcut opens the app with it (no
//! arguments). A GUI program, so Windows opens no console for it either.
//! Does nothing elsewhere.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
#[path = "../service/windows_flags.rs"]
mod flags;

fn main() {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        use flags::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};

        let Ok(exe) = std::env::current_exe() else {
            std::process::exit(1);
        };
        let mut node = std::process::Command::new(exe.with_file_name("capture-room.exe"));
        node.args(std::env::args_os().skip(1));
        // Out of Task Scheduler's job for the task, where it allows that,
        // so ending the task can't end the node.
        let started = node
            .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .or_else(|_| node.creation_flags(CREATE_NO_WINDOW).spawn());
        std::process::exit(if started.is_ok() { 0 } else { 1 });
    }
}
