//! Hinterland mountain fortress: ridge-following curtain, keep, mine and camp.
//!
//! Occupancy is a function of seed + height grid. Stamp looks the layout up
//! from the scene rect so each column stays independent.

use std::cell::RefCell;

use crate::buildings::{BuildingKind, Lot};
use crate::castle::{RoofKind, TowerShape, TowerSpec, fill_shape};
use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham, near_poly_edge, point_in_poly};
use crate::height::HeightGrid;
use crate::rng::TownRng;
use crate::scenes::{Scene, SceneKind};

/// Smallest fortress side, matching the live castle rule.
pub const FORTRESS_MIN: i32 = 128;

const CURTAIN_THICK: i32 = 2;
const KEEP_RADIUS: i32 = 8;
const HALL_W: i32 = 16;
const HALL_D: i32 = 22;
const CHAPEL_W: i32 = 11;
const CHAPEL_D: i32 = 16;
const KITCHEN_W: i32 = 9;
const KITCHEN_D: i32 = 11;
const BARRACKS_W: i32 = 12;
const BARRACKS_D: i32 = 14;
const ARMOURY_W: i32 = 10;
const ARMOURY_D: i32 = 12;
const MINE_W: i32 = 28;
const MINE_D: i32 = 22;
const TUNNEL_LEN: i32 = 28;
const SETTLEMENT_GAP: i32 = 16;

/// Planned pads inside a mountain-fortress yard.
#[derive(Clone, Debug)]
pub struct MountainFortressLayout {
    pub yard: Rect,
    pub verts: Vec<(i32, i32)>,
    pub gate_dir: Dir,
    pub gate: (i32, i32),
    pub gatehouse: Rect,
    pub barbican: Rect,
    pub keep: TowerSpec,
    pub keep_turret: TowerSpec,
    pub hall: Rect,
    pub chapel: Rect,
    pub kitchen: Rect,
    pub barracks: Rect,
    pub armoury: Rect,
    pub towers: Vec<TowerSpec>,
    pub flag: (i32, i32),
    pub mine: Rect,
    pub adit: Rect,
    pub head: Rect,
    pub spoil: Rect,
    pub camp: Rect,
    pub smith: Rect,
    pub crane: Rect,
    pub tunnel: Vec<(i32, i32)>,
    pub tunnel_y: i32,
}

/// Which fortress mass occupies this column.
#[derive(Clone, Copy, Debug)]
pub enum FortressFeature {
    Keep,
    Hall,
    Chapel,
    Kitchen,
    Barracks,
    Armoury,
    Tower(TowerSpec),
    Gatehouse,
    Barbican,
    Curtain { inner: bool },
    Court,
    Flag,
    MineAdit,
    MineHead,
    MineRail,
    MineSpoil,
    Camp,
    Smith,
    Crane,
}

impl MountainFortressLayout {
    /// Mass covering `(x, z)`, if any.
    #[must_use]
    pub fn feature_at(&self, x: i32, z: i32) -> Option<FortressFeature> {
        if self.keep.contains(x, z) {
            return Some(FortressFeature::Keep);
        }
        if let Some(&tower) = self.towers.iter().find(|t| t.contains(x, z)) {
            return Some(FortressFeature::Tower(tower));
        }
        if self.barbican.contains(x, z) {
            return Some(FortressFeature::Barbican);
        }
        if self.gatehouse.contains(x, z) {
            return Some(FortressFeature::Gatehouse);
        }
        if self.hall.contains(x, z) {
            return Some(FortressFeature::Hall);
        }
        if self.chapel.contains(x, z) {
            return Some(FortressFeature::Chapel);
        }
        if self.kitchen.contains(x, z) {
            return Some(FortressFeature::Kitchen);
        }
        if self.barracks.contains(x, z) {
            return Some(FortressFeature::Barracks);
        }
        if self.armoury.contains(x, z) {
            return Some(FortressFeature::Armoury);
        }
        if self.flag.0 == x && self.flag.1 == z {
            return Some(FortressFeature::Flag);
        }
        if self.adit.contains(x, z) {
            return Some(FortressFeature::MineAdit);
        }
        if self.head.contains(x, z) {
            return Some(FortressFeature::MineHead);
        }
        if self.spoil.contains(x, z) {
            return Some(FortressFeature::MineSpoil);
        }
        if self.crane.contains(x, z) {
            return Some(FortressFeature::Crane);
        }
        if self.smith.contains(x, z) {
            return Some(FortressFeature::Smith);
        }
        if self.camp.contains(x, z) {
            return Some(FortressFeature::Camp);
        }
        if self.tunnel.iter().any(|&(tx, tz)| tx == x && tz == z) {
            return Some(FortressFeature::MineRail);
        }
        if near_poly_edge(x, z, &self.verts, CURTAIN_THICK) {
            return Some(FortressFeature::Curtain {
                inner: point_in_poly(x, z, &self.verts),
            });
        }
        if point_in_poly(x, z, &self.verts) {
            return Some(FortressFeature::Court);
        }
        None
    }

