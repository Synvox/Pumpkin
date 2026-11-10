//! Automated envelope check: interiors must not flood to the outside.

use std::collections::{HashSet, VecDeque};

use pumpkin_data::block_properties::{HorizontalFacing, StairsProperties};
use pumpkin_data::{Block, BlockState};

use crate::buildings::{BuildingKind, Lot};
use crate::cells::CellKind;
use crate::geom::{Dir, Rect};
use crate::plan::TownPlan;
use crate::stamp::VolumeSink;

/// Result for one building envelope.
#[derive(Clone, Debug)]
pub struct BuildingCheck {
    pub name: String,
    pub columns: usize,
    pub seed: Option<(i32, i32, i32)>,
    pub leak: Option<(i32, i32, i32)>,
    pub floating_roofs: usize,
    /// Interior floor cells that can see the sky (missing roof).
    pub sky_open: usize,
    /// Keep/hall roof ridges that leave a slot (opposing stairs or walls in the peak).
    pub open_ridges: usize,
}

impl BuildingCheck {
    /// Whether the envelope held and roofs had support.
    ///
    /// A barred gatehouse may be solid above the passage (no 2-high room)
    /// when the tunnel is not boxed in by masonry on all four sides.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.floating_roofs == 0
            && self.leak.is_none()
            && self.sky_open == 0
            && self.open_ridges == 0
    }
}

/// All building checks for one town.
#[derive(Clone, Debug)]
pub struct TownCheck {
    pub seed: u64,
    pub buildings: Vec<BuildingCheck>,
}

impl TownCheck {
    /// Whether every building passed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.buildings.iter().all(BuildingCheck::ok)
    }

    /// Failures only.
    #[must_use]
    pub fn failures(&self) -> Vec<&BuildingCheck> {
        self.buildings.iter().filter(|b| !b.ok()).collect()
    }

    /// One-line summary.
    #[must_use]
    pub fn summary(&self) -> String {
        let pass = self.buildings.iter().filter(|b| b.ok()).count();
        let n = self.buildings.len();
        if self.ok() {
            format!("seed {} watertight {pass}/{n}", self.seed)
        } else {
            let fails: Vec<String> = self
                .failures()
                .iter()
                .map(|b| {
                    format!(
                        "{} leak={:?} float={} sky={} ridge={} seed={:?}",
                        b.name, b.leak, b.floating_roofs, b.sky_open, b.open_ridges, b.seed
                    )
                })
                .collect();
            format!("seed {} FAIL {pass}/{n}: {}", self.seed, fails.join("; "))
        }
    }
}

/// Leaves or plants with air below, plus feature trees on tree-strip columns.
///
/// Covers the whole town plan (walled town, market crop, lots, scene yards).
/// Lily pads fail even when they sit on water.
#[must_use]
pub fn check_scene_vegetation(plan: &TownPlan, volume: &VolumeSink) -> Vec<String> {
    let mut fails = Vec::new();
    let min_z = volume.min_z.max(plan.bounds.min_z);
    let max_z = (volume.min_z + volume.sz - 1).min(plan.bounds.max_z);
    let min_x = volume.min_x.max(plan.bounds.min_x);
    let max_x = (volume.min_x + volume.sx - 1).min(plan.bounds.max_x);
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            if !plan.strips_trees(x, z) {
                continue;
            }
            let kind = plan.occ.kind(x, z);
            let where_ = plan
                .scene_at(x, z)
                .map_or_else(|| "town".to_string(), |scene| format!("{:?}", scene.kind));
            for y in volume.min_y..volume.min_y + volume.sy {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                if state.is_air() {
                    continue;
                }
                let block = Block::from_state_id(state.id);
                if is_water_plant(block.id) {
                    fails.push(format!(
                        "{where_} water-plant {} at {x},{y},{z}",
                        block.name
                    ));
                    continue;
                }
                if is_leaf_or_plant(block.id) {
                    let below_air = volume
                        .get(x, y - 1, z)
                        .is_none_or(pumpkin_data::BlockState::is_air);
                    if below_air
                        && !adjacent_log(volume, x, y, z)
                        && y + 4 >= plan.heights.height(x, z)
                        && !planned_timber(kind)
                    {
                        fails.push(format!(
                            "{where_} floating {} at {x},{y},{z} kind={kind:?}",
                            block.name
                        ));
                    }
                }
                if is_log(block.id)
                    && !planned_timber(kind)
                    && nearby_leaves(volume, x, y, z)
                    && y + 4 >= plan.heights.height(x, z)
                {
                    fails.push(format!(
                        "{where_} feature tree at {x},{y},{z} kind={kind:?}"
                    ));
                }
            }
        }
    }
    fails
}

fn is_leaf_or_plant(id: pumpkin_data::BlockId) -> bool {
    id == Block::OAK_LEAVES.id
        || id == Block::SPRUCE_LEAVES.id
        || id == Block::BIRCH_LEAVES.id
        || id == Block::JUNGLE_LEAVES.id
        || id == Block::ACACIA_LEAVES.id
        || id == Block::DARK_OAK_LEAVES.id
        || id == Block::CHERRY_LEAVES.id
        || id == Block::MANGROVE_LEAVES.id
        || id == Block::AZALEA_LEAVES.id
        || id == Block::FLOWERING_AZALEA_LEAVES.id
        || id == Block::OAK_SAPLING.id
        || id == Block::SPRUCE_SAPLING.id
        || id == Block::BIRCH_SAPLING.id
        || id == Block::JUNGLE_SAPLING.id
        || id == Block::ACACIA_SAPLING.id
        || id == Block::DARK_OAK_SAPLING.id
}

fn is_water_plant(id: pumpkin_data::BlockId) -> bool {
    id == Block::LILY_PAD.id
        || id == Block::SEAGRASS.id
        || id == Block::TALL_SEAGRASS.id
        || id == Block::KELP.id
        || id == Block::KELP_PLANT.id
}

fn is_log(id: pumpkin_data::BlockId) -> bool {
    id == Block::OAK_LOG.id
        || id == Block::SPRUCE_LOG.id
        || id == Block::BIRCH_LOG.id
        || id == Block::JUNGLE_LOG.id
        || id == Block::ACACIA_LOG.id
        || id == Block::DARK_OAK_LOG.id
        || id == Block::CHERRY_LOG.id
        || id == Block::MANGROVE_LOG.id
}

fn planned_timber(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Building
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Crane
            | CellKind::Dock
            | CellKind::MineHead
            | CellKind::MineAdit
            | CellKind::MineRail
            | CellKind::Palisade
            | CellKind::PalisadeWalk
            | CellKind::PalisadeGate
            | CellKind::WindSail
            | CellKind::WatchPost
            | CellKind::Orchard
            | CellKind::Grave
            | CellKind::Lychgate
            | CellKind::Arcade
            | CellKind::Hedge
            | CellKind::Tent
            | CellKind::WarTent
            | CellKind::SiegeTreb
            | CellKind::SiegeRam
            | CellKind::SiegeLadder
            | CellKind::Burnt
            | CellKind::Eave
            | CellKind::WaterWheel
            | CellKind::Sluice
            | CellKind::Vine
            | CellKind::Shrine
            | CellKind::FieldWall
    )
}

fn adjacent_log(volume: &VolumeSink, x: i32, y: i32, z: i32) -> bool {
    for (dx, dy, dz) in [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        if let Some(state) = volume.get(x + dx, y + dy, z + dz)
            && is_log(Block::from_state_id(state.id).id)
        {
            return true;
        }
    }
    false
}

