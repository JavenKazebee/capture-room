//! The service on OSes it isn't built for yet (Linux: a systemd user unit;
//! Windows: a logon task). Run the node from a shell there.

use std::path::PathBuf;

use anyhow::{bail, Result};

use super::Installed;

pub fn data_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

pub fn app_path() -> Option<PathBuf> {
    None
}

pub fn install(_controller: bool, _port: Option<u16>) -> Result<Installed> {
    bail!("installing as a service isn't supported on this OS yet")
}

pub fn uninstall(_purge: bool) -> Result<()> {
    bail!("installing as a service isn't supported on this OS yet")
}

pub fn installed_port() -> Option<u16> {
    None
}

pub fn installed_db() -> Option<PathBuf> {
    None
}

pub fn restart() -> Result<()> {
    bail!("installing as a service isn't supported on this OS yet")
}

pub fn is_running() -> bool {
    false
}

pub fn start_at_login() -> bool {
    false
}

pub fn set_start_at_login(_on: bool) -> Result<()> {
    bail!("installing as a service isn't supported on this OS yet")
}

pub fn start() -> Result<()> {
    bail!("installing as a service isn't supported on this OS yet")
}