    /// Whether this column is a carved mine tunnel (walkable from the adit).
    #[must_use]
    pub fn tunnel_at(&self, x: i32, z: i32) -> bool {
        self.tunnel.iter().any(|&(tx, tz)| tx == x && tz == z)
    }
}

/// Geometry of a mountain fortress, a function of yard, heights and seed.
#[must_use]
pub fn mountain_fortress_layout(
    yard: Rect,
    grid: &HeightGrid,
    seed: u64,
) -> MountainFortressLayout {
    cached_layout(yard, grid, seed)
}

fn cached_layout(yard: Rect, grid: &HeightGrid, seed: u64) -> MountainFortressLayout {
    thread_local! {
        static LAST: RefCell<Option<(Rect, u64, MountainFortressLayout)>> =
            const { RefCell::new(None) };
    }
    LAST.with(|slot| {
        if let Some((cached_yard, cached_seed, layout)) = slot.borrow().as_ref()
            && *cached_yard == yard
            && *cached_seed == seed
        {
            return layout.clone();
        }
        let layout = compute_layout(yard, grid, seed);
        *slot.borrow_mut() = Some((yard, seed, layout.clone()));
        layout
    })
}

fn compute_layout(yard: Rect, grid: &HeightGrid, seed: u64) -> MountainFortressLayout {
    let mut rng = TownRng::salted(seed, yard.min_x as u64, yard.min_z as u64);
    let gate_dir = downhill_edge(grid, yard);
    let verts = ridge_contour(grid, yard, gate_dir);
    let (px, pz, peak_h) = grid.peak(
        yard.min_x + 16,
        yard.min_z + 16,
        yard.max_x - 16,
        yard.max_z - 16,
    );
    let keep = TowerSpec {
        cx: px,
        cz: pz,
        radius: KEEP_RADIUS,
        height: 20,
        shape: TowerShape::Octagon,
        roof: RoofKind::Spire,
        inner: true,
    };
    let turret_side = match gate_dir {
        Dir::North | Dir::South => Dir::East,
        Dir::East | Dir::West => Dir::North,
    };
    let keep_turret = TowerSpec {
        cx: px + turret_side.dx() * 14,
        cz: pz + turret_side.dz() * 14,
        radius: 4,
        height: 16,
        shape: TowerShape::Round,
        roof: RoofKind::Cone,
        inner: true,
    };
    let inner = Rect::new(
        yard.min_x + 18,
        yard.min_z + 18,
        yard.max_x - 18,
        yard.max_z - 18,
    );
    let keep_box = spec_pad(keep, 3);
    let turret_box = spec_pad(keep_turret, 2);
    let hall = place_clear(
        px,
        pz,
        gate_dir.opposite(),
        HALL_W,
        HALL_D,
        28,
        inner,
        &[keep_box, turret_box],
    );
    let chapel_dir = match gate_dir {
        Dir::North | Dir::South => Dir::West,
        Dir::East | Dir::West => Dir::North,
    };
    let chapel = place_clear(
        px,
        pz,
        chapel_dir,
        CHAPEL_W,
        CHAPEL_D,
        26,
        inner,
        &[keep_box, turret_box, hall],
    );
    let kitchen = attach_room(
        hall,
        KITCHEN_W,
        KITCHEN_D,
        inner,
        &[keep_box, turret_box, hall, chapel],
    );
    let (gate_i, gate) = downhill_vertex(&verts, yard.center(), gate_dir);
    let (gatehouse, barbican, flank_a, flank_b) = gate_pads(gate, gate_dir);
    let barracks = place_clear(
        gate.0,
        gate.1,
        gate_dir.opposite(),
        BARRACKS_W,
        BARRACKS_D,
        22,
        inner,
        &[
            keep_box, turret_box, hall, chapel, kitchen, gatehouse, barbican,
        ],
    );
    let armoury = attach_room(
        barracks,
        ARMOURY_W,
        ARMOURY_D,
        inner,
        &[
            keep_box, turret_box, hall, chapel, kitchen, barracks, gatehouse, barbican,
        ],
    );
    let mut towers = ring_towers(&verts, &mut rng, Some(gate));
    towers.push(keep_turret);
    towers.push(flank_a);
    towers.push(flank_b);
    let _ = gate_i;
    let flag = flag_pad(keep, hall, inner);
    let mine = mine_yard(yard, gate_dir);
    let (cx, cz) = mine.center();
    let adit = match gate_dir.opposite() {
        Dir::North => Rect::new(cx - 2, mine.min_z, cx + 2, mine.min_z + 3),
        Dir::South => Rect::new(cx - 2, mine.max_z - 3, cx + 2, mine.max_z),
        Dir::West => Rect::new(mine.min_x, cz - 2, mine.min_x + 3, cz + 2),
        Dir::East => Rect::new(mine.max_x - 3, cz - 2, mine.max_x, cz + 2),
    };
    let head = Rect::new(cx - 3, cz - 3, cx + 3, cz + 3);
    let spoil = match gate_dir {
        Dir::North => Rect::new(mine.min_x + 1, mine.min_z, mine.max_x - 1, mine.min_z + 3),
        Dir::South => Rect::new(mine.min_x + 1, mine.max_z - 3, mine.max_x - 1, mine.max_z),
        Dir::West => Rect::new(mine.min_x, mine.min_z + 1, mine.min_x + 3, mine.max_z - 1),
        Dir::East => Rect::new(mine.max_x - 3, mine.min_z + 1, mine.max_x, mine.max_z - 1),
    };
    let camp = match gate_dir {
        Dir::South => Rect::new(
            mine.min_x + 2,
            mine.min_z + 4,
            mine.min_x + 9,
            mine.min_z + 10,
        ),
        Dir::North => Rect::new(
            mine.min_x + 2,
            mine.max_z - 10,
            mine.min_x + 9,
            mine.max_z - 4,
        ),
        Dir::East => Rect::new(
            mine.min_x + 4,
            mine.min_z + 2,
            mine.min_x + 10,
            mine.min_z + 8,
        ),
        Dir::West => Rect::new(
            mine.max_x - 10,
            mine.min_z + 2,
            mine.max_x - 4,
            mine.min_z + 8,
        ),
    };
    let smith = match gate_dir {
        Dir::South => Rect::new(
            mine.max_x - 12,
            mine.min_z + 4,
            mine.max_x - 3,
            mine.min_z + 11,
        ),
        Dir::North => Rect::new(
            mine.max_x - 12,
            mine.max_z - 11,
            mine.max_x - 3,
            mine.max_z - 4,
        ),
        Dir::East => Rect::new(
            mine.min_x + 4,
            mine.max_z - 10,
            mine.min_x + 13,
            mine.max_z - 3,
        ),
        Dir::West => Rect::new(
            mine.max_x - 13,
            mine.max_z - 10,
            mine.max_x - 4,
            mine.max_z - 3,
        ),
    };
    let crane = Rect::new(cx + 4, cz - 1, cx + 10, cz + 1);
    let inward = gate_dir.opposite();
    let (ax, az) = adit.center();
    let mut tunnel = Vec::new();
    for step in 0..=TUNNEL_LEN {
        let x = ax + inward.dx() * step;
        let z = az + inward.dz() * step;
        for o in -1..=1 {
            let (px, pz) = if inward.dx() == 0 {
                (x + o, z)
            } else {
                (x, z + o)
            };
            tunnel.push((px, pz));
        }
        if step > 8 && step % 8 == 0 {
            let side = if step % 16 == 0 { Dir::East } else { Dir::West };
            for branch in 1..=6 {
                tunnel.push((x + side.dx() * branch, z + side.dz() * branch));
            }
        }
    }
    let tunnel_y = grid.height(ax, az) - 1;
    let _ = peak_h;
    MountainFortressLayout {
        yard,
        verts,
        gate_dir,
        gate,
        gatehouse,
        barbican,
        keep,
        keep_turret,
        hall,
        chapel,
        kitchen,
        barracks,
        armoury,
        towers,
        flag,
        mine,
        adit,
        head,
        spoil,
        camp,
        smith,
        crane,
        tunnel,
        tunnel_y,
    }
}

