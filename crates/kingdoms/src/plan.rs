//! City-level plan: castle site, walls, roads, lots, farms.
//!
//! Pure function of `(seed, cell, height grid, palette)`. Cached by the decorator.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::{Arc, OnceLock, RwLock};

use pumpkin_data::biome::Biome;
use pumpkin_world::biome::BiomeSupplier;
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::noise::router::multi_noise_sampler::MultiNoiseSampler;

use crate::buildings::{BuildingKind, Lot};
use crate::castle::{CASTLE_MIN, CASTLE_SIZE, CastlePlan, paint_castle, site_castle};
use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham};
use crate::height::{HeightGrid, SAMPLE_STEP, world_gen_for};
use crate::info::{GateInfo, GateKind, RoadInfo};
use crate::palette::PaletteKind;
use crate::rng::TownRng;
use crate::town::{TOWN_INFLUENCE, TownPos, town_in_cell};

/// Cached plans, keyed by seed + cell.
fn cache() -> &'static RwLock<HashMap<(u64, i32, i32), Arc<TownPlan>>> {
    static CACHE: OnceLock<RwLock<HashMap<(u64, i32, i32), Arc<TownPlan>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Full planned town.
#[derive(Clone, Debug)]
pub struct TownPlan {
    pub seed: u64,
    pub origin: TownPos,
    pub palette: PaletteKind,
    pub bounds: Rect,
    pub castle: CastlePlan,
    pub market: Rect,
    pub well: (i32, i32),
    /// Town-wall vertices, clockwise, not necessarily closed.
    pub town_wall: Vec<(i32, i32)>,
    pub gates: Vec<GateInfo>,
    pub town_towers: Vec<Rect>,
    pub roads: Vec<RoadInfo>,
    pub lots: Vec<Lot>,
    pub occ: Occupancy,
    pub heights: HeightGrid,
}

impl TownPlan {
    /// Castle footprint in blocks. Must be at least 128×128.
    #[must_use]
    pub fn castle_footprint(&self) -> (i32, i32) {
        self.castle.footprint()
    }
}

/// Returns the cached plan, building it from `world_gen` if needed.
#[must_use]
pub fn plan_for(seed: u64, cell_x: i32, cell_z: i32, world_gen: &WorldGenerator) -> Arc<TownPlan> {
    let key = (seed, cell_x, cell_z);
    if let Some(plan) = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return Arc::clone(plan);
    }
    let origin = town_in_cell(seed, cell_x, cell_z);
    let pad = TOWN_INFLUENCE;
    let grid = HeightGrid::sample(
        world_gen,
        origin.x - pad,
        origin.z - pad,
        origin.x + pad,
        origin.z + pad,
        SAMPLE_STEP,
    );
    let palette = sample_palette(world_gen, origin.x, origin.z);
    let plan = Arc::new(build_plan(seed, origin, grid, palette));
    let mut write = cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    write.entry(key).or_insert_with(|| Arc::clone(&plan));
    Arc::clone(write.get(&key).unwrap_or(&plan))
}

/// Builds or fetches a plan, using a registered generator when the cache misses.
///
/// Samples noise heights only. Does not generate or load chunks.
#[must_use]
pub fn plan_from_cache(seed: u64, cell_x: i32, cell_z: i32) -> Option<Arc<TownPlan>> {
    if let Some(plan) = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(seed, cell_x, cell_z))
    {
        return Some(Arc::clone(plan));
    }
    let world_gen = world_gen_for(seed)?;
    Some(plan_for(seed, cell_x, cell_z, &world_gen))
}

/// Plan for the cell that contains `(block_x, block_z)`. Same as
/// [`plan_from_cache`] after mapping the column to its cell.
#[must_use]
pub fn plan_at(seed: u64, block_x: i32, block_z: i32) -> Option<Arc<TownPlan>> {
    let town = crate::town::town_containing(seed, block_x, block_z);
    plan_from_cache(seed, town.cell_x, town.cell_z)
}

