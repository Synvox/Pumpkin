//! Parametric castle complex. Minimum footprint is 128×128 (8×8 chunks).
//!
//! Walls follow a terrain contour (not a rectangle). Towers, roofs and keep
//! masses are reusable specs so later siege / repair code can name them.

use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham, dist2, point_in_poly, poly_bounds};
use crate::height::HeightGrid;
use crate::rng::TownRng;

/// Smallest castle side, in blocks.
pub const CASTLE_MIN: i32 = 128;
/// Default outer footprint. 9×9 chunks.
pub const CASTLE_SIZE: i32 = 144;

/// Which silhouette family a castle uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastleArchetype {
    /// Elongated stacked ridge (Hohenzollern / Mont-Saint-Michel).
    Ridge,
    /// Double octagonal wards (Bodiam / Carcassonne).
    Concentric,
    /// Keep at the high tip, bailey dropping downhill (Conwy / Eltz).
    Spur,
    /// Polygonal curtain hugging dry land (Caernarfon).
    Coastal,
}

/// Plan-view of a tower or keep mass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TowerShape {
    Square,
    Round,
    Octagon,
    /// Half-round, flat face toward `flat`.
    DShape {
        flat: Dir,
    },
}

/// How the top of a mass is finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoofKind {
    Crenellated,
    Cone,
    Hip,
    Spire,
    Gable { along_x: bool },
}

/// One reusable tower.
#[derive(Clone, Copy, Debug)]
pub struct TowerSpec {
    pub cx: i32,
    pub cz: i32,
    pub radius: i32,
    pub height: i32,
    pub shape: TowerShape,
    pub roof: RoofKind,
    pub inner: bool,
}

impl TowerSpec {
    /// Whether the column sits in this tower.
    #[must_use]
    pub fn contains(self, x: i32, z: i32) -> bool {
        shape_contains(self.shape, self.cx, self.cz, self.radius, x, z)
    }

    /// Outer one-block shell.
    #[must_use]
    pub fn on_rim(self, x: i32, z: i32) -> bool {
        self.contains(x, z) && !shape_contains(self.shape, self.cx, self.cz, self.radius - 1, x, z)
    }

    /// Axis-aligned bounds.
    #[must_use]
    pub const fn bounds(self) -> Rect {
        Rect::new(
            self.cx - self.radius,
            self.cz - self.radius,
            self.cx + self.radius,
            self.cz + self.radius,
        )
    }
}

/// One mass of a multi-part keep.
#[derive(Clone, Copy, Debug)]
pub struct KeepMass {
    pub spec: TowerSpec,
}

/// Closed wall ring (outer or inner ward).
#[derive(Clone, Debug)]
pub struct WallRing {
    pub verts: Vec<(i32, i32)>,
    pub thickness: i32,
    pub height: i32,
}

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
    pub archetype: CastleArchetype,
    pub outer_ring: WallRing,
    pub inner_ring: WallRing,
    pub tower_specs: Vec<TowerSpec>,
    pub keep_masses: Vec<KeepMass>,
}

impl CastlePlan {
    /// Footprint width and depth.
    #[must_use]
    pub const fn footprint(&self) -> (i32, i32) {
        (self.bounds.width(), self.bounds.depth())
    }

    /// Tower spec covering `(x, z)`, if any.
    #[must_use]
    pub fn tower_at(&self, x: i32, z: i32) -> Option<TowerSpec> {
        self.tower_specs.iter().copied().find(|t| t.contains(x, z))
    }

    /// Tallest keep mass covering `(x, z)`.
    #[must_use]
    pub fn keep_mass_at(&self, x: i32, z: i32) -> Option<KeepMass> {
        self.keep_masses
            .iter()
            .copied()
            .filter(|m| m.spec.contains(x, z))
            .max_by_key(|m| m.spec.height)
    }
}