/// Layout for a fortress scene on `plan`, if one was placed.
#[must_use]
pub fn layout_for_plan(
    plan: &crate::plan::TownPlan,
    x: i32,
    z: i32,
) -> Option<MountainFortressLayout> {
    let scene = plan
        .scenes
        .iter()
        .find(|scene| scene.kind == SceneKind::MountainFortress)?;
    if !scene.rect.inflate(48).contains(x, z) {
        return None;
    }
    let layout = mountain_fortress_layout(scene.rect, &plan.heights, plan.seed);
    (scene.rect.contains(x, z)
        || layout.mine.inflate(4).contains(x, z)
        || layout.tunnel_at(x, z)
        || layout.feature_at(x, z).is_some())
    .then_some(layout)
}

/// Places a 128×128 ridge fortress plus a working mine on the downhill face.
pub fn place_mountain_fortress(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    if scenes
        .iter()
        .any(|scene| scene.kind == SceneKind::MountainFortress)
    {
        return;
    }
    let Some(yard) = find_mountain_fortress_site(occ, grid, search, town, keep_out, lots, scenes)
    else {
        return;
    };
    crate::scenes::evict_lots_from(occ, lots, yard.inflate(4));
    reclaim_pad(occ, yard);
    let layout = mountain_fortress_layout(yard, grid, seed);
    crate::scenes::evict_lots_from(occ, lots, layout.mine.inflate(4));
    reclaim_pad(occ, layout.mine);
    scenes.retain(|scene| {
        !matches!(
            scene.kind,
            SceneKind::Farmstead | SceneKind::Hamlet | SceneKind::Orchard | SceneKind::Pasture
        ) || !(scene.rect.intersects(yard.inflate(8))
            || scene.rect.intersects(layout.mine.inflate(4)))
    });
    paint_fortress_occupancy(occ, &layout);
    let facing = layout.gate_dir.opposite();
    let _ = crate::scenes::force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        layout.camp,
        facing,
    );
    let _ = crate::scenes::force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Blacksmith,
        layout.smith,
        facing,
    );
    scenes.push(Scene {
        kind: SceneKind::MountainFortress,
        rect: yard,
    });
    scenes.push(Scene {
        kind: SceneKind::Mine,
        rect: layout.mine.inflate(4),
    });
}

