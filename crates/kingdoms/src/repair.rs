//! Post-siege repair: builders diff the live world against the plan.
//!
//! Gates, the keep and walls are scanned first, then civilian lots. Placement
//! is rate-limited so a squad walks to a hole and puts blocks one at a time.

use pumpkin_core::world::World;
use pumpkin_util::math::position::BlockPos;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::plan::TownPlan;
use crate::scene::TownScene;
use crate::stamp::PlannedBlock;

/// One missing or wrong block to put back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepairJob {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub state: u16,
    /// [`RepairBand`] as `u8` so a restart still sorts gates before houses.
    #[serde(default)]
    pub band: u8,
}

impl RepairJob {
    #[must_use]
    pub fn from_planned(block: PlannedBlock, band: RepairBand) -> Self {
        Self {
            x: block.x,
            y: block.y,
            z: block.z,
            state: block.state,
            band: band as u8,
        }
    }

    #[must_use]
    pub fn pos(self) -> BlockPos {
        BlockPos::new(self.x, self.y, self.z)
    }
}

/// Scan order: threatened gates, keep, remaining walls, then houses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepairBand {
    Gate = 0,
    Keep = 1,
    Wall = 2,
    House = 3,
}

impl RepairBand {
    #[must_use]
    pub const fn from_u8(raw: u8) -> Self {
        match raw {
            0 => Self::Gate,
            1 => Self::Keep,
            2 => Self::Wall,
            _ => Self::House,
        }
    }
}

/// Columns a builder squad should inspect, gates and keep before walls.
#[must_use]
pub fn repair_columns(
    scene: &dyn TownScene,
    threatened: Option<(i32, i32)>,
) -> Vec<(i32, i32, RepairBand)> {
    let mut out = Vec::new();
    let mut gates: Vec<_> = scene.gates().iter().map(|g| (g.x, g.z)).collect();
    if let Some((tx, tz)) = threatened {
        gates.sort_by_key(|(x, z)| (x - tx).abs() + (z - tz).abs());
    }
    for (gx, gz) in gates {
        for z in gz - 3..=gz + 3 {
            for x in gx - 3..=gx + 3 {
                out.push((x, z, RepairBand::Gate));
            }
        }
    }
    let keep = scene.keep();
    for z in keep.z - 4..=keep.z + 4 {
        for x in keep.x - 4..=keep.x + 4 {
            out.push((x, z, RepairBand::Keep));
        }
    }
    let kr = scene.keep_rect();
    for z in (kr.min_z..=kr.max_z).step_by(8) {
        for x in (kr.min_x..=kr.max_x).step_by(8) {
            out.push((x, z, RepairBand::Keep));
        }
    }
    for &[x, z] in scene.wall().iter().step_by(8) {
        for dz in -1..=1 {
            for dx in -1..=1 {
                out.push((x + dx, z + dz, RepairBand::Wall));
            }
        }
    }
    for home in scene.civilian_homes().iter().take(8) {
        for z in home.z - 2..=home.z + 2 {
            for x in home.x - 2..=home.x + 2 {
                out.push((x, z, RepairBand::House));
            }
        }
    }
    out.sort_by_key(|c| c.2);
    out.dedup_by_key(|c| (c.0, c.1));
    out
}

/// Diff of a scan pass: jobs to place plus how much of the plan still matches.
#[derive(Clone, Debug, Default)]
pub struct RepairScan {
    pub jobs: Vec<RepairJob>,
    pub planned: u32,
    pub mismatched: u32,
}

impl RepairScan {
    /// Percent of scanned planned blocks that already match the live world.
    #[must_use]
    pub fn pct_restored(&self) -> u32 {
        100u32
            .saturating_mul(self.planned.saturating_sub(self.mismatched))
            .checked_div(self.planned)
            .unwrap_or(100)
    }
}