/// Picks a 144×144 high-ground window near `origin`.
#[must_use]
pub fn site_castle(grid: &HeightGrid, origin_x: i32, origin_z: i32, rng: &mut TownRng) -> Rect {
    let size = CASTLE_SIZE;
    let search = 128;
    let mut best = Rect::new(
        origin_x - size / 2,
        origin_z - size / 2,
        origin_x - size / 2 + size - 1,
        origin_z - size / 2 + size - 1,
    );
    let mut best_score = i64::MIN;
    let mut z = origin_z - search;
    while z <= origin_z + search {
        let mut x = origin_x - search;
        while x <= origin_x + search {
            let rect = Rect::new(x, z, x + size - 1, z + size - 1);
            let mean = i64::from(grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z));
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let dry = i64::from(grid.dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z));
            if mean <= i64::from(grid.sea_level) {
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
            let score = mean * 2 + (mean - surround) * 3 - (roughness - 10).abs()
                + dry * 14
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
    let archetype = pick_archetype(grid, bounds, rng);
    let outer_verts = contour_polygon(grid, bounds, gate_dir, archetype, rng);
    let peak = grid.peak(
        bounds.min_x + 16,
        bounds.min_z + 16,
        bounds.max_x - 16,
        bounds.max_z - 16,
    );
    let inner_verts = scale_poly(&outer_verts, peak.0, peak.1, inner_scale(archetype));
    let inner = poly_bounds(&inner_verts).unwrap_or_else(|| bounds.deflate(28));

    let outer_ring = WallRing {
        verts: outer_verts,
        thickness: 2,
        height: 9,
    };
    let inner_ring = WallRing {
        verts: inner_verts,
        thickness: 1,
        height: 11,
    };

    let keep_masses = build_keep(peak.0, peak.1, archetype, rng);
    let keep = masses_bounds(&keep_masses)
        .unwrap_or_else(|| Rect::new(peak.0 - 12, peak.1 - 12, peak.0 + 12, peak.1 + 12));

    let hall = place_in_poly(&inner_ring.verts, keep, 16, 28, rng, inner);
    let chapel = place_in_poly(&inner_ring.verts, keep, 11, 18, rng, inner);
    let kitchen = place_in_poly(&inner_ring.verts, keep, 9, 11, rng, inner);
    let stables = place_in_poly(&outer_ring.verts, keep, 13, 20, rng, bounds.deflate(10));
    let barracks = place_in_poly(&outer_ring.verts, keep, 12, 16, rng, bounds.deflate(10));

    let (gate_i, gate_pt) = gate_vertex(&outer_ring.verts, bounds.center(), gate_dir);
    let (gatehouse, barbican, flank_a, flank_b) =
        gate_complex(gate_pt, gate_dir, &outer_ring.verts, gate_i);

    let mut tower_specs = ring_towers(&outer_ring.verts, false, archetype, rng, Some(gate_pt));
    tower_specs.extend(ring_towers(&inner_ring.verts, true, archetype, rng, None));
    tower_specs.push(flank_a);
    tower_specs.push(flank_b);
    if archetype == CastleArchetype::Concentric || archetype == CastleArchetype::Ridge {
        let (cx, cz) = bounds.center();
        tower_specs.push(TowerSpec {
            cx,
            cz: outer_ring.verts.first().map_or(cz, |v| (v.1 + cz) / 2),
            radius: 3,
            height: 16,
            shape: TowerShape::Round,
            roof: RoofKind::Cone,
            inner: false,
        });
    }

    occ.fill_poly(&outer_ring.verts, CellKind::Green, false);
    paint_court_paths(occ, &keep, &hall, gate_pt);
    occ.stroke_poly(
        &outer_ring.verts,
        outer_ring.thickness,
        CellKind::CastleWall,
        true,
    );
    occ.stroke_poly(
        &inner_ring.verts,
        inner_ring.thickness,
        CellKind::CastleInnerWall,
        true,
    );
    paint_cross_walls(occ, &keep, gate_pt, &inner_ring.verts);

    for mass in &keep_masses {
        fill_shape(occ, mass.spec, CellKind::CastleKeep);
    }
    occ.fill(hall, CellKind::CastleHall, true);
    paint_buttresses(occ, hall, CellKind::CastleHall);
    occ.fill(chapel, CellKind::CastleChapel, true);
    paint_apse(occ, chapel, gate_dir.opposite());
    occ.fill(stables, CellKind::CastleStables, true);
    occ.fill(barracks, CellKind::CastleBarracks, true);
    occ.fill(kitchen, CellKind::CastleKitchen, true);
    let passage = match gate_dir {
        Dir::North | Dir::South => {
            Rect::new(gate_pt.0 - 2, gate_pt.1 - 6, gate_pt.0 + 2, gate_pt.1 + 6)
        }
        Dir::East | Dir::West => {
            Rect::new(gate_pt.0 - 6, gate_pt.1 - 2, gate_pt.0 + 6, gate_pt.1 + 2)
        }
    };
    occ.fill(passage, CellKind::CastleGate, true);
    fill_shape(
        occ,
        barbican_spec(barbican, gate_dir),
        CellKind::CastleBarbican,
    );
    for spec in &tower_specs {
        fill_shape(occ, *spec, CellKind::CastleTower);
    }
    paint_approach(occ, gate_pt, gate_dir);
    paint_moat(occ, grid, &outer_ring.verts, gate_pt, gate_dir);

    let towers = tower_specs.iter().map(|t| t.bounds()).collect();

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
        archetype,
        outer_ring,
        inner_ring,
        tower_specs,
        keep_masses,
    }
}