/// 3-wide switchback from the fortress gate down to the town road.
pub fn paint_fortress_switchback(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    scenes: &[Scene],
) {
    let Some(scene) = scenes
        .iter()
        .find(|scene| scene.kind == SceneKind::MountainFortress)
    else {
        return;
    };
    let layout = mountain_fortress_layout(scene.rect, grid, 0);
    let (gx, gz) = layout.gate;
    let (tx, tz) = nearest_town_road(occ, town, gx, gz);
    let mut pts = vec![(gx, gz)];
    let legs = 8;
    for i in 1..=legs {
        let mx = gx + (tx - gx) * i / legs;
        let mz = gz + (tz - gz) * i / legs;
        if i % 2 == 1 {
            pts.push((mx, pts.last().copied().unwrap_or((gx, gz)).1));
        } else {
            pts.push((pts.last().copied().unwrap_or((gx, gz)).0, mz));
        }
        pts.push((mx, mz));
    }
    pts.push((tx, tz));
    let mut painted = Vec::new();
    for window in pts.windows(2) {
        let (ax0, az0) = window[0];
        let (ax1, az1) = window[1];
        let along_x = (ax1 - ax0).abs() >= (az1 - az0).abs();
        for (x, z) in bresenham(ax0, az0, ax1, az1) {
            if !occ.bounds.contains(x, z) {
                continue;
            }
            let extras = if along_x {
                [(0, -1), (0, 0), (0, 1)]
            } else {
                [(-1, 0), (0, 0), (1, 0)]
            };
            for (dx, dz) in extras {
                let px = x + dx;
                let pz = z + dz;
                if !occ.bounds.contains(px, pz) {
                    continue;
                }
                if matches!(occ.kind(px, pz), CellKind::FortressGate) {
                    painted.push((px, pz));
                    continue;
                }
                if !road_cell_ok(occ.kind(px, pz)) {
                    continue;
                }
                let dh = (grid.height(px, pz) - grid.height(ax0, az0)).abs();
                let kind = if dh >= 2 {
                    stair_toward(ax0, az0, ax1, az1)
                } else {
                    CellKind::Road
                };
                occ.set(px, pz, kind, true);
                painted.push((px, pz));
            }
            let shoulders = if along_x {
                [(0, -2), (0, 2)]
            } else {
                [(-2, 0), (2, 0)]
            };
            for (dx, dz) in shoulders {
                let sx = x + dx;
                let sz = z + dz;
                if !occ.bounds.contains(sx, sz) {
                    continue;
                }
                if matches!(
                    occ.kind(sx, sz),
                    CellKind::Empty
                        | CellKind::Green
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Hedge
                        | CellKind::Pasture
                        | CellKind::Orchard
                ) {
                    occ.set(sx, sz, CellKind::Retaining, true);
                }
            }
        }
    }
    for (x, z) in painted {
        let h = grid.height(x, z);
        for dir in Dir::ALL {
            let nx = x + dir.dx();
            let nz = z + dir.dz();
            if !occ.bounds.contains(nx, nz) {
                continue;
            }
            if grid.height(nx, nz) > h {
                continue;
            }
            if matches!(
                occ.kind(nx, nz),
                CellKind::Empty
                    | CellKind::Green
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Pasture
                    | CellKind::Hedge
                    | CellKind::Orchard
            ) {
                occ.set(nx, nz, CellKind::Retaining, true);
            }
        }
    }
}

