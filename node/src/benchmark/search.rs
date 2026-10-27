//! Choosing how many feeds each step runs.

use std::collections::BTreeMap;

#[derive(Debug, PartialEq)]
pub(super) enum Next {
    /// Run this many feeds and measure them, in full or quick-checked first.
    Measure { feeds: u32, full: bool },
    /// The search is over: the node sustains this many.
    Done(u32),
}

/// The search for the most feeds a node sustains: double (1, 2, 4, …) with
/// quick checks until one fails or the limit, then bisect with full steps.
/// The answer must pass a full step; if a quick-passed count fails one, the
/// search continues below.
pub(super) struct Search {
    limit: u32,
    /// Counts that passed, and whether a full step showed it.
    passes: BTreeMap<u32, bool>,
    /// The fewest feeds that failed.
    pub(super) fail: Option<u32>,
}

impl Search {
    pub(super) fn new(limit: u32) -> Self {
        Self {
            limit: limit.max(1),
            passes: BTreeMap::new(),
            fail: None,
        }
    }

    pub(super) fn best(&self) -> u32 {
        self.passes.keys().next_back().copied().unwrap_or(0)
    }

    pub(super) fn next(&self) -> Next {
        let lo = self.best();
        let narrowing = self.fail.is_some_and(|hi| hi - lo > 1);
        if narrowing {
            let hi = self.fail.unwrap();
            return Next::Measure {
                feeds: lo + (hi - lo) / 2,
                full: true,
            };
        }
        let settled = self.fail.is_some() || lo >= self.limit;
        if !settled {
            let feeds = if lo == 0 { 1 } else { (lo * 2).min(self.limit) };
            return Next::Measure { feeds, full: false };
        }
        if lo == 0 || self.passes[&lo] {
            Next::Done(lo)
        } else {
            Next::Measure {
                feeds: lo,
                full: true,
            }
        }
    }

    pub(super) fn passed(&mut self, feeds: u32, full: bool) {
        let entry = self.passes.entry(feeds).or_default();
        *entry |= full;
    }

    pub(super) fn failed(&mut self, feeds: u32) {
        self.fail = Some(self.fail.map_or(feeds, |f| f.min(feeds)));
        self.passes.retain(|&n, _| n < feeds);
    }

    /// Go no higher than `feeds` (memory or disk is running short).
    pub(super) fn cap(&mut self, feeds: u32) {
        self.limit = self.limit.min(feeds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Search a node sustaining `capacity` feeds, where quick checks also pass
    /// up to `quick_ok`. Returns the answer and the counts measured, in order.
    fn simulate(limit: u32, capacity: u32, quick_ok: u32) -> (u32, Vec<(u32, bool)>) {
        let mut search = Search::new(limit);
        let mut measured = Vec::new();
        loop {
            match search.next() {
                Next::Done(n) => return (n, measured),
                Next::Measure { feeds, full } => {
                    measured.push((feeds, full));
                    let ok = feeds
                        <= if full {
                            capacity
                        } else {
                            quick_ok.max(capacity)
                        };
                    if ok {
                        search.passed(feeds, full);
                    } else if !full && feeds <= capacity {
                        // Not clear from the quick check: measured in full.
                        search.passed(feeds, true);
                    } else {
                        search.failed(feeds);
                    }
                }
            }
            assert!(measured.len() < 50, "search didn't end");
        }
    }

    #[test]
    fn search_finds_the_capacity() {
        for capacity in 0..=70 {
            let (found, measured) = simulate(64, capacity, 0);
            assert_eq!(found, capacity.min(64), "capacity {capacity}: {measured:?}");
            assert!(
                measured.len() <= 15,
                "capacity {capacity}: {} steps",
                measured.len()
            );
        }
    }

    #[test]
    fn search_doubles_then_narrows() {
        let (found, measured) = simulate(64, 40, 0);
        assert_eq!(found, 40);
        let quick: Vec<u32> = measured
            .iter()
            .filter(|(_, full)| !full)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(quick, [1, 2, 4, 8, 16, 32, 64]);
        let full: Vec<u32> = measured
            .iter()
            .filter(|(_, full)| *full)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(full, [48, 40, 44, 42, 41]);
    }

    #[test]
    fn answer_is_confirmed_by_a_full_step() {
        // Quick checks pass up to 40, but only 30 keep up in full.
        let (found, measured) = simulate(64, 30, 40);
        assert_eq!(found, 30);
        assert!(measured.contains(&(30, true)), "{measured:?}");

        // Every quick check passes up to the limit: the limit is confirmed in full.
        let (found, measured) = simulate(16, 100, 0);
        assert_eq!(found, 16);
        assert_eq!(measured.last(), Some(&(16, true)));
    }

    #[test]
    fn capping_stops_the_search() {
        let mut search = Search::new(64);
        search.passed(1, false);
        search.passed(2, false);
        search.passed(4, true);
        search.cap(4);
        assert_eq!(search.next(), Next::Done(4));
    }
}