fn pick_archetype(grid: &HeightGrid, bounds: Rect, rng: &mut TownRng) -> CastleArchetype {
    let dry = grid.dry_ratio(bounds.min_x, bounds.min_z, bounds.max_x, bounds.max_z);
    let (lo, hi) = grid.range(bounds.min_x, bounds.min_z, bounds.max_x, bounds.max_z);
    let relief = hi - lo;
    if dry < 62 {
        return CastleArchetype::Coastal;
    }
    if relief >= 20 {
        return if rng.chance(1, 2) {
            CastleArchetype::Spur
        } else {
            CastleArchetype::Ridge
        };
    }
    if relief <= 8 {
        return CastleArchetype::Concentric;
    }
    match rng.gen_range(0, 3) {
        0 => CastleArchetype::Ridge,
        1 => CastleArchetype::Concentric,
        _ => CastleArchetype::Spur,
    }
}

fn inner_scale(archetype: CastleArchetype) -> i32 {
    match archetype {
        CastleArchetype::Concentric => 58,
        CastleArchetype::Ridge | CastleArchetype::Spur => 50,
        CastleArchetype::Coastal => 54,
    }
}

fn contour_polygon(
    grid: &HeightGrid,
    bounds: Rect,
    gate_dir: Dir,
    archetype: CastleArchetype,
    rng: &mut TownRng,
) -> Vec<(i32, i32)> {
    let n = match archetype {
        CastleArchetype::Concentric => 8,
        CastleArchetype::Coastal => 10,
        CastleArchetype::Ridge => 9,
        CastleArchetype::Spur => 7,
    };
    let (cx, cz) = bounds.center();
    let (_, _, peak_h) = grid.peak(bounds.min_x, bounds.min_z, bounds.max_x, bounds.max_z);
    let target = peak_h
        - match archetype {
            CastleArchetype::Spur => 10,
            CastleArchetype::Ridge => 8,
            _ => 6,
        };
    let pad = 6;
    let mut verts = Vec::with_capacity(n as usize);
    for i in 0..n {
        let ang = (i as f64) * std::f64::consts::TAU / f64::from(n);
        let (s, c) = (ang.sin(), ang.cos());
        let mut last = (cx, cz);
        let mut r = 18;
        let max_r = 62;
        while r < max_r {
            let x = cx + (r as f64 * c).round() as i32;
            let z = cz + (r as f64 * s).round() as i32;
            if !bounds.deflate(pad).contains(x, z) {
                break;
            }
            if !grid.is_dry(x, z) && archetype != CastleArchetype::Coastal {
                break;
            }
            if grid.height(x, z) < target {
                break;
            }
            last = (x, z);
            r += 2;
        }
        let jitter = rng.gen_range(-2, 3);
        last.0 += (c * f64::from(jitter)).round() as i32;
        last.1 += (s * f64::from(jitter)).round() as i32;
        verts.push(clamp_pt(last, bounds.deflate(pad)));
    }
    match archetype {
        CastleArchetype::Ridge => elongate(&mut verts, cx, cz, gate_dir.opposite(), 18),
        CastleArchetype::Spur => pinch_uphill(&mut verts, cx, cz, gate_dir),
        CastleArchetype::Concentric => regularize(&mut verts, cx, cz, 48, n),
        CastleArchetype::Coastal => pull_off_water(&mut verts, grid, cx, cz),
    }
    for v in &mut verts {
        *v = clamp_pt(*v, bounds.deflate(pad));
    }
    verts
}

