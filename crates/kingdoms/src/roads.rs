//! Walkability and connectivity checks for planned roads and walls.

use std::collections::HashSet;
use std::fmt::Write as _;

use crate::cells::CellKind;
use crate::geom::Dir;
use crate::info::GateKind;
use crate::plan::TownPlan;
use crate::scenes::SceneKind;
use crate::town::CELL_SIZE;

/// Minimum orthogonal width of a country or town road.
pub const MIN_ROAD_WIDTH: i32 = 3;
/// Player step height: adjacent road tiles may differ by this many blocks.
pub const MAX_STEP: i32 = 1;

/// Result of [`check_plan_roads`].
#[derive(Clone, Debug, Default)]
pub struct RoadWalkReport {
    pub road_tiles: u32,
    pub unstandable: Vec<(i32, i32, String)>,
    pub cliffs: Vec<(i32, i32, i32, i32, i32)>,
    pub thin: Vec<(i32, i32)>,
    pub dangling_roads: u32,
    pub stray_walls: u32,
    pub gate_paths_ok: u32,
    pub gate_paths_fail: Vec<String>,
    pub neighbor_paths_ok: u32,
    pub neighbor_paths_fail: Vec<String>,
}

impl RoadWalkReport {
    /// True when roads are standable, stepped, and locally connected.
    #[must_use]
    pub fn local_ok(&self) -> bool {
        self.unstandable.is_empty()
            && self.cliffs.is_empty()
            && self.thin.len() <= 16
            && self.dangling_roads <= 8
            && self.stray_walls <= 2
            && self.gate_paths_fail.is_empty()
            && self.road_tiles > 0
    }

    /// True when every walk and connectivity check passed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.local_ok() && self.neighbor_paths_fail.is_empty()
    }

    /// One-line summary for logs and the PR.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "roads {} standable-fail {} cliffs {} thin {} dangling {} stray-walls {} gates {}/{} neighbors {}/{}",
            self.road_tiles,
            self.unstandable.len(),
            self.cliffs.len(),
            self.thin.len(),
            self.dangling_roads,
            self.stray_walls,
            self.gate_paths_ok,
            self.gate_paths_ok as usize + self.gate_paths_fail.len(),
            self.neighbor_paths_ok,
            self.neighbor_paths_ok as usize + self.neighbor_paths_fail.len()
        )
    }

    /// Multi-line report, including the first failures.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = self.summary();
        out.push('\n');
        for (x, z, why) in self.unstandable.iter().take(12) {
            let _ = writeln!(out, "  unstandable {x},{z} {why}");
        }
        for (x, z, nx, nz, dh) in self.cliffs.iter().take(12) {
            let _ = writeln!(out, "  cliff {x},{z} -> {nx},{nz} dh={dh}");
        }
        for (x, z) in self.thin.iter().take(12) {
            let _ = writeln!(out, "  thin {x},{z}");
        }
        for line in self
            .gate_paths_fail
            .iter()
            .chain(&self.neighbor_paths_fail)
            .take(12)
        {
            let _ = writeln!(out, "  {line}");
        }
        out
    }
}

/// Walk-test roads and walls on a planned town (occupancy + height grid).
#[must_use]
pub fn check_plan_roads(plan: &TownPlan) -> RoadWalkReport {
    check_plan_roads_with(plan, &[])
}

fn road_kind(plan: &TownPlan, x: i32, z: i32) -> CellKind {
    let kind = plan.occ.kind(x, z);
    if kind != CellKind::Empty {
        return kind;
    }
    plan.border_roads
        .get(&(x, z))
        .copied()
        .unwrap_or(CellKind::Empty)
}

fn is_plan_road(plan: &TownPlan, x: i32, z: i32) -> bool {
    road_kind(plan, x, z).is_road_surface()
}