/// Diff live blocks against the plan for `columns`. Caps the job queue.
#[must_use]
pub fn collect_jobs(
    world: &World,
    plan: &TownPlan,
    columns: &[(i32, i32, RepairBand)],
    cap: usize,
) -> RepairScan {
    let mut scan = RepairScan::default();
    for &(x, z, band) in columns {
        if crate::siege::is_flag_column(plan.castle.flag, x, z) {
            continue;
        }
        let planned = crate::stamp::planned_column(plan, x, z);
        for block in planned {
            let Some(live) =
                world.get_block_state_id_if_loaded(&BlockPos::new(block.x, block.y, block.z))
            else {
                // Far towns hold no tickets. Count the hole and queue it so
                // the director can restore abstractly without a fetch.
                scan.planned = scan.planned.saturating_add(1);
                scan.mismatched = scan.mismatched.saturating_add(1);
                if scan.jobs.len() < cap {
                    scan.jobs.push(RepairJob::from_planned(block, band));
                }
                continue;
            };
            scan.planned = scan.planned.saturating_add(1);
            if live.as_u16() != block.state {
                scan.mismatched = scan.mismatched.saturating_add(1);
                if scan.jobs.len() < cap {
                    scan.jobs.push(RepairJob::from_planned(block, band));
                }
            }
        }
    }
    scan
}

/// A repair job when live world state does not match the plan.
#[must_use]
pub fn job_if_mismatch(live: u16, planned: crate::stamp::PlannedBlock) -> Option<RepairJob> {
    job_if_mismatch_band(live, planned, RepairBand::Wall)
}

/// Same as [`job_if_mismatch`], with an explicit scan band.
#[must_use]
pub fn job_if_mismatch_band(
    live: u16,
    planned: crate::stamp::PlannedBlock,
    band: RepairBand,
) -> Option<RepairJob> {
    (live != planned.state).then(|| RepairJob::from_planned(planned, band))
}

/// Jobs that must survive a restart: the queue plus work already in a builder's hands.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn persistable_jobs(
    queued: &VecDeque<RepairJob>,
    assigned: &HashMap<i32, RepairJob>,
) -> Vec<RepairJob> {
    let mut out: Vec<RepairJob> = queued.iter().copied().collect();
    out.extend(assigned.values().copied());
    out.sort_by_key(|j| (j.band, j.y, j.x, j.z));
    out
}

/// Append unique incoming jobs, keep the cap, sort gates/keep/walls first.
#[allow(clippy::implicit_hasher)]
pub fn merge_jobs(
    queued: &mut VecDeque<RepairJob>,
    incoming: impl IntoIterator<Item = RepairJob>,
    restored: &HashSet<(i32, i32, i32)>,
    assigned: &HashMap<i32, RepairJob>,
    cap: usize,
) -> usize {
    let mut have: HashSet<(i32, i32, i32)> = queued.iter().map(|j| (j.x, j.y, j.z)).collect();
    have.extend(assigned.values().map(|j| (j.x, j.y, j.z)));
    have.extend(restored.iter().copied());
    let mut added = 0usize;
    for job in incoming {
        if queued.len() >= cap {
            break;
        }
        if have.insert((job.x, job.y, job.z)) {
            queued.push_back(job);
            added += 1;
        }
    }
    if added > 0 {
        let mut jobs: Vec<RepairJob> = queued.drain(..).collect();
        jobs.sort_by_key(|j| (j.band, j.y, j.x, j.z));
        queued.extend(jobs);
    }
    added
}