fn find_mountain_fortress_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &[Lot],
    scenes: &[Scene],
) -> Option<Rect> {
    const PAD: i32 = 36;
    let sizes = [(128, 128), (132, 128), (128, 136), (136, 128)];
    for min_relief in [14, 8, 4, 0] {
        let mut best = None;
        let mut best_score = i32::MIN;
        for &(w, d) in &sizes {
            let mut z = search.min_z + PAD;
            while z + d + PAD < search.max_z {
                let mut x = search.min_x + PAD;
                while x + w + PAD < search.max_x {
                    let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                    x += 12;
                    if town.inflate(24).intersects(rect) || keep_out.inflate(8).intersects(rect) {
                        continue;
                    }
                    if landmark_hits(scenes, rect.inflate(SETTLEMENT_GAP)) {
                        continue;
                    }
                    if crate::scenes::mill_sail_keepout(lots, rect) {
                        continue;
                    }
                    if !fortress_site_ok(occ, grid, rect) {
                        continue;
                    }
                    let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                    let relief = hi - lo;
                    if relief < min_relief {
                        continue;
                    }
                    let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                    let surround = grid.mean(
                        rect.min_x - 24,
                        rect.min_z - 24,
                        rect.max_x + 24,
                        rect.max_z + 24,
                    );
                    let jitter = (rect.min_x.wrapping_mul(0x9E37) ^ rect.min_z.rotate_left(7))
                        .unsigned_abs() as i32
                        % 6;
                    let score = mean * 4 + relief * 3 + (mean - surround) * 2 + jitter;
                    if score > best_score {
                        best_score = score;
                        best = Some(rect);
                    }
                }
                z += 12;
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

fn fortress_site_ok(occ: &Occupancy, grid: &HeightGrid, rect: Rect) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    if !grid.rect_is_dry(rect) {
        return false;
    }
    let (lo, _) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
    if lo <= grid.sea_level + 4 {
        return false;
    }
    let mut n = 0;
    let mut blocked = 0;
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            n += 1;
            if !reclaimable(occ.kind(x, z)) {
                blocked += 1;
            }
        }
    }
    n > 0 && blocked * 8 < n
}

fn reclaimable(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Empty
            | CellKind::Green
            | CellKind::Farm
            | CellKind::FarmEdge
            | CellKind::Orchard
            | CellKind::Pasture
            | CellKind::Hedge
            | CellKind::FieldWall
            | CellKind::Eave
            | CellKind::Road
            | CellKind::RoadStairN
            | CellKind::RoadStairS
            | CellKind::RoadStairE
            | CellKind::RoadStairW
    )
}

fn fill_tower(occ: &mut Occupancy, spec: TowerSpec) {
    let b = spec.bounds();
    for z in b.min_z..=b.max_z {
        for x in b.min_x..=b.max_x {
            if !spec.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::FortressKeep
                    | CellKind::FortressHall
                    | CellKind::FortressChapel
                    | CellKind::FortressKitchen
                    | CellKind::FortressBarracks
                    | CellKind::FortressArmoury
                    | CellKind::FortressTower
            ) {
                continue;
            }
            occ.set(x, z, CellKind::FortressTower, true);
        }
    }
}

fn fill_room(occ: &mut Occupancy, rect: Rect, kind: CellKind) {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if !matches!(
                occ.kind(x, z),
                CellKind::FortressCourt | CellKind::Green | CellKind::Empty
            ) {
                return;
            }
        }
    }
    occ.fill(rect, kind, true);
}