fn nearby_leaves(volume: &VolumeSink, x: i32, y: i32, z: i32) -> bool {
    for dy in -1..=3 {
        for dz in -2..=2 {
            for dx in -2..=2 {
                if let Some(state) = volume.get(x + dx, y + dy, z + dz) {
                    let id = Block::from_state_id(state.id).id;
                    if id == Block::OAK_LEAVES.id
                        || id == Block::SPRUCE_LEAVES.id
                        || id == Block::BIRCH_LEAVES.id
                        || id == Block::JUNGLE_LEAVES.id
                        || id == Block::ACACIA_LEAVES.id
                        || id == Block::DARK_OAK_LEAVES.id
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Runs the flood-fill and roof-support checks for every roofed building.
#[must_use]
pub fn check_town(plan: &TownPlan, volume: &VolumeSink) -> TownCheck {
    let mut buildings = Vec::new();
    push_if_any(
        &mut buildings,
        "keep",
        collect_kind(plan, CellKind::CastleKeep, plan.castle.keep.inflate(12)),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "hall",
        collect_kind(plan, CellKind::CastleHall, plan.castle.hall.inflate(2)),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "chapel",
        collect_kind(plan, CellKind::CastleChapel, plan.castle.chapel.inflate(6)),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "kitchen",
        collect_kind(plan, CellKind::CastleKitchen, plan.castle.kitchen),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "barracks",
        collect_kind(plan, CellKind::CastleBarracks, plan.castle.barracks),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "stables",
        collect_kind(plan, CellKind::CastleStables, plan.castle.stables),
        volume,
        plan,
    );
    push_if_any_sky(
        &mut buildings,
        "gatehouse",
        collect_kind(plan, CellKind::CastleGate, plan.bounds),
        volume,
        plan,
        false,
    );
    for (i, spec) in plan.castle.tower_specs.iter().enumerate() {
        let cols = collect_spec(plan, CellKind::CastleTower, |x, z| spec.contains(x, z));
        if cols.len() < 8 {
            continue;
        }
        // Crenellated wall-walks are supposed to see the sky.
        buildings.push(check_columns(
            &format!("tower{i}"),
            cols,
            volume,
            plan,
            false,
        ));
    }
    for (i, mass) in plan.castle.keep_masses.iter().enumerate() {
        let cols = collect_spec(plan, CellKind::CastleKeep, |x, z| mass.spec.contains(x, z));
        if cols.len() < 8 {
            continue;
        }
        buildings.push(check_columns(
            &format!("keep-mass{i}"),
            cols,
            volume,
            plan,
            !matches!(mass.spec.roof, crate::castle::RoofKind::Crenellated),
        ));
    }
    push_if_any(
        &mut buildings,
        "fortress-keep",
        collect_kind(plan, CellKind::FortressKeep, plan.bounds),
        volume,
        plan,
    );
    push_if_any(
        &mut buildings,
        "cathedral",
        collect_cathedral(plan),
        volume,
        plan,
    );
    for lot in &plan.lots {
        if matches!(lot.kind, BuildingKind::Stall) {
            continue;
        }
        let mut cols = if lot.kind == BuildingKind::Watermill {
            collect_watermill(plan, lot.rect)
        } else {
            collect_lot(plan, lot.id, lot.rect)
        };
        if matches!(lot.kind, BuildingKind::Windmill) {
            let (cx, cz) = lot.rect.center();
            cols.retain(|&(x, z)| (x - cx).abs().max((z - cz).abs()) <= 2);
        }
        if cols.is_empty() {
            continue;
        }
        let check_sky = !matches!(
            lot.kind,
            BuildingKind::Watchtower | BuildingKind::BellTower | BuildingKind::TownHall
        );
        buildings.push(check_columns(
            &format!("lot{}-{:?}", lot.id, lot.kind),
            cols,
            volume,
            plan,
            check_sky,
        ));
        if lot.kind == BuildingKind::Windmill {
            buildings.push(check_mill_connected(plan, lot, volume));
        }
    }
    TownCheck {
        seed: plan.seed,
        buildings,
    }
}

fn push_if_any(
    out: &mut Vec<BuildingCheck>,
    name: &str,
    columns: Vec<(i32, i32)>,
    volume: &VolumeSink,
    plan: &TownPlan,
) {
    push_if_any_sky(out, name, columns, volume, plan, true);
}

fn push_if_any_sky(
    out: &mut Vec<BuildingCheck>,
    name: &str,
    columns: Vec<(i32, i32)>,
    volume: &VolumeSink,
    plan: &TownPlan,
    check_sky: bool,
) {
    if !columns.is_empty() {
        out.push(check_columns(name, columns, volume, plan, check_sky));
    }
}

fn collect_kind(plan: &TownPlan, kind: CellKind, bounds: Rect) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    let min_x = bounds.min_x.max(plan.bounds.min_x);
    let max_x = bounds.max_x.min(plan.bounds.max_x);
    let min_z = bounds.min_z.max(plan.bounds.min_z);
    let max_z = bounds.max_z.min(plan.bounds.max_z);
    if min_x > max_x || min_z > max_z {
        return cols;
    }
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            if plan.occ.kind(x, z) == kind {
                cols.push((x, z));
            }
        }
    }
    cols
}

fn collect_spec(
    plan: &TownPlan,
    kind: CellKind,
    contains: impl Fn(i32, i32) -> bool,
) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    for z in plan.bounds.min_z..=plan.bounds.max_z {
        for x in plan.bounds.min_x..=plan.bounds.max_x {
            if plan.occ.kind(x, z) == kind && contains(x, z) {
                cols.push((x, z));
            }
        }
    }
    cols
}

fn collect_cathedral(plan: &TownPlan) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    for z in plan.bounds.min_z..=plan.bounds.max_z {
        for x in plan.bounds.min_x..=plan.bounds.max_x {
            if matches!(
                plan.occ.kind(x, z),
                CellKind::CathedralNave
                    | CellKind::CathedralTransept
                    | CellKind::CathedralChoir
                    | CellKind::CathedralTower
            ) {
                cols.push((x, z));
            }
        }
    }
    cols
}

fn collect_lot(plan: &TownPlan, lot_id: u16, rect: Rect) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if plan.occ.lot(x, z) == lot_id && plan.occ.kind(x, z) == CellKind::Building {
                cols.push((x, z));
            }
        }
    }
    cols
}

fn collect_watermill(plan: &TownPlan, rect: Rect) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if plan.occ.kind(x, z) != CellKind::WaterWheel {
                cols.push((x, z));
            }
        }
    }
    cols
}

fn check_columns(
    name: &str,
    columns: Vec<(i32, i32)>,
    volume: &VolumeSink,
    plan: &TownPlan,
    check_sky: bool,
) -> BuildingCheck {
    if columns.is_empty() {
        return BuildingCheck {
            name: name.to_string(),
            columns: 0,
            seed: None,
            leak: None,
            floating_roofs: 0,
            sky_open: 0,
            open_ridges: 0,
        };
    }
    let set: std::collections::HashSet<(i32, i32)> = columns.iter().copied().collect();
    let seed = find_seed(&columns, volume);
    let leak = seed.and_then(|s| flood_leak(s, &set, volume, plan));
    let floating_roofs = count_floating_roofs(&columns, volume);
    let keep_or_hall = name.contains("keep") || name.contains("hall");
    let sky_open = if check_sky {
        let mut n = count_sky_open(&set, volume);
        if name.contains("keep-mass") || name == "hall" || name == "fortress-keep" {
            n += count_roof_wells(&set, volume, Some(plan));
        }
        n
    } else {
        0
    };
    let open_ridges = if check_sky && keep_or_hall {
        count_open_ridges(&set, volume)
    } else {
        0
    };
    BuildingCheck {
        name: name.to_string(),
        columns: columns.len(),
        seed,
        leak,
        floating_roofs,
        sky_open,
        open_ridges,
    }
}

