//! Queryable, serializable town metadata for later gameplay systems.
//!
//! This is geometry only: no capture, siege, or AI logic. Kingdoms that later
//! fight over towns can look up walls, gates, the keep, the market capture
//! point, the road graph, hinterland scenes, and spawn points from [`TownInfo`].
//! Every field is a pure function of seed + cell.

use serde::{Deserialize, Serialize};

use crate::buildings::BuildingKind;
use crate::geom::{Dir, Rect};
use crate::palette::PaletteKind;
use crate::plan::TownPlan;
use crate::scenes::{Scene, SceneKind};
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
    /// Inner-bailey flag plinth (mast column).
    pub flag: [i32; 2],
}

/// Why an AI layer would spawn or path to a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpawnRole {
    /// Bed in a villager dwelling (cottage, timber, townhouse, longhouse, tavern).
    VillagerBed,
    /// Workstation: smith, church, mill, barn, stall, monastery.
    Workstation,
    /// Guard post on a wall walk, town tower, castle tower, or watchtower.
    GuardPost,
    /// Market well / plaza rally.
    RallyMarket,
    /// Keep courtyard rally.
    RallyKeep,
    /// Flag plinth in the inner bailey.
    Flag,
    /// Town or castle gate.
    Gate,
    /// Pillager tent / camp spawn.
    PillagerSpawn,
    /// Pillager camp court rally.
    PillagerRally,
    /// Covered ram in the camp siege yard.
    SiegeRam,
    /// Stacked siege ladders in the camp siege yard.
    SiegeLadder,
    /// Trebuchet in the camp siege yard.
    SiegeTreb,
}

/// One spawn or rally column. `y` is planner surface + 1 (standing height).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnPoint {
    pub role: SpawnRole,
    pub x: i32,
    pub y: i32,
    pub z: i32,
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
    /// Hinterland and civic scenes (farms, hamlet, camp, monastery, …).
    pub scenes: Vec<Scene>,
    /// Villager / pillager spawn and rally points. Pure function of seed+cell.
    pub spawns: Vec<SpawnPoint>,
}

impl TownInfo {
    /// Builds a snapshot from a live plan.
    #[must_use]
    pub fn from_plan(plan: &TownPlan) -> Self {
        let (cx, cz) = plan.bounds.center();
        let castle_wall = if plan.castle.outer_ring.verts.len() >= 3 {
            plan.castle
                .outer_ring
                .verts
                .iter()
                .map(|&(x, z)| [x, z])
                .collect()
        } else {
            vec![
                [plan.castle.bounds.min_x, plan.castle.bounds.min_z],
                [plan.castle.bounds.max_x, plan.castle.bounds.min_z],
                [plan.castle.bounds.max_x, plan.castle.bounds.max_z],
                [plan.castle.bounds.min_x, plan.castle.bounds.max_z],
            ]
        };
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
                wall: castle_wall,
                flag: [plan.castle.flag.0, plan.castle.flag.1],
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
            scenes: plan.scenes.clone(),
            spawns: collect_spawns(plan),
        }
    }

    /// JSON for dumps and later game systems.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

