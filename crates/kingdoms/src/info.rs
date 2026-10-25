//! Queryable, serializable town metadata for later gameplay systems.
//!
//! This is geometry only: no capture, siege, or AI logic. Kingdoms that later
//! fight over towns can look up walls, gates, the keep, the market capture
//! point, and the road graph from [`TownInfo`].

use serde::{Deserialize, Serialize};

use crate::buildings::BuildingKind;
use crate::geom::{Dir, Rect};
use crate::palette::PaletteKind;
use crate::plan::TownPlan;
use crate::town::TownPos;

/// Stable id for a planned town: `seed:cell_x:cell_z`.
#[must_use]
pub fn town_id(seed: u64, cell_x: i32, cell_z: i32) -> String {
    format!("{seed}:{cell_x}:{cell_z}")
}

/// Which opening a gate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GateKind {
    /// Outer town-wall gate (approach from the countryside).
    Town,
    /// Castle gatehouse (inner bailey / keep approach).
    Castle,
}

/// A gate a later siege or march can path to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateInfo {
    pub kind: GateKind,
    pub x: i32,
    pub z: i32,
    pub facing: Dir,
}

/// One planned road, as a polyline of block columns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoadInfo {
    pub from: [i32; 2],
    pub to: [i32; 2],
    pub points: Vec<[i32; 2]>,
}

/// A key building later systems may occupy or defend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingInfo {
    pub id: u16,
    pub kind: BuildingKind,
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub facing: Dir,
    pub center: [i32; 2],
}

/// Castle sub-rects used for siege / garrison later.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastleInfo {
    pub bounds: Rect,
    pub gate_dir: Dir,
    pub keep: Rect,
    pub hall: Rect,
    pub chapel: Rect,
    pub stables: Rect,
    pub barracks: Rect,
    pub kitchen: Rect,
    pub inner: Rect,
    pub barbican: Rect,
    pub gatehouse: Rect,
    pub towers: Vec<Rect>,
    pub wall: Vec<[i32; 2]>,
}

/// Snapshot of one town. Safe to serialize and to hold without the occupancy grid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TownInfo {
    pub id: String,
    pub seed: u64,
    pub cell: [i32; 2],
    pub origin: TownPos,
    pub center: [i32; 2],
    pub bounds: Rect,
    pub palette: PaletteKind,
    /// Market well; intended capture / rally point.
    pub capture_point: [i32; 2],
    pub market: Rect,
    pub well: [i32; 2],
    pub town_wall: Vec<[i32; 2]>,
    pub gates: Vec<GateInfo>,
    pub towers: Vec<Rect>,
    pub castle: CastleInfo,
    pub roads: Vec<RoadInfo>,
    pub buildings: Vec<BuildingInfo>,
}

impl TownInfo {
    /// Builds a snapshot from a live plan.
    #[must_use]
    pub fn from_plan(plan: &TownPlan) -> Self {
        let (cx, cz) = plan.bounds.center();
        let castle_wall = [
            [plan.castle.bounds.min_x, plan.castle.bounds.min_z],
            [plan.castle.bounds.max_x, plan.castle.bounds.min_z],
            [plan.castle.bounds.max_x, plan.castle.bounds.max_z],
            [plan.castle.bounds.min_x, plan.castle.bounds.max_z],
        ];
        Self {
            id: town_id(plan.seed, plan.origin.cell_x, plan.origin.cell_z),
            seed: plan.seed,
            cell: [plan.origin.cell_x, plan.origin.cell_z],
            origin: plan.origin,
            center: [cx, cz],
            bounds: plan.bounds,
            palette: plan.palette,
            capture_point: [plan.well.0, plan.well.1],
            market: plan.market,
            well: [plan.well.0, plan.well.1],
            town_wall: plan.town_wall.iter().map(|&(x, z)| [x, z]).collect(),
            gates: plan.gates.clone(),
            towers: plan
                .town_towers
                .iter()
                .copied()
                .chain(plan.castle.towers.iter().copied())
                .collect(),
            castle: CastleInfo {
                bounds: plan.castle.bounds,
                gate_dir: plan.castle.gate_dir,
                keep: plan.castle.keep,
                hall: plan.castle.hall,
                chapel: plan.castle.chapel,
                stables: plan.castle.stables,
                barracks: plan.castle.barracks,
                kitchen: plan.castle.kitchen,
                inner: plan.castle.inner,
                barbican: plan.castle.barbican,
                gatehouse: plan.castle.gatehouse,
                towers: plan.castle.towers.clone(),
                wall: castle_wall.to_vec(),
            },
            roads: plan.roads.clone(),
            buildings: plan
                .lots
                .iter()
                .map(|lot| {
                    let (bx, bz) = lot.rect.center();
                    BuildingInfo {
                        id: lot.id,
                        kind: lot.kind,
                        min_x: lot.rect.min_x,
                        min_z: lot.rect.min_z,
                        max_x: lot.rect.max_x,
                        max_z: lot.rect.max_z,
                        facing: lot.facing,
                        center: [bx, bz],
                    }
                })
                .collect(),
        }
    }

    /// JSON for dumps and later game systems.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

/// Snapshot of a cached plan, if one has been built for this cell.
#[must_use]
pub fn town_info(seed: u64, cell_x: i32, cell_z: i32) -> Option<TownInfo> {
    crate::plan::plan_from_cache(seed, cell_x, cell_z).map(|plan| TownInfo::from_plan(&plan))
}

/// Snapshot of the town whose cell contains `(block_x, block_z)`, if planned.
#[must_use]
pub fn town_info_at(seed: u64, block_x: i32, block_z: i32) -> Option<TownInfo> {
    let town = crate::town::town_containing(seed, block_x, block_z);
    town_info(seed, town.cell_x, town.cell_z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::plan::build_plan;
    use crate::town::town_in_cell;

    #[test]
    fn info_round_trips_json_and_names_capture_point() {
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
        let info = TownInfo::from_plan(&plan);
        assert_eq!(info.id, "3:0:0");
        assert_eq!(info.capture_point, [plan.well.0, plan.well.1]);
        assert!(!info.castle.wall.is_empty());
        assert!(!info.gates.is_empty());
        let json = info.to_json().unwrap();
        let back: TownInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, info.id);
        assert_eq!(back.castle.keep, info.castle.keep);
    }
}
