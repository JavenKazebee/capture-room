//! Keeping a run from taking the machine's memory.

use sysinfo::System;

/// The machine's available memory is never let fall below this…
const MIN_FREE_MEMORY: u64 = 3 << 29;
/// …or this share of all of it, whichever is more. See [`MemoryBudget`].
const MIN_FREE_MEMORY_SHARE: u64 = 8;
/// Headroom over the most any feed has used so far, when deciding whether more fit.
const MEMORY_MARGIN: f64 = 1.25;

/// How much memory feeds take, and how many more fit. Feeds (decoder, monitor,
/// encoders, VideoToolbox's service on macOS) use memory a step doesn't
/// measure, and an encoder that keeps up never fails a step, so without this a
/// run adds feeds until the machine runs out.
#[derive(Debug, Clone, Copy)]
pub(super) struct MemoryBudget {
    /// Available before any feed ran.
    pub(super) baseline: u64,
    /// Never go below this much available.
    floor: u64,
    /// The most any one feed has taken: the drop from baseline over feeds running.
    per_feed: u64,
}

impl MemoryBudget {
    fn new(total: u64, available: u64) -> Self {
        Self {
            baseline: available,
            floor: MIN_FREE_MEMORY.max(total / MIN_FREE_MEMORY_SHARE),
            per_feed: 0,
        }
    }

    pub(super) fn short(&self, available: u64) -> bool {
        available < self.floor
    }

    fn observe(&mut self, available: u64, feeds: u32) {
        if feeds > 0 {
            let used = self.baseline.saturating_sub(available) / u64::from(feeds);
            self.per_feed = self.per_feed.max(used);
        }
    }

    /// How many feeds fit, with a margin; `None` until feeds have shown what they take.
    pub(super) fn fits(&self) -> Option<u32> {
        (self.per_feed > 0).then(|| {
            let room = self.baseline.saturating_sub(self.floor) as f64;
            (room / (self.per_feed as f64 * MEMORY_MARGIN)) as u32
        })
    }
}

/// [`MemoryBudget`] over the machine's available memory.
pub(super) struct MemoryWatch {
    system: System,
    pub(super) budget: MemoryBudget,
}

impl MemoryWatch {
    pub(super) fn new() -> Self {
        let mut system = System::new();
        system.refresh_memory();
        let total = system.total_memory();
        let mut watch = Self {
            system,
            budget: MemoryBudget::new(total, 0),
        };
        watch.budget = MemoryBudget::new(total, watch.available());
        watch
    }

    /// Memory new feeds could have. On macOS, the kernel's free share (as
    /// `memory_pressure` shows): sysinfo's subtracts compressed memory and reads
    /// nothing on a Mac that's merely compressing. Elsewhere, sysinfo's.
    fn available(&mut self) -> u64 {
        #[cfg(target_os = "macos")]
        if let Some(pct) = macos_free_pct() {
            return self.system.total_memory() / 100 * pct;
        }
        self.system.refresh_memory();
        self.system.available_memory()
    }

    pub(super) fn short(&mut self) -> bool {
        let available = self.available();
        self.budget.short(available)
    }

    pub(super) fn observe(&mut self, feeds: u32) {
        let available = self.available();
        self.budget.observe(available, feeds);
    }
}

/// `kern.memorystatus_level`: the percentage of memory macOS counts as free.
#[cfg(target_os = "macos")]
fn macos_free_pct() -> Option<u64> {
    let mut level: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is a C string and `level` is an int `len` bytes long,
    // which is what this sysctl writes.
    let ok = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_level".as_ptr(),
            (&mut level as *mut libc::c_int).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    } == 0;
    (ok && (0..=100).contains(&level)).then_some(level as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_budget() {
        const GB: u64 = 1 << 30;
        const MB: u64 = 1 << 20;
        // 8 GB with 3 GB available: never below 1.5 GB, so 1.5 GB to use.
        let mut budget = MemoryBudget::new(8 * GB, 3 * GB);
        assert_eq!(budget.floor, 1536 * MB);
        assert_eq!(budget.fits(), None);
        assert!(!budget.short(1536 * MB));
        assert!(budget.short(1535 * MB));
        // 4 feeds took 400 MB: 100 MB each, 125 MB with the margin.
        budget.observe(3 * GB - 400 * MB, 4);
        assert_eq!(budget.fits(), Some(12));
        // A step where they took less doesn't lower the estimate.
        budget.observe(3 * GB - 400 * MB, 8);
        assert_eq!(budget.fits(), Some(12));
        // On 32 GB, an eighth is kept free.
        assert_eq!(MemoryBudget::new(32 * GB, 20 * GB).floor, 4 * GB);
    }
}