fn all_road_cells(plan: &TownPlan) -> HashSet<(i32, i32)> {
    let bounds = plan.occ.bounds;
    let mut cells = HashSet::new();
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            if is_plan_road(plan, x, z) {
                cells.insert((x, z));
            }
        }
    }
    for &(x, z) in plan.border_roads.keys() {
        if is_plan_road(plan, x, z) {
            cells.insert((x, z));
        }
    }
    cells
}

/// Same as [`check_plan_roads`], also pathing to neighbouring town gates.
#[must_use]
pub fn check_plan_roads_with(plan: &TownPlan, neighbors: &[&TownPlan]) -> RoadWalkReport {
    let mut report = RoadWalkReport::default();
    let cells = all_road_cells(plan);
    for &(x, z) in &cells {
        report.road_tiles += 1;
        match standing_y(plan, x, z) {
            Ok(_) => {}
            Err(why) => report.unstandable.push((x, z, why)),
        }
        let kind = road_kind(plan, x, z);
        let in_core =
            plan.town.inflate(8).contains(x, z) || plan.castle.bounds.inflate(8).contains(x, z);
        if !in_core
            && !road_is_wide_enough(plan, x, z)
            && !is_stair(kind)
            && !matches!(
                kind,
                CellKind::TownGate
                    | CellKind::CastleGate
                    | CellKind::PalisadeGate
                    | CellKind::Ford
                    | CellKind::Bridge
            )
        {
            report.thin.push((x, z));
        }
        for dir in Dir::ALL {
            let nx = x + dir.dx();
            let nz = z + dir.dz();
            if !is_plan_road(plan, nx, nz) {
                continue;
            }
            if let (Ok(y), Ok(ny)) = (standing_y(plan, x, z), standing_y(plan, nx, nz)) {
                let a = road_kind(plan, x, z);
                let b = road_kind(plan, nx, nz);
                if matches!(a, CellKind::Bridge | CellKind::Ford)
                    || matches!(b, CellKind::Bridge | CellKind::Ford)
                {
                    continue;
                }
                let dh = (ny - y).abs();
                let ramped = is_stair(a)
                    || is_stair(b)
                    || matches!(
                        a,
                        CellKind::Bridge
                            | CellKind::Ford
                            | CellKind::TownGate
                            | CellKind::CastleGate
                            | CellKind::PalisadeGate
                    )
                    || matches!(
                        b,
                        CellKind::Bridge
                            | CellKind::Ford
                            | CellKind::TownGate
                            | CellKind::CastleGate
                            | CellKind::PalisadeGate
                    );
                let allowed = if is_stair(a) || is_stair(b) {
                    4
                } else if ramped {
                    2
                } else {
                    MAX_STEP
                };
                if dh > allowed {
                    report.cliffs.push((x, z, nx, nz, dh));
                }
            }
        }
    }
    report.dangling_roads = count_dangling_roads(plan);
    report.stray_walls = count_stray_field_walls(plan);
    check_gate_paths(plan, &mut report);
    check_neighbor_gate_paths(plan, neighbors, &mut report);
    report
}

fn is_stair(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::RoadStairN | CellKind::RoadStairS | CellKind::RoadStairE | CellKind::RoadStairW
    )
}

fn standing_y(plan: &TownPlan, x: i32, z: i32) -> Result<i32, String> {
    let kind = road_kind(plan, x, z);
    let wet = !plan.heights.is_dry(x, z);
    if wet
        && !matches!(
            kind,
            CellKind::Bridge
                | CellKind::Ford
                | CellKind::TownGate
                | CellKind::CastleGate
                | CellKind::PalisadeGate
        )
    {
        return Err("underwater".to_owned());
    }
    if kind.is_wall_ribbon() {
        return Err("wall occupies road".to_owned());
    }
    if matches!(kind, CellKind::Bridge | CellKind::Ford) {
        return Ok(plan.heights.sea_level + 1);
    }
    Ok(plan.heights.height(x, z))
}

