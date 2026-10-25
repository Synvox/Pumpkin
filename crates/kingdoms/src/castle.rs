//! Parametric castle complex. Minimum footprint is 128×128 (8×8 chunks).

use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham};
use crate::height::HeightGrid;
use crate::rng::TownRng;

/// Smallest castle side, in blocks.
pub const CASTLE_MIN: i32 = 128;
/// Default outer footprint. 9×9 chunks.
pub const CASTLE_SIZE: i32 = 144;

/// Planned castle geometry in world space.
#[derive(Clone, Debug)]
pub struct CastlePlan {
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
}

impl CastlePlan {
    /// Footprint width and depth.
    #[must_use]
    pub const fn footprint(&self) -> (i32, i32) {
        (self.bounds.width(), self.bounds.depth())
    }
}

/// Picks a 144×144 high-ground window near `origin`.
#[must_use]
pub fn site_castle(grid: &HeightGrid, origin_x: i32, origin_z: i32, rng: &mut TownRng) -> Rect {
    let size = CASTLE_SIZE;
    let search = 96;
    let mut best = Rect::new(origin_x - size / 2, origin_z - size / 2, 0, 0);
    best = Rect::new(
        best.min_x,
        best.min_z,
        best.min_x + size - 1,
        best.min_z + size - 1,
    );
    let mut best_score = i64::MIN;
    let mut z = origin_z - search;
    while z <= origin_z + search {
        let mut x = origin_x - search;
        while x <= origin_x + search {
            let rect = Rect::new(x, z, x + size - 1, z + size - 1);
            let mean = i64::from(grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z));
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            if mean <= i64::from(grid.sea_level + 2) {
                x += 16;
                continue;
            }
            let surround = i64::from(grid.mean(
                rect.min_x - 32,
                rect.min_z - 32,
                rect.max_x + 32,
                rect.max_z + 32,
            ));
            let roughness = i64::from((hi - lo).abs());
            // Prefer high ground that is a ridge, with terrace-scale (not cliff) relief.
            let score = mean * 3 + (mean - surround) * 4 - (roughness - 8).abs() * 2
                + i64::from(rng.gen_range(0, 3));
            if score > best_score || (score == best_score && (x, z) < (best.min_x, best.min_z)) {
                best_score = score;
                best = rect;
            }
            x += 16;
        }
        z += 16;
    }
    best
}

/// Builds castle sub-rects and paints them into `occ`.
pub fn paint_castle(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    bounds: Rect,
    rng: &mut TownRng,
) -> CastlePlan {
    let gate_dir = downhill_side(grid, bounds);
    let inner = bounds.deflate(28);
    let peak = grid.peak(inner.min_x, inner.min_z, inner.max_x, inner.max_z);
    let keep_w = 32;
    let keep_d = 36;
    let keep = clamp_rect(
        Rect::new(
            peak.0 - keep_w / 2,
            peak.1 - keep_d / 2,
            peak.0 + keep_w / 2 - 1,
            peak.1 + keep_d / 2 - 1,
        ),
        inner,
    );

    let hall = place_beside(inner, keep, 18, 36, rng);
    let chapel = place_beside(inner, keep, 12, 20, rng);
    let stables = place_beside(bounds.deflate(8), keep, 14, 22, rng);
    let barracks = place_beside(bounds.deflate(8), keep, 14, 20, rng);
    let kitchen = place_beside(inner, keep, 10, 12, rng);

    let (gatehouse, barbican) = gate_rects(bounds, gate_dir);

    let mut towers = corner_towers(bounds, 9);
    towers.extend(interval_towers(bounds, 7, 28, gate_dir));
    towers.extend(corner_towers(inner, 7));

    occ.fill(bounds.deflate(4), CellKind::CastleCourt, false);
    paint_wall_ring(occ, bounds.deflate(2), 2, CellKind::CastleWall);
    paint_wall_ring(occ, inner, 1, CellKind::CastleInnerWall);
    occ.fill(keep, CellKind::CastleKeep, true);
    occ.fill(hall, CellKind::CastleHall, true);
    occ.fill(chapel, CellKind::CastleChapel, true);
    occ.fill(stables, CellKind::CastleStables, true);
    occ.fill(barracks, CellKind::CastleBarracks, true);
    occ.fill(kitchen, CellKind::CastleKitchen, true);
    occ.fill(gatehouse, CellKind::CastleGate, true);
    occ.fill(barbican, CellKind::CastleBarbican, true);
    for tower in &towers {
        occ.fill(*tower, CellKind::CastleTower, true);
    }
    paint_moat(occ, grid, bounds, gate_dir);

    CastlePlan {
        bounds,
        gate_dir,
        keep,
        hall,
        chapel,
        stables,
        barracks,
        kitchen,
        inner,
        barbican,
        gatehouse,
        towers,
    }
}

