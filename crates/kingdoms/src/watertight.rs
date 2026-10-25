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
}

impl BuildingCheck {
    /// Whether the envelope held and roofs had support.
    ///
    /// A barred gatehouse may be solid above the passage (no 2-high room)
    /// when the tunnel is not boxed in by masonry on all four sides.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.floating_roofs == 0 && self.leak.is_none()
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
                        "{} leak={:?} float={} seed={:?}",
                        b.name, b.leak, b.floating_roofs, b.seed
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
    push_if_any(
        &mut buildings,
        "gatehouse",
        collect_kind(plan, CellKind::CastleGate, plan.bounds),
        volume,
        plan,
    );
    for (i, spec) in plan.castle.tower_specs.iter().enumerate() {
        let cols = collect_spec(plan, CellKind::CastleTower, |x, z| spec.contains(x, z));
        if cols.len() < 8 {
            continue;
        }
        buildings.push(check_columns(&format!("tower{i}"), cols, volume, plan));
    }
    for (i, mass) in plan.castle.keep_masses.iter().enumerate() {
        let cols = collect_spec(plan, CellKind::CastleKeep, |x, z| mass.spec.contains(x, z));
        if cols.len() < 8 {
            continue;
        }
        buildings.push(check_columns(&format!("keep-mass{i}"), cols, volume, plan));
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
    if !columns.is_empty() {
        out.push(check_columns(name, columns, volume, plan));
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
) -> BuildingCheck {
    if columns.is_empty() {
        return BuildingCheck {
            name: name.to_string(),
            columns: 0,
            seed: None,
            leak: None,
            floating_roofs: 0,
        };
    }
    let set: std::collections::HashSet<(i32, i32)> = columns.iter().copied().collect();
    let seed = find_seed(&columns, volume);
    let leak = seed.and_then(|s| flood_leak(s, &set, volume, plan));
    let floating_roofs = count_floating_roofs(&columns, volume);
    BuildingCheck {
        name: name.to_string(),
        columns: columns.len(),
        seed,
        leak,
        floating_roofs,
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
    use crate::stamp::fill_volume;
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
}