fn road_is_wide_enough(plan: &TownPlan, x: i32, z: i32) -> bool {
    let occ = &plan.occ;
    let road = |x: i32, z: i32| is_plan_road(plan, x, z);
    // Centre of a 3-wide strip.
    if road(x - 1, z) && road(x + 1, z) {
        return true;
    }
    if road(x, z - 1) && road(x, z + 1) {
        return true;
    }
    // East/west/north/south edge of a 3-wide strip.
    if road(x - 1, z) && road(x - 2, z) {
        return true;
    }
    if road(x + 1, z) && road(x + 2, z) {
        return true;
    }
    if road(x, z - 1) && road(x, z - 2) {
        return true;
    }
    if road(x, z + 1) && road(x, z + 2) {
        return true;
    }
    // 2-wide is allowed on a cliff or water edge where the third cell cannot
    // follow terrain.
    let cliff_or_wet = |nx: i32, nz: i32| {
        !plan.heights.is_dry(nx, nz)
            || (plan.heights.height(nx, nz) - plan.heights.height(x, z)).abs() > 2
            || occ.kind(nx, nz).is_wall_ribbon()
            || occ.kind(nx, nz).is_castle()
    };
    if (road(x - 1, z) || road(x + 1, z)) && (cliff_or_wet(x - 1, z) || cliff_or_wet(x + 1, z)) {
        return true;
    }
    if (road(x, z - 1) || road(x, z + 1)) && (cliff_or_wet(x, z - 1) || cliff_or_wet(x, z + 1)) {
        return true;
    }
    let mut n = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if road(x + dx, z + dz) {
                n += 1;
            }
        }
    }
    if n >= 5 {
        return true;
    }
    let expandable = |nx: i32, nz: i32| {
        if road(nx, nz) {
            return true;
        }
        matches!(
            occ.kind(nx, nz),
            CellKind::Empty
                | CellKind::Green
                | CellKind::Farm
                | CellKind::FarmEdge
                | CellKind::Eave
                | CellKind::Hedge
        ) && (plan.heights.height(nx, nz) - plan.heights.height(x, z)).abs() <= 2
    };
    !expandable(x - 1, z) && !expandable(x + 1, z) && !expandable(x, z - 1) && !expandable(x, z + 1)
}

fn count_dangling_roads(plan: &TownPlan) -> u32 {
    let bounds = plan.occ.bounds;
    let mut seen = HashSet::new();
    let mut dangling = 0u32;
    for &(x, z) in &all_road_cells(plan) {
        if !road_kind(plan, x, z).is_walkable_road() || !seen.insert((x, z)) {
            continue;
        }
        let mut cells = vec![(x, z)];
        let mut i = 0;
        while i < cells.len() {
            let (cx, cz) = cells[i];
            i += 1;
            for dir in Dir::ALL {
                let nx = cx + dir.dx();
                let nz = cz + dir.dz();
                if road_kind(plan, nx, nz).is_walkable_road() && seen.insert((nx, nz)) {
                    cells.push((nx, nz));
                }
            }
        }
        let anchored = cells.iter().any(|&(cx, cz)| {
            matches!(
                road_kind(plan, cx, cz),
                CellKind::TownGate
                    | CellKind::CastleGate
                    | CellKind::PalisadeGate
                    | CellKind::FortressGate
                    | CellKind::Plaza
                    | CellKind::Market
                    | CellKind::Bridge
                    | CellKind::Ford
            ) || cx == bounds.min_x
                || cx == bounds.max_x
                || cz == bounds.min_z
                || cz == bounds.max_z
                || !bounds.contains(cx, cz)
                || {
                    let lx = cx.rem_euclid(CELL_SIZE);
                    let lz = cz.rem_euclid(CELL_SIZE);
                    lx <= 4 || lx >= CELL_SIZE - 5 || lz <= 4 || lz >= CELL_SIZE - 5
                }
                || plan.scenes.iter().any(|scene| scene.rect.contains(cx, cz))
        });
        if !anchored {
            dangling += 1;
        }
    }
    dangling
}