fn elongate(verts: &mut [(i32, i32)], cx: i32, cz: i32, along: Dir, extra: i32) {
    for (x, z) in verts.iter_mut() {
        let dx = *x - cx;
        let dz = *z - cz;
        match along {
            Dir::North | Dir::South => *z += dz.signum() * extra / 3,
            Dir::East | Dir::West => *x += dx.signum() * extra / 3,
        }
    }
}

fn pinch_uphill(verts: &mut [(i32, i32)], cx: i32, cz: i32, downhill: Dir) {
    for (x, z) in verts.iter_mut() {
        let toward = match downhill {
            Dir::South => *z > cz,
            Dir::North => *z < cz,
            Dir::East => *x > cx,
            Dir::West => *x < cx,
        };
        if toward {
            *x += (*x - cx).signum() * 6;
            *z += (*z - cz).signum() * 6;
        } else {
            *x = cx + (*x - cx) * 3 / 4;
            *z = cz + (*z - cz) * 3 / 4;
        }
    }
}

fn regularize(verts: &mut [(i32, i32)], cx: i32, cz: i32, radius: i32, n: i32) {
    for (i, v) in verts.iter_mut().enumerate() {
        let ang = (i as f64) * std::f64::consts::TAU / f64::from(n);
        let rx = cx + (f64::from(radius) * ang.cos()).round() as i32;
        let rz = cz + (f64::from(radius) * ang.sin()).round() as i32;
        v.0 = (v.0 * 2 + rx) / 3;
        v.1 = (v.1 * 2 + rz) / 3;
    }
}

fn pull_off_water(verts: &mut [(i32, i32)], grid: &HeightGrid, cx: i32, cz: i32) {
    for (x, z) in verts.iter_mut() {
        if grid.is_dry(*x, *z) {
            continue;
        }
        let dx = cx - *x;
        let dz = cz - *z;
        *x += dx.signum() * 10;
        *z += dz.signum() * 10;
    }
}

fn scale_poly(verts: &[(i32, i32)], cx: i32, cz: i32, permille: i32) -> Vec<(i32, i32)> {
    verts
        .iter()
        .map(|&(x, z)| {
            (
                cx + (x - cx) * permille / 1000,
                cz + (z - cz) * permille / 1000,
            )
        })
        .collect()
}

fn clamp_pt(pt: (i32, i32), within: Rect) -> (i32, i32) {
    (
        pt.0.clamp(within.min_x, within.max_x),
        pt.1.clamp(within.min_z, within.max_z),
    )
}