/// Builds a plan from an already-sampled height grid (tests / pregen).
#[must_use]
pub fn build_plan(seed: u64, origin: TownPos, grid: HeightGrid, palette: PaletteKind) -> TownPlan {
    let mut rng = TownRng::salted(seed, origin.cell_x as u64, origin.cell_z as u64);
    let castle_rect = site_castle(&grid, origin.x, origin.z, &mut rng);
    let gate = downhill_town_dir(&grid, castle_rect);
    let town_rect = town_beside(castle_rect, gate, 132, 120);
    let bounds = union(castle_rect.inflate(6), town_rect.inflate(18));
    let mut occ = Occupancy::new(bounds);
    let castle = paint_castle(&mut occ, &grid, castle_rect, &mut rng);

    let market = flattest(&grid, town_rect, 28, 28);
    occ.fill(market, CellKind::Market, false);
    let well = market.center();
    occ.set(well.0, well.1, CellKind::Well, true);
    for dz in -1..=1 {
        for dx in -1..=1 {
            occ.set(well.0 + dx, well.1 + dz, CellKind::Well, true);
        }
    }

    let (town_wall, town_towers) = paint_town_wall(&mut occ, town_rect, castle_rect, gate);
    let gate_pts = town_gates(town_rect, castle.gatehouse.center(), gate);
    let gates = vec![
        GateInfo {
            kind: GateKind::Town,
            x: gate_pts[0].0,
            z: gate_pts[0].1,
            facing: gate,
        },
        GateInfo {
            kind: GateKind::Castle,
            x: gate_pts[1].0,
            z: gate_pts[1].1,
            facing: castle.gate_dir,
        },
    ];
    for gate_info in &gates {
        for dz in -2..=2 {
            for dx in -2..=2 {
                occ.set(gate_info.x + dx, gate_info.z + dz, CellKind::TownGate, true);
            }
        }
    }

    let mut waypoints = vec![well, castle.gatehouse.center()];
    waypoints.extend(gate_pts);
    let roads = paint_roads(&mut occ, &grid, &waypoints);

    let lots = place_lots(&mut occ, town_rect, market, &mut rng, seed);
    place_farms(&mut occ, &grid, town_rect, castle_rect, &mut rng);

    debug_assert!(castle.bounds.width() >= CASTLE_MIN);
    debug_assert!(castle.bounds.depth() >= CASTLE_MIN);
    let _ = CASTLE_SIZE;

    TownPlan {
        seed,
        origin,
        palette,
        bounds,
        castle,
        market,
        well,
        town_wall,
        gates,
        town_towers,
        roads,
        lots,
        occ,
        heights: grid,
    }
}

fn sample_palette(world_gen: &WorldGenerator, x: i32, z: i32) -> PaletteKind {
    let WorldGenerator::Noise(vanilla) = world_gen else {
        return PaletteKind::Oak;
    };
    let mut sampler = MultiNoiseSampler::generate(&vanilla.base_router.multi_noise);
    let biome: &Biome = vanilla
        .biome_supplier
        .biome(x >> 2, 16, z >> 2, &mut sampler);
    PaletteKind::from_biome(biome)
}

fn downhill_town_dir(grid: &HeightGrid, castle: Rect) -> Dir {
    let (cx, cz) = castle.center();
    let dirs = [
        (
            Dir::South,
            grid.mean(cx - 40, castle.max_z + 8, cx + 40, castle.max_z + 80),
        ),
        (
            Dir::North,
            grid.mean(cx - 40, castle.min_z - 80, cx + 40, castle.min_z - 8),
        ),
        (
            Dir::East,
            grid.mean(castle.max_x + 8, cz - 40, castle.max_x + 80, cz + 40),
        ),
        (
            Dir::West,
            grid.mean(castle.min_x - 80, cz - 40, castle.min_x - 8, cz + 40),
        ),
    ];
    dirs.iter()
        .min_by_key(|(_, h)| *h)
        .map_or(Dir::South, |(d, _)| *d)
}