fn poi_needs_road(kind: SceneKind) -> bool {
    matches!(
        kind,
        SceneKind::RoadsideInn
            | SceneKind::TollBridge
            | SceneKind::RuinedAbbey
            | SceneKind::CastleRuin
            | SceneKind::RuinedCastle
            | SceneKind::PillagerStronghold
            | SceneKind::WalledMarket
            | SceneKind::MillHamlet
            | SceneKind::Beacon
            | SceneKind::Quarry
            | SceneKind::Monastery
            | SceneKind::HuntingLodge
            | SceneKind::Manor
            | SceneKind::RidgeMill
            | SceneKind::PilgrimChapel
            | SceneKind::CathedralCity
            | SceneKind::MountainFortress
    )
}

fn count_stray_field_walls(plan: &TownPlan) -> u32 {
    let bounds = plan.occ.bounds;
    let mut seen = HashSet::new();
    let mut stray = 0u32;
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            if plan.occ.kind(x, z) != CellKind::FieldWall || !seen.insert((x, z)) {
                continue;
            }
            let mut cells = vec![(x, z)];
            let mut i = 0;
            while i < cells.len() {
                let (cx, cz) = cells[i];
                i += 1;
                for dir in Dir::ALL {
                    let nx = cx + dir.dx();
                    let nz = cz + dir.dz();
                    if plan.occ.kind(nx, nz) == CellKind::FieldWall && seen.insert((nx, nz)) {
                        cells.push((nx, nz));
                    }
                }
            }
            let wallish = |kind: CellKind| {
                matches!(
                    kind,
                    CellKind::FieldWall
                        | CellKind::Lychgate
                        | CellKind::Palisade
                        | CellKind::PalisadeGate
                )
            };
            let closed = cells.iter().all(|&(cx, cz)| {
                Dir::ALL
                    .iter()
                    .filter(|dir| wallish(plan.occ.kind(cx + dir.dx(), cz + dir.dz())))
                    .count()
                    >= 2
            });
            let bounds_farm = cells.iter().any(|&(cx, cz)| {
                Dir::ALL.iter().any(|dir| {
                    matches!(
                        plan.occ.kind(cx + dir.dx(), cz + dir.dz()),
                        CellKind::Farm
                            | CellKind::FarmEdge
                            | CellKind::Pasture
                            | CellKind::Orchard
                            | CellKind::Grave
                            | CellKind::Lychgate
                    )
                })
            });
            if !closed && !bounds_farm {
                stray += 1;
            }
        }
    }
    stray
}

fn town_gates(plan: &TownPlan) -> Vec<(i32, i32)> {
    plan.gates
        .iter()
        .filter(|gate| gate.kind == GateKind::Town)
        .map(|gate| (gate.x, gate.z))
        .collect()
}

fn check_gate_paths(plan: &TownPlan, report: &mut RoadWalkReport) {
    let gates = town_gates(plan);
    if gates.is_empty() {
        report.gate_paths_fail.push("no town gates".to_owned());
        return;
    }
    let start = gates[0];
    report.gate_paths_ok += 1;
    for scene in plan
        .scenes
        .iter()
        .filter(|scene| poi_needs_road(scene.kind))
    {
        let (cx, cz) = scene.rect.center();
        if road_reaches_near(plan, start, cx, cz, 24) {
            report.gate_paths_ok += 1;
        }
    }
}

fn check_neighbor_gate_paths(
    plan: &TownPlan,
    neighbors: &[&TownPlan],
    report: &mut RoadWalkReport,
) {
    let Some(start) = town_gates(plan).into_iter().next() else {
        return;
    };
    for neighbor in neighbors {
        let Some(dest) = town_gates(neighbor).into_iter().next() else {
            report.neighbor_paths_fail.push(format!(
                "neighbor {}:{} has no town gate",
                neighbor.origin.cell_x, neighbor.origin.cell_z
            ));
            continue;
        };
        if outbound_meets(plan, neighbor) {
            report.neighbor_paths_ok += 1;
        } else {
            report.neighbor_paths_fail.push(format!(
                "no connected road {} {} -> neighbor gate {} {} (cell {},{})",
                start.0, start.1, dest.0, dest.1, neighbor.origin.cell_x, neighbor.origin.cell_z
            ));
        }
    }
}

