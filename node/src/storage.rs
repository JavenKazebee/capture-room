//! Storage volumes this node can record to, and browsing for media files and
//! for folders to record into.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use sysinfo::Disks;

use crate::api::types::{
    DirEntryDto, DirListingDto, FolderDto, FolderKind, FolderListingDto, StorageVolumeDto,
};
use crate::pipeline::profile::expand_home;
use crate::sources::file::MEDIA_EXTENSIONS;

pub const DEFAULT_FOLDER: &str = "capture-room";

/// `~/capture-room`: default recording path, replay buffers and exported
/// clips. In the temp folder on a node with no home folder.
pub fn default_folder() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(DEFAULT_FOLDER)
}

/// Filesystems that never hold recordings.
const PSEUDO_FS: &[&str] = &[
    "autofs", "devfs", "devtmpfs", "tmpfs", "overlay", "squashfs", "proc", "sysfs", "nullfs",
];

/// Writable mounted volumes, one per filesystem. Read-only mounts (disk images,
/// the sealed macOS system volume) are left out. Blocking.
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

/// Mounts of the same device node (btrfs subvolumes, bind mounts) fold into
/// the one with the shortest mount point. Only device paths are compared:
/// elsewhere `name` is a volume label, which two drives can share.
fn merge_shared(mut volumes: Vec<StorageVolumeDto>) -> Vec<StorageVolumeDto> {
    volumes.sort_by(|a, b| {
        (a.mount_point.len(), &a.mount_point).cmp(&(b.mount_point.len(), &b.mount_point))
    });
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

/// Which of `volumes` `path` is on (its nearest existing folder). On Unix the
/// device decides, which follows firmlinks (macOS `/Users` is on
/// `/System/Volumes/Data`) and bind mounts; elsewhere the longest mount point
/// prefix. Blocking.
pub fn volume_of(path: &Path, volumes: &[StorageVolumeDto]) -> Option<usize> {
    let existing = path.ancestors().find(|p| p.exists())?;
    let mounts = |v: &StorageVolumeDto| {
        std::iter::once(v.mount_point.clone())
            .chain(v.other_mounts.clone())
            .collect::<Vec<_>>()
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

/// The first volume of `paths` with under `min` bytes free. Blocking.
pub fn short_volume(paths: &[PathBuf], min: u64) -> Option<StorageVolumeDto> {
    let volumes = list_volumes();
    paths.iter().find_map(|p| {
        let v = &volumes[volume_of(p, &volumes)?];
        (v.available_bytes < min).then(|| v.clone())
    })
}

/// `bytes` in GB, to a tenth under 10.
pub fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_000_000_000.0;
    let gb = bytes as f64 / GB;
    if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
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

/// Each leg's write rate since its session started, keyed by its current
/// file. Takes each leg's files and recorded duration. Blocking.
pub fn leg_write_rates(legs: &[(Vec<String>, Duration)]) -> Vec<(PathBuf, u64)> {
    legs.iter()
        .filter(|(files, elapsed)| !files.is_empty() && *elapsed >= MIN_MEASURE)
        .map(|(files, elapsed)| {
            let bytes: u64 = files
                .iter()
                .filter_map(|f| std::fs::metadata(f).ok())
                .map(|m| m.len())
                .sum();
            (
                PathBuf::from(files.last().unwrap()),
                (bytes as f64 / elapsed.as_secs_f64()) as u64,
            )
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

/// Subdirectories and media files of `dir` (home if `None`), skipping hidden
/// entries. Blocking.
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
        let Ok(meta) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let is_media = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| MEDIA_EXTENSIONS.contains(&e.to_lowercase().as_str()));
        if !(meta.is_dir() || meta.is_file() && is_media) {
            continue;
        }
        entries.push(DirEntryDto {
            name,
            path: entry.path().display().to_string(),
            is_dir: meta.is_dir(),
            size: meta.is_file().then_some(meta.len()),
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(DirListingDto {
        path: dir.display().to_string(),
        parent: dir.parent().map(|p| p.display().to_string()),
        home: home.map(|h| h.display().to_string()),
        entries,
    })
}

/// Where browsing for a folder starts: home, the storage folder, then the
/// volumes. Blocking.
pub fn folder_roots() -> Vec<FolderDto> {
    let mut roots = Vec::new();
    let folder = |name: String, path: &Path, kind| FolderDto {
        name,
        path: path.display().to_string(),
        kind,
    };
    if let Some(home) = std::env::home_dir() {
        roots.push(folder("Home".into(), &home, FolderKind::Home));
    }
    let storage = default_folder();
    if storage.is_dir() {
        roots.push(folder(DEFAULT_FOLDER.into(), &storage, FolderKind::Storage));
    }
    roots.extend(
        volume_paths()
            .into_iter()
            .map(|(name, path)| folder(name, &path, FolderKind::Volume)),
    );
    roots
}

/// macOS mounts every volume, the startup disk included, under `/Volumes`;
/// [`list_volumes`] would name the startup disk by its `/System/Volumes/Data`
/// firmlink instead.
#[cfg(target_os = "macos")]
fn volume_paths() -> Vec<(String, PathBuf)> {
    let Ok(dir) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    let mut volumes: Vec<(String, PathBuf)> = dir
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(name, _)| !name.starts_with('.'))
        .collect();
    volumes.sort_by_key(|(name, _)| name.to_lowercase());
    volumes
}

/// Mount points on Linux, drive letters on Windows.
#[cfg(not(target_os = "macos"))]
fn volume_paths() -> Vec<(String, PathBuf)> {
    list_volumes()
        .into_iter()
        .map(|v| {
            let path = PathBuf::from(&v.mount_point);
            let name = path
                .file_name()
                .map_or(v.mount_point.clone(), |n| n.to_string_lossy().into_owned());
            (name, path)
        })
        .collect()
}

/// The subfolders of `dir` (absolute, or from `~`), never its files.
/// Blocking.
pub fn list_folders(dir: &str) -> Result<FolderListingDto> {
    let dir = expand_home(dir);
    if !dir.is_absolute() {
        bail!("path must be absolute");
    }
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(&dir).with_context(|| format!("can't read {}", dir.display()))? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        // Follows symlinks, so a linked folder lists like a folder.
        if name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        folders.push(FolderDto {
            name,
            path: entry.path().display().to_string(),
            kind: FolderKind::Folder,
        });
    }
    folders.sort_by_key(|f| f.name.to_lowercase());
    Ok(FolderListingDto {
        path: Some(dir.display().to_string()),
        parent: dir.parent().map(|p| p.display().to_string()),
        folders,
    })
}

/// Create the folder `name` in `parent`. One level only, and not one that's
/// already there. Blocking.
pub fn create_folder(parent: &str, name: &str) -> Result<FolderDto> {
    let parent = expand_home(parent);
    if !parent.is_absolute() {
        bail!("path must be absolute");
    }
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        bail!("{name:?} isn't a folder name");
    }
    let path = parent.join(name);
    std::fs::create_dir(&path).with_context(|| format!("can't create {}", path.display()))?;
    Ok(FolderDto {
        name: name.to_string(),
        path: path.display().to_string(),
        kind: FolderKind::Folder,
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
        let merged = merge_shared(vec![
            vol("BACKUP", "D:\\"),
            vol("BACKUP", "E:\\"),
            vol("", "F:\\"),
        ]);
        assert_eq!(merged.len(), 3);
    }

    /// A fresh empty folder in the temp dir, removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("capture-room-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn lists_only_visible_folders() {
        let tmp = TempDir::new("list-folders");
        for dir in ["b", "A", ".hidden"] {
            std::fs::create_dir(tmp.0.join(dir)).unwrap();
        }
        std::fs::write(tmp.0.join("clip.mov"), b"").unwrap();
        let listing = list_folders(&tmp.path()).unwrap();
        let names: Vec<_> = listing.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["A", "b"]);
        assert_eq!(
            listing.folders[1].path,
            tmp.0.join("b").display().to_string()
        );
        assert_eq!(listing.path.as_deref(), Some(tmp.path().as_str()));
        assert_eq!(
            listing.parent.as_deref(),
            tmp.0.parent().map(|p| p.display().to_string()).as_deref()
        );
    }

    #[test]
    fn missing_or_relative_folders_fail() {
        let tmp = TempDir::new("list-missing");
        let err = list_folders(&tmp.0.join("nope").display().to_string()).unwrap_err();
        let io = err.downcast_ref::<std::io::Error>().unwrap();
        assert_eq!(io.kind(), std::io::ErrorKind::NotFound);
        assert!(list_folders("relative/dir").is_err());
    }

    #[test]
    fn creates_one_folder() {
        let tmp = TempDir::new("create-folder");
        let made = create_folder(&tmp.path(), " Show 1 ").unwrap();
        assert_eq!(made.name, "Show 1");
        assert!(tmp.0.join("Show 1").is_dir());
        // Already there.
        assert!(create_folder(&tmp.path(), "Show 1").is_err());
        for bad in ["", " ", ".", "..", "a/b", "a\\b", "../up"] {
            assert!(create_folder(&tmp.path(), bad).is_err(), "{bad:?}");
        }
        assert!(create_folder("relative", "x").is_err());
    }

    #[test]
    fn roots_start_with_home() {
        let roots = folder_roots();
        if let Some(home) = std::env::home_dir() {
            assert_eq!(roots[0].kind, FolderKind::Home);
            assert_eq!(roots[0].path, home.display().to_string());
        }
        assert!(roots.iter().any(|r| r.kind == FolderKind::Volume));
    }
}