fn collect_spawns(plan: &TownPlan) -> Vec<SpawnPoint> {
    let mut spawns = Vec::new();
    let at = |role: SpawnRole, x: i32, z: i32| SpawnPoint {
        role,
        x,
        y: plan.heights.height(x, z) + 1,
        z,
    };

    for lot in &plan.lots {
        let (cx, cz) = lot.rect.center();
        match lot.kind {
            BuildingKind::Cottage
            | BuildingKind::Timber
            | BuildingKind::Townhouse
            | BuildingKind::Longhouse
            | BuildingKind::Tavern => spawns.push(at(SpawnRole::VillagerBed, cx, cz)),
            BuildingKind::Blacksmith
            | BuildingKind::Church
            | BuildingKind::Monastery
            | BuildingKind::Cathedral
            | BuildingKind::Windmill
            | BuildingKind::Watermill
            | BuildingKind::Barn
            | BuildingKind::Stall
            | BuildingKind::Winepress
            | BuildingKind::StaveChurch => spawns.push(at(SpawnRole::Workstation, cx, cz)),
            BuildingKind::Watchtower
            | BuildingKind::Outpost
            | BuildingKind::BellTower
            | BuildingKind::TownHall => {
                spawns.push(at(SpawnRole::GuardPost, cx, cz));
            }
        }
    }

    for tower in &plan.town_towers {
        let (cx, cz) = tower.center();
        spawns.push(at(SpawnRole::GuardPost, cx, cz));
    }
    for tower in &plan.castle.towers {
        let (cx, cz) = tower.center();
        spawns.push(at(SpawnRole::GuardPost, cx, cz));
    }
    for &(x, z) in plan.town_wall.iter().step_by(12) {
        spawns.push(at(SpawnRole::GuardPost, x, z));
    }

    spawns.push(at(SpawnRole::RallyMarket, plan.well.0, plan.well.1));
    let (kx, kz) = plan.castle.inner.center();
    spawns.push(at(SpawnRole::RallyKeep, kx, kz));
    spawns.push(at(SpawnRole::Flag, plan.castle.flag.0, plan.castle.flag.1));
    for gate in &plan.gates {
        spawns.push(at(SpawnRole::Gate, gate.x, gate.z));
    }

    for scene in &plan.scenes {
        match scene.kind {
            SceneKind::PillagerCamp => {
                let (cx, cz) = scene.rect.center();
                spawns.push(at(SpawnRole::PillagerRally, cx, cz));
                // Walk every column. A coarse `step_by(6)` misses the rebuilt
                // 4×3 tents (they sit between the old sample points).
                let mut tents = 0u32;
                let mut rams = 0u32;
                let mut ladders = 0u32;
                let mut trebs = 0u32;
                for z in scene.rect.min_z..=scene.rect.max_z {
                    for x in scene.rect.min_x..=scene.rect.max_x {
                        match plan.occ.kind(x, z) {
                            crate::cells::CellKind::Tent | crate::cells::CellKind::WarTent
                                if tents < 16 && (x + z) % 2 == 0 =>
                            {
                                spawns.push(at(SpawnRole::PillagerSpawn, x, z));
                                tents += 1;
                            }
                            crate::cells::CellKind::SiegeRam
                                if rams < 8 && ((x + z) % 3 == 0 || rams == 0) =>
                            {
                                spawns.push(at(SpawnRole::SiegeRam, x, z));
                                rams += 1;
                            }
                            crate::cells::CellKind::SiegeLadder
                                if ladders < 16 && ((x + z) % 2 == 0 || ladders == 0) =>
                            {
                                spawns.push(at(SpawnRole::SiegeLadder, x, z));
                                ladders += 1;
                            }
                            crate::cells::CellKind::SiegeTreb
                                if trebs < 4 && ((x + z) % 4 == 0 || trebs == 0) =>
                            {
                                spawns.push(at(SpawnRole::SiegeTreb, x, z));
                                trebs += 1;
                            }
                            _ => {}
                        }
                    }
                }
                if tents == 0 {
                    for z in (scene.rect.min_z + 3..scene.rect.max_z - 2).step_by(4) {
                        for x in (scene.rect.min_x + 3..scene.rect.max_x - 2).step_by(4) {
                            if matches!(
                                plan.occ.kind(x, z),
                                crate::cells::CellKind::CampCourt
                                    | crate::cells::CellKind::PalisadeGate
                            ) {
                                spawns.push(at(SpawnRole::PillagerSpawn, x, z));
                            }
                        }
                    }
                }
            }
            SceneKind::Watchtower | SceneKind::MountainFortress => {
                let (cx, cz) = scene.rect.center();
                spawns.push(at(SpawnRole::GuardPost, cx, cz));
            }
            SceneKind::Cathedral
            | SceneKind::CathedralClose
            | SceneKind::Mine
            | SceneKind::Riverside
            | SceneKind::Vineyard
            | SceneKind::StaveChurch
            | SceneKind::Caravanserai
            | SceneKind::MarketQuarter
            | SceneKind::CraftsmenRow
            | SceneKind::DocksQuarter
            | SceneKind::LumberCamp
            | SceneKind::Extramural => {
                let (cx, cz) = scene.rect.center();
                spawns.push(at(SpawnRole::Workstation, cx, cz));
            }
            SceneKind::Frontier | SceneKind::Manor | SceneKind::Gorge | SceneKind::NobleQuarter => {
                let (cx, cz) = scene.rect.center();
                spawns.push(at(SpawnRole::GuardPost, cx, cz));
            }
            _ => {}
        }
    }
    spawns
}

/// Snapshot of a cached plan, if one has been built for this cell.
///
/// Never generates on the caller. A miss queues background planning and
/// returns `None` so the tick can skip that town.
#[must_use]
pub fn town_info(seed: u64, cell_x: i32, cell_z: i32) -> Option<TownInfo> {
    if let Some(info) = crate::plan::cached_town_info(seed, cell_x, cell_z) {
        return Some(info);
    }
    crate::plan::request_plan(seed, cell_x, cell_z);
    None
}

/// Parse `seed:cell_x:cell_z` and return the cached snapshot.
#[must_use]
pub fn town_info_from_id(_seed: u64, town_id: &str) -> Option<TownInfo> {
    let mut parts = town_id.split(':');
    let parsed_seed: u64 = parts.next()?.parse().ok()?;
    let cell_x: i32 = parts.next()?.parse().ok()?;
    let cell_z: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    town_info(parsed_seed, cell_x, cell_z)
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
        assert!(!info.spawns.is_empty());
        let json = info.to_json().unwrap();
        let back: TownInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, info.id);
        assert_eq!(back.castle.keep, info.castle.keep);
        assert_eq!(back.spawns.len(), info.spawns.len());
    }

    #[test]
    fn camp_spawns_sit_on_rebuilt_tents_and_siege_engines() {
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
        let tents: Vec<_> = info
            .spawns
            .iter()
            .filter(|s| s.role == SpawnRole::PillagerSpawn)
            .collect();
        assert!(!tents.is_empty(), "raiders should spawn from camp tents");
        let on_tent = tents.iter().filter(|s| {
            matches!(
                plan.occ.kind(s.x, s.z),
                crate::cells::CellKind::Tent | crate::cells::CellKind::WarTent
            )
        });
        assert!(
            on_tent.count() >= 4,
            "PillagerSpawn posts should land on Tent/WarTent cells, got {tents:?}"
        );
        assert!(
            info.spawns.iter().any(|s| s.role == SpawnRole::SiegeRam),
            "covered ram should be a spawn post"
        );
        assert!(
            info.spawns.iter().any(|s| s.role == SpawnRole::SiegeLadder),
            "siege ladders should be spawn posts"
        );
        assert!(
            info.spawns.iter().any(|s| s.role == SpawnRole::SiegeTreb),
            "trebuchet should be a spawn post"
        );
    }
}