fn reclaim_pad(occ: &mut Occupancy, rect: Rect) {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if reclaimable(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
}

fn landmark_hits(scenes: &[Scene], rect: Rect) -> bool {
    scenes.iter().any(|scene| {
        matches!(
            scene.kind,
            SceneKind::Cathedral
                | SceneKind::CathedralClose
                | SceneKind::CathedralCity
                | SceneKind::Monastery
                | SceneKind::Manor
                | SceneKind::Harbour
                | SceneKind::MountainFortress
                | SceneKind::RuinedCastle
                | SceneKind::PillagerStronghold
                | SceneKind::PillagerCamp
                | SceneKind::Frontier
                | SceneKind::Gorge
        ) && scene.rect.intersects(rect)
    })
}

fn paint_fortress_occupancy(occ: &mut Occupancy, layout: &MountainFortressLayout) {
    occ.fill(layout.yard, CellKind::Green, true);
    occ.fill_poly(&layout.verts, CellKind::FortressCourt, true);
    occ.stroke_poly(&layout.verts, CURTAIN_THICK, CellKind::FortressWall, true);
    fill_shape(occ, layout.keep, CellKind::FortressKeep);
    fill_room(occ, layout.hall, CellKind::FortressHall);
    fill_room(occ, layout.chapel, CellKind::FortressChapel);
    fill_room(occ, layout.kitchen, CellKind::FortressKitchen);
    fill_room(occ, layout.barracks, CellKind::FortressBarracks);
    fill_room(occ, layout.armoury, CellKind::FortressArmoury);
    for spec in &layout.towers {
        fill_tower(occ, *spec);
    }
    occ.fill(layout.gatehouse, CellKind::FortressWall, true);
    occ.fill(layout.barbican, CellKind::FortressBarbican, true);
    let passage = match layout.gate_dir {
        Dir::North | Dir::South => Rect::new(
            layout.gate.0 - 1,
            layout.gate.1 - 6,
            layout.gate.0 + 1,
            layout.gate.1 + 6,
        ),
        Dir::East | Dir::West => Rect::new(
            layout.gate.0 - 6,
            layout.gate.1 - 1,
            layout.gate.0 + 6,
            layout.gate.1 + 1,
        ),
    };
    occ.fill(passage, CellKind::FortressGate, true);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let x = layout.flag.0 + dx;
            let z = layout.flag.1 + dz;
            if matches!(
                occ.kind(x, z),
                CellKind::FortressCourt | CellKind::Green | CellKind::Empty | CellKind::Plaza
            ) {
                occ.set(x, z, CellKind::FlagPlinth, true);
            }
        }
    }
    occ.set(layout.flag.0, layout.flag.1, CellKind::FlagPlinth, true);
    occ.fill(layout.mine, CellKind::Green, true);
    occ.fill(layout.adit, CellKind::MineAdit, true);
    occ.fill(layout.head, CellKind::MineHead, true);
    occ.fill(layout.spoil, CellKind::MineSpoil, true);
    occ.fill(layout.crane, CellKind::Crane, true);
    let (ax, az) = layout.adit.center();
    let (hx, hz) = layout.head.center();
    for (x, z) in bresenham(ax, az, hx, hz) {
        if matches!(
            occ.kind(x, z),
            CellKind::Green | CellKind::MineHead | CellKind::MineAdit | CellKind::Empty
        ) {
            occ.set(x, z, CellKind::MineRail, true);
        }
        let (ox, oz) = if layout.gate_dir.dx() == 0 {
            (x + 1, z)
        } else {
            (x, z + 1)
        };
        if matches!(
            occ.kind(ox, oz),
            CellKind::Green | CellKind::MineHead | CellKind::MineAdit | CellKind::Empty
        ) {
            occ.set(ox, oz, CellKind::MineRail, true);
        }
    }
}

fn ridge_contour(grid: &HeightGrid, yard: Rect, gate_dir: Dir) -> Vec<(i32, i32)> {
    let n: i32 = 8;
    let (cx, cz) = yard.center();
    let (_, _, peak_h) = grid.peak(yard.min_x, yard.min_z, yard.max_x, yard.max_z);
    let target = peak_h - 10;
    let pad = 8;
    let inner = yard.deflate(pad);
    let mut verts = Vec::with_capacity(n as usize);
    for i in 0..n {
        let ang = (i as f64) * std::f64::consts::TAU / f64::from(n);
        let (s, c) = (ang.sin(), ang.cos());
        let mut last = (cx, cz);
        let mut r = 20;
        while r < 72 {
            let x = cx + (r as f64 * c).round() as i32;
            let z = cz + (r as f64 * s).round() as i32;
            if !inner.contains(x, z) {
                break;
            }
            if !grid.is_dry(x, z) {
                break;
            }
            if grid.height(x, z) < target {
                break;
            }
            last = (x, z);
            r += 2;
        }
        last.0 = last.0.clamp(inner.min_x, inner.max_x);
        last.1 = last.1.clamp(inner.min_z, inner.max_z);
        verts.push(last);
    }
    for (x, z) in &mut verts {
        match gate_dir {
            Dir::North | Dir::South => *z += (*z - cz).signum() * 6,
            Dir::East | Dir::West => *x += (*x - cx).signum() * 6,
        }
        *x = (*x).clamp(inner.min_x, inner.max_x);
        *z = (*z).clamp(inner.min_z, inner.max_z);
    }
    verts
}