fn mill_hub_y(plan: &TownPlan, mill: &Lot) -> i32 {
    plan.heights
        .mean(
            mill.rect.min_x,
            mill.rect.min_z,
            mill.rect.max_x,
            mill.rect.max_z,
        )
        .max(plan.heights.sea_level + 2)
        + 18
}

fn is_mill_part(plan: &TownPlan, mill: &Lot, x: i32, z: i32) -> bool {
    if mill.rect.contains(x, z) {
        return true;
    }
    if plan.occ.kind(x, z) != CellKind::WindSail
        || !crate::stamp::mill_owns_sail(&plan.lots, mill, x, z)
    {
        return false;
    }
    let (cx, cz) = mill.rect.center();
    let dx = x - cx;
    let dz = z - cz;
    dx != 0
        && dz != 0
        && (dx.abs() - dz.abs()).abs() <= 1
        && (1..=10).contains(&dx.abs().max(dz.abs()))
}

fn is_mill_sail_block(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::WHITE_WOOL.id
        || id == Block::WHITE_BANNER.id
        || id == Block::OAK_TRAPDOOR.id
        || id == Block::OAK_LOG.id
        || id == Block::DARK_OAK_LOG.id
        || id == Block::OAK_FENCE.id
        || id == Block::DARK_OAK_FENCE.id
        || id == Block::OAK_SLAB.id
        || id == Block::OAK_STAIRS.id
        || id == Block::DARK_OAK_SLAB.id
        || id == Block::DARK_OAK_STAIRS.id
        || id == Block::SPRUCE_SLAB.id
        || id == Block::SPRUCE_STAIRS.id
        || id == Block::SMOOTH_SANDSTONE_SLAB.id
        || id == Block::SANDSTONE_STAIRS.id
}

fn check_mill_connected(plan: &TownPlan, mill: &Lot, volume: &VolumeSink) -> BuildingCheck {
    let (cx, cz) = mill.rect.center();
    let hub_y = mill_hub_y(plan, mill);
    let mut parts = HashSet::new();
    for z in mill.rect.min_z - 12..=mill.rect.max_z + 12 {
        for x in mill.rect.min_x - 12..=mill.rect.max_x + 12 {
            if !is_mill_part(plan, mill, x, z) {
                continue;
            }
            let dx = x - cx;
            let dz = z - cz;
            let diagonal = dx.abs() == dz.abs() && dx != 0;
            let on_tower = mill.rect.contains(x, z);
            let y0 = if diagonal { hub_y - 3 } else { hub_y - 1 };
            let y1 = if diagonal { hub_y + 4 } else { hub_y + 1 };
            for y in y0..=y1 {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                if state.is_air() {
                    continue;
                }
                if on_tower && y < hub_y - 1 && !is_mill_sail_block(state) {
                    continue;
                }
                if !on_tower && !is_mill_sail_block(state) {
                    continue;
                }
                parts.insert((x, y, z));
            }
        }
    }
    let start = (cx, hub_y, cz);
    let mut reached = HashSet::new();
    let mut queue = VecDeque::new();
    if parts.contains(&start) {
        reached.insert(start);
        queue.push_back(start);
    }
    while let Some((x, y, z)) = queue.pop_front() {
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            let n = (x + dx, y + dy, z + dz);
            if parts.contains(&n) && reached.insert(n) {
                queue.push_back(n);
            }
        }
    }
    let leak = parts.iter().copied().find(|p| !reached.contains(p));
    BuildingCheck {
        name: format!("mill{}-sails", mill.id),
        columns: parts.len(),
        seed: Some(start),
        leak,
        floating_roofs: 0,
        sky_open: 0,
        open_ridges: 0,
    }
}

fn find_seed(columns: &[(i32, i32)], volume: &VolumeSink) -> Option<(i32, i32, i32)> {
    let mut best: Option<(i32, i32, i32, i32)> = None;
    for &(x, z) in columns {
        let Some((top, _)) = volume.top(x, z) else {
            continue;
        };
        let mut floor = None;
        let mut roof = None;
        for y in volume.min_y..=top {
            if is_envelope_solid(volume.get(x, y, z)) {
                if floor.is_none() {
                    floor = Some(y);
                }
                roof = Some(y);
            }
        }
        let (Some(floor), Some(roof)) = (floor, roof) else {
            continue;
        };
        if roof < floor + 2 {
            continue;
        }
        let mut run = 0;
        let mut first_air = None;
        for y in floor + 1..roof {
            if is_air(volume, x, y, z) {
                if first_air.is_none() {
                    first_air = Some(y);
                }
                run += 1;
            } else if run > 0 {
                break;
            }
        }
        if run >= 2
            && let Some(y) = first_air
            && is_envelope_solid(volume.get(x, y - 1, z))
            && best.is_none_or(|(_, sy, _, _)| y < sy)
        {
            best = Some((x, y, z, run));
        }
    }
    best.map(|(x, y, z, _)| (x, y, z))
}

fn flood_leak(
    seed: (i32, i32, i32),
    columns: &std::collections::HashSet<(i32, i32)>,
    volume: &VolumeSink,
    plan: &TownPlan,
) -> Option<(i32, i32, i32)> {
    const DIRS: [(i32, i32, i32); 6] = [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];
    let mut seen = std::collections::HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(seed);
    seen.insert(seed);
    let mut steps = 0u32;
    while let Some((x, y, z)) = q.pop_front() {
        steps += 1;
        if steps > 80_000 {
            break;
        }
        if !columns.contains(&(x, z)) {
            if is_exterior(plan.occ.kind(x, z)) {
                return Some((x, y, z));
            }
            continue;
        }
        if y < volume.min_y || y >= volume.min_y + volume.sy {
            return Some((x, y, z));
        }
        for (dx, dy, dz) in DIRS {
            let n = (x + dx, y + dy, z + dz);
            if !seen.insert(n) {
                continue;
            }
            // Air above a building column's own roof is outside, not a room.
            if columns.contains(&(n.0, n.2)) && volume.top(n.0, n.2).is_some_and(|(ty, _)| n.1 > ty)
            {
                continue;
            }
            if is_air(volume, n.0, n.1, n.2) {
                q.push_back(n);
            }
        }
    }
    None
}

fn is_exterior(kind: CellKind) -> bool {
    !matches!(
        kind,
        CellKind::CastleKeep
            | CellKind::CastleHall
            | CellKind::CastleChapel
            | CellKind::CastleKitchen
            | CellKind::CastleBarracks
            | CellKind::CastleStables
            | CellKind::CastleGate
            | CellKind::CastleTower
            | CellKind::CastleBarbican
            | CellKind::CastleWall
            | CellKind::CastleInnerWall
            | CellKind::Building
            | CellKind::TownWall
            | CellKind::TownTower
            | CellKind::TownGate
            | CellKind::FortressKeep
            | CellKind::FortressTower
            | CellKind::FortressWall
            | CellKind::CathedralNave
            | CellKind::CathedralTransept
            | CellKind::CathedralChoir
            | CellKind::CathedralTower
    )
}

fn count_floating_roofs(columns: &[(i32, i32)], volume: &VolumeSink) -> usize {
    let mut n = 0;
    for &(x, z) in columns {
        let Some((top, state)) = volume.top(x, z) else {
            continue;
        };
        if !is_roof_block(state) {
            continue;
        }
        if !is_envelope_solid(volume.get(x, top - 1, z)) {
            n += 1;
        }
    }
    n
}