fn town_beside(castle: Rect, dir: Dir, w: i32, d: i32) -> Rect {
    match dir {
        Dir::South => Rect::new(
            castle.min_x - 16,
            castle.max_z - 8,
            castle.min_x - 16 + w,
            castle.max_z - 8 + d,
        ),
        Dir::North => Rect::new(
            castle.min_x - 16,
            castle.min_z - d + 8,
            castle.min_x - 16 + w,
            castle.min_z + 8,
        ),
        Dir::East => Rect::new(
            castle.max_x - 8,
            castle.min_z - 16,
            castle.max_x - 8 + w,
            castle.min_z - 16 + d,
        ),
        Dir::West => Rect::new(
            castle.min_x - w + 8,
            castle.min_z - 16,
            castle.min_x + 8,
            castle.min_z - 16 + d,
        ),
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.min_x.min(b.min_x),
        a.min_z.min(b.min_z),
        a.max_x.max(b.max_x),
        a.max_z.max(b.max_z),
    )
}

fn flattest(grid: &HeightGrid, within: Rect, w: i32, d: i32) -> Rect {
    let mut best = Rect::new(
        within.min_x,
        within.min_z,
        within.min_x + w - 1,
        within.min_z + d - 1,
    );
    let mut best_range = i32::MAX;
    let mut z = within.min_z;
    while z + d <= within.max_z {
        let mut x = within.min_x;
        while x + w <= within.max_x {
            let (lo, hi) = grid.range(x, z, x + w - 1, z + d - 1);
            let r = hi - lo;
            if r < best_range || (r == best_range && (x, z) < (best.min_x, best.min_z)) {
                best_range = r;
                best = Rect::new(x, z, x + w - 1, z + d - 1);
            }
            x += 8;
        }
        z += 8;
    }
    best
}

fn paint_town_wall(
    occ: &mut Occupancy,
    town: Rect,
    castle: Rect,
    gate: Dir,
) -> (Vec<(i32, i32)>, Vec<Rect>) {
    let wall = town.inflate(2);
    let verts = [
        (wall.min_x, wall.min_z),
        (wall.max_x, wall.min_z),
        (wall.max_x, wall.max_z),
        (wall.min_x, wall.max_z),
    ];
    for i in 0..4 {
        let a = verts[i];
        let b = verts[(i + 1) % 4];
        // Skip the side that attaches to the castle.
        let skip = match gate {
            Dir::South => a.1 == wall.min_z && b.1 == wall.min_z,
            Dir::North => a.1 == wall.max_z && b.1 == wall.max_z,
            Dir::East => a.0 == wall.min_x && b.0 == wall.min_x,
            Dir::West => a.0 == wall.max_x && b.0 == wall.max_x,
        };
        if skip {
            continue;
        }
        let line = bresenham(a.0, a.1, b.0, b.1);
        occ.stroke(&line, 1, CellKind::TownWall, false);
    }
    let mut towers = Vec::new();
    for &(x, z) in &verts {
        if !castle.contains(x, z) {
            let tower = Rect::new(x - 2, z - 2, x + 2, z + 2);
            towers.push(tower);
            for dz in -2..=2 {
                for dx in -2..=2 {
                    occ.set(x + dx, z + dz, CellKind::TownTower, false);
                }
            }
        }
    }
    (verts.to_vec(), towers)
}

fn town_gates(town: Rect, castle_gate: (i32, i32), dir: Dir) -> Vec<(i32, i32)> {
    let (cx, cz) = town.center();
    let outer = match dir {
        Dir::South => (cx, town.max_z + 2),
        Dir::North => (cx, town.min_z - 2),
        Dir::East => (town.max_x + 2, cz),
        Dir::West => (town.min_x - 2, cz),
    };
    vec![outer, castle_gate]
}

fn paint_roads(occ: &mut Occupancy, grid: &HeightGrid, waypoints: &[(i32, i32)]) -> Vec<RoadInfo> {
    let mut roads = Vec::new();
    let bounds = occ.bounds;
    let mut push = |path: Vec<(i32, i32)>| {
        if let (Some(&first), Some(&last)) = (path.first(), path.last()) {
            paint_path(occ, grid, &path);
            roads.push(RoadInfo {
                from: [first.0, first.1],
                to: [last.0, last.1],
                points: path.iter().map(|&(x, z)| [x, z]).collect(),
            });
        }
    };
    for pair in waypoints.windows(2) {
        if let Some(path) = astar(grid, bounds, pair[0], pair[1]) {
            push(path);
        } else {
            push(bresenham(pair[0].0, pair[0].1, pair[1].0, pair[1].1));
        }
    }
    let (mx, mz) = waypoints[0];
    for dir in Dir::ALL {
        let end = (mx + dir.dx() * 40, mz + dir.dz() * 40);
        if bounds.contains(end.0, end.1)
            && let Some(path) = astar(grid, bounds, (mx, mz), end)
        {
            push(path);
        }
    }
    roads
}