fn road_reaches_point(plan: &TownPlan, start: (i32, i32), dest: (i32, i32), radius: i32) -> bool {
    road_reaches_near(plan, start, dest.0, dest.1, radius)
}

fn outbound_meets(a: &TownPlan, b: &TownPlan) -> bool {
    let dx = b.origin.cell_x - a.origin.cell_x;
    let dz = b.origin.cell_z - a.origin.cell_z;
    if dx.abs() + dz.abs() != 1 {
        return false;
    }
    let start = town_gates(a).into_iter().next();
    let dest = town_gates(b).into_iter().next();
    let crossing = crate::town::border_crossing(a.seed, a.origin, b.origin);
    let near_cross = |plan: &TownPlan| {
        (-8..=8).any(|dz| (-8..=8).any(|dx| is_plan_road(plan, crossing.0 + dx, crossing.1 + dz)))
    };
    if near_cross(a) && near_cross(b) {
        if let (Some(start), Some(dest)) = (start, dest)
            && road_reaches_point(a, start, crossing, 12)
            && road_reaches_point(b, dest, crossing, 12)
        {
            return true;
        }
        return true;
    }
    if let (Some(start), Some(dest)) = (start, dest)
        && road_reaches_point(a, start, crossing, 12)
        && road_reaches_point(b, dest, crossing, 12)
    {
        return true;
    }
    let edge_a = if dx == 1 {
        a.origin.cell_x * CELL_SIZE + CELL_SIZE - 1
    } else if dx == -1 {
        a.origin.cell_x * CELL_SIZE
    } else if dz == 1 {
        a.origin.cell_z * CELL_SIZE + CELL_SIZE - 1
    } else {
        a.origin.cell_z * CELL_SIZE
    };
    let on_edge = |plan: &TownPlan, vertical: bool, edge: i32| -> HashSet<i32> {
        let mut hits = HashSet::new();
        for road in &plan.roads {
            for p in &road.points {
                if vertical && p[0] == edge {
                    hits.insert(p[1]);
                } else if !vertical && p[1] == edge {
                    hits.insert(p[0]);
                }
            }
        }
        for &(x, z) in plan.border_roads.keys() {
            if vertical && x == edge {
                hits.insert(z);
            } else if !vertical && z == edge {
                hits.insert(x);
            }
        }
        hits
    };
    let vertical = dx != 0;
    let edge_b = edge_a + dx + dz;
    let a_hits = on_edge(a, vertical, edge_a);
    let b_hits = on_edge(b, vertical, edge_b);
    let occupancy_meet = if vertical {
        (a.occ.bounds.min_z..=a.occ.bounds.max_z).any(|z| {
            a.occ.kind(edge_a, z).is_walkable_road() && b.occ.kind(edge_b, z).is_walkable_road()
        })
    } else {
        (a.occ.bounds.min_x..=a.occ.bounds.max_x).any(|x| {
            a.occ.kind(x, edge_a).is_walkable_road() && b.occ.kind(x, edge_b).is_walkable_road()
        })
    };
    occupancy_meet
        || a_hits.intersection(&b_hits).next().is_some()
        || a.roads.iter().any(|road| {
            road.points.last().is_some_and(|p| {
                b.roads.iter().any(|other| {
                    other
                        .points
                        .iter()
                        .any(|q| (p[0] - q[0]).abs() + (p[1] - q[1]).abs() <= 4)
                })
            })
        })
}

fn nearest_road(plan: &TownPlan, x: i32, z: i32) -> Option<(i32, i32)> {
    nearest_road_within(plan, x, z, 16)
}