fn build_keep(px: i32, pz: i32, archetype: CastleArchetype, rng: &mut TownRng) -> Vec<KeepMass> {
    let main_shape = match archetype {
        CastleArchetype::Coastal | CastleArchetype::Concentric => TowerShape::Octagon,
        CastleArchetype::Ridge => TowerShape::Square,
        CastleArchetype::Spur => TowerShape::Round,
    };
    let main_r = match archetype {
        CastleArchetype::Concentric => 8,
        _ => 10,
    };
    let mut masses = vec![KeepMass {
        spec: TowerSpec {
            cx: px,
            cz: pz,
            radius: main_r,
            height: 24,
            shape: main_shape,
            roof: if archetype == CastleArchetype::Ridge {
                RoofKind::Spire
            } else {
                RoofKind::Hip
            },
            inner: true,
        },
    }];
    let ox = rng.gen_range(-1, 2).max(1);
    let oz = if rng.chance(1, 2) { 1 } else { -1 };
    masses.push(KeepMass {
        spec: TowerSpec {
            cx: px + ox * (main_r + 5),
            cz: pz + oz * 3,
            radius: 6,
            height: 16,
            shape: TowerShape::Square,
            roof: RoofKind::Gable { along_x: oz == 0 },
            inner: true,
        },
    });
    masses.push(KeepMass {
        spec: TowerSpec {
            cx: px - main_r + 1,
            cz: pz - main_r + 1,
            radius: 3,
            height: 28,
            shape: TowerShape::Round,
            roof: RoofKind::Cone,
            inner: true,
        },
    });
    if archetype == CastleArchetype::Spur || archetype == CastleArchetype::Ridge {
        masses.push(KeepMass {
            spec: TowerSpec {
                cx: px + 2,
                cz: pz + main_r + 4,
                radius: 5,
                height: 12,
                shape: TowerShape::Octagon,
                roof: RoofKind::Hip,
                inner: true,
            },
        });
    }
    masses
}

fn masses_bounds(masses: &[KeepMass]) -> Option<Rect> {
    let mut rects = masses.iter().map(|m| m.spec.bounds());
    let first = rects.next()?;
    Some(rects.fold(first, |a, b| {
        Rect::new(
            a.min_x.min(b.min_x),
            a.min_z.min(b.min_z),
            a.max_x.max(b.max_x),
            a.max_z.max(b.max_z),
        )
    }))
}

fn place_in_poly(
    verts: &[(i32, i32)],
    keep: Rect,
    w: i32,
    d: i32,
    rng: &mut TownRng,
    within: Rect,
) -> Rect {
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
        let (cx, cz) = clamped.center();
        if clamped.width() >= w - 3
            && clamped.depth() >= d - 3
            && !clamped.intersects(keep)
            && point_in_poly(cx, cz, verts)
        {
            return clamped;
        }
    }
    let ox = rng.gen_range(within.min_x, (within.max_x - w).max(within.min_x + 1));
    let oz = rng.gen_range(within.min_z, (within.max_z - d).max(within.min_z + 1));
    clamp_rect(Rect::new(ox, oz, ox + w - 1, oz + d - 1), within)
}