fn downhill_side(grid: &HeightGrid, bounds: Rect) -> Dir {
    let (cx, cz) = bounds.center();
    let north = grid.mean(bounds.min_x, bounds.min_z - 24, bounds.max_x, bounds.min_z);
    let south = grid.mean(bounds.min_x, bounds.max_z, bounds.max_x, bounds.max_z + 24);
    let west = grid.mean(bounds.min_x - 24, bounds.min_z, bounds.min_x, bounds.max_z);
    let east = grid.mean(bounds.max_x, bounds.min_z, bounds.max_x + 24, bounds.max_z);
    let _ = cx;
    let _ = cz;
    let mut best = Dir::South;
    let mut best_h = south;
    if north < best_h {
        best = Dir::North;
        best_h = north;
    }
    if west < best_h {
        best = Dir::West;
        best_h = west;
    }
    if east < best_h {
        best = Dir::East;
    }
    best
}

fn clamp_rect(rect: Rect, within: Rect) -> Rect {
    let w = rect.width();
    let d = rect.depth();
    let min_x = rect
        .min_x
        .clamp(within.min_x, (within.max_x - w + 1).max(within.min_x));
    let min_z = rect
        .min_z
        .clamp(within.min_z, (within.max_z - d + 1).max(within.min_z));
    Rect::new(min_x, min_z, min_x + w - 1, min_z + d - 1)
}

fn place_beside(within: Rect, keep: Rect, w: i32, d: i32, rng: &mut TownRng) -> Rect {
    let candidates = [
        Rect::new(
            keep.max_x + 3,
            keep.min_z,
            keep.max_x + 2 + w,
            keep.min_z + d - 1,
        ),
        Rect::new(
            keep.min_x - 2 - w,
            keep.min_z,
            keep.min_x - 3,
            keep.min_z + d - 1,
        ),
        Rect::new(
            keep.min_x,
            keep.max_z + 3,
            keep.min_x + w - 1,
            keep.max_z + 2 + d,
        ),
        Rect::new(
            keep.min_x,
            keep.min_z - 2 - d,
            keep.min_x + w - 1,
            keep.min_z - 3,
        ),
    ];
    for rect in candidates {
        let clamped = clamp_rect(rect, within);
        if clamped.width() >= w - 2 && clamped.depth() >= d - 2 && !clamped.intersects(keep) {
            return clamped;
        }
    }
    let ox = rng.gen_range(within.min_x, (within.max_x - w).max(within.min_x + 1));
    let oz = rng.gen_range(within.min_z, (within.max_z - d).max(within.min_z + 1));
    clamp_rect(Rect::new(ox, oz, ox + w - 1, oz + d - 1), within)
}