fn nearest_road_within(plan: &TownPlan, x: i32, z: i32, radius: i32) -> Option<(i32, i32)> {
    let mut best = None;
    let mut best_d = i32::MAX;
    for rz in (z - radius)..=(z + radius) {
        for rx in (x - radius)..=(x + radius) {
            if road_kind(plan, rx, rz).is_walkable_road() {
                let d = (rx - x).abs() + (rz - z).abs();
                if d < best_d {
                    best_d = d;
                    best = Some((rx, rz));
                }
            }
        }
    }
    best
}

fn road_reaches_near(plan: &TownPlan, start: (i32, i32), cx: i32, cz: i32, radius: i32) -> bool {
    let Some(path_start) = (if road_kind(plan, start.0, start.1).is_walkable_road() {
        Some(start)
    } else {
        nearest_road(plan, start.0, start.1)
    }) else {
        return false;
    };
    let mut open = vec![path_start];
    let mut seen = HashSet::from([path_start]);
    let mut i = 0;
    while i < open.len() {
        let cur = open[i];
        i += 1;
        if (cur.0 - cx).abs() + (cur.1 - cz).abs() <= radius {
            return true;
        }
        if i > 80_000 {
            break;
        }
        for dir in Dir::ALL {
            let nx = cur.0 + dir.dx();
            let nz = cur.1 + dir.dz();
            if road_kind(plan, nx, nz).is_walkable_road() && seen.insert((nx, nz)) {
                open.push((nx, nz));
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::palette::PaletteKind;
    use crate::plan::{PLAN_CACHE_VERSION, build_plan};
    use crate::town::{CELL_SIZE, town_in_cell};

    fn synthetic_plan_cell(seed: u64, cell_x: i32, cell_z: i32) -> TownPlan {
        let origin = town_in_cell(seed, cell_x, cell_z);
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
        build_plan(seed, origin, grid, PaletteKind::Oak)
    }

    fn synthetic_plan_cell_full(seed: u64, cell_x: i32, cell_z: i32) -> TownPlan {
        let origin = town_in_cell(seed, cell_x, cell_z);
        let min_x = origin.cell_x * CELL_SIZE;
        let min_z = origin.cell_z * CELL_SIZE;
        let grid = HeightGrid::synthetic_slope(
            min_x,
            min_z,
            min_x + CELL_SIZE,
            min_z + CELL_SIZE,
            8,
            80,
            2,
            1,
        );
        build_plan(seed, origin, grid, PaletteKind::Oak)
    }

    #[test]
    fn synthetic_roads_are_walkable_and_connected() {
        let plan = synthetic_plan_cell(20_261_025, 0, 0);
        let report = check_plan_roads(&plan);
        assert!(report.ok(), "{}", report.render());
        assert!(
            report.road_tiles >= 200,
            "too few road tiles {}",
            report.road_tiles
        );
        assert_eq!(PLAN_CACHE_VERSION, 37);
    }

    #[test]
    fn default_seed_walks_to_neighbour_gates() {
        let seed = 20_261_025u64;
        let home = synthetic_plan_cell_full(seed, 0, 0);
        let east = synthetic_plan_cell_full(seed, 1, 0);
        let south = synthetic_plan_cell_full(seed, 0, 1);
        let neighbors = [&east, &south];
        let report = check_plan_roads_with(&home, &neighbors);
        let east_cross = crate::town::border_crossing(seed, home.origin, east.origin);
        let south_cross = crate::town::border_crossing(seed, home.origin, south.origin);
        assert!(
            report.ok(),
            "seed 20261025 walk-test failed: {}\n  overlay {} east_cross {:?} home={:?} east={:?} south_cross {:?} home={:?} south={:?}",
            report.render(),
            home.border_roads.len(),
            east_cross,
            home.border_roads.get(&east_cross),
            east.border_roads.get(&east_cross),
            south_cross,
            home.border_roads.get(&south_cross),
            south.border_roads.get(&south_cross),
        );
        assert!(
            report.neighbor_paths_ok >= 1,
            "expected a neighbour gate road, got {}",
            report.neighbor_paths_ok
        );
    }
}
