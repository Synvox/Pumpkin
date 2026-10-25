//! Automated envelope check: interiors must not flood to the outside.

use std::collections::VecDeque;

use pumpkin_data::{Block, BlockState};

use crate::buildings::BuildingKind;
use crate::cells::CellKind;
use crate::geom::Rect;
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
}

impl BuildingCheck {
    /// Whether the envelope held and roofs had support.
    ///
    /// A barred gatehouse may be solid above the passage (no 2-high room)
    /// when the tunnel is not boxed in by masonry on all four sides.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.floating_roofs == 0 && self.leak.is_none() && self.sky_open == 0
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
                        "{} leak={:?} float={} sky={} seed={:?}",
                        b.name, b.leak, b.floating_roofs, b.sky_open, b.seed
                    )
                })
                .collect();
            format!("seed {} FAIL {pass}/{n}: {}", self.seed, fails.join("; "))
        }
    }
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
    for lot in &plan.lots {
        if matches!(lot.kind, BuildingKind::Stall) {
            continue;
        }
        let mut cols = collect_lot(plan, lot.id, lot.rect);
        if matches!(lot.kind, BuildingKind::Windmill) {
            let (cx, cz) = lot.rect.center();
            cols.retain(|&(x, z)| {
                let dx = x - cx;
                let dz = z - cz;
                dx * dx + dz * dz <= 4
            });
        }
        if cols.is_empty() {
            continue;
        }
        buildings.push(check_columns(
            &format!("lot{}-{:?}", lot.id, lot.kind),
            cols,
            volume,
            plan,
            true,
        ));
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

fn collect_lot(plan: &TownPlan, lot_id: u16, rect: Rect) -> Vec<(i32, i32)> {
    let mut cols = Vec::new();
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if plan.occ.lot(x, z) == lot_id {
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
        };
    }
    let set: std::collections::HashSet<(i32, i32)> = columns.iter().copied().collect();
    let seed = find_seed(&columns, volume);
    let leak = seed.and_then(|s| flood_leak(s, &set, volume, plan));
    let floating_roofs = count_floating_roofs(&columns, volume);
    let sky_open = if check_sky {
        count_sky_open(&set, volume)
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
    solids >= 4
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