fn gate_rects(bounds: Rect, dir: Dir) -> (Rect, Rect) {
    let (cx, cz) = bounds.center();
    match dir {
        Dir::South => (
            Rect::new(cx - 8, bounds.max_z - 14, cx + 8, bounds.max_z - 2),
            Rect::new(cx - 6, bounds.max_z - 24, cx + 6, bounds.max_z - 15),
        ),
        Dir::North => (
            Rect::new(cx - 8, bounds.min_z + 2, cx + 8, bounds.min_z + 14),
            Rect::new(cx - 6, bounds.min_z + 15, cx + 6, bounds.min_z + 24),
        ),
        Dir::East => (
            Rect::new(bounds.max_x - 14, cz - 8, bounds.max_x - 2, cz + 8),
            Rect::new(bounds.max_x - 24, cz - 6, bounds.max_x - 15, cz + 6),
        ),
        Dir::West => (
            Rect::new(bounds.min_x + 2, cz - 8, bounds.min_x + 14, cz + 8),
            Rect::new(bounds.min_x + 15, cz - 6, bounds.min_x + 24, cz + 6),
        ),
    }
}

fn corner_towers(bounds: Rect, size: i32) -> Vec<Rect> {
    let s = size - 1;
    vec![
        Rect::new(
            bounds.min_x,
            bounds.min_z,
            bounds.min_x + s,
            bounds.min_z + s,
        ),
        Rect::new(
            bounds.max_x - s,
            bounds.min_z,
            bounds.max_x,
            bounds.min_z + s,
        ),
        Rect::new(
            bounds.min_x,
            bounds.max_z - s,
            bounds.min_x + s,
            bounds.max_z,
        ),
        Rect::new(
            bounds.max_x - s,
            bounds.max_z - s,
            bounds.max_x,
            bounds.max_z,
        ),
    ]
}

fn interval_towers(bounds: Rect, size: i32, spacing: i32, gate: Dir) -> Vec<Rect> {
    let mut towers = Vec::new();
    let s = size - 1;
    let mut x = bounds.min_x + spacing;
    while x + s < bounds.max_x - 8 {
        if gate != Dir::North {
            towers.push(Rect::new(x, bounds.min_z, x + s, bounds.min_z + s));
        }
        if gate != Dir::South {
            towers.push(Rect::new(x, bounds.max_z - s, x + s, bounds.max_z));
        }
        x += spacing;
    }
    let mut z = bounds.min_z + spacing;
    while z + s < bounds.max_z - 8 {
        if gate != Dir::West {
            towers.push(Rect::new(bounds.min_x, z, bounds.min_x + s, z + s));
        }
        if gate != Dir::East {
            towers.push(Rect::new(bounds.max_x - s, z, bounds.max_x, z + s));
        }
        z += spacing;
    }
    towers
}

fn paint_wall_ring(occ: &mut Occupancy, bounds: Rect, thickness: i32, kind: CellKind) {
    let verts = [
        (bounds.min_x, bounds.min_z),
        (bounds.max_x, bounds.min_z),
        (bounds.max_x, bounds.max_z),
        (bounds.min_x, bounds.max_z),
        (bounds.min_x, bounds.min_z),
    ];
    for pair in verts.windows(2) {
        let line = bresenham(pair[0].0, pair[0].1, pair[1].0, pair[1].1);
        occ.stroke(&line, thickness - 1, kind, true);
    }
}

fn paint_moat(occ: &mut Occupancy, grid: &HeightGrid, bounds: Rect, gate: Dir) {
    let ring = bounds.inflate(3);
    let (cx, cz) = bounds.center();
    for z in ring.min_z..=ring.max_z {
        for x in ring.min_x..=ring.max_x {
            if bounds.contains(x, z) {
                continue;
            }
            let toward_gate = match gate {
                Dir::South => z > bounds.max_z && (x - cx).abs() < 10,
                Dir::North => z < bounds.min_z && (x - cx).abs() < 10,
                Dir::East => x > bounds.max_x && (z - cz).abs() < 10,
                Dir::West => x < bounds.min_x && (z - cz).abs() < 10,
            };
            if toward_gate {
                continue;
            }
            let h = grid.height(x, z);
            if h <= grid.sea_level + 8 {
                occ.set(x, z, CellKind::CastleMoat, false);
            }
        }
    }
}
