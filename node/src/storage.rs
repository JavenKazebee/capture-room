//! Storage volumes this node can record to, and browsing its filesystem for
//! media files.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sysinfo::Disks;

use crate::api::types::{DirEntryDto, DirListingDto, StorageVolumeDto};
use crate::sources::file::MEDIA_EXTENSIONS;

/// Filesystems that never hold recordings.
const PSEUDO_FS: &[&str] = &[
    "autofs", "devfs", "devtmpfs", "tmpfs", "overlay", "squashfs", "proc", "sysfs", "nullfs",
];

/// List writable mounted volumes, skipping pseudo filesystems, one entry per
/// filesystem. Read-only mounts (disk images, the sealed macOS system volume)
/// can't hold recordings, so they're left out. Blocking: call from
/// `spawn_blocking`.
pub fn list_volumes() -> Vec<StorageVolumeDto> {
    let disks = Disks::new_with_refreshed_list();
    let volumes: Vec<StorageVolumeDto> = disks
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
                other_mounts: Vec::new(),
                file_system: d.file_system().to_string_lossy().into_owned(),
                total_bytes: d.total_space(),
                available_bytes: d.available_space(),
                removable: d.is_removable(),
                write_bytes_per_sec: 0,
                seconds_left: None,
            }
        })
        .collect();
    merge_shared(volumes)
}

/// One entry per filesystem. Mounts of the same device node (btrfs
/// subvolumes, bind mounts) share its space, so they're folded into the entry
/// with the shortest mount point. Only device paths are compared: elsewhere
/// `name` is a volume label, which two different drives can share.
fn merge_shared(mut volumes: Vec<StorageVolumeDto>) -> Vec<StorageVolumeDto> {
    volumes.sort_by(|a, b| (a.mount_point.len(), &a.mount_point).cmp(&(b.mount_point.len(), &b.mount_point)));
    let mut merged: Vec<StorageVolumeDto> = Vec::new();
    for v in volumes {
        let same = |e: &&mut StorageVolumeDto| {
            e.mount_point == v.mount_point || (v.name.starts_with("/dev/") && e.name == v.name)
        };
        match merged.iter_mut().find(same) {
            Some(e) if e.mount_point == v.mount_point => {}
            Some(e) => e.other_mounts.push(v.mount_point),
            None => merged.push(v),
        }
    }
    merged.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    merged
}

/// Which of `volumes` `path` is on. `path` needn't exist yet: its nearest
/// existing folder is used. On Unix the device decides, which follows
/// firmlinks (macOS `/Users` is on `/System/Volumes/Data`) and bind mounts;
/// elsewhere, the longest mount point the path starts with. Blocking.
pub fn volume_of(path: &Path, volumes: &[StorageVolumeDto]) -> Option<usize> {
    let existing = path.ancestors().find(|p| p.exists())?;
    let mounts = |v: &StorageVolumeDto| {
        std::iter::once(v.mount_point.clone()).chain(v.other_mounts.clone()).collect::<Vec<_>>()
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(dev) = std::fs::metadata(existing).map(|m| m.dev()) {
            let same = |m: &String| std::fs::metadata(m).is_ok_and(|meta| meta.dev() == dev);
            if let Some(i) = volumes.iter().position(|v| mounts(v).iter().any(same)) {
                return Some(i);
            }
        }
    }
    volumes
        .iter()
        .enumerate()
        .flat_map(|(i, v)| mounts(v).into_iter().map(move |m| (i, m)))
        .filter(|(_, m)| existing.starts_with(m))
        .max_by_key(|(_, m)| m.len())
        .map(|(i, _)| i)
}

/// Recording time left on a volume with `available` bytes, written at
/// `bytes_per_sec`; `None` when nothing is written.
pub fn seconds_left(available: u64, bytes_per_sec: u64) -> Option<u64> {
    (bytes_per_sec > 0).then(|| available / bytes_per_sec)
}

/// A recording isn't started on a volume with less free space than this.
pub const MIN_FREE_TO_RECORD: u64 = 1_000_000_000;

/// How long a leg must have recorded before its write rate is trusted: the
/// first seconds include headers and encoder start-up.
const MIN_MEASURE: Duration = Duration::from_secs(5);

/// What each recording leg writes per second, averaged since its session
/// started, keyed by the file it's writing now. Takes each leg's files and
/// how long it has recorded. Blocking: reads file sizes.
pub fn leg_write_rates(legs: &[(Vec<String>, Duration)]) -> Vec<(PathBuf, u64)> {
    legs.iter()
        .filter(|(files, elapsed)| !files.is_empty() && *elapsed >= MIN_MEASURE)
        .map(|(files, elapsed)| {
            let bytes: u64 = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
            (PathBuf::from(files.last().unwrap()), (bytes as f64 / elapsed.as_secs_f64()) as u64)
        })
        .collect()
}

/// Add each `(file, bytes per second)` being written to its volume's write
/// rate, and work out the time left. Blocking.
pub fn apply_write_rates(volumes: &mut [StorageVolumeDto], writes: &[(PathBuf, u64)]) {
    for (file, rate) in writes {
        if let Some(i) = volume_of(file, volumes) {
            volumes[i].write_bytes_per_sec += rate;
        }
    }
    for v in volumes {
        v.seconds_left = seconds_left(v.available_bytes, v.write_bytes_per_sec);
    }
}

/// List `dir` (the home directory if `None`): its subdirectories and media
/// files, skipping hidden entries. Blocking: call from `spawn_blocking`.
pub fn list_dir(dir: Option<&str>) -> Result<DirListingDto> {
    let home = std::env::home_dir();
    let dir = match dir.filter(|d| !d.is_empty()) {
        Some(d) => Path::new(d).to_path_buf(),
        None => home.clone().context("no home directory")?,
    };
    if !dir.is_absolute() {
        bail!("path must be absolute");
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("can't read {}", dir.display()))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // Follows symlinks, so a linked folder browses like a folder.
        let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
        let is_media = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_lowercase().as_str()));
        if !meta.is_dir() && !(meta.is_file() && is_media) {
            continue;
        }
        entries.push(DirEntryDto {
            name,
            path: entry.path().display().to_string(),
            is_dir: meta.is_dir(),
            size: meta.is_file().then_some(meta.len()),
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(DirListingDto {
        path: dir.display().to_string(),
        parent: dir.parent().map(|p| p.display().to_string()),
        home: home.map(|h| h.display().to_string()),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(name: &str, mount: &str) -> StorageVolumeDto {
        StorageVolumeDto {
            name: name.into(),
            mount_point: mount.into(),
            other_mounts: Vec::new(),
            file_system: "btrfs".into(),
            total_bytes: 100,
            available_bytes: 50,
            removable: false,
            write_bytes_per_sec: 0,
            seconds_left: None,
        }
    }

    #[test]
    fn merges_mounts_of_one_device() {
        let merged = merge_shared(vec![
            vol("/dev/sda2", "/home"),
            vol("/dev/sda2", "/"),
            vol("/dev/sdb1", "/media/rec"),
            vol("/dev/sda2", "/var/log"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].mount_point, "/");
        assert_eq!(merged[0].other_mounts, ["/home", "/var/log"]);
        assert_eq!(merged[1].mount_point, "/media/rec");
    }

    #[test]
    fn keeps_drives_that_only_share_a_label() {
        let merged = merge_shared(vec![vol("BACKUP", "D:\\"), vol("BACKUP", "E:\\"), vol("", "F:\\")]);
        assert_eq!(merged.len(), 3);
    }
}