fn is_air(volume: &VolumeSink, x: i32, y: i32, z: i32) -> bool {
    volume.get(x, y, z).is_some_and(BlockState::is_air)
}

/// Panes, doors, trapdoors, stairs, slabs, fences, bars and walls count as closed.
fn is_envelope_solid(state: Option<&BlockState>) -> bool {
    let Some(state) = state else {
        return false;
    };
    if state.is_air() {
        return false;
    }
    true
}

fn count_sky_open(columns: &std::collections::HashSet<(i32, i32)>, volume: &VolumeSink) -> usize {
    let mut n = 0;
    for &(x, z) in columns {
        if !is_strict_interior(x, z, columns) {
            continue;
        }
        if room_floors(volume, x, z)
            .any(|floor| sees_sky(volume, x, floor, z) && !is_roof_deck(volume, x, floor, z))
        {
            n += 1;
        }
    }
    n
}

/// Interior roof crater: the column sits below the roof around it with no downhill.
fn count_roof_wells(
    columns: &std::collections::HashSet<(i32, i32)>,
    volume: &VolumeSink,
    plan: Option<&TownPlan>,
) -> usize {
    let mut n = 0;
    for &(x, z) in columns {
        if !is_strict_interior(x, z, columns) {
            continue;
        }
        let Some((top, _)) = volume.top(x, z) else {
            n += 1;
            continue;
        };
        let this_mass = plan.and_then(|p| p.castle.keep_mass_at(x, z));
        let mut higher = 0;
        let mut lower = 0;
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let nx = x + dx;
            let nz = z + dz;
            if !columns.contains(&(nx, nz)) {
                continue;
            }
            if let (Some(here), Some(there)) =
                (this_mass, plan.and_then(|p| p.castle.keep_mass_at(nx, nz)))
                && (here.spec.cx != there.spec.cx || here.spec.cz != there.spec.cz)
            {
                continue;
            }
            match volume.top(nx, nz) {
                Some((n_top, _)) if n_top >= top + 3 => higher += 1,
                Some((n_top, _)) if n_top + 3 <= top => lower += 1,
                None => lower += 1,
                Some(_) => {}
            }
        }
        if higher >= 2 && lower == 0 {
            n += 1;
        }
    }
    n
}

fn count_open_ridges(
    columns: &std::collections::HashSet<(i32, i32)>,
    volume: &VolumeSink,
) -> usize {
    let mut n = 0;
    for &(x, z) in columns {
        if !is_strict_interior(x, z, columns) {
            continue;
        }
        let Some((mut y, _)) = volume.top(x, z) else {
            continue;
        };
        while y > volume.min_y {
            let Some(state) = volume.get(x, y, z) else {
                break;
            };
            if is_furniture(Some(state)) || is_peak_trim(state) {
                y -= 1;
                continue;
            }
            break;
        }
        let Some(state) = volume.get(x, y, z) else {
            continue;
        };
        if is_ridge_cap(state) {
            continue;
        }
        let seam = opposite_roof_seam(volume, x, y, z);
        if seam && (is_keep_masonry(state) || is_roof_stair(state)) {
            n += 1;
        }
    }
    n
}

fn opposite_roof_seam(volume: &VolumeSink, x: i32, y: i32, z: i32) -> bool {
    let ew = roof_stair_dir(volume, x + 1, y, z).zip(roof_stair_dir(volume, x - 1, y, z));
    let ns = roof_stair_dir(volume, x, y, z - 1).zip(roof_stair_dir(volume, x, y, z + 1));
    matches!(ew, Some((Dir::East, Dir::West))) || matches!(ns, Some((Dir::North, Dir::South)))
}

fn roof_stair_dir(volume: &VolumeSink, x: i32, y: i32, z: i32) -> Option<Dir> {
    let state = volume.get(x, y, z)?;
    if !is_roof_stair(state) {
        return None;
    }
    let props = StairsProperties::from_state_id(state.id);
    Some(match props.facing {
        HorizontalFacing::North => Dir::North,
        HorizontalFacing::South => Dir::South,
        HorizontalFacing::East => Dir::East,
        HorizontalFacing::West => Dir::West,
    })
}

fn is_roof_stair(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::DARK_OAK_STAIRS.id
        || id == Block::SPRUCE_STAIRS.id
        || id == Block::SANDSTONE_STAIRS.id
        || id == Block::OAK_STAIRS.id
}

fn is_ridge_cap(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::DARK_OAK_PLANKS.id
        || id == Block::SPRUCE_PLANKS.id
        || id == Block::SMOOTH_SANDSTONE.id
        || id == Block::DARK_OAK_SLAB.id
        || id == Block::SPRUCE_SLAB.id
        || id == Block::SMOOTH_SANDSTONE_SLAB.id
}

fn is_keep_masonry(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::STONE_BRICKS.id
        || id == Block::MOSSY_STONE_BRICKS.id
        || id == Block::CRACKED_STONE_BRICKS.id
        || id == Block::COBBLESTONE.id
        || id == Block::MOSSY_COBBLESTONE.id
        || id == Block::WHITE_TERRACOTTA.id
        || id == Block::ANDESITE.id
        || id == Block::CALCITE.id
        || id == Block::CUT_SANDSTONE.id
}

fn is_peak_trim(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::STONE_BRICK_WALL.id
        || id == Block::COBBLESTONE_WALL.id
        || id == Block::OAK_FENCE.id
        || id == Block::SPRUCE_FENCE.id
        || id == Block::DARK_OAK_FENCE.id
}

fn is_strict_interior(x: i32, z: i32, columns: &std::collections::HashSet<(i32, i32)>) -> bool {
    columns.contains(&(x, z))
        && columns.contains(&(x - 1, z))
        && columns.contains(&(x + 1, z))
        && columns.contains(&(x, z - 1))
        && columns.contains(&(x, z + 1))
}

fn room_floors(volume: &VolumeSink, x: i32, z: i32) -> impl Iterator<Item = i32> + '_ {
    let top = volume.top(x, z).map_or(volume.min_y, |(y, _)| y);
    (volume.min_y..=top).filter(move |&y| {
        let state = volume.get(x, y, z);
        if !is_sky_cover(state) || state.is_some_and(is_roof_block) {
            return false;
        }
        if volume.get(x, y - 1, z).is_some_and(is_roof_block) {
            return false;
        }
        let a1 = volume.get(x, y + 1, z);
        let a2 = volume.get(x, y + 2, z);
        is_room_gap(a1) && is_room_gap(a2)
    })
}

fn is_room_gap(state: Option<&BlockState>) -> bool {
    state.is_none_or(BlockState::is_air) || is_furniture(state)
}

fn is_roof_deck(volume: &VolumeSink, x: i32, floor: i32, z: i32) -> bool {
    let Some((top, _)) = volume.top(x, z) else {
        return false;
    };
    if floor + 3 < top {
        return false;
    }
    let mut solids = 0;
    let mut y = top;
    while y >= volume.min_y && y >= top - 8 {
        if is_sky_cover(volume.get(x, y, z)) {
            solids += 1;
        } else if !is_furniture(volume.get(x, y, z)) {
            break;
        }
        y -= 1;
    }
    solids >= 4 || (solids >= 2 && floor + 1 >= top)
}

fn sees_sky(volume: &VolumeSink, x: i32, floor: i32, z: i32) -> bool {
    let max_y = volume.min_y + volume.sy;
    for y in floor + 1..max_y {
        let state = volume.get(x, y, z);
        if is_sky_cover(state) {
            return false;
        }
    }
    true
}

fn is_sky_cover(state: Option<&BlockState>) -> bool {
    is_envelope_solid(state) && !is_furniture(state)
}