fn downhill_edge(grid: &HeightGrid, yard: Rect) -> Dir {
    let north = grid.mean(yard.min_x, yard.min_z, yard.max_x, yard.min_z + 4);
    let south = grid.mean(yard.min_x, yard.max_z - 4, yard.max_x, yard.max_z);
    let west = grid.mean(yard.min_x, yard.min_z, yard.min_x + 4, yard.max_z);
    let east = grid.mean(yard.max_x - 4, yard.min_z, yard.max_x, yard.max_z);
    let mut best = Dir::South;
    let mut h = south;
    if north < h {
        best = Dir::North;
        h = north;
    }
    if west < h {
        best = Dir::West;
        h = west;
    }
    if east < h {
        best = Dir::East;
    }
    best
}

fn downhill_vertex(verts: &[(i32, i32)], center: (i32, i32), dir: Dir) -> (usize, (i32, i32)) {
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

fn gate_pads(gate: (i32, i32), dir: Dir) -> (Rect, Rect, TowerSpec, TowerSpec) {
    let perp = match dir {
        Dir::North | Dir::South => (1, 0),
        Dir::East | Dir::West => (0, 1),
    };
    let gatehouse = Rect::new(
        gate.0 - perp.0 * 7 - 4,
        gate.1 - perp.1 * 7 - 4,
        gate.0 + perp.0 * 7 + 4,
        gate.1 + perp.1 * 7 + 4,
    );
    let barbican = Rect::new(
        gate.0 + dir.dx() * 9 - 4,
        gate.1 + dir.dz() * 9 - 4,
        gate.0 + dir.dx() * 9 + 4,
        gate.1 + dir.dz() * 9 + 4,
    );
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
        shape: TowerShape::Square,
        roof: RoofKind::Spire,
        inner: false,
    };
    (gatehouse, barbican, flank_a, flank_b)
}

fn ring_towers(
    verts: &[(i32, i32)],
    rng: &mut TownRng,
    skip: Option<(i32, i32)>,
) -> Vec<TowerSpec> {
    let shapes = [
        TowerShape::Round,
        TowerShape::Square,
        TowerShape::Octagon,
        TowerShape::Round,
        TowerShape::Square,
        TowerShape::Octagon,
        TowerShape::Round,
        TowerShape::Square,
    ];
    let roofs = [
        RoofKind::Cone,
        RoofKind::Spire,
        RoofKind::Hip,
        RoofKind::Crenellated,
        RoofKind::Cone,
        RoofKind::Spire,
        RoofKind::Hip,
        RoofKind::Crenellated,
    ];
    let mut towers = Vec::new();
    for (i, &(x, z)) in verts.iter().enumerate() {
        if let Some((sx, sz)) = skip
            && (x - sx).abs() + (z - sz).abs() < 12
        {
            continue;
        }
        towers.push(TowerSpec {
            cx: x,
            cz: z,
            radius: 4 + rng.gen_range(0, 2),
            height: 14 + rng.gen_range(0, 6),
            shape: shapes[i % shapes.len()],
            roof: roofs[i % roofs.len()],
            inner: false,
        });
    }
    towers
}

fn spec_pad(spec: TowerSpec, extra: i32) -> Rect {
    Rect::new(
        spec.cx - spec.radius - extra,
        spec.cz - spec.radius - extra,
        spec.cx + spec.radius + extra,
        spec.cz + spec.radius + extra,
    )
}

fn clear_of(rect: Rect, blocked: &[Rect]) -> bool {
    blocked.iter().all(|other| !rect.intersects(*other))
}

fn place_clear(
    cx: i32,
    cz: i32,
    prefer: Dir,
    w: i32,
    d: i32,
    dist: i32,
    inner: Rect,
    blocked: &[Rect],
) -> Rect {
    for dir in [prefer, Dir::North, Dir::South, Dir::East, Dir::West] {
        for extra in [0, 4, 8, 12, 16, -4] {
            let rect = clamp_room(offset_rect(cx, cz, dir, w, d, dist + extra), inner);
            if clear_of(rect, blocked) {
                return rect;
            }
        }
    }
    clamp_room(offset_rect(cx, cz, prefer, w, d, dist), inner)
}

