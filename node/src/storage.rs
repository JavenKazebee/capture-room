//! Storage volumes this node can record to.

use sysinfo::Disks;

use crate::api::types::StorageVolumeDto;

/// Filesystems that never hold recordings.
const PSEUDO_FS: &[&str] = &[
    "autofs", "devfs", "devtmpfs", "tmpfs", "overlay", "squashfs", "proc", "sysfs", "nullfs",
];

/// List writable mounted volumes, skipping pseudo filesystems. Read-only
/// mounts (disk images, the sealed macOS system volume) can't hold
/// recordings, so they're left out. Blocking: call from `spawn_blocking`.
pub fn list_volumes() -> Vec<StorageVolumeDto> {
    let disks = Disks::new_with_refreshed_list();
    let mut volumes: Vec<StorageVolumeDto> = disks
        .list()
        .iter()
        .filter(|d| {
            let fs = d.file_system().to_string_lossy().to_lowercase();
            let mount = d.mount_point().to_string_lossy();
            !PSEUDO_FS.contains(&fs.as_str())
                && d.total_space() > 0
                && !d.is_read_only()
                // macOS: user data lives on /System/Volumes/Data; the other
                // system sub-volumes (VM, Preboot, …) are not recording targets.
                && (!mount.starts_with("/System/Volumes/") || mount == "/System/Volumes/Data")
                && !mount.starts_with("/boot")
                && !mount.starts_with("/snap/")
        })
        .map(|d| {
            let mount = d.mount_point();
            StorageVolumeDto {
                name: d.name().to_string_lossy().into_owned(),
                mount_point: mount.to_string_lossy().into_owned(),
                file_system: d.file_system().to_string_lossy().into_owned(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                removable: d.is_removable(),
            }
        })
        .collect();
    volumes.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    volumes.dedup_by(|a, b| a.mount_point == b.mount_point);
    volumes
}