fn ring_towers(
    verts: &[(i32, i32)],
    inner: bool,
    archetype: CastleArchetype,
    rng: &mut TownRng,
    skip: Option<(i32, i32)>,
) -> Vec<TowerSpec> {
    let mut towers = Vec::new();
    let shapes = match archetype {
        CastleArchetype::Coastal => [
            TowerShape::Octagon,
            TowerShape::DShape { flat: Dir::South },
            TowerShape::Round,
            TowerShape::Square,
        ],
        CastleArchetype::Concentric => [
            TowerShape::Round,
            TowerShape::Round,
            TowerShape::Octagon,
            TowerShape::Square,
        ],
        CastleArchetype::Ridge => [
            TowerShape::Square,
            TowerShape::Round,
            TowerShape::Octagon,
            TowerShape::DShape { flat: Dir::East },
        ],
        CastleArchetype::Spur => [
            TowerShape::DShape { flat: Dir::South },
            TowerShape::Round,
            TowerShape::Square,
            TowerShape::Octagon,
        ],
    };
    let roofs = [
        RoofKind::Cone,
        RoofKind::Crenellated,
        RoofKind::Cone,
        RoofKind::Crenellated,
    ];
    for (i, &(x, z)) in verts.iter().enumerate() {
        if inner && i % 2 == 1 {
            continue;
        }
        if let Some((sx, sz)) = skip
            && (x - sx).abs() + (z - sz).abs() < 10
        {
            continue;
        }
        let shape = shapes[i % shapes.len()];
        let mut roof = roofs[i % roofs.len()];
        if inner && matches!(roof, RoofKind::Crenellated) {
            roof = RoofKind::Cone;
        }
        let radius = if inner {
            3 + rng.gen_range(0, 2)
        } else {
            4 + rng.gen_range(0, 3)
        };
        let height = if inner {
            18 + rng.gen_range(0, 6)
        } else {
            14 + rng.gen_range(0, 6)
        };
        towers.push(TowerSpec {
            cx: x,
            cz: z,
            radius,
            height,
            shape,
            roof,
            inner,
        });
        if !inner && i % 3 == 0 {
            towers.push(TowerSpec {
                cx: x + 2,
                cz: z + 2,
                radius: 2,
                height: height + 4,
                shape: TowerShape::Round,
                roof: RoofKind::Cone,
                inner: false,
            });
        }
    }
    towers
}

fn gate_vertex(verts: &[(i32, i32)], center: (i32, i32), dir: Dir) -> (usize, (i32, i32)) {
    let (tx, tz) = (dir.dx(), dir.dz());
    let mut best_i = 0;
    let mut best_dot = i32::MIN;
    for (i, &(x, z)) in verts.iter().enumerate() {
        let dot = (x - center.0) * tx + (z - center.1) * tz;
        if dot > best_dot {
            best_dot = dot;
            best_i = i;
        }
    }
    (best_i, verts[best_i])
}

fn gate_complex(
    gate: (i32, i32),
    dir: Dir,
    _verts: &[(i32, i32)],
    _gate_i: usize,
) -> (Rect, Rect, TowerSpec, TowerSpec) {
    let perp = match dir {
        Dir::North | Dir::South => (1, 0),
        Dir::East | Dir::West => (0, 1),
    };
    let back = dir.opposite();
    let gatehouse = Rect::new(
        gate.0 - perp.0 * 8 - 5,
        gate.1 - perp.1 * 8 - 5,
        gate.0 + perp.0 * 8 + 5,
        gate.1 + perp.1 * 8 + 5,
    );
    let barbican = Rect::new(
        gate.0 + dir.dx() * 10 - 5,
        gate.1 + dir.dz() * 10 - 5,
        gate.0 + dir.dx() * 10 + 5,
        gate.1 + dir.dz() * 10 + 5,
    );
    let _ = back;
    let flank_a = TowerSpec {
        cx: gate.0 + perp.0 * 6,
        cz: gate.1 + perp.1 * 6,
        radius: 5,
        height: 18,
        shape: TowerShape::Round,
        roof: RoofKind::Cone,
        inner: false,
    };
    let flank_b = TowerSpec {
        cx: gate.0 - perp.0 * 6,
        cz: gate.1 - perp.1 * 6,
        radius: 5,
        height: 18,
        shape: TowerShape::Round,
        roof: RoofKind::Cone,
        inner: false,
    };
    (gatehouse, barbican, flank_a, flank_b)
}

fn barbican_spec(rect: Rect, dir: Dir) -> TowerSpec {
    let (cx, cz) = rect.center();
    TowerSpec {
        cx,
        cz,
        radius: 5,
        height: 10,
        shape: TowerShape::DShape {
            flat: dir.opposite(),
        },
        roof: RoofKind::Crenellated,
        inner: false,
    }
}