/// Place one planned block. Returns true if the world changed.
pub fn place_one(world: &Arc<World>, job: RepairJob) -> bool {
    let pos = job.pos();
    let Some(live) = world.get_block_state_id_if_loaded(&pos) else {
        return false;
    };
    if live.as_u16() == job.state {
        return false;
    }
    world.set_block_state(
        &pos,
        pumpkin_data::BlockStateId::new_or_air(job.state),
        crate::siege::SIEGE_PLACE_FLAGS,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::TownInfo;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::scene::TownInfoScene;
    use crate::town::town_in_cell;

    fn scene() -> TownInfoScene {
        let origin = town_in_cell(3, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            70,
            8,
            4,
        );
        let plan = build_plan(3, origin, grid, PaletteKind::Oak);
        TownInfoScene::from_info(TownInfo::from_plan(&plan))
    }

    #[test]
    fn gates_keep_and_walls_are_inspected_before_houses() {
        let scene = scene();
        let cols = repair_columns(&scene, scene.gates().first().map(|g| (g.x, g.z)));
        assert!(!cols.is_empty());
        let first_gate = cols.iter().position(|c| c.2 == RepairBand::Gate);
        let first_keep = cols.iter().position(|c| c.2 == RepairBand::Keep);
        let first_wall = cols.iter().position(|c| c.2 == RepairBand::Wall);
        let first_house = cols.iter().position(|c| c.2 == RepairBand::House);
        assert!(first_gate.is_some());
        assert!(first_keep.is_some(), "keep columns must be in the scan");
        if let (Some(g), Some(k)) = (first_gate, first_keep) {
            assert!(g < k, "gates before keep");
        }
        if let (Some(k), Some(w)) = (first_keep, first_wall) {
            assert!(k < w, "keep before walls");
        }
        if let (Some(w), Some(h)) = (first_wall, first_house) {
            assert!(w < h, "walls before houses");
        }
    }

    #[test]
    fn punched_air_becomes_a_repair_job() {
        let planned = crate::stamp::PlannedBlock {
            x: 4,
            y: 80,
            z: 6,
            state: pumpkin_data::Block::STONE_BRICKS.default_state.id.as_u16(),
        };
        let air = pumpkin_data::Block::AIR.default_state.id.as_u16();
        let job = job_if_mismatch(air, planned).expect("hole should queue repair");
        assert_eq!(job.x, 4);
        assert_eq!(job.y, 80);
        assert_eq!(job.z, 6);
        assert_eq!(job.state, planned.state);
        assert_eq!(job.band, RepairBand::Wall as u8);
        assert!(job_if_mismatch(planned.state, planned).is_none());
        let empty = RepairScan::default();
        assert_eq!(empty.pct_restored(), 100);
        let half = RepairScan {
            jobs: Vec::new(),
            planned: 10,
            mismatched: 4,
        };
        assert_eq!(half.pct_restored(), 60);
    }

    #[test]
    fn merge_jobs_keeps_existing_work_and_sorts_by_band() {
        let mut queued = VecDeque::from([RepairJob {
            x: 1,
            y: 70,
            z: 1,
            state: 9,
            band: RepairBand::House as u8,
        }]);
        let incoming = [
            RepairJob {
                x: 1,
                y: 70,
                z: 1,
                state: 9,
                band: RepairBand::House as u8,
            },
            RepairJob {
                x: 2,
                y: 70,
                z: 2,
                state: 8,
                band: RepairBand::Gate as u8,
            },
            RepairJob {
                x: 3,
                y: 70,
                z: 3,
                state: 7,
                band: RepairBand::Keep as u8,
            },
        ];
        let restored = HashSet::from([(3, 70, 3)]);
        let assigned = HashMap::new();
        let added = merge_jobs(&mut queued, incoming, &restored, &assigned, 24);
        assert_eq!(added, 1, "duplicate and restored jobs are skipped");
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].band, RepairBand::Gate as u8);
        assert_eq!(queued[1].band, RepairBand::House as u8);
    }

    #[test]
    fn persistable_jobs_include_assigned_builder_work() {
        let queued = VecDeque::from([RepairJob {
            x: 1,
            y: 64,
            z: 1,
            state: 4,
            band: RepairBand::Gate as u8,
        }]);
        let assigned = HashMap::from([(
            42,
            RepairJob {
                x: 2,
                y: 64,
                z: 2,
                state: 5,
                band: RepairBand::Keep as u8,
            },
        )]);
        let jobs = persistable_jobs(&queued, &assigned);
        assert_eq!(jobs.len(), 2);
        assert!(
            jobs.iter()
                .any(|j| j.x == 2 && j.band == RepairBand::Keep as u8)
        );
    }
}