fn paint_path(occ: &mut Occupancy, grid: &HeightGrid, path: &[(i32, i32)]) {
    for window in path.windows(2) {
        let (x, z) = window[0];
        let (nx, nz) = window[1];
        let dh = grid.height(nx, nz) - grid.height(x, z);
        let stair = if dh.abs() >= 2 {
            if nx > x {
                CellKind::RoadStairE
            } else if nx < x {
                CellKind::RoadStairW
            } else if nz > z {
                CellKind::RoadStairS
            } else {
                CellKind::RoadStairN
            }
        } else {
            CellKind::Road
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let kind = if dx == 0 && dz == 0 {
                    stair
                } else {
                    CellKind::Road
                };
                let cell = occ.kind(x + dx, z + dz);
                if !cell.is_castle() && cell != CellKind::Well {
                    occ.set(
                        x + dx,
                        z + dz,
                        kind,
                        matches!(cell, CellKind::Empty | CellKind::Farm | CellKind::Market),
                    );
                }
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct Node {
    f: i32,
    x: i32,
    z: i32,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .cmp(&self.f)
            .then_with(|| self.x.cmp(&other.x))
            .then_with(|| self.z.cmp(&other.z))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn astar(
    grid: &HeightGrid,
    bounds: Rect,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    let step = 4;
    let snap = |v: i32| v / step * step;
    let start = (snap(start.0), snap(start.1));
    let goal = (snap(goal.0), snap(goal.1));
    let mut open = BinaryHeap::new();
    let mut came: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut gscore: HashMap<(i32, i32), i32> = HashMap::new();
    open.push(Node {
        f: heur(start, goal),
        x: start.0,
        z: start.1,
    });
    gscore.insert(start, 0);
    let mut guard = 0u32;
    while let Some(Node { x, z, .. }) = open.pop() {
        guard += 1;
        if guard > 8_000 {
            break;
        }
        if (x, z) == goal {
            return Some(rebuild(&came, start, goal));
        }
        let g = *gscore.get(&(x, z)).unwrap_or(&i32::MAX);
        for dir in Dir::ALL {
            let nx = x + dir.dx() * step;
            let nz = z + dir.dz() * step;
            if !bounds.inflate(8).contains(nx, nz) {
                continue;
            }
            let dh = (grid.height(nx, nz) - grid.height(x, z)).abs();
            let slope = dh * 100 / step;
            // 1:4 is 25. Penalize steeper; allow stairs.
            let mut cost = step + slope.max(0);
            if slope > 25 {
                cost += (slope - 25) * 4;
            }
            if slope > 50 {
                cost += 40;
            }
            if grid.height(nx, nz) <= grid.sea_level {
                cost += 80;
            }
            let tentative = g.saturating_add(cost);
            if tentative < *gscore.get(&(nx, nz)).unwrap_or(&i32::MAX) {
                came.insert((nx, nz), (x, z));
                gscore.insert((nx, nz), tentative);
                open.push(Node {
                    f: tentative + heur((nx, nz), goal),
                    x: nx,
                    z: nz,
                });
            }
        }
    }
    None
}

fn heur(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

fn rebuild(
    came: &HashMap<(i32, i32), (i32, i32)>,
    start: (i32, i32),
    goal: (i32, i32),
) -> Vec<(i32, i32)> {
    let mut cur = goal;
    let mut points = vec![cur];
    while cur != start {
        if let Some(&prev) = came.get(&cur) {
            let line = bresenham(prev.0, prev.1, cur.0, cur.1);
            points.extend(line.into_iter().rev().skip(1));
            cur = prev;
        } else {
            break;
        }
    }
    points.reverse();
    points
}

fn place_lots(
    occ: &mut Occupancy,
    town: Rect,
    market: Rect,
    rng: &mut TownRng,
    seed: u64,
) -> Vec<Lot> {
    let mut lots = Vec::new();
    let kinds = [
        BuildingKind::Church,
        BuildingKind::Tavern,
        BuildingKind::Blacksmith,
        BuildingKind::Timber,
        BuildingKind::Timber,
        BuildingKind::Longhouse,
        BuildingKind::Cottage,
        BuildingKind::Cottage,
        BuildingKind::Cottage,
        BuildingKind::Windmill,
        BuildingKind::Barn,
        BuildingKind::Stall,
        BuildingKind::Stall,
    ];
    let mut id = 1u16;
    for z in (town.min_z + 4..town.max_z - 4).step_by(12) {
        for x in (town.min_x + 4..town.max_x - 4).step_by(12) {
            if market.inflate(2).contains(x, z) {
                continue;
            }
            let Some(&kind) = rng.choose(&kinds) else {
                continue;
            };
            let (w, d) = kind.size();
            let facing = *rng.choose(&Dir::ALL).unwrap_or(&Dir::South);
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            if !occ.is_free(rect.inflate(1)) {
                continue;
            }
            if !town.contains(rect.max_x, rect.max_z) {
                continue;
            }
            let lot = Lot::new(id, kind, rect, facing, seed ^ (u64::from(id) << 8));
            occ.fill_building(rect, id, false);
            lots.push(lot);
            id += 1;
            if lots.len() >= 36 {
                return lots;
            }
        }
    }
    lots
}

fn place_farms(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    castle: Rect,
    rng: &mut TownRng,
) {
    let ring = town.inflate(36);
    let mut placed = 0;
    let mut z = ring.min_z;
    while z < ring.max_z && placed < 8 {
        let mut x = ring.min_x;
        while x < ring.max_x && placed < 8 {
            let rect = Rect::new(x, z, x + 18, z + 12);
            if castle.inflate(8).intersects(rect) || town.intersects(rect) {
                x += 20;
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let near_water = lo <= grid.sea_level + 6;
            let gentle = hi - lo <= 8;
            if gentle && (near_water || rng.chance(1, 2)) && occ.is_free(rect) {
                occ.fill(rect.deflate(1), CellKind::Farm, false);
                occ.fill(rect, CellKind::FarmEdge, false);
                occ.fill(rect.deflate(1), CellKind::Farm, true);
                placed += 1;
            }
            x += 22;
        }
        z += 16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::town::town_in_cell;

    #[test]
    fn plan_is_deterministic_and_castle_is_large() {
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
        let a = build_plan(20_261_025, origin, grid.clone(), PaletteKind::Oak);
        let b = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert_eq!(a.castle.bounds, b.castle.bounds);
        assert_eq!(a.well, b.well);
        assert_eq!(a.lots.len(), b.lots.len());
        let (w, d) = a.castle_footprint();
        assert!(w >= 128, "castle width {w}");
        assert!(d >= 128, "castle depth {d}");
        assert_eq!(a.occ.kind(a.well.0, a.well.1), CellKind::Well);
        let keep = a.castle.keep.center();
        let col_a = a.planned_column(keep.0, keep.1);
        let col_b = b.planned_column(keep.0, keep.1);
        assert!(!col_a.is_empty(), "keep column should have intended blocks");
        assert_eq!(col_a, col_b);
        let sample = col_a[col_a.len() / 2];
        assert_eq!(
            a.planned_block(sample.x, sample.y, sample.z)
                .map(|p| p.state),
            Some(sample.state)
        );
        assert_eq!(
            a.planned_at(pumpkin_util::math::position::BlockPos::new(
                sample.x, sample.y, sample.z
            ))
            .map(|p| p.state),
            Some(sample.state)
        );
        assert!(a.planned_keep_blocks().len() >= col_a.len());
        let lot = a.lots.first().expect("town should have a building lot");
        let house = a.planned_building_blocks(lot.id);
        assert!(
            !house.is_empty(),
            "civilian lot {} should have intended blocks",
            lot.id
        );
        assert_eq!(house, a.planned_blocks_in(lot.rect));
        let mid = house[house.len() / 2];
        assert_eq!(
            a.planned_block(mid.x, mid.y, mid.z).map(|p| p.state),
            Some(mid.state)
        );
    }
}