fn fill_shape(occ: &mut Occupancy, spec: TowerSpec, kind: CellKind) {
    let b = spec.bounds();
    for z in b.min_z..=b.max_z {
        for x in b.min_x..=b.max_x {
            if spec.contains(x, z) {
                occ.set(x, z, kind, true);
            }
        }
    }
}

fn paint_buttresses(occ: &mut Occupancy, hall: Rect, kind: CellKind) {
    let along_x = hall.width() >= hall.depth();
    if along_x {
        let mut x = hall.min_x + 2;
        while x < hall.max_x - 1 {
            occ.set(x, hall.min_z - 1, kind, true);
            occ.set(x, hall.max_z + 1, kind, true);
            x += 4;
        }
    } else {
        let mut z = hall.min_z + 2;
        while z < hall.max_z - 1 {
            occ.set(hall.min_x - 1, z, kind, true);
            occ.set(hall.max_x + 1, z, kind, true);
            z += 4;
        }
    }
}

fn paint_apse(occ: &mut Occupancy, chapel: Rect, toward: Dir) {
    let (cx, cz) = match toward {
        Dir::South => (chapel.center().0, chapel.max_z),
        Dir::North => (chapel.center().0, chapel.min_z),
        Dir::East => (chapel.max_x, chapel.center().1),
        Dir::West => (chapel.min_x, chapel.center().1),
    };
    for dz in -4..=4 {
        for dx in -4..=4 {
            if dx * dx + dz * dz <= 16 {
                occ.set(cx + dx, cz + dz, CellKind::CastleChapel, true);
            }
        }
    }
}

fn paint_cross_walls(occ: &mut Occupancy, keep: &Rect, gate: (i32, i32), inner: &[(i32, i32)]) {
    let (kx, kz) = keep.center();
    let line = bresenham(kx, kz, gate.0, gate.1);
    occ.stroke(&line, 1, CellKind::CastleInnerWall, false);
    if let Some(&(ix, iz)) = inner.first() {
        occ.stroke(&bresenham(ix, iz, kx, kz), 0, CellKind::CastleCourt, false);
    }
}

fn paint_court_paths(occ: &mut Occupancy, keep: &Rect, hall: &Rect, gate: (i32, i32)) {
    let (kx, kz) = keep.center();
    let (hx, hz) = hall.center();
    for line in [bresenham(gate.0, gate.1, kx, kz), bresenham(kx, kz, hx, hz)] {
        occ.stroke(&line, 1, CellKind::CastleCourt, true);
    }
}

fn paint_approach(occ: &mut Occupancy, gate: (i32, i32), dir: Dir) {
    let perp = match dir {
        Dir::North | Dir::South => Dir::East,
        Dir::East | Dir::West => Dir::South,
    };
    let mut x = gate.0 + dir.dx() * 8;
    let mut z = gate.1 + dir.dz() * 8;
    for step in 0..5 {
        let nx = x + dir.dx() * 8;
        let nz = z + dir.dz() * 8;
        let side = if step % 2 == 0 { perp } else { perp.opposite() };
        let mid = (nx + side.dx() * 10, nz + side.dz() * 10);
        occ.stroke(
            &bresenham(x, z, mid.0, mid.1),
            1,
            CellKind::CastleCourt,
            false,
        );
        occ.stroke(
            &bresenham(mid.0, mid.1, nx, nz),
            1,
            CellKind::CastleCourt,
            false,
        );
        x = nx;
        z = nz;
    }
}

fn paint_moat(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    verts: &[(i32, i32)],
    gate: (i32, i32),
    _dir: Dir,
) {
    let Some(bounds) = poly_bounds(verts) else {
        return;
    };
    let ring = bounds.inflate(5);
    for z in ring.min_z..=ring.max_z {
        for x in ring.min_x..=ring.max_x {
            if point_in_poly(x, z, verts) {
                continue;
            }
            if dist2(x, z, gate.0, gate.1) < 12 * 12 {
                continue;
            }
            if crate::geom::near_poly_edge(x, z, verts, 4)
                && grid.height(x, z) <= grid.sea_level + 8
            {
                occ.set(x, z, CellKind::CastleMoat, false);
            }
        }
    }
}