fn attach_room(host: Rect, w: i32, d: i32, inner: Rect, blocked: &[Rect]) -> Rect {
    let candidates = [
        Rect::new(
            host.min_x - w - 1,
            host.min_z,
            host.min_x - 2,
            host.min_z + d - 1,
        ),
        Rect::new(
            host.max_x + 2,
            host.min_z,
            host.max_x + 1 + w,
            host.min_z + d - 1,
        ),
        Rect::new(
            host.min_x,
            host.min_z - d - 1,
            host.min_x + w - 1,
            host.min_z - 2,
        ),
        Rect::new(
            host.min_x,
            host.max_z + 2,
            host.min_x + w - 1,
            host.max_z + 1 + d,
        ),
    ];
    for candidate in candidates {
        let rect = clamp_room(candidate, inner);
        if clear_of(rect, blocked) {
            return rect;
        }
    }
    clamp_room(candidates[0], inner)
}

fn offset_rect(cx: i32, cz: i32, dir: Dir, w: i32, d: i32, dist: i32) -> Rect {
    let x0 = cx + dir.dx() * dist - w / 2;
    let z0 = cz + dir.dz() * dist - d / 2;
    Rect::new(x0, z0, x0 + w - 1, z0 + d - 1)
}

fn clamp_room(rect: Rect, within: Rect) -> Rect {
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

fn flag_pad(keep: TowerSpec, hall: Rect, inner: Rect) -> (i32, i32) {
    let (kx, kz) = (keep.cx, keep.cz);
    let (hx, hz) = hall.center();
    let fx = ((kx + hx) / 2).clamp(inner.min_x + 2, inner.max_x - 2);
    let fz = ((kz + hz) / 2).clamp(inner.min_z + 2, inner.max_z - 2);
    if keep.contains(fx, fz) {
        (hx, hz.clamp(inner.min_z + 2, inner.max_z - 2))
    } else {
        (fx, fz)
    }
}

fn mine_yard(yard: Rect, gate_dir: Dir) -> Rect {
    match gate_dir {
        Dir::South => Rect::new(
            yard.min_x + 8,
            yard.max_z - 2,
            yard.min_x + 8 + MINE_W - 1,
            yard.max_z - 2 + MINE_D - 1,
        ),
        Dir::North => Rect::new(
            yard.min_x + 8,
            yard.min_z - MINE_D + 3,
            yard.min_x + 8 + MINE_W - 1,
            yard.min_z + 2,
        ),
        Dir::East => Rect::new(
            yard.max_x - 2,
            yard.min_z + 8,
            yard.max_x - 2 + MINE_W - 1,
            yard.min_z + 8 + MINE_D - 1,
        ),
        Dir::West => Rect::new(
            yard.min_x - MINE_W + 3,
            yard.min_z + 8,
            yard.min_x + 2,
            yard.min_z + 8 + MINE_D - 1,
        ),
    }
}

fn nearest_town_road(occ: &Occupancy, town: Rect, gx: i32, gz: i32) -> (i32, i32) {
    let mut best = town.center();
    let mut best_d = i32::MAX;
    let mut z = town.min_z;
    while z <= town.max_z {
        let mut x = town.min_x;
        while x <= town.max_x {
            if occ.kind(x, z).is_walkable_road() {
                let d = (x - gx).abs() + (z - gz).abs();
                if d < best_d {
                    best_d = d;
                    best = (x, z);
                }
            }
            x += 3;
        }
        z += 3;
    }
    best
}

fn road_cell_ok(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Empty
            | CellKind::Green
            | CellKind::Farm
            | CellKind::FarmEdge
            | CellKind::Hedge
            | CellKind::Pasture
            | CellKind::Orchard
            | CellKind::Road
            | CellKind::RoadStairN
            | CellKind::RoadStairS
            | CellKind::RoadStairE
            | CellKind::RoadStairW
            | CellKind::FortressCourt
            | CellKind::FortressGate
    )
}

fn stair_toward(x0: i32, z0: i32, x1: i32, z1: i32) -> CellKind {
    if (x1 - x0).abs() >= (z1 - z0).abs() {
        if x1 >= x0 {
            CellKind::RoadStairE
        } else {
            CellKind::RoadStairW
        }
    } else if z1 >= z0 {
        CellKind::RoadStairS
    } else {
        CellKind::RoadStairN
    }
}
