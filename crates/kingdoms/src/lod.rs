//! LOD-style AI throttling for hundred-entity sieges.
//!
//! Far / idle combatants skip most ticks. Path searches are capped per tick.
//! Target acquisition uses 16-block spatial buckets instead of scanning the
//! whole town.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::faction::Faction;

/// Block size of one spatial bucket.
pub const BUCKET: i32 = 16;
/// Max `navigator.set_progress` calls the director will start this tick.
pub const DEFAULT_PATH_CAP: u32 = 48;
/// Follow / acquire radius in blocks.
pub const ACQUIRE_RANGE: f64 = 28.0;
/// Pillagers this close to a gate raise the town alarm.
pub const ALARM_RANGE: f64 = 48.0;
/// Idle civilians / guards farther than this from every player sleep.
pub const CIVILIAN_SLEEP_RANGE: f64 = 64.0;
/// Outside this, combatants skip targeting, pathing, and collision.
pub const PLAYER_SIM_RANGE: f64 = 64.0;

/// How often a combatant should run AI this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LodTier {
    /// In melee or shooting. Full rate.
    Combat,
    /// Town is alert, moving to post.
    Alert,
    /// Garrison idle.
    Idle,
    /// Unloaded / far from any fight or player.
    Far,
}

impl LodTier {
    #[must_use]
    pub const fn period(self) -> u32 {
        match self {
            Self::Combat => 1,
            Self::Alert => 2,
            Self::Idle => 8,
            Self::Far => 40,
        }
    }

    #[must_use]
    pub fn should_tick(self, entity_id: i32, tick: u32) -> bool {
        let period = self.period();
        let id = entity_id.unsigned_abs();
        tick % period == id % period
    }
}

/// Per-tick pathfinding budget shared by kingdom goals.
#[derive(Debug)]
pub struct PathBudget {
    remaining: AtomicU32,
    cap: u32,
}

impl PathBudget {
    #[must_use]
    pub const fn new(cap: u32) -> Self {
        Self {
            remaining: AtomicU32::new(cap),
            cap,
        }
    }

    pub fn reset(&self) {
        self.remaining.store(self.cap, Ordering::Relaxed);
    }

    /// Returns true if this search is allowed. Failed tries still consume a slot
    /// so a burst of failures cannot monopolize the tick.
    pub fn try_acquire(&self) -> bool {
        loop {
            let cur = self.remaining.load(Ordering::Relaxed);
            if cur == 0 {
                return false;
            }
            if self
                .remaining
                .compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }
}

/// One combatant as seen by cheap target acquisition.
#[derive(Clone, Debug)]
pub struct BucketEntry {
    pub entity_id: i32,
    pub x: f64,
    pub z: f64,
    pub faction: Faction,
}

impl BucketEntry {
    /// Squared horizontal distance to `(x, z)`.
    #[must_use]
    pub fn dist2(&self, x: f64, z: f64) -> f64 {
        let dx = self.x - x;
        let dz = self.z - z;
        dx * dx + dz * dz
    }

    /// True when this entry is within `range` blocks of `(x, z)`.
    #[must_use]
    pub fn within(&self, x: f64, z: f64, range: f64) -> bool {
        self.dist2(x, z) <= range * range
    }
}

/// 16×16 column buckets for O(k) neighbour queries.
#[derive(Clone, Debug, Default)]
pub struct SpatialBuckets {
    cells: HashMap<(i32, i32), Vec<BucketEntry>>,
}

impl SpatialBuckets {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cells: HashMap::new(),
        }
    }

    pub fn insert(&mut self, entry: BucketEntry) {
        let key = bucket_key(entry.x, entry.z);
        self.cells.entry(key).or_default().push(entry);
    }

    /// Closest hostile in `range` blocks, or `None`.
    #[must_use]
    pub fn nearest_hostile(
        &self,
        x: f64,
        z: f64,
        faction: &Faction,
        range: f64,
    ) -> Option<&BucketEntry> {
        let range_sq = range * range;
        let cx = (x.floor() as i32).div_euclid(BUCKET);
        let cz = (z.floor() as i32).div_euclid(BUCKET);
        let reach = ((range / f64::from(BUCKET)).ceil() as i32).max(1);
        let mut best: Option<(&BucketEntry, f64)> = None;
        for dx in -reach..=reach {
            for dz in -reach..=reach {
                let Some(list) = self.cells.get(&(cx + dx, cz + dz)) else {
                    continue;
                };
                for entry in list {
                    if !crate::faction::is_hostile(faction, &entry.faction) {
                        continue;
                    }
                    let ddx = entry.x - x;
                    let ddz = entry.z - z;
                    let d2 = ddx * ddx + ddz * ddz;
                    if d2 > range_sq {
                        continue;
                    }
                    if best.is_none_or(|(_, best_d)| d2 < best_d) {
                        best = Some((entry, d2));
                    }
                }
            }
        }
        best.map(|(e, _)| e)
    }

    /// Any hostile within `range` (for alarm).
    #[must_use]
    pub fn any_hostile_near(&self, x: f64, z: f64, faction: &Faction, range: f64) -> bool {
        self.nearest_hostile(x, z, faction, range).is_some()
    }
}

#[must_use]
pub const fn bucket_key(x: f64, z: f64) -> (i32, i32) {
    (
        (x.floor() as i32).div_euclid(BUCKET),
        (z.floor() as i32).div_euclid(BUCKET),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faction::KingdomId;

    fn entry(id: i32, x: f64, z: f64, faction: Faction) -> BucketEntry {
        BucketEntry {
            entity_id: id,
            x,
            z,
            faction,
        }
    }

    #[test]
    fn buckets_find_nearest_hostile_not_ally() {
        let alder = Faction::Kingdom(KingdomId::new("alder"));
        let mut buckets = SpatialBuckets::new();
        buckets.insert(entry(1, 0.0, 0.0, alder.clone()));
        buckets.insert(entry(2, 8.0, 0.0, Faction::Pillager));
        buckets.insert(entry(3, 4.0, 0.0, alder.clone()));
        let found = buckets.nearest_hostile(0.0, 0.0, &alder, 32.0).unwrap();
        assert_eq!(found.entity_id, 2);
        assert!(buckets.nearest_hostile(0.0, 0.0, &alder, 4.0).is_none());
    }

    #[test]
    fn path_budget_caps_and_resets() {
        let budget = PathBudget::new(2);
        assert!(budget.try_acquire());
        assert!(budget.try_acquire());
        assert!(!budget.try_acquire());
        budget.reset();
        assert!(budget.try_acquire());
    }

    #[test]
    fn lod_staggers_idle_ticks() {
        assert!(LodTier::Combat.should_tick(7, 100));
        assert!(!LodTier::Idle.should_tick(1, 0)); // 0 % 8 == 0, 1 % 8 == 1
        assert!(LodTier::Idle.should_tick(0, 8));
        assert!(LodTier::Alert.should_tick(2, 2));
        assert!(!LodTier::Alert.should_tick(2, 3));
        assert_eq!(LodTier::Far.period(), 40);
        assert_eq!(CIVILIAN_SLEEP_RANGE, 64.0);
        assert!((PLAYER_SIM_RANGE - 64.0).abs() < f64::EPSILON);
    }

    #[test]
    fn bucket_entry_range_check_avoids_a_second_scan() {
        let e = entry(2, 8.0, 0.0, Faction::Pillager);
        assert!(e.within(0.0, 0.0, 16.0));
        assert!(!e.within(0.0, 0.0, 4.0));
        assert!((e.dist2(0.0, 0.0) - 64.0).abs() < f64::EPSILON);
    }
}