fn is_furniture(state: Option<&BlockState>) -> bool {
    let Some(state) = state else {
        return false;
    };
    let id = Block::from_state_id(state.id).id;
    id == Block::RED_BED.id
        || id == Block::WHITE_BED.id
        || id == Block::RED_CARPET.id
        || id == Block::LANTERN.id
        || id == Block::CANDLE.id
        || id == Block::CHEST.id
        || id == Block::BARREL.id
        || id == Block::FURNACE.id
        || id == Block::SMOKER.id
        || id == Block::BLAST_FURNACE.id
        || id == Block::CRAFTING_TABLE.id
        || id == Block::ANVIL.id
        || id == Block::CAULDRON.id
        || id == Block::BOOKSHELF.id
        || id == Block::LECTERN.id
        || id == Block::CAMPFIRE.id
        || id == Block::OAK_FENCE.id
        || id == Block::SPRUCE_FENCE.id
        || id == Block::HAY_BLOCK.id
        || id == Block::LADDER.id
        || id == Block::GRINDSTONE.id
        || id == Block::FLETCHING_TABLE.id
        || id == Block::SMITHING_TABLE.id
        || id == Block::CARTOGRAPHY_TABLE.id
        || id == Block::RED_BANNER.id
        || id == Block::OAK_DOOR.id
        || id == Block::SPRUCE_DOOR.id
        || id == Block::GLASS_PANE.id
        || id == Block::RED_STAINED_GLASS_PANE.id
        || id == Block::YELLOW_STAINED_GLASS_PANE.id
        || id == Block::PURPLE_STAINED_GLASS_PANE.id
        || id == Block::IRON_BARS.id
        || id == Block::OAK_TRAPDOOR.id
}

