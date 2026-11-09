//! Post-siege repair: builders diff the live world against the plan.
//!
//! Gates and walls are scanned first, then civilian lots. Placement is
//! rate-limited so a squad walks to a hole and puts blocks one at a time.

use pumpkin_core::world::World;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use std::sync::Arc;

use crate::plan::TownPlan;
use crate::scene::TownScene;
use crate::stamp::PlannedBlock;

/// One missing or wrong block to put back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepairJob {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub state: u16,
}

impl RepairJob {
    #[must_use]
    pub fn from_planned(block: PlannedBlock) -> Self {
        Self {
            x: block.x,
            y: block.y,
            z: block.z,
            state: block.state,
        }
    }

    #[must_use]
    pub fn pos(self) -> BlockPos {
        BlockPos::new(self.x, self.y, self.z)
    }
}

/// Scan order: threatened gates, remaining gates, wall samples, then houses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepairBand {
    Gate = 0,
    Wall = 1,
    House = 2,
}

/// Columns a builder squad should inspect, gates before walls before houses.
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

/// Diff live blocks against the plan for `columns`. Caps the queue.
#[must_use]
pub fn collect_jobs(
    world: &World,
    plan: &TownPlan,
    columns: &[(i32, i32, RepairBand)],
    cap: usize,
) -> Vec<RepairJob> {
    let mut jobs = Vec::new();
    for &(x, z, _) in columns {
        if jobs.len() >= cap {
            break;
        }
        let planned = crate::stamp::planned_column(plan, x, z);
        for block in planned {
            if jobs.len() >= cap {
                break;
            }
            let live = world.get_block_state_id(&BlockPos::new(block.x, block.y, block.z));
            if live.as_u16() != block.state {
                jobs.push(RepairJob::from_planned(block));
            }
        }
    }
    jobs
}

/// Place one planned block. Returns true if the world changed.
pub fn place_one(world: &Arc<World>, job: RepairJob) -> bool {
    let pos = job.pos();
    let live = world.get_block_state_id(&pos);
    if live.as_u16() == job.state {
        return false;
    }
    world.set_block_state(
        &pos,
        pumpkin_data::BlockStateId::new_or_air(job.state),
        BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_DROPS,
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
    fn gates_are_inspected_before_walls_and_houses() {
        let scene = scene();
        let cols = repair_columns(&scene, scene.gates().first().map(|g| (g.x, g.z)));
        assert!(!cols.is_empty());
        let first_gate = cols.iter().position(|c| c.2 == RepairBand::Gate);
        let first_wall = cols.iter().position(|c| c.2 == RepairBand::Wall);
        let first_house = cols.iter().position(|c| c.2 == RepairBand::House);
        assert!(first_gate.is_some());
        if let (Some(g), Some(w)) = (first_gate, first_wall) {
            assert!(g < w, "gates before walls");
        }
        if let (Some(w), Some(h)) = (first_wall, first_house) {
            assert!(w < h, "walls before houses");
        }
    }
}