fn downhill_side(grid: &HeightGrid, bounds: Rect) -> Dir {
    let north = grid.mean(bounds.min_x, bounds.min_z - 24, bounds.max_x, bounds.min_z);
    let south = grid.mean(bounds.min_x, bounds.max_z, bounds.max_x, bounds.max_z + 24);
    let west = grid.mean(bounds.min_x - 24, bounds.min_z, bounds.min_x, bounds.max_z);
    let east = grid.mean(bounds.max_x, bounds.min_z, bounds.max_x + 24, bounds.max_z);
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

/// Whether `(x, z)` lies in a shaped footprint.
#[must_use]
pub fn shape_contains(shape: TowerShape, cx: i32, cz: i32, radius: i32, x: i32, z: i32) -> bool {
    if radius <= 0 {
        return x == cx && z == cz;
    }
    let dx = x - cx;
    let dz = z - cz;
    match shape {
        TowerShape::Square => dx.abs() <= radius && dz.abs() <= radius,
        TowerShape::Round => dx * dx + dz * dz <= radius * radius,
        TowerShape::Octagon => {
            dx.abs() <= radius && dz.abs() <= radius && dx.abs() + dz.abs() <= radius + radius / 2
        }
        TowerShape::DShape { flat } => {
            if dx * dx + dz * dz > radius * radius {
                return false;
            }
            match flat {
                Dir::North => dz >= -1,
                Dir::South => dz <= 1,
                Dir::West => dx >= -1,
                Dir::East => dx <= 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;

    #[test]
    fn wet_search_still_sites_near_origin() {
        let mut grid = HeightGrid::synthetic_slope(0, 400, 800, 1200, 8, 58, 6, 4);
        grid.sea_level = 63;
        let mut rng = TownRng::new(42);
        let rect = site_castle(&grid, 558, 979, &mut rng);
        assert!(
            (rect.center().0 - 558).abs() < 200,
            "castle wandered to {rect:?}"
        );
        assert_eq!(rect.width(), CASTLE_SIZE);
        assert_eq!(rect.depth(), CASTLE_SIZE);
    }

    #[test]
    fn contour_castle_is_not_a_rectangle() {
        let grid = HeightGrid::synthetic_slope(0, 0, 400, 400, 8, 80, 16, 8);
        let mut rng = TownRng::new(7);
        let bounds = Rect::new(40, 40, 183, 183);
        let mut occ = Occupancy::new(bounds.inflate(8));
        let plan = paint_castle(&mut occ, &grid, bounds, &mut rng);
        assert!(
            plan.outer_ring.verts.len() >= 6,
            "outer ring too simple: {:?}",
            plan.outer_ring.verts
        );
        let corners = [
            (bounds.min_x + 2, bounds.min_z + 2),
            (bounds.max_x - 2, bounds.min_z + 2),
            (bounds.max_x - 2, bounds.max_z - 2),
            (bounds.min_x + 2, bounds.max_z - 2),
        ];
        let all_corners = corners.iter().all(|c| {
            plan.outer_ring
                .verts
                .iter()
                .any(|v| (v.0 - c.0).abs() <= 4 && (v.1 - c.1).abs() <= 4)
        });
        assert!(
            !all_corners,
            "outer ring still hugs the AABB corners: {:?}",
            plan.outer_ring.verts
        );
        assert!(
            plan.tower_specs
                .iter()
                .any(|t| !matches!(t.shape, TowerShape::Square)),
            "every tower is still square"
        );
        assert!(
            plan.keep_masses.len() >= 3,
            "keep should be multi-mass, got {}",
            plan.keep_masses.len()
        );
        assert_eq!(plan.bounds.width(), CASTLE_SIZE);
    }
}