fn is_roof_block(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::DARK_OAK_STAIRS.id
        || id == Block::DARK_OAK_SLAB.id
        || id == Block::SPRUCE_STAIRS.id
        || id == Block::SPRUCE_SLAB.id
        || id == Block::STONE_BRICK_STAIRS.id
        || id == Block::SANDSTONE_STAIRS.id
        || id == Block::OAK_STAIRS.id
        || id == Block::DEEPSLATE_TILE_STAIRS.id
        || id == Block::DEEPSLATE_TILE_SLAB.id
        || id == Block::DEEPSLATE_TILES.id
        || id == Block::SANDSTONE_SLAB.id
        || id == Block::SMOOTH_SANDSTONE_SLAB.id
        || id == Block::SMOOTH_SANDSTONE.id
        || id == Block::SNOW.id
        || id == Block::SNOW_BLOCK.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::CellKind;
    use crate::height::HeightGrid;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::stamp::{BlockSink, fill_volume};
    use crate::town::town_in_cell;

    fn synthetic_town(seed: u64) -> TownCheck {
        let origin = town_in_cell(seed, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            8,
            6,
        );
        let plan = build_plan(seed, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        check_town(&plan, &volume)
    }

    #[test]
    fn synthetic_seed_batch_is_watertight() {
        let seeds = [1u64, 2, 9, 99, 123, 777, 2024, 4096, 99991, 123_456];
        let mut failed = Vec::new();
        for seed in seeds {
            let check = synthetic_town(seed);
            if !check.ok() {
                failed.push(check.summary());
            }
        }
        assert!(
            failed.is_empty(),
            "watertight failures:\n{}",
            failed.join("\n")
        );
    }

    #[test]
    fn keep_and_hall_roofs_have_no_open_ridge() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            6,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        let check = check_town(&plan, &volume);
        let ridge: Vec<_> = check
            .buildings
            .iter()
            .filter(|b| (b.name.contains("keep") || b.name.contains("hall")) && b.open_ridges > 0)
            .map(|b| format!("{} ridge={}", b.name, b.open_ridges))
            .collect();
        assert!(
            ridge.is_empty(),
            "keep/hall roofs should be enclosed at the ridge: {ridge:?}"
        );
        let sky: Vec<_> = check
            .buildings
            .iter()
            .filter(|b| (b.name.contains("keep") || b.name.contains("hall")) && b.sky_open > 0)
            .map(|b| format!("{} sky={}", b.name, b.sky_open))
            .collect();
        assert!(
            sky.is_empty(),
            "keep/hall interiors must not see the sky: {sky:?}"
        );
        let named = check
            .buildings
            .iter()
            .filter(|b| b.name.contains("keep") || b.name.contains("hall"))
            .count();
        assert!(named >= 1, "expected keep/hall envelopes, got {named}");
    }

    #[test]
    fn showcase_and_random_world_seeds_are_watertight() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

        let seeds = [20_261_025u64, 7, 42, 314_159, 11, 19, 101, 2026];
        let mut failed = Vec::new();
        for seed in seeds {
            let boxed = get_world_gen(
                Seed(seed),
                Dimension::OVERWORLD,
                false,
                Vec::new(),
                String::new(),
            );
            let world_gen = Arc::from(boxed);
            crate::height::register_world_gen(Arc::clone(&world_gen));
            let plan = crate::plan::plan_for(seed, 0, 0, &world_gen);
            let volume = fill_volume(&plan);
            let check = check_town(&plan, &volume);
            if !check.ok() {
                failed.push(check.summary());
            }
        }
        assert!(
            failed.is_empty(),
            "watertight failures:\n{}",
            failed.join("\n")
        );
    }

    #[test]
    fn interiors_are_air_with_floors() {
        let origin = town_in_cell(7, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            10,
            6,
        );
        let plan = build_plan(7, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        let check = check_town(&plan, &volume);
        assert!(
            check.buildings.iter().any(|b| b.name == "hall" && b.ok()),
            "{}",
            check.summary()
        );
        let hall = plan.castle.hall;
        let (cx, cz) = hall.center();
        let mut saw_air = false;
        let mut saw_floor = false;
        if let Some((top, _)) = volume.top(cx, cz) {
            for y in volume.min_y..=top {
                if let Some(state) = volume.get(cx, y, cz) {
                    if state.is_air() {
                        saw_air = true;
                    }
                    let id = Block::from_state_id(state.id).id;
                    if id == Block::OAK_PLANKS.id || id == Block::SPRUCE_PLANKS.id {
                        saw_floor = true;
                    }
                }
            }
        }
        assert!(saw_air, "hall interior is not cleared");
        assert!(saw_floor, "hall has no floor");
        let hall_blocks = crate::stamp::planned_blocks_in(&plan, hall);
        assert!(
            hall_blocks.iter().any(|b| {
                let id = Block::from_state_id(pumpkin_data::BlockStateId::new_or_air(b.state)).id;
                id == Block::LANTERN.id || id == Block::RED_CARPET.id
            }),
            "planned_block API missed hall furniture"
        );
    }

    #[test]
    fn missing_roof_fails_sky_visibility() {
        let mut volume = crate::stamp::VolumeSink::for_bounds(0, 0, 6, 6, 0, 20);
        let planks = Block::OAK_PLANKS.default_state;
        for z in 0..=6 {
            for x in 0..=6 {
                volume.put(x, 5, z, planks);
                if x == 0 || x == 6 || z == 0 || z == 6 {
                    for y in 6..=10 {
                        volume.put(x, y, z, planks);
                    }
                }
            }
        }
        let cols: std::collections::HashSet<(i32, i32)> =
            (0..=6).flat_map(|z| (0..=6).map(move |x| (x, z))).collect();
        assert!(
            count_sky_open(&cols, &volume) > 0,
            "open box must fail sky-visibility"
        );
        volume.put(3, 6, 3, Block::RED_BED.default_state);
        assert!(
            count_sky_open(&cols, &volume) > 0,
            "a bed must not count as a roof"
        );
        for z in 0..=6 {
            for x in 0..=6 {
                volume.put(x, 11, z, Block::DARK_OAK_STAIRS.default_state);
            }
        }
        assert_eq!(
            count_sky_open(&cols, &volume),
            0,
            "closed roof must pass sky-visibility"
        );
    }

    #[test]
    fn keep_roof_well_fails_sky_to_interior() {
        let mut volume = crate::stamp::VolumeSink::for_bounds(0, 0, 8, 8, 0, 24);
        let planks = Block::OAK_PLANKS.default_state;
        let masonry = Block::STONE_BRICKS.default_state;
        let cap = Block::DARK_OAK_PLANKS.default_state;
        for z in 0..=8 {
            for x in 0..=8 {
                volume.put(x, 5, z, planks);
                if x == 0 || x == 8 || z == 0 || z == 8 {
                    for y in 6..=12 {
                        volume.put(x, y, z, masonry);
                    }
                } else {
                    volume.put(x, 12, z, cap);
                }
            }
        }
        let cols: std::collections::HashSet<(i32, i32)> =
            (0..=8).flat_map(|z| (0..=8).map(move |x| (x, z))).collect();
        assert_eq!(
            count_roof_wells(&cols, &volume, None),
            0,
            "a flat keep cap must not look like a well"
        );
        for z in 2..=6 {
            for x in 2..=6 {
                volume.put(x, 12, z, Block::AIR.default_state);
                volume.put(x, 11, z, Block::AIR.default_state);
                volume.put(x, 10, z, Block::AIR.default_state);
            }
        }
        assert!(
            count_roof_wells(&cols, &volume, None) > 0,
            "a sunken keep roof well must fail"
        );
        for z in 2..=6 {
            for x in 2..=6 {
                volume.put(x, 12, z, cap);
            }
        }
        assert_eq!(
            count_roof_wells(&cols, &volume, None),
            0,
            "filling the well must pass"
        );
    }

    #[test]
    fn timber_houses_have_plaster_and_closed_roofs() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            8,
            6,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        let check = check_town(&plan, &volume);
        assert!(check.ok(), "{}", check.summary());
        let mut plaster_lots = 0u32;
        let mut roofed_lots = 0u32;
        let mut timber_lots = 0u32;
        for lot in &plan.lots {
            if !lot.kind.timber_frame() {
                continue;
            }
            timber_lots += 1;
            let mut plaster = 0u32;
            let mut open = 0u32;
            for z in lot.rect.min_z..=lot.rect.max_z {
                for x in lot.rect.min_x..=lot.rect.max_x {
                    if lot.rect.on_edge(x, z) && !lot.rect.is_corner(x, z) {
                        if let Some((top, _)) = volume.top(x, z) {
                            for y in (top - 16).max(volume.min_y)..=top {
                                if let Some(state) = volume.get(x, y, z) {
                                    let id = Block::from_state_id(state.id).id;
                                    if id == Block::WHITE_TERRACOTTA.id
                                        || id == Block::CALCITE.id
                                        || id == Block::BIRCH_PLANKS.id
                                    {
                                        plaster += 1;
                                    }
                                }
                            }
                        }
                    } else if !lot.rect.on_edge(x, z)
                        && let Some((top, state)) = volume.top(x, z)
                    {
                        let id = Block::from_state_id(state.id).id;
                        let covered = id == Block::DARK_OAK_STAIRS.id
                            || id == Block::DARK_OAK_SLAB.id
                            || id == Block::CAMPFIRE.id;
                        if !covered {
                            let below = volume
                                .get(x, top - 1, z)
                                .map(|s| Block::from_state_id(s.id).id);
                            if below != Some(Block::DARK_OAK_STAIRS.id)
                                && below != Some(Block::DARK_OAK_SLAB.id)
                            {
                                open += 1;
                            }
                        }
                    }
                }
            }
            if plaster > 0 {
                plaster_lots += 1;
            }
            if open == 0 {
                roofed_lots += 1;
            }
        }
        assert!(
            timber_lots > 0 && plaster_lots == timber_lots,
            "timber lots {timber_lots} plaster {plaster_lots}"
        );
        assert_eq!(
            roofed_lots, timber_lots,
            "timber lots {timber_lots} roofed {roofed_lots}"
        );
    }

    #[test]
    fn tower_mill_and_rose_window() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            8,
            6,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        if let Some(mill) = plan
            .lots
            .iter()
            .find(|lot| lot.kind == BuildingKind::Windmill)
        {
            let (cx, cz) = mill.rect.center();
            let floor = plan
                .heights
                .mean(
                    mill.rect.min_x,
                    mill.rect.min_z,
                    mill.rect.max_x,
                    mill.rect.max_z,
                )
                .max(plan.heights.sea_level + 2);
            let mut base_r = 0;
            let mut upper_r = 0;
            for z in mill.rect.min_z..=mill.rect.max_z {
                for x in mill.rect.min_x..=mill.rect.max_x {
                    let cheb = (x - cx).abs().max((z - cz).abs());
                    if volume.get(x, floor + 3, z).is_some_and(|s| !s.is_air()) {
                        base_r = base_r.max(cheb);
                    }
                    if volume.get(x, floor + 12, z).is_some_and(|s| !s.is_air()) {
                        upper_r = upper_r.max(cheb);
                    }
                }
            }
            assert!(
                base_r >= 3,
                "tower mill base should be 7 wide, radius={base_r}"
            );
            assert!(
                upper_r == 2,
                "tower mill should taper to 5 wide, radius={upper_r}"
            );
            let mut cloth = 0;
            let mut span = 0;
            let hub = floor + 18;
            for z in mill.rect.min_z - 12..=mill.rect.max_z + 12 {
                for x in mill.rect.min_x - 12..=mill.rect.max_x + 12 {
                    let dist = (x - cx).abs().max((z - cz).abs());
                    let diagonal = (x - cx).abs() == (z - cz).abs() && (x - cx) != 0;
                    for y in hub - 4..=hub + 5 {
                        let Some(state) = volume.get(x, y, z) else {
                            continue;
                        };
                        let id = Block::from_state_id(state.id).id;
                        if id == Block::WHITE_WOOL.id && diagonal {
                            cloth += 1;
                            span = span.max(dist);
                        }
                    }
                }
            }
            assert!(cloth >= 16, "lattice sails at hub {hub} count {cloth}");
            assert!(span >= 9, "blade length {span}");
            let mill_check = check_mill_connected(&plan, mill, &volume);
            assert!(
                mill_check.ok(),
                "mill sails must be 6-connected, leak={:?}",
                mill_check.leak
            );
            let mut stones = 0;
            for z in mill.rect.min_z..=mill.rect.max_z {
                for x in mill.rect.min_x..=mill.rect.max_x {
                    if volume
                        .get(x, floor + 1, z)
                        .is_some_and(|s| Block::from_state_id(s.id).id == Block::GRINDSTONE.id)
                    {
                        stones += 1;
                    }
                }
            }
            assert!(stones >= 1, "millstones");
        }
        if let Some(cat) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == crate::scenes::SceneKind::Cathedral)
        {
            let mut nave_west = i32::MAX;
            let mut nave_east = i32::MIN;
            let mut min_z = i32::MAX;
            let mut max_z = i32::MIN;
            let mut n = 0;
            for z in cat.rect.min_z..=cat.rect.max_z {
                for x in cat.rect.min_x..=cat.rect.max_x {
                    if plan.occ.kind(x, z) == CellKind::CathedralNave {
                        n += 1;
                        nave_west = nave_west.min(x);
                        nave_east = nave_east.max(x);
                        min_z = min_z.min(z);
                        max_z = max_z.max(z);
                    }
                }
            }
            assert!(n > 0, "cathedral nave");
            let cz = min_z.midpoint(max_z);
            let floor = crate::stamp::ridge_floor(
                &plan,
                crate::geom::Rect::new(nave_west, min_z, nave_east, max_z),
            );
            let mut glass = 0;
            let mut stone = 0;
            let mut rose_lo = i32::MAX;
            let mut rose_hi = i32::MIN;
            let cy = floor + 13;
            let mut purple = 0;
            let mut yellow = 0;
            let mut blue = 0;
            let mut red = 0;
            let mut petals = 0;
            let mut oculus = 0;
            let mut oculus_ring = 0;
            for z in min_z..=max_z {
                for y in cy - 7..=cy + 7 {
                    if let Some(state) = volume.get(nave_west, y, z) {
                        let id = Block::from_state_id(state.id).id;
                        let is_glass = id == Block::PURPLE_STAINED_GLASS.id
                            || id == Block::YELLOW_STAINED_GLASS.id
                            || id == Block::BLUE_STAINED_GLASS.id
                            || id == Block::RED_STAINED_GLASS.id
                            || id == Block::PURPLE_STAINED_GLASS_PANE.id
                            || id == Block::YELLOW_STAINED_GLASS_PANE.id;
                        let dy = y - cy;
                        let dz = z - cz;
                        let d2 = dy * dy + dz * dz;
                        if d2 > 49 {
                            continue;
                        }
                        if is_glass || id == Block::CALCITE.id || id == Block::STONE_BRICKS.id {
                            rose_lo = rose_lo.min(z);
                            rose_hi = rose_hi.max(z);
                        }
                        if is_glass {
                            glass += 1;
                            if d2 <= 3 {
                                oculus += 1;
                            }
                        }
                        if id == Block::PURPLE_STAINED_GLASS.id
                            || id == Block::PURPLE_STAINED_GLASS_PANE.id
                        {
                            purple += 1;
                        }
                        if id == Block::YELLOW_STAINED_GLASS.id
                            || id == Block::YELLOW_STAINED_GLASS_PANE.id
                        {
                            yellow += 1;
                        }
                        if id == Block::BLUE_STAINED_GLASS.id {
                            blue += 1;
                        }
                        if id == Block::RED_STAINED_GLASS.id {
                            red += 1;
                        }
                        if id == Block::CALCITE.id {
                            stone += 1;
                            if (4..=8).contains(&d2) {
                                oculus_ring += 1;
                            }
                            let ady = dy.abs();
                            let adz = dz.abs();
                            if ady == adz
                                || ady * 2 == adz
                                || adz * 2 == ady
                                || ady * 3 == adz * 2
                                || adz * 3 == ady * 2
                            {
                                petals += 1;
                            }
                        }
                    }
                }
            }
            assert!(glass >= 12, "rose glass {glass}");
            assert!(stone >= 16, "rose tracery {stone}");
            assert!(purple >= 4, "rose purple glass {purple}");
            assert!(yellow >= 4, "rose yellow glass {yellow}");
            assert!(blue >= 2, "rose blue glass {blue}");
            assert!(red >= 2, "rose red glass {red}");
            assert!(petals >= 8, "rose petals {petals}");
            assert!(oculus >= 1, "rose central oculus {oculus}");
            assert!(oculus_ring >= 4, "rose oculus ring {oculus_ring}");
            assert!(
                rose_hi - rose_lo >= 12,
                "rose should be at least 13 wide, span={}",
                rose_hi - rose_lo + 1
            );
        }
    }

    #[test]
    fn desert_houses_meet_the_roof() {
        let origin = town_in_cell(314_159, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            8,
            6,
        );
        let plan = build_plan(314_159, origin, grid, PaletteKind::Sandstone);
        let volume = fill_volume(&plan);
        let check = check_town(&plan, &volume);
        let desert_fails: Vec<_> = check
            .failures()
            .into_iter()
            .filter(|b| {
                b.name.contains("Cottage")
                    || b.name.contains("Townhouse")
                    || b.name.contains("Tavern")
                    || b.name.contains("Church")
            })
            .map(|b| format!("{} leak={:?} sky={}", b.name, b.leak, b.sky_open))
            .collect();
        assert!(
            desert_fails.is_empty(),
            "desert envelopes failed: {}",
            desert_fails.join("; ")
        );
        let mut sealed = 0;
        let mut dome_span = 0;
        for lot in &plan.lots {
            if !matches!(
                lot.kind,
                BuildingKind::Cottage
                    | BuildingKind::Townhouse
                    | BuildingKind::Tavern
                    | BuildingKind::Church
            ) {
                continue;
            }
            let floor = plan
                .heights
                .mean(
                    lot.rect.min_x,
                    lot.rect.min_z,
                    lot.rect.max_x,
                    lot.rect.max_z,
                )
                .max(plan.heights.sea_level + 2);
            let eaves = floor + lot.kind.wall_h();
            for z in lot.rect.min_z..=lot.rect.max_z {
                for x in lot.rect.min_x..=lot.rect.max_x {
                    if !lot.rect.on_edge(x, z) {
                        continue;
                    }
                    let roof = volume.get(x, eaves, z);
                    let under = volume.get(x, eaves - 1, z);
                    let roof_id = roof.map(|s| Block::from_state_id(s.id).id);
                    assert!(
                        roof.is_some_and(|s| !s.is_air()),
                        "desert wall/roof gap at {x},{eaves},{z}"
                    );
                    assert!(
                        under.is_some_and(|s| !s.is_air()),
                        "desert wall does not meet roof at {x},{},{z}",
                        eaves - 1
                    );
                    assert_ne!(
                        roof_id,
                        Some(Block::SPRUCE_LOG.id),
                        "desert eave should not be a dark log seam"
                    );
                    sealed += 1;
                }
            }
            if matches!(lot.kind, BuildingKind::Church | BuildingKind::Townhouse) {
                let (cx, cz) = lot.rect.center();
                let mut xs = Vec::new();
                for z in lot.rect.min_z..=lot.rect.max_z {
                    for x in lot.rect.min_x..=lot.rect.max_x {
                        if volume.get(x, eaves + 2, z).is_some_and(|s| !s.is_air()) {
                            xs.push(x);
                        }
                    }
                }
                if !xs.is_empty() {
                    dome_span = dome_span.max(
                        xs.iter().max().copied().unwrap_or(cx)
                            - xs.iter().min().copied().unwrap_or(cx),
                    );
                }
                let _ = cz;
            }
        }
        assert!(sealed >= 20, "desert eave columns {sealed}");
        assert!(
            dome_span >= 4,
            "desert dome should be at least 5 wide, span={}",
            dome_span + 1
        );
    }

    #[test]
    fn scene_yards_have_no_floating_plants_or_feature_trees() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            8,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let volume = fill_volume(&plan);
        let fails = check_scene_vegetation(&plan, &volume);
        assert!(fails.is_empty(), "scene vegetation:\n{}", fails.join("\n"));
    }

    #[test]
    fn west_front_rose_has_no_roof_tiles_in_the_wheel() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            8,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let Some(scene) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == crate::scenes::SceneKind::Cathedral)
        else {
            return;
        };
        let mut nave: Option<Rect> = None;
        for z in scene.rect.min_z..=scene.rect.max_z {
            for x in scene.rect.min_x..=scene.rect.max_x {
                if plan.occ.kind(x, z) != CellKind::CathedralNave {
                    continue;
                }
                nave = Some(nave.map_or_else(
                    || Rect::new(x, z, x, z),
                    |rect| {
                        Rect::new(
                            rect.min_x.min(x),
                            rect.min_z.min(z),
                            rect.max_x.max(x),
                            rect.max_z.max(z),
                        )
                    },
                ));
            }
        }
        let Some(nave) = nave else {
            return;
        };
        let volume = fill_volume(&plan);
        let floor = crate::stamp::ridge_floor(&plan, nave);
        let cz = nave.center().1;
        let wall_x = nave.min_x;
        let mut dark = 0;
        for z in cz - 6..=cz + 6 {
            for y in floor + 7..=floor + 19 {
                if let Some(state) = volume.get(wall_x, y, z) {
                    let id = Block::from_state_id(state.id).id;
                    if id == Block::DEEPSLATE_TILES.id
                        || id == Block::DEEPSLATE_TILE_STAIRS.id
                        || id == Block::DEEPSLATE_TILE_SLAB.id
                    {
                        dark += 1;
                    }
                }
            }
        }
        assert_eq!(dark, 0, "deepslate roof tiles inside the west-front rose");
    }

    #[test]
    fn feature_trees_are_stripped_from_world_scene_yards() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

        let seed = 20_261_025u64;
        let boxed = get_world_gen(
            Seed(seed),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let world_gen = Arc::from(boxed);
        crate::height::register_world_gen(Arc::clone(&world_gen));
        crate::marker::register_hooks(world_gen.extensions());
        let plan = crate::plan::plan_for(seed, 0, 0, &world_gen);
        // Whole town plan: walled town plus the market crop. Hinterland
        // scenes are stamped in their own chunks; unioning them here would
        // generate the forest between town and harbour.
        let market = plan.market.inflate(16);
        let region = Rect::new(
            plan.town.min_x.min(market.min_x) - 4,
            plan.town.min_z.min(market.min_z) - 4,
            plan.town.max_x.max(market.max_x) + 4,
            plan.town.max_z.max(market.max_z) + 4,
        );
        let volume = crate::chunk_fill::fill_volume_from_chunks_in(&plan, &world_gen, region);
        let mut fails = check_scene_vegetation(&plan, &volume);
        for scene in plan
            .scenes
            .iter()
            .filter(|scene| scene.kind == crate::scenes::SceneKind::Frontier)
        {
            let pad = scene
                .rect
                .inflate(crate::scenes::SCENE_YARD_MARGIN + crate::scenes::FRONTIER_TREE_MARGIN);
            let front = crate::chunk_fill::fill_volume_from_chunks_in(&plan, &world_gen, pad);
            fails.extend(check_scene_vegetation(&plan, &front));
        }
        assert!(
            fails.is_empty(),
            "feature trees / floating plants in the town plan:\n{}",
            fails.join("\n")
        );
    }

    #[test]
    fn planned_block_returns_damaged_variant_until_repaired() {
        let origin = town_in_cell(99, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            8,
        );
        let plan = build_plan(99, origin, grid, PaletteKind::Oak);
        let id = crate::info::town_id(plan.seed, plan.origin.cell_x, plan.origin.cell_z);
        let _guard = DamageGuard(id.clone());
        crate::damage::set_damage_state(&id, crate::damage::DamageState::Scarred);
        assert_eq!(
            crate::damage::damage_state(&id),
            crate::damage::DamageState::Scarred
        );
        let volume = fill_volume(&plan);
        let mut scars = 0;
        for z in plan.town.min_z..=plan.town.max_z {
            for x in plan.town.min_x..=plan.town.max_x {
                if !matches!(
                    plan.occ.kind(x, z),
                    CellKind::TownWall | CellKind::TownTower
                ) {
                    continue;
                }
                let floor = crate::stamp::ridge_floor(&plan, Rect::new(x, z, x, z));
                for y in floor + 2..=floor + 4 {
                    if volume
                        .get(x, y, z)
                        .is_some_and(pumpkin_data::BlockState::is_air)
                    {
                        scars += 1;
                    }
                }
                if volume
                    .get(x, plan.heights.height(x, z), z)
                    .is_some_and(|s| Block::from_state_id(s.id).id == Block::COAL_BLOCK.id)
                {
                    scars += 1;
                }
            }
        }
        assert!(
            scars >= 4,
            "scarred curtain should punch holes/scorch {scars}"
        );
    }

    #[test]
    fn watermills_are_watertight() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_river(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            8,
            origin.z + 140,
        );
        let plan = build_plan(11, origin, grid, PaletteKind::Oak);
        assert!(
            plan.lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::Watermill),
            "riverside watermills"
        );
        for mill in plan
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Watermill)
        {
            for z in mill.rect.min_z..=mill.rect.max_z {
                for x in mill.rect.min_x..=mill.rect.max_x {
                    assert_eq!(
                        plan.occ.kind(x, z),
                        CellKind::Building,
                        "watermill {} envelope at {x},{z} is {:?}",
                        mill.id,
                        plan.occ.kind(x, z)
                    );
                }
            }
        }
        let volume = fill_volume(&plan);
        let check = check_town(&plan, &volume);
        let mill_fails: Vec<_> = check
            .failures()
            .into_iter()
            .filter(|b| b.name.contains("Watermill"))
            .map(|b| {
                format!(
                    "{} leak={:?} sky={} float={}",
                    b.name, b.leak, b.sky_open, b.floating_roofs
                )
            })
            .collect();
        assert!(
            mill_fails.is_empty(),
            "watermill envelope should close except the barred wheel opening: {mill_fails:?}"
        );
        let mut rim = 0;
        let mut below = 0;
        let mut spokes = 0;
        let mut axle = 0;
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        let sea = plan.heights.sea_level;
        let mill = plan
            .lots
            .iter()
            .find(|lot| lot.kind == BuildingKind::Watermill)
            .expect("watermill lot");
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                if plan.occ.kind(x, z) != CellKind::WaterWheel {
                    continue;
                }
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_z = min_z.min(z);
                max_z = max_z.max(z);
                for y in sea - 7..=sea + 7 {
                    let Some(state) = volume.get(x, y, z) else {
                        continue;
                    };
                    let id = Block::from_state_id(state.id).id;
                    if id == Block::STRIPPED_DARK_OAK_LOG.id
                        || id == Block::DARK_OAK_LOG.id
                        || id == Block::DARK_OAK_PLANKS.id
                    {
                        rim += 1;
                        if y < sea {
                            below += 1;
                        }
                    }
                    if id == Block::BIRCH_PLANKS.id {
                        spokes += 1;
                    }
                    if id == Block::STRIPPED_OAK_LOG.id {
                        axle += 1;
                    }
                }
            }
        }
        let across = if matches!(mill.facing, crate::geom::Dir::East | crate::geom::Dir::West) {
            max_z - min_z + 1
        } else {
            max_x - min_x + 1
        };
        let stand_out = if matches!(mill.facing, crate::geom::Dir::East | crate::geom::Dir::West) {
            max_x - min_x + 1
        } else {
            max_z - min_z + 1
        };
        assert!(
            across >= 9,
            "wheel should be at least 9 across, got {across}"
        );
        assert!(
            stand_out >= 3,
            "wheel should stand out from the mill wall, thickness {stand_out}"
        );
        assert!(rim > 30, "wheel rim/paddles {rim}");
        assert!(
            below >= rim / 3 && below * 2 <= rim + rim / 4,
            "about half the wheel should sit below the waterline, rim={rim} below={below}"
        );
        assert!(spokes >= 4, "wheel spokes {spokes}");
        assert!(axle >= 1, "visible axle {axle}");
    }

    struct DamageGuard(String);
    impl Drop for DamageGuard {
        fn drop(&mut self) {
            crate::damage::set_damage_state(&self.0, crate::damage::DamageState::Intact);
        }
    }
}
