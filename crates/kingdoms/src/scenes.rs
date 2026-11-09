//! Hinterland scenes around a castle town: farms, hamlets, camps, ruins.
//!
//! Placement is a pure function of seed + cell through the same RNG stream
//! the planner already uses. Each scene is an axis-aligned rect of occupancy
//! cells; buildings are extra [`Lot`]s so the stamper and watertight check
//! reuse the existing house / church paths.

use crate::buildings::{BuildingKind, Lot};
use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham};
use crate::height::HeightGrid;
use crate::palette::PaletteKind;
use crate::rng::TownRng;
use crate::town::TownPos;

/// Extra blocks around every scene where feature trees and floating plants
/// are stripped. Occupancy Green/Harbor fill plus stamp clear.
pub const SCENE_YARD_MARGIN: i32 = 6;

/// Keep feature trees this far from a roofed lot so trunks do not punch eaves.
pub const BUILDING_TREE_MARGIN: i32 = 4;

/// Cog hull along the keel. Beam is 5.
const COG_LEN: i32 = 15;
const COG_BEAM: i32 = 5;

/// What a hinterland rect is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SceneKind {
    Hamlet,
    Farmstead,
    Orchard,
    Pasture,
    Watchtower,
    Ruin,
    Monastery,
    PillagerCamp,
    Graveyard,
    Harbour,
    Plaza,
    Cathedral,
    MountainFortress,
    Mine,
    Tourney,
    /// Siege-scarred border town between Alder and Raven.
    Frontier,
    /// Riverside hamlet with a stone bridge, mills and docks.
    Riverside,
}

impl SceneKind {
    /// Short label for PNG titles.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hamlet => "Hamlet",
            Self::Farmstead => "Farmstead",
            Self::Orchard => "Orchard",
            Self::Pasture => "Pasture",
            Self::Watchtower => "Watchtower",
            Self::Ruin => "Ruined village",
            Self::Monastery => "Monastery",
            Self::PillagerCamp => "Pillager camp",
            Self::Graveyard => "Graveyard",
            Self::Harbour => "Harbour",
            Self::Plaza => "Plaza",
            Self::Cathedral => "Cathedral",
            Self::MountainFortress => "Mountain fortress",
            Self::Mine => "Mine",
            Self::Tourney => "Jousting lists",
            Self::Frontier => "Frontier town",
            Self::Riverside => "Riverside town",
        }
    }
}

/// One placed scene. Geometry only; AI reads spawn points from [`crate::info::TownInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Scene {
    pub kind: SceneKind,
    pub rect: Rect,
}

/// Paints hinterland occupancy and extra lots. Call after town lots and the
/// first farm ring, before water-use remapping.
pub fn place_scenes(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    castle: Rect,
    town: Rect,
    origin: TownPos,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    palette: PaletteKind,
) -> Vec<Scene> {
    let mut scenes = Vec::new();
    let search = occ.bounds;
    let keep_out = castle.inflate(10);

    place_farmsteads(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        rng,
        &mut scenes,
    );
    place_orchard_and_pasture(occ, grid, search, town, keep_out, rng, &mut scenes);
    place_hamlet(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        rng,
        palette,
        &mut scenes,
    );
    place_watchtower(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        rng,
        &mut scenes,
    );
    place_ruin(occ, grid, search, town, keep_out, rng, &mut scenes);
    place_monastery(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        rng,
        &mut scenes,
    );
    place_graveyard(occ, grid, town, lots, rng, &mut scenes);
    place_pillager_camp(
        occ,
        grid,
        castle,
        town,
        search,
        lots,
        seed,
        rng,
        &mut scenes,
    );
    place_hero(occ, grid, castle, town, search, rng, &mut scenes);
    place_mine(occ, grid, search, town, keep_out, rng, &mut scenes);
    place_tourney(occ, grid, castle, town, search, rng, &mut scenes);
    place_frontier(occ, grid, search, town, keep_out, origin, rng, &mut scenes);
    place_riverside(occ, grid, search, town, keep_out, lots, seed, &mut scenes);
    place_edge_mills(occ, grid, search, town, keep_out, lots, seed, rng);
    paint_wind_sails(occ, lots);
    paint_forge_yards(occ, lots);
    paint_monastery_switchback(occ, grid, town, &scenes);
    scenes
}

/// Piers, cog ships, warehouses and a lighthouse after wet cells are remapped.
pub fn paint_harbour_town(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    _rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let mut docks = Vec::new();
    let mut harbors = Vec::new();
    for z in search.min_z..=search.max_z {
        for x in search.min_x..=search.max_x {
            match occ.kind(x, z) {
                CellKind::Dock => docks.push((x, z)),
                CellKind::Harbor => harbors.push((x, z)),
                _ => {}
            }
        }
    }
    if docks.is_empty() {
        for &(x, z) in &harbors {
            if Dir::ALL
                .iter()
                .any(|dir| grid.is_dry(x + dir.dx(), z + dir.dz()))
            {
                occ.set(x, z, CellKind::Dock, true);
                docks.push((x, z));
                if docks.len() >= 8 {
                    break;
                }
            }
        }
    }
    docks.retain(|&(x, z)| berth_on_large_water(occ, grid, search, x, z));
    harbors.retain(|&(x, z)| water_body_ok(grid, search, x, z));
    if docks.is_empty() {
        return;
    }
    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_z = i32::MIN;
    for &(x, z) in &docks {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x);
        max_z = max_z.max(z);
    }
    paint_quay_and_cranes(occ, grid, search, &docks);
    paint_breakwater(occ, grid, search, &docks);
    for (i, &(x, z)) in docks.iter().enumerate() {
        if i % 4 != 0 {
            continue;
        }
        let Some(dir) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        for step in 1..=12 {
            let px = x + dir.dx() * step;
            let pz = z + dir.dz() * step;
            if !search.contains(px, pz) {
                break;
            }
            if wet_berth(occ, grid, px, pz) {
                occ.set(px, pz, CellKind::Dock, true);
            }
        }
    }
    let _ = place_harbour_cog(occ, grid, search, &docks);
    let mut boats = 0;
    for &(x, z) in &docks {
        if boats >= 3 {
            break;
        }
        let Some(dir) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        let side = match dir {
            Dir::North | Dir::South => Dir::East,
            Dir::East | Dir::West => Dir::South,
        };
        let bx = x + dir.dx() * 4 + side.dx() * 2;
        let bz = z + dir.dz() * 4 + side.dz() * 2;
        if place_hull(occ, grid, search, bx, bz, dir, false) {
            boats += 1;
        }
    }
    if boats < 3 {
        for &(x, z) in &harbors {
            if boats >= 3 {
                break;
            }
            if let Some(dir) = harbour_out(occ, grid, x, z)
                && place_hull(occ, grid, search, x, z, dir, false)
            {
                boats += 1;
            }
        }
    }
    let mut warehouses = 0;
    let mut x = min_x - 2;
    while x <= max_x + 2 && warehouses < 3 {
        let mut z = min_z - 2;
        while z <= max_z + 2 && warehouses < 3 {
            let rect = Rect::new(x, z, x + 9, z + 7);
            z += 10;
            if town.intersects(rect) {
                continue;
            }
            if try_lot(occ, grid, lots, seed, BuildingKind::Barn, rect, Dir::South).is_some() {
                warehouses += 1;
            }
        }
        x += 12;
    }
    let light = Rect::new(max_x + 2, min_z - 8, max_x + 8, min_z - 2);
    let _ = try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Watchtower,
        light,
        Dir::South,
    )
    .or_else(|| {
        try_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Watchtower,
            Rect::new(min_x - 8, max_z + 2, min_x - 2, max_z + 8),
            Dir::South,
        )
    });
    for z in search.min_z..=search.max_z {
        for x in search.min_x..=search.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Dock
                    | CellKind::Breakwater
            ) {
                min_x = min_x.min(x);
                min_z = min_z.min(z);
                max_x = max_x.max(x);
                max_z = max_z.max(z);
            }
        }
    }
    scenes.push(Scene {
        kind: SceneKind::Harbour,
        rect: harbour_focus_rect(occ, Rect::new(min_x, min_z, max_x, max_z)),
    });
}

fn harbour_focus_rect(occ: &Occupancy, docks: Rect) -> Rect {
    let mut hull: Option<Rect> = None;
    for z in docks.min_z..=docks.max_z {
        for x in docks.min_x..=docks.max_x {
            if occ.kind(x, z) != CellKind::Ship {
                continue;
            }
            hull = Some(hull.map_or_else(
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
    let core = hull.map_or(docks, |h| {
        Rect::new(
            h.min_x.min(docks.min_x),
            h.min_z.min(docks.min_z),
            h.max_x.max(docks.max_x),
            h.max_z.max(docks.max_z),
        )
    });
    let mut rect = hull.map_or(core.inflate(8), |h| h.inflate(16));
    // Keep the pier in frame without swallowing the whole coast forest.
    if rect.width() > 56 {
        let cx = hull.map_or(rect.center().0, |h| h.center().0);
        rect = Rect::new(cx - 28, rect.min_z, cx + 27, rect.max_z);
    }
    if rect.depth() > 56 {
        let cz = hull.map_or(rect.center().1, |h| h.center().1);
        rect = Rect::new(rect.min_x, cz - 28, rect.max_x, cz + 27);
    }
    rect
}

/// Remap leftover Empty around lots, the market crop and the walled town so
/// Features trees cannot sit against roofs.
pub fn paint_tree_keepout(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &[Lot],
    market: Rect,
    town: Rect,
) {
    let mut pads = vec![market.inflate(16), town];
    for lot in lots {
        if matches!(lot.kind, BuildingKind::Stall) {
            continue;
        }
        pads.push(lot.rect.inflate(BUILDING_TREE_MARGIN));
    }
    for pad in pads {
        for z in pad.min_z..=pad.max_z {
            for x in pad.min_x..=pad.max_x {
                if !occ.bounds.contains(x, z) {
                    continue;
                }
                let kind = occ.kind(x, z);
                if !grid.is_dry(x, z) {
                    if matches!(
                        kind,
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Farm
                            | CellKind::FarmEdge
                            | CellKind::Orchard
                            | CellKind::Pasture
                    ) {
                        occ.set(x, z, CellKind::Harbor, true);
                    }
                    continue;
                }
                if kind == CellKind::Empty {
                    occ.set(x, z, CellKind::Green, false);
                }
            }
        }
    }
}

/// Remap leftover Empty columns in every scene yard so Features trees and
/// lily pads cannot survive the stamper.
pub fn paint_scene_yards(occ: &mut Occupancy, grid: &HeightGrid, scenes: &[Scene]) {
    for scene in scenes {
        let pad = scene.rect.inflate(SCENE_YARD_MARGIN);
        for z in pad.min_z..=pad.max_z {
            for x in pad.min_x..=pad.max_x {
                if !occ.bounds.contains(x, z) {
                    continue;
                }
                let kind = occ.kind(x, z);
                if !grid.is_dry(x, z) {
                    if matches!(
                        kind,
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Farm
                            | CellKind::FarmEdge
                            | CellKind::Orchard
                            | CellKind::Pasture
                            | CellKind::Plaza
                            | CellKind::Market
                            | CellKind::Hedge
                            | CellKind::Eave
                    ) {
                        occ.set(x, z, CellKind::Harbor, true);
                    }
                    continue;
                }
                if kind != CellKind::Empty {
                    continue;
                }
                let fill = if scene.kind == SceneKind::Plaza {
                    CellKind::Plaza
                } else {
                    CellKind::Green
                };
                occ.set(x, z, fill, false);
            }
        }
    }
}

fn harbour_out(occ: &Occupancy, grid: &HeightGrid, x: i32, z: i32) -> Option<Dir> {
    Dir::ALL
        .iter()
        .copied()
        .find(|dir| wet_berth(occ, grid, x + dir.dx(), z + dir.dz()))
}

fn wet_berth(occ: &Occupancy, grid: &HeightGrid, x: i32, z: i32) -> bool {
    !grid.is_dry(x, z)
        && matches!(
            occ.kind(x, z),
            CellKind::Harbor
                | CellKind::Dock
                | CellKind::Empty
                | CellKind::Quay
                | CellKind::Green
                | CellKind::Breakwater
        )
}

fn berth_on_large_water(occ: &Occupancy, grid: &HeightGrid, search: Rect, x: i32, z: i32) -> bool {
    harbour_out(occ, grid, x, z)
        .is_some_and(|dir| water_body_ok(grid, search, x + dir.dx(), z + dir.dz()))
}

/// Coast (wet flood hits the hinterland edge) or a lake at least 40×40 with
/// depth ≥ 3. Small inland pools (seed 42) fail this.
pub(crate) fn water_body_ok(grid: &HeightGrid, search: Rect, x: i32, z: i32) -> bool {
    const CAP: usize = 2_400;
    if grid.is_dry(x, z) {
        return false;
    }
    let mut seen = std::collections::HashSet::new();
    let mut q = std::collections::VecDeque::new();
    q.push_back((x, z));
    seen.insert((x, z));
    let mut min_x = x;
    let mut max_x = x;
    let mut min_z = z;
    let mut max_z = z;
    let mut deep = 0i32;
    while let Some((cx, cz)) = q.pop_front() {
        min_x = min_x.min(cx);
        max_x = max_x.max(cx);
        min_z = min_z.min(cz);
        max_z = max_z.max(cz);
        if grid.water_depth(cx, cz) >= 3 {
            deep += 1;
        }
        if seen.len() >= CAP {
            break;
        }
        for dir in Dir::ALL {
            let nx = cx + dir.dx();
            let nz = cz + dir.dz();
            if !search.contains(nx, nz) {
                continue;
            }
            if grid.is_dry(nx, nz) || !seen.insert((nx, nz)) {
                continue;
            }
            q.push_back((nx, nz));
        }
    }
    if deep < 3 {
        return false;
    }
    // Ocean / open coast fills the flood cap. Inland water must be a real
    // lake (40×40 and well filled), not a stream or a farm pond.
    if seen.len() >= CAP {
        return true;
    }
    let width = max_x - min_x + 1;
    let depth = max_z - min_z + 1;
    width >= 40 && depth >= 40 && seen.len() >= 800
}

fn hull_rect(sx: i32, sz: i32, dir: Dir, len: i32, beam: i32) -> Rect {
    let along_x = dir == Dir::East || dir == Dir::West;
    let hl = (len - 1) / 2;
    let hb = (beam - 1) / 2;
    if along_x {
        Rect::new(sx - hl, sz - hb, sx + hl, sz + hb)
    } else {
        Rect::new(sx - hb, sz - hl, sx + hb, sz + hl)
    }
}

fn hull_cells_ok(occ: &Occupancy, grid: &HeightGrid, search: Rect, hull: Rect) -> bool {
    let (hx, hz) = hull.center();
    if !water_body_ok(grid, search, hx, hz) {
        return false;
    }
    let mut deep = 0;
    for z in hull.min_z..=hull.max_z {
        for x in hull.min_x..=hull.max_x {
            if !search.contains(x, z) {
                return false;
            }
            if grid.is_dry(x, z) {
                return false;
            }
            if grid.water_depth(x, z) >= 3 {
                deep += 1;
            }
            if !matches!(
                occ.kind(x, z),
                CellKind::Harbor
                    | CellKind::Dock
                    | CellKind::Empty
                    | CellKind::Quay
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Green
                    | CellKind::Breakwater
            ) {
                return false;
            }
        }
    }
    deep >= 3
}

fn place_hull(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    sx: i32,
    sz: i32,
    dir: Dir,
    cog: bool,
) -> bool {
    let hull = if cog {
        hull_rect(sx, sz, dir, COG_LEN, COG_BEAM)
    } else {
        hull_rect(sx, sz, dir, 5, 3)
    };
    if !hull_cells_ok(occ, grid, search, hull) {
        return false;
    }
    if cog {
        occ.fill(hull, CellKind::Ship, true);
    } else {
        occ.fill(hull, CellKind::Boat, true);
    }
    true
}

fn place_harbour_cog(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    docks: &[(i32, i32)],
) -> bool {
    let mut best: Option<(Rect, i32)> = None;
    for &(x, z) in docks {
        let Some(dir) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        let side = match dir {
            Dir::North | Dir::South => Dir::East,
            Dir::East | Dir::West => Dir::South,
        };
        for step in 5..=14 {
            for offset in [0, 2, -2, 3, -3, 4, -4] {
                let sx = x + dir.dx() * step + side.dx() * offset;
                let sz = z + dir.dz() * step + side.dz() * offset;
                for &(len, beam) in &[(COG_LEN, COG_BEAM), (COG_LEN, 3)] {
                    let hull = hull_rect(sx, sz, dir, len, beam);
                    if !hull_cells_ok(occ, grid, search, hull) {
                        continue;
                    }
                    let mut score = 0;
                    for hz in hull.min_z..=hull.max_z {
                        for hx in hull.min_x..=hull.max_x {
                            score += grid.water_depth(hx, hz);
                        }
                    }
                    score += hull.width() * hull.depth();
                    if best.is_none_or(|(_, s)| score > s) {
                        best = Some((hull, score));
                    }
                }
            }
        }
    }
    if let Some((hull, _)) = best {
        occ.fill(hull, CellKind::Ship, true);
        return true;
    }
    force_harbour_cog(occ, grid, search, docks)
}

fn force_harbour_cog(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    docks: &[(i32, i32)],
) -> bool {
    for &(x, z) in docks {
        let Some(dir) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        let side = match dir {
            Dir::North | Dir::South => Dir::East,
            Dir::East | Dir::West => Dir::South,
        };
        let sx = x + dir.dx() * 9 + side.dx() * 3;
        let sz = z + dir.dz() * 9 + side.dz() * 3;
        let hull = hull_rect(sx, sz, dir, COG_LEN, COG_BEAM);
        let mut wet = 0;
        let mut cells = 0;
        for hz in hull.min_z..=hull.max_z {
            for hx in hull.min_x..=hull.max_x {
                if !search.contains(hx, hz) {
                    continue;
                }
                cells += 1;
                if !grid.is_dry(hx, hz) {
                    wet += 1;
                    if matches!(
                        occ.kind(hx, hz),
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Harbor
                            | CellKind::Dock
                            | CellKind::Quay
                            | CellKind::Boat
                    ) {
                        occ.set(hx, hz, CellKind::Harbor, true);
                    }
                }
            }
        }
        if wet >= (cells * 3) / 4 && wet >= 30 && hull_cells_ok(occ, grid, search, hull) {
            occ.fill(hull, CellKind::Ship, true);
            return true;
        }
    }
    false
}

fn paint_quay_and_cranes(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    docks: &[(i32, i32)],
) {
    let mut quay = Vec::new();
    for &(x, z) in docks {
        for dir in Dir::ALL {
            for step in 1..=6 {
                let nx = x + dir.dx() * step;
                let nz = z + dir.dz() * step;
                if !search.contains(nx, nz) {
                    continue;
                }
                if grid.is_dry(nx, nz)
                    && matches!(
                        occ.kind(nx, nz),
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Farm
                            | CellKind::Plaza
                            | CellKind::FarmEdge
                            | CellKind::Hedge
                            | CellKind::Eave
                            | CellKind::Road
                            | CellKind::Market
                    )
                {
                    occ.set(nx, nz, CellKind::Quay, true);
                    quay.push((nx, nz));
                    for side in Dir::ALL {
                        let sx = nx + side.dx();
                        let sz = nz + side.dz();
                        if search.contains(sx, sz)
                            && grid.is_dry(sx, sz)
                            && matches!(
                                occ.kind(sx, sz),
                                CellKind::Empty | CellKind::Green | CellKind::Farm
                            )
                        {
                            occ.set(sx, sz, CellKind::Quay, true);
                            quay.push((sx, sz));
                        }
                    }
                }
            }
        }
    }
    let mut cranes = 0;
    for &(x, z) in &quay {
        if cranes >= 1 {
            break;
        }
        if (x + z).rem_euclid(9) != 0 && cranes > 0 {
            continue;
        }
        let Some(out) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let pz = z + dz;
                if search.contains(px, pz) && grid.is_dry(px, pz) {
                    occ.set(px, pz, CellKind::Crane, true);
                }
            }
        }
        for boom in 1..=4 {
            let bx = x + out.dx() * boom;
            let bz = z + out.dz() * boom;
            if search.contains(bx, bz) {
                occ.set(bx, bz, CellKind::Crane, true);
            }
        }
        cranes += 1;
    }
    if cranes == 0
        && let Some(&(x, z)) = quay.first()
    {
        occ.set(x, z, CellKind::Crane, true);
        for dz in -1..=1 {
            for dx in -1..=1 {
                if search.contains(x + dx, z + dz) {
                    occ.set(x + dx, z + dz, CellKind::Crane, true);
                }
            }
        }
    }
}

fn paint_breakwater(occ: &mut Occupancy, grid: &HeightGrid, search: Rect, docks: &[(i32, i32)]) {
    let Some(&(x, z)) = docks.first() else {
        return;
    };
    let Some(out) = harbour_out(occ, grid, x, z) else {
        return;
    };
    let along = match out {
        Dir::North | Dir::South => Dir::East,
        Dir::East | Dir::West => Dir::South,
    };
    let ox = x + out.dx() * 10;
    let oz = z + out.dz() * 10;
    for t in -10..=10 {
        for thick in 0..=1 {
            let bx = ox + along.dx() * t + out.dx() * thick;
            let bz = oz + along.dz() * t + out.dz() * thick;
            if !search.contains(bx, bz) || grid.is_dry(bx, bz) {
                continue;
            }
            if matches!(
                occ.kind(bx, bz),
                CellKind::Empty
                    | CellKind::Harbor
                    | CellKind::Green
                    | CellKind::Dock
                    | CellKind::Boat
            ) {
                occ.set(bx, bz, CellKind::Breakwater, true);
            }
        }
    }
}

fn place_frontier(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    origin: TownPos,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let dir = frontier_dir(origin);
    let Some(yard) = find_frontier_site(occ, grid, search, town, keep_out, dir, rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 28, 22, false, rng))
    else {
        return;
    };
    occ.fill(yard.inflate(SCENE_YARD_MARGIN), CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if !yard.on_edge(x, z) {
                continue;
            }
            let mix = (x.wrapping_mul(13) + z.wrapping_mul(7)).unsigned_abs();
            let corner = yard.is_corner(x, z);
            if mix % 5 == 0 {
                occ.set(x, z, CellKind::Ruin, true);
                continue;
            }
            occ.set(
                x,
                z,
                if corner {
                    CellKind::TownTower
                } else if mix % 7 == 0 {
                    CellKind::Ruin
                } else {
                    CellKind::TownWall
                },
                true,
            );
        }
    }
    let houses = [
        Rect::new(
            yard.min_x + 3,
            yard.min_z + 3,
            yard.min_x + 9,
            yard.min_z + 8,
        ),
        Rect::new(
            yard.min_x + 12,
            yard.min_z + 4,
            yard.min_x + 18,
            yard.min_z + 9,
        ),
        Rect::new(
            yard.min_x + 4,
            yard.max_z - 9,
            yard.min_x + 11,
            yard.max_z - 4,
        ),
        Rect::new(
            yard.max_x - 10,
            yard.max_z - 10,
            yard.max_x - 4,
            yard.max_z - 4,
        ),
        Rect::new(
            yard.max_x - 12,
            yard.min_z + 11,
            yard.max_x - 5,
            yard.min_z + 16,
        ),
    ];
    for house in houses {
        if yard.contains(house.min_x, house.min_z) && yard.contains(house.max_x, house.max_z) {
            occ.fill(house, CellKind::Burnt, true);
        }
    }
    let (cx, cz) = yard.center();
    let trenches = [
        Rect::new(
            yard.min_x - 5,
            yard.min_z + 4,
            yard.min_x - 2,
            yard.max_z - 4,
        ),
        Rect::new(
            yard.max_x + 2,
            yard.min_z + 4,
            yard.max_x + 5,
            yard.max_z - 4,
        ),
        Rect::new(
            yard.min_x + 4,
            yard.min_z - 5,
            yard.max_x - 4,
            yard.min_z - 2,
        ),
        Rect::new(cx - 3, cz + 1, cx + 3, cz + 3),
    ];
    for trench in trenches {
        fill_green(occ, trench, CellKind::SiegeTrench);
    }
    let pits = [
        Rect::new(
            yard.min_x - 8,
            yard.min_z + 6,
            yard.min_x - 6,
            yard.min_z + 10,
        ),
        Rect::new(
            yard.max_x + 6,
            yard.max_z - 12,
            yard.max_x + 8,
            yard.max_z - 8,
        ),
    ];
    for pit in pits {
        fill_green(occ, pit, CellKind::SiegeTreb);
    }
    occ.set(cx, cz, CellKind::Fountain, true);
    scenes.push(Scene {
        kind: SceneKind::Frontier,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn frontier_dir(origin: TownPos) -> Dir {
    // Cardinal neighbours are always the other kingdom. Face east from
    // Alder (even cell sum) and west from Raven so the scar sits on the
    // border the war clock later marches.
    if origin.cell_x.wrapping_add(origin.cell_z).rem_euclid(2) == 0 {
        Dir::East
    } else {
        Dir::West
    }
}

fn paint_monastery_switchback(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    scenes: &[Scene],
) {
    let Some(abbey) = scenes
        .iter()
        .find(|scene| scene.kind == SceneKind::Monastery)
    else {
        return;
    };
    let (tx, tz) = town.center();
    let (ax, az) = abbey.rect.center();
    let mut pts = vec![(tx, tz)];
    let legs = 6;
    for i in 1..=legs {
        let mx = tx + (ax - tx) * i / legs;
        let mz = tz + (az - tz) * i / legs;
        if i % 2 == 1 {
            pts.push((mx, pts.last().copied().unwrap_or((tx, tz)).1));
        } else {
            pts.push((pts.last().copied().unwrap_or((tx, tz)).0, mz));
        }
        pts.push((mx, mz));
    }
    pts.push((ax, az));
    for window in pts.windows(2) {
        for (x, z) in bresenham(window[0].0, window[0].1, window[1].0, window[1].1) {
            if !occ.bounds.contains(x, z) {
                continue;
            }
            if !matches!(
                occ.kind(x, z),
                CellKind::Empty
                    | CellKind::Green
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Hedge
                    | CellKind::Pasture
            ) {
                continue;
            }
            let dh = (grid.height(x, z) - grid.height(window[0].0, window[0].1)).abs();
            let kind = if dh >= 2 {
                if (x - window[0].0).abs() > (z - window[0].1).abs() {
                    if x > window[0].0 {
                        CellKind::RoadStairE
                    } else {
                        CellKind::RoadStairW
                    }
                } else if z > window[0].1 {
                    CellKind::RoadStairS
                } else {
                    CellKind::RoadStairN
                }
            } else {
                CellKind::Road
            };
            occ.set(x, z, kind, false);
        }
    }
}

fn place_riverside(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    let mut rng = TownRng::salted(seed, 0x5135_51DE, origin_salt(search));
    let Some(yard) = find_riverside_site(occ, grid, search, town, keep_out, &mut rng) else {
        return;
    };
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    let (cx, cz) = yard.center();
    let along_x = wet_span(grid, yard) == Dir::East;
    if along_x {
        for x in yard.min_x..=yard.max_x {
            if !grid.is_dry(x, cz) {
                occ.set(x, cz, CellKind::Bridge, true);
                occ.set(x, cz - 1, CellKind::Bridge, true);
            }
        }
        for z in [cz - 3, cz + 3] {
            for x in (yard.min_x + 2..=yard.max_x - 2).step_by(4) {
                if grid.is_dry(x, z) {
                    occ.set(x, z, CellKind::Dock, true);
                }
            }
        }
    } else {
        for z in yard.min_z..=yard.max_z {
            if !grid.is_dry(cx, z) {
                occ.set(cx, z, CellKind::Bridge, true);
                occ.set(cx - 1, z, CellKind::Bridge, true);
            }
        }
        for x in [cx - 3, cx + 3] {
            for z in (yard.min_z + 2..=yard.max_z - 2).step_by(4) {
                if grid.is_dry(x, z) {
                    occ.set(x, z, CellKind::Dock, true);
                }
            }
        }
    }
    let mill_pads = [
        Rect::new(
            yard.min_x + 2,
            yard.min_z + 2,
            yard.min_x + 6,
            yard.min_z + 6,
        ),
        Rect::new(
            yard.max_x - 6,
            yard.max_z - 6,
            yard.max_x - 2,
            yard.max_z - 2,
        ),
        Rect::new(
            yard.max_x - 6,
            yard.min_z + 2,
            yard.max_x - 2,
            yard.min_z + 6,
        ),
        Rect::new(
            yard.min_x + 2,
            yard.max_z - 6,
            yard.min_x + 6,
            yard.max_z - 2,
        ),
    ];
    let mut mills = 0;
    for pad in mill_pads {
        if mills >= 2 {
            break;
        }
        if force_lot(occ, grid, lots, seed, BuildingKind::Barn, pad, Dir::South).is_some() {
            mills += 1;
        }
    }
    let house_pads = [
        Rect::new(
            yard.min_x + 8,
            yard.min_z + 2,
            yard.min_x + 14,
            yard.min_z + 7,
        ),
        Rect::new(
            yard.max_x - 14,
            yard.max_z - 7,
            yard.max_x - 8,
            yard.max_z - 2,
        ),
    ];
    for house in house_pads {
        if force_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Cottage,
            house,
            Dir::South,
        )
        .is_some()
        {
            break;
        }
    }
    scenes.push(Scene {
        kind: SceneKind::Riverside,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn riverside_reclaim(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Empty
            | CellKind::Green
            | CellKind::Farm
            | CellKind::FarmEdge
            | CellKind::Orchard
            | CellKind::Pasture
            | CellKind::Hedge
            | CellKind::Eave
    )
}

fn riverside_yard_free(occ: &Occupancy, rect: Rect) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if !riverside_reclaim(occ.kind(x, z)) {
                return false;
            }
        }
    }
    true
}

fn origin_salt(search: Rect) -> u64 {
    (search.min_x as u64).wrapping_mul(0x9E37) ^ (search.min_z as u64).rotate_left(11)
}

fn wet_span(grid: &HeightGrid, yard: Rect) -> Dir {
    let mut wet_x = 0;
    let mut wet_z = 0;
    let (cx, cz) = yard.center();
    for x in yard.min_x..=yard.max_x {
        if !grid.is_dry(x, cz) {
            wet_x += 1;
        }
    }
    for z in yard.min_z..=yard.max_z {
        if !grid.is_dry(cx, z) {
            wet_z += 1;
        }
    }
    if wet_x >= wet_z {
        Dir::East
    } else {
        Dir::South
    }
}

fn find_riverside_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 36;
    let d = 48;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + d < search.max_z {
        let mut x = search.min_x + 8;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 12;
            if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !riverside_yard_free(occ, rect) {
                continue;
            }
            let (cx, cz) = rect.center();
            let wet = i32::from(!grid.is_dry(cx, cz))
                + i32::from(!grid.is_dry(rect.min_x + 4, cz))
                + i32::from(!grid.is_dry(rect.max_x - 4, cz))
                + i32::from(!grid.is_dry(cx, rect.min_z + 4))
                + i32::from(!grid.is_dry(cx, rect.max_z - 4));
            let banks = i32::from(grid.is_dry(cx, rect.min_z + 1))
                + i32::from(grid.is_dry(cx, rect.max_z - 1))
                + i32::from(grid.is_dry(rect.min_x + 1, cz))
                + i32::from(grid.is_dry(rect.max_x - 1, cz));
            if wet < 2 || banks < 2 {
                continue;
            }
            let score = wet * 8 + banks * 4 + rng.gen_range(0, 5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 10;
    }
    best
}

fn find_frontier_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    dir: Dir,
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 28;
    let d = 22;
    let strip = match dir {
        Dir::East => Rect::new(
            town.max_x + 8,
            town.min_z - 24,
            search.max_x - 4,
            town.max_z + 24,
        ),
        Dir::West => Rect::new(
            search.min_x + 4,
            town.min_z - 24,
            town.min_x - 8,
            town.max_z + 24,
        ),
        Dir::South => Rect::new(
            town.min_x - 24,
            town.max_z + 8,
            town.max_x + 24,
            search.max_z - 4,
        ),
        Dir::North => Rect::new(
            town.min_x - 24,
            search.min_z + 4,
            town.max_x + 24,
            town.min_z - 8,
        ),
    };
    let clipped = Rect::new(
        strip.min_x.max(search.min_x + 6),
        strip.min_z.max(search.min_z + 6),
        strip.max_x.min(search.max_x - 6),
        strip.max_z.min(search.max_z - 6),
    );
    if clipped.width() < w || clipped.depth() < d {
        return None;
    }
    find_site(occ, grid, clipped, town, keep_out, w, d, false, rng)
}

fn place_farmsteads(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let mut placed = 0;
    let mut z = search.min_z + 4;
    while z + 22 < search.max_z && placed < 6 {
        let mut x = search.min_x + 4;
        while x + 28 < search.max_x && placed < 6 {
            let rect = Rect::new(x, z, x + 26, z + 16);
            x += 30;
            if town.inflate(6).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 7) || !rng.chance(3, 4) {
                continue;
            }
            occ.fill(rect, CellKind::FarmEdge, false);
            occ.fill(rect.deflate(1), CellKind::Farm, true);
            if let Some(barn) = try_lot(
                occ,
                grid,
                lots,
                seed,
                BuildingKind::Barn,
                Rect::new(
                    rect.min_x + 2,
                    rect.min_z + 2,
                    rect.min_x + 14,
                    rect.min_z + 10,
                ),
                Dir::South,
            ) {
                let _ = barn;
            }
            scenes.push(Scene {
                kind: SceneKind::Farmstead,
                rect,
            });
            placed += 1;
        }
        z += 20;
    }
}

fn place_orchard_and_pasture(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    if let Some(rect) = find_site(occ, grid, search, town, keep_out, 18, 14, false, rng) {
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Orchard, true);
        scenes.push(Scene {
            kind: SceneKind::Orchard,
            rect,
        });
    }
    if let Some(rect) = find_site(occ, grid, search, town, keep_out, 20, 16, false, rng) {
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Pasture, true);
        scenes.push(Scene {
            kind: SceneKind::Pasture,
            rect,
        });
    }
}

fn place_hamlet(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    palette: PaletteKind,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_site_near_road(occ, grid, search, town, keep_out, 28, 22, rng) else {
        return;
    };
    occ.fill(yard, CellKind::Green, false);
    let cottages = [
        Rect::new(
            yard.min_x + 2,
            yard.min_z + 2,
            yard.min_x + 9,
            yard.min_z + 8,
        ),
        Rect::new(
            yard.min_x + 12,
            yard.min_z + 2,
            yard.min_x + 20,
            yard.min_z + 9,
        ),
        match palette {
            PaletteKind::Spruce => Rect::new(
                yard.min_x + 2,
                yard.min_z + 11,
                yard.min_x + 19,
                yard.min_z + 18,
            ),
            PaletteKind::Sandstone => Rect::new(
                yard.min_x + 2,
                yard.min_z + 12,
                yard.min_x + 10,
                yard.min_z + 19,
            ),
            PaletteKind::Oak => Rect::new(
                yard.min_x + 2,
                yard.min_z + 12,
                yard.min_x + 11,
                yard.min_z + 19,
            ),
        },
    ];
    let mut any = false;
    for (i, rect) in cottages.into_iter().enumerate() {
        let facing = [Dir::South, Dir::East, Dir::North][i];
        let kind = if i == 2 {
            match palette {
                PaletteKind::Spruce => BuildingKind::Longhouse,
                PaletteKind::Sandstone => BuildingKind::Townhouse,
                PaletteKind::Oak => BuildingKind::Cottage,
            }
        } else {
            BuildingKind::Cottage
        };
        if try_lot(occ, grid, lots, seed, kind, rect, facing).is_some() {
            any = true;
        }
    }
    let well = yard.center();
    for dz in -1..=1 {
        for dx in -1..=1 {
            if occ.kind(well.0 + dx, well.1 + dz) == CellKind::Green {
                occ.set(well.0 + dx, well.1 + dz, CellKind::Fountain, true);
            }
        }
    }
    if any {
        scenes.push(Scene {
            kind: SceneKind::Hamlet,
            rect: yard,
        });
    }
}

fn place_watchtower(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(rect) = find_peak_site(occ, grid, search, town, keep_out, 7, 7, 0, rng) else {
        return;
    };
    if try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Watchtower,
        rect,
        Dir::South,
    )
    .is_some()
    {
        scenes.push(Scene {
            kind: SceneKind::Watchtower,
            rect,
        });
    }
}

fn place_ruin(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_site(occ, grid, search, town, keep_out, 26, 20, true, rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 20, 16, true, rng))
    else {
        return;
    };
    occ.fill(yard.inflate(6), CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    let houses = [
        Rect::new(
            yard.min_x + 2,
            yard.min_z + 2,
            yard.min_x + 8,
            yard.min_z + 7,
        ),
        Rect::new(
            yard.min_x + 12,
            yard.min_z + 3,
            yard.min_x + 18,
            yard.min_z + 8,
        ),
        Rect::new(
            yard.min_x + 3,
            yard.min_z + 11,
            yard.min_x + 10,
            yard.min_z + 16,
        ),
        Rect::new(
            yard.max_x - 9,
            yard.max_z - 8,
            yard.max_x - 3,
            yard.max_z - 3,
        ),
    ];
    for house in houses {
        if yard.contains(house.min_x, house.min_z) && yard.contains(house.max_x, house.max_z) {
            occ.fill(house, CellKind::Ruin, true);
        }
    }
    let (cx, cz) = yard.center();
    occ.set(cx, cz, CellKind::Fountain, true);
    for dz in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx.abs() + dz.abs() == 1 && occ.kind(cx + dx, cz + dz) == CellKind::Green {
                occ.set(cx + dx, cz + dz, CellKind::Plaza, true);
            }
        }
    }
    scenes.push(Scene {
        kind: SceneKind::Ruin,
        rect: yard.inflate(6),
    });
}

fn place_monastery(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_site(occ, grid, search, town, keep_out, 34, 30, true, rng) else {
        return;
    };
    occ.fill(yard, CellKind::Green, false);
    occ.fill(yard.deflate(1), CellKind::Hedge, false);
    occ.fill(yard.deflate(2), CellKind::Green, true);
    let nave = Rect::new(
        yard.min_x + 8,
        yard.min_z + 3,
        yard.min_x + 25,
        yard.min_z + 14,
    );
    let cloister = Rect::new(
        yard.min_x + 10,
        yard.min_z + 16,
        yard.min_x + 23,
        yard.min_z + 27,
    );
    occ.fill(cloister, CellKind::Arcade, true);
    occ.fill(cloister.deflate(2), CellKind::Green, true);
    let bell = Rect::new(
        yard.min_x + 3,
        yard.min_z + 3,
        yard.min_x + 7,
        yard.min_z + 7,
    );
    let nave_ok = try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Monastery,
        nave,
        Dir::South,
    )
    .is_some();
    let _ = try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::BellTower,
        bell,
        Dir::South,
    );
    if nave_ok {
        scenes.push(Scene {
            kind: SceneKind::Monastery,
            rect: yard,
        });
    }
}

fn place_graveyard(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    lots: &[Lot],
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let anchor = lots
        .iter()
        .find(|lot| matches!(lot.kind, BuildingKind::Church | BuildingKind::Monastery))
        .map_or(town, |lot| lot.rect);
    let candidates = [
        Rect::new(
            anchor.max_x + 2,
            anchor.min_z,
            anchor.max_x + 19,
            anchor.min_z + 13,
        ),
        Rect::new(
            anchor.min_x - 19,
            anchor.min_z,
            anchor.min_x - 2,
            anchor.min_z + 13,
        ),
        Rect::new(
            anchor.min_x,
            anchor.max_z + 2,
            anchor.min_x + 17,
            anchor.max_z + 15,
        ),
    ];
    for rect in candidates {
        if !occ.bounds.contains(rect.min_x, rect.min_z)
            || !occ.bounds.contains(rect.max_x, rect.max_z)
        {
            continue;
        }
        if !site_ok(occ, grid, rect, 8) {
            continue;
        }
        if !rng.chance(4, 5) {
            continue;
        }
        paint_graveyard(occ, rect);
        scenes.push(Scene {
            kind: SceneKind::Graveyard,
            rect,
        });
        return;
    }
    if let Some(rect) = find_site(
        occ,
        grid,
        occ.bounds,
        town,
        town.inflate(8),
        18,
        14,
        false,
        rng,
    ) {
        paint_graveyard(occ, rect);
        scenes.push(Scene {
            kind: SceneKind::Graveyard,
            rect,
        });
    }
}

fn paint_graveyard(occ: &mut Occupancy, rect: Rect) {
    occ.fill(rect, CellKind::FarmEdge, false);
    occ.fill(rect.deflate(1), CellKind::Grave, true);
    let gate = Rect::new(
        rect.center().0 - 1,
        rect.max_z - 2,
        rect.center().0 + 1,
        rect.max_z,
    );
    occ.fill(gate, CellKind::Lychgate, true);
}

fn place_pillager_camp(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    castle: Rect,
    town: Rect,
    search: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let (tx, tz) = town.center();
    let (cx, cz) = castle.center();
    let dx = (tx - cx).signum();
    let dz = (tz - cz).signum();
    let aim = (
        (tx + dx * 108).clamp(search.min_x + 18, search.max_x - 18),
        (tz + dz * 108).clamp(search.min_z + 18, search.max_z - 18),
    );
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + 32 < search.max_z {
        let mut x = search.min_x + 8;
        while x + 36 < search.max_x {
            let rect = Rect::new(x, z, x + 34, z + 28);
            x += 16;
            if town.inflate(36).intersects(rect) || castle.inflate(20).intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 12) {
                continue;
            }
            let (mx, mz) = rect.center();
            let dist_aim = (mx - aim.0).abs() + (mz - aim.1).abs();
            let score = 240 - dist_aim + rng.gen_range(0, 8);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 14;
    }
    let Some(rect) = best else {
        return;
    };
    paint_staked_palisade(occ, rect);
    occ.fill(rect.deflate(1), CellKind::CampCourt, true);
    let face = gate_face_toward(rect, town);
    paint_palisade_gate(occ, rect, face);
    paint_palisade_walk(occ, rect);

    let (_, cz) = rect.center();
    let tent_z0 = match face {
        Dir::South => rect.min_z + 3,
        Dir::North => rect.min_z + 11,
        _ => rect.min_z + 4,
    };
    for row in 0..2 {
        for col in 0..3 {
            let tx = rect.min_x + 4 + col * 7;
            let tz = tent_z0 + row * 7;
            occ.fill(Rect::new(tx, tz, tx + 4, tz + 3), CellKind::Tent, true);
        }
    }
    occ.fill(
        Rect::new(rect.min_x + 24, tent_z0 + 11, rect.min_x + 30, tent_z0 + 15),
        CellKind::WarTent,
        true,
    );
    occ.fill(
        Rect::new(rect.min_x + 4, cz - 1, rect.min_x + 6, cz + 1),
        CellKind::Cage,
        true,
    );

    let outpost = match face {
        Dir::South => Rect::new(
            rect.max_x - 8,
            rect.min_z + 4,
            rect.max_x - 4,
            rect.min_z + 8,
        ),
        Dir::North => Rect::new(
            rect.max_x - 8,
            rect.max_z - 9,
            rect.max_x - 4,
            rect.max_z - 5,
        ),
        Dir::East => Rect::new(
            rect.min_x + 4,
            rect.max_z - 8,
            rect.min_x + 8,
            rect.max_z - 4,
        ),
        Dir::West => Rect::new(
            rect.max_x - 8,
            rect.max_z - 8,
            rect.max_x - 4,
            rect.max_z - 4,
        ),
    };
    paint_siege_yard(occ, rect, face);
    let _ = try_lot(occ, grid, lots, seed, BuildingKind::Outpost, outpost, face);
    scenes.push(Scene {
        kind: SceneKind::PillagerCamp,
        rect,
    });
}

fn gate_face_toward(rect: Rect, town: Rect) -> Dir {
    let (tx, tz) = town.center();
    let (cx, cz) = rect.center();
    if (tx - cx).abs() >= (tz - cz).abs() {
        if tx >= cx { Dir::East } else { Dir::West }
    } else if tz >= cz {
        Dir::South
    } else {
        Dir::North
    }
}

fn paint_staked_palisade(occ: &mut Occupancy, rect: Rect) {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if !rect.on_edge(x, z) {
                continue;
            }
            let along = if x == rect.min_x || x == rect.max_x {
                z
            } else {
                x
            };
            if along % 2 == 0 {
                occ.set(x, z, CellKind::Palisade, true);
            }
        }
    }
}

fn paint_palisade_walk(occ: &mut Occupancy, rect: Rect) {
    let inner = rect.deflate(1);
    for z in inner.min_z..=inner.max_z {
        for x in inner.min_x..=inner.max_x {
            if inner.on_edge(x, z) && occ.kind(x, z) == CellKind::CampCourt {
                occ.set(x, z, CellKind::PalisadeWalk, true);
            }
        }
    }
}

fn paint_palisade_gate(occ: &mut Occupancy, rect: Rect, face: Dir) {
    let (cx, cz) = rect.center();
    match face {
        Dir::North => {
            for x in cx - 1..=cx + 1 {
                occ.set(x, rect.min_z, CellKind::PalisadeGate, true);
            }
        }
        Dir::South => {
            for x in cx - 1..=cx + 1 {
                occ.set(x, rect.max_z, CellKind::PalisadeGate, true);
            }
        }
        Dir::West => {
            for z in cz - 1..=cz + 1 {
                occ.set(rect.min_x, z, CellKind::PalisadeGate, true);
            }
        }
        Dir::East => {
            for z in cz - 1..=cz + 1 {
                occ.set(rect.max_x, z, CellKind::PalisadeGate, true);
            }
        }
    }
}

fn paint_siege_yard(occ: &mut Occupancy, rect: Rect, face: Dir) {
    let inner = rect.deflate(2);
    let yard = match face {
        Dir::South => Rect::new(
            inner.min_x + 1,
            inner.max_z - 7,
            inner.max_x - 1,
            inner.max_z,
        ),
        Dir::North => Rect::new(
            inner.min_x + 1,
            inner.min_z,
            inner.max_x - 1,
            inner.min_z + 7,
        ),
        Dir::East => Rect::new(
            inner.max_x - 7,
            inner.min_z + 1,
            inner.max_x,
            inner.max_z - 1,
        ),
        Dir::West => Rect::new(
            inner.min_x,
            inner.min_z + 1,
            inner.min_x + 7,
            inner.max_z - 1,
        ),
    };
    occ.fill(yard, CellKind::CampCourt, true);
    match face {
        Dir::South | Dir::North => {
            let z0 = if face == Dir::South {
                yard.max_z - 3
            } else {
                yard.min_z + 1
            };
            occ.fill(
                Rect::new(yard.min_x + 2, z0, yard.min_x + 9, z0 + 2),
                CellKind::SiegeTreb,
                true,
            );
            occ.fill(
                Rect::new(yard.min_x + 12, z0, yard.min_x + 17, z0 + 2),
                CellKind::SiegeRam,
                true,
            );
            for x in (yard.min_x + 20..yard.max_x).step_by(3) {
                occ.set(x, z0 + 1, CellKind::SiegeLadder, true);
            }
        }
        Dir::East | Dir::West => {
            let x0 = if face == Dir::East {
                yard.max_x - 3
            } else {
                yard.min_x + 1
            };
            occ.fill(
                Rect::new(x0, yard.min_z + 2, x0 + 2, yard.min_z + 9),
                CellKind::SiegeTreb,
                true,
            );
            occ.fill(
                Rect::new(x0, yard.min_z + 12, x0 + 2, yard.min_z + 17),
                CellKind::SiegeRam,
                true,
            );
            for z in (yard.min_z + 20..yard.max_z).step_by(3) {
                occ.set(x0 + 1, z, CellKind::SiegeLadder, true);
            }
        }
    }
}

const MILL_BLADE_LEN: i32 = 10;
/// Clear disc around the hub so spinning sails never hit a house or tree.
pub const MILL_SAIL_RADIUS: i32 = 11;

fn place_edge_mills(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
) {
    let mut placed = 0;
    if let Some(peak) = find_peak_site(occ, grid, search, town.inflate(8), keep_out, 7, 7, 0, rng)
        && try_mill_site(occ, grid, lots, seed, peak)
    {
        placed += 1;
    }
    let ring = town.inflate(16);
    let mut z = ring.min_z;
    while z <= ring.max_z && placed < 3 {
        let mut x = ring.min_x;
        while x <= ring.max_x && placed < 3 {
            let on_edge = x == ring.min_x || x == ring.max_x || z == ring.min_z || z == ring.max_z;
            x += 10;
            if !on_edge || town.contains(x, z) || keep_out.contains(x, z) {
                continue;
            }
            let rect = Rect::new(x - 3, z - 3, x + 3, z + 3);
            if !search.contains(rect.min_x, rect.min_z) || !search.contains(rect.max_x, rect.max_z)
            {
                continue;
            }
            if rng.chance(1, 2) && try_mill_site(occ, grid, lots, seed, rect) {
                placed += 1;
            }
            if placed >= 2 {
                break;
            }
        }
        z += 10;
    }
    if placed == 0 {
        let fallback = Rect::new(
            town.max_x + 8,
            town.min_z + 8,
            town.max_x + 14,
            town.min_z + 14,
        );
        let _ = try_mill_site(occ, grid, lots, seed, fallback);
    }
}

fn try_mill_site(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    rect: Rect,
) -> bool {
    if !grid.rect_is_dry(rect) {
        return false;
    }
    let (cx, cz) = rect.center();
    if !occ
        .bounds
        .contains(cx - MILL_SAIL_RADIUS, cz - MILL_SAIL_RADIUS)
        || !occ
            .bounds
            .contains(cx + MILL_SAIL_RADIUS, cz + MILL_SAIL_RADIUS)
    {
        return false;
    }
    if mill_disc_blocked(occ, grid, lots, cx, cz, None) {
        return false;
    }
    if try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Windmill,
        rect,
        Dir::South,
    )
    .is_none()
    {
        return false;
    }
    reserve_sail_disc(occ, cx, cz, rect);
    true
}

fn mill_disc_blocked(
    occ: &Occupancy,
    grid: &HeightGrid,
    lots: &[Lot],
    cx: i32,
    cz: i32,
    skip: Option<u16>,
) -> bool {
    for dz in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
        for dx in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
            if dx * dx + dz * dz > MILL_SAIL_RADIUS * MILL_SAIL_RADIUS {
                continue;
            }
            let x = cx + dx;
            let z = cz + dz;
            if mill_keepout(occ.kind(x, z), occ.lot(x, z), skip) {
                return true;
            }
            if !grid.is_dry(x, z) {
                return true;
            }
            if lots.iter().any(|lot| {
                if Some(lot.id) == skip {
                    return false;
                }
                if lot.kind == BuildingKind::Windmill {
                    let (ox, oz) = lot.rect.center();
                    let gap = MILL_SAIL_RADIUS * 2 + 2;
                    (cx - ox).abs().max((cz - oz).abs()) <= gap
                } else {
                    lot.rect.contains(x, z)
                }
            }) {
                return true;
            }
        }
    }
    false
}

fn mill_keepout(kind: CellKind, lot: u16, skip: Option<u16>) -> bool {
    if skip.is_some_and(|id| lot == id) {
        return false;
    }
    matches!(
        kind,
        CellKind::Building
            | CellKind::TownWall
            | CellKind::TownTower
            | CellKind::TownGate
            | CellKind::Well
            | CellKind::Market
            | CellKind::WindSail
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Dock
            | CellKind::Harbor
            | CellKind::Quay
            | CellKind::Crane
            | CellKind::MineHead
            | CellKind::MineRail
            | CellKind::MineSpoil
            | CellKind::MineAdit
            | CellKind::Burnt
            | CellKind::Breakwater
            | CellKind::SiegeTrench
            | CellKind::TourneyList
            | CellKind::TourneyStand
            | CellKind::CathedralNave
            | CellKind::CathedralTransept
            | CellKind::CathedralChoir
            | CellKind::CathedralTower
            | CellKind::CathedralButtress
            | CellKind::CastleWall
            | CellKind::CastleKeep
            | CellKind::CastleTower
            | CellKind::CastleGate
            | CellKind::CastleInnerWall
            | CellKind::Palisade
            | CellKind::FortressWall
            | CellKind::FortressKeep
    )
}

fn reserve_sail_disc(occ: &mut Occupancy, cx: i32, cz: i32, mill: Rect) {
    for dz in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
        for dx in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
            if dx * dx + dz * dz > MILL_SAIL_RADIUS * MILL_SAIL_RADIUS {
                continue;
            }
            let x = cx + dx;
            let z = cz + dz;
            if mill.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Empty
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Green
                    | CellKind::Eave
                    | CellKind::Orchard
                    | CellKind::Pasture
                    | CellKind::Hedge
                    | CellKind::Plaza
            ) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
}

fn paint_wind_sails(occ: &mut Occupancy, lots: &[Lot]) {
    for lot in lots {
        if lot.kind != BuildingKind::Windmill {
            continue;
        }
        let (cx, cz) = lot.rect.center();
        reserve_sail_disc(occ, cx, cz, lot.rect);
        for (sx, sz) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
            for dist in 1..=MILL_BLADE_LEN {
                let x = cx + sx * dist;
                let z = cz + sz * dist;
                let stitch_z = cz + sz * (dist - 1);
                if !lot.rect.contains(x, z) && crate::stamp::mill_owns_sail(lots, lot, x, z) {
                    occupy_sail(occ, x, z);
                }
                if !lot.rect.contains(x, stitch_z)
                    && crate::stamp::mill_owns_sail(lots, lot, x, stitch_z)
                {
                    occupy_sail(occ, x, stitch_z);
                }
            }
        }
    }
}

fn occupy_sail(occ: &mut Occupancy, x: i32, z: i32) -> bool {
    if mill_keepout(occ.kind(x, z), occ.lot(x, z), None) {
        return false;
    }
    occ.set(x, z, CellKind::WindSail, true);
    true
}

fn place_hero(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    castle: Rect,
    town: Rect,
    search: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let keep = castle.inflate(10);
    place_cathedral_district(occ, grid, town, keep, search, rng, scenes);
    if let Some(yard) = find_peak_site(occ, grid, search, town, keep, 28, 24, 0, rng) {
        paint_mountain_fortress(occ, yard, scenes);
    }
}

fn place_cathedral_district(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    keep_out: Rect,
    search: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let yard = find_cathedral_site(occ, grid, search, town, keep_out, rng);
    let Some(yard) = yard else {
        return;
    };
    paint_gothic_cathedral(occ, yard);
    scenes.push(Scene {
        kind: SceneKind::Cathedral,
        rect: yard,
    });
}

fn paint_gothic_cathedral(occ: &mut Occupancy, yard: Rect) {
    occ.fill(yard, CellKind::Green, false);
    let (_, cz) = yard.center();
    let nave_h = if yard.depth() >= 26 { 6 } else { 5 };
    let nave = Rect::new(yard.min_x + 6, cz - nave_h, yard.max_x - 10, cz + nave_h);
    let west = Rect::new(
        yard.min_x + 1,
        nave.min_z - 1,
        nave.min_x - 1,
        nave.max_z + 1,
    );
    occ.fill(west, CellKind::Plaza, true);
    occ.set(west.center().0, west.center().1, CellKind::Fountain, true);
    let south = Rect::new(
        nave.min_x + 4,
        yard.max_z - 4,
        yard.max_x - 3,
        yard.max_z - 1,
    );
    occ.fill(south, CellKind::Plaza, true);
    for x in west.min_x..=nave.min_x {
        occ.set(x, cz, CellKind::Plaza, true);
    }

    occ.fill(nave, CellKind::CathedralNave, true);
    let tx = nave.min_x.midpoint(nave.max_x);
    let transept = Rect::new(tx - 4, yard.min_z + 3, tx + 4, yard.max_z - 4);
    occ.fill(transept, CellKind::CathedralTransept, true);

    let choir = Rect::new(nave.max_x + 1, nave.min_z, nave.max_x + 5, nave.max_z);
    occ.fill(choir, CellKind::CathedralChoir, true);
    let apse_x0 = choir.max_x + 1;
    for x in apse_x0..=yard.max_x - 2 {
        let half = (4 - (x - apse_x0)).max(1);
        for z in cz - half..=cz + half {
            occ.set(x, z, CellKind::CathedralChoir, true);
        }
    }

    let tw = 4;
    occ.fill(
        Rect::new(
            nave.min_x,
            nave.min_z - tw - 1,
            nave.min_x + tw,
            nave.min_z - 1,
        ),
        CellKind::CathedralTower,
        true,
    );
    occ.fill(
        Rect::new(
            nave.min_x,
            nave.max_z + 1,
            nave.min_x + tw,
            nave.max_z + tw + 1,
        ),
        CellKind::CathedralTower,
        true,
    );

    for x in (nave.min_x + 3..=nave.max_x).step_by(3) {
        if x <= nave.min_x + 4 {
            continue;
        }
        if x >= transept.min_x - 1 && x <= transept.max_x + 1 {
            continue;
        }
        occ.set(x, nave.min_z - 1, CellKind::CathedralButtress, true);
        occ.set(x, nave.min_z - 2, CellKind::CathedralButtress, true);
        occ.set(x, nave.max_z + 1, CellKind::CathedralButtress, true);
        occ.set(x, nave.max_z + 2, CellKind::CathedralButtress, true);
    }
    occ.set(
        choir.min_x + 2,
        nave.min_z - 1,
        CellKind::CathedralButtress,
        true,
    );
    occ.set(
        choir.min_x + 2,
        nave.min_z - 2,
        CellKind::CathedralButtress,
        true,
    );
    occ.set(
        choir.min_x + 2,
        nave.max_z + 1,
        CellKind::CathedralButtress,
        true,
    );
    occ.set(
        choir.min_x + 2,
        nave.max_z + 2,
        CellKind::CathedralButtress,
        true,
    );
}

fn paint_mountain_fortress(occ: &mut Occupancy, yard: Rect, scenes: &mut Vec<Scene>) {
    occ.fill(yard, CellKind::FortressWall, false);
    occ.fill(yard.deflate(1), CellKind::FortressCourt, true);
    let keep = Rect::new(
        yard.min_x + 9,
        yard.min_z + 7,
        yard.min_x + 18,
        yard.min_z + 16,
    );
    occ.fill(keep, CellKind::FortressKeep, true);
    let towers = [
        Rect::new(
            yard.min_x + 1,
            yard.min_z + 1,
            yard.min_x + 5,
            yard.min_z + 5,
        ),
        Rect::new(
            yard.max_x - 5,
            yard.min_z + 1,
            yard.max_x - 1,
            yard.min_z + 5,
        ),
        Rect::new(
            yard.min_x + 1,
            yard.max_z - 5,
            yard.min_x + 5,
            yard.max_z - 1,
        ),
        Rect::new(
            yard.max_x - 5,
            yard.max_z - 5,
            yard.max_x - 1,
            yard.max_z - 1,
        ),
    ];
    for tower in towers {
        occ.fill(tower, CellKind::FortressTower, true);
    }
    let (cx, _) = yard.center();
    for x in cx - 1..=cx + 1 {
        occ.set(x, yard.max_z, CellKind::PalisadeGate, true);
        occ.set(x, yard.max_z - 1, CellKind::FortressCourt, true);
    }
    scenes.push(Scene {
        kind: SceneKind::MountainFortress,
        rect: yard,
    });
}

fn place_mine(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_peak_site(occ, grid, search, town, keep_out, 22, 16, 10, rng) else {
        return;
    };
    occ.fill(yard.inflate(6), CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    let (cx, cz) = yard.center();
    let uphill = mine_uphill(grid, yard);
    let adit = match uphill {
        Dir::North => Rect::new(cx - 2, yard.min_z, cx + 2, yard.min_z + 3),
        Dir::South => Rect::new(cx - 2, yard.max_z - 3, cx + 2, yard.max_z),
        Dir::West => Rect::new(yard.min_x, cz - 2, yard.min_x + 3, cz + 2),
        Dir::East => Rect::new(yard.max_x - 3, cz - 2, yard.max_x, cz + 2),
    };
    occ.fill(adit, CellKind::MineAdit, true);
    occ.fill(
        Rect::new(cx - 3, cz - 3, cx + 3, cz + 3),
        CellKind::MineHead,
        true,
    );
    match uphill {
        Dir::North | Dir::South => {
            let x0 = cx;
            let z0 = adit.center().1;
            let z1 = if uphill == Dir::North {
                yard.max_z - 2
            } else {
                yard.min_z + 2
            };
            let (lo, hi) = if z0 <= z1 { (z0, z1) } else { (z1, z0) };
            for z in lo..=hi {
                if matches!(
                    occ.kind(x0, z),
                    CellKind::Green | CellKind::MineHead | CellKind::MineAdit
                ) {
                    occ.set(x0, z, CellKind::MineRail, true);
                }
                if matches!(
                    occ.kind(x0 + 1, z),
                    CellKind::Green | CellKind::MineHead | CellKind::MineAdit
                ) {
                    occ.set(x0 + 1, z, CellKind::MineRail, true);
                }
            }
        }
        Dir::East | Dir::West => {
            let z0 = cz;
            let x0 = adit.center().0;
            let x1 = if uphill == Dir::West {
                yard.max_x - 2
            } else {
                yard.min_x + 2
            };
            let (lo, hi) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
            for x in lo..=hi {
                if matches!(
                    occ.kind(x, z0),
                    CellKind::Green | CellKind::MineHead | CellKind::MineAdit
                ) {
                    occ.set(x, z0, CellKind::MineRail, true);
                }
                if matches!(
                    occ.kind(x, z0 + 1),
                    CellKind::Green | CellKind::MineHead | CellKind::MineAdit
                ) {
                    occ.set(x, z0 + 1, CellKind::MineRail, true);
                }
            }
        }
    }
    let spoil = match uphill.opposite() {
        Dir::North => Rect::new(yard.min_x + 1, yard.min_z, yard.max_x - 1, yard.min_z + 3),
        Dir::South => Rect::new(yard.min_x + 1, yard.max_z - 3, yard.max_x - 1, yard.max_z),
        Dir::West => Rect::new(yard.min_x, yard.min_z + 1, yard.min_x + 3, yard.max_z - 1),
        Dir::East => Rect::new(yard.max_x - 3, yard.min_z + 1, yard.max_x, yard.max_z - 1),
    };
    occ.fill(spoil, CellKind::MineSpoil, true);
    scenes.push(Scene {
        kind: SceneKind::Mine,
        rect: yard.inflate(6),
    });
}

fn mine_uphill(grid: &HeightGrid, yard: Rect) -> Dir {
    let (cx, cz) = yard.center();
    let north = grid.mean(yard.min_x, yard.min_z, yard.max_x, yard.min_z + 2);
    let south = grid.mean(yard.min_x, yard.max_z - 2, yard.max_x, yard.max_z);
    let west = grid.mean(yard.min_x, yard.min_z, yard.min_x + 2, yard.max_z);
    let east = grid.mean(yard.max_x - 2, yard.min_z, yard.max_x, yard.max_z);
    let _ = (cx, cz);
    let mut best = Dir::North;
    let mut h = north;
    if south > h {
        best = Dir::South;
        h = south;
    }
    if west > h {
        best = Dir::West;
        h = west;
    }
    if east > h {
        best = Dir::East;
    }
    best
}

fn place_tourney(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    castle: Rect,
    town: Rect,
    search: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let keep_out = castle.inflate(8);
    let Some(yard) = find_gentle_site(occ, grid, search, town, keep_out, 36, 22, 14, rng)
        .or_else(|| find_gentle_site(occ, grid, search, town, keep_out, 30, 18, 16, rng))
    else {
        return;
    };
    occ.fill(yard.inflate(6), CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    let (cx, cz) = yard.center();
    if yard.width() >= yard.depth() {
        occ.fill(
            Rect::new(yard.min_x + 2, cz - 1, yard.max_x - 2, cz + 1),
            CellKind::TourneyList,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 4,
                yard.min_z + 1,
                yard.max_x - 4,
                yard.min_z + 3,
            ),
            CellKind::TourneyStand,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 4,
                yard.max_z - 3,
                yard.max_x - 4,
                yard.max_z - 1,
            ),
            CellKind::TourneyStand,
            true,
        );
    } else {
        occ.fill(
            Rect::new(cx - 1, yard.min_z + 2, cx + 1, yard.max_z - 2),
            CellKind::TourneyList,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 1,
                yard.min_z + 4,
                yard.min_x + 3,
                yard.max_z - 4,
            ),
            CellKind::TourneyStand,
            true,
        );
        occ.fill(
            Rect::new(
                yard.max_x - 3,
                yard.min_z + 4,
                yard.max_x - 1,
                yard.max_z - 4,
            ),
            CellKind::TourneyStand,
            true,
        );
    }
    let pavilions = [
        Rect::new(yard.min_x, yard.min_z, yard.min_x + 4, yard.min_z + 4),
        Rect::new(yard.max_x - 4, yard.min_z, yard.max_x, yard.min_z + 4),
        Rect::new(yard.min_x, yard.max_z - 4, yard.min_x + 4, yard.max_z),
        Rect::new(yard.max_x - 4, yard.max_z - 4, yard.max_x, yard.max_z),
    ];
    for pad in pavilions {
        occ.fill(pad, CellKind::TourneyStand, true);
    }
    scenes.push(Scene {
        kind: SceneKind::Tourney,
        rect: yard.inflate(6),
    });
}

fn paint_forge_yards(occ: &mut Occupancy, lots: &[Lot]) {
    for lot in lots {
        if !matches!(lot.kind, BuildingKind::Blacksmith | BuildingKind::Tavern) {
            continue;
        }
        let pad = if lot.kind == BuildingKind::Blacksmith {
            2
        } else {
            1
        };
        let yard = lot.rect.inflate(pad);
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                if lot.rect.contains(x, z) {
                    continue;
                }
                if occ.kind(x, z) == CellKind::Empty {
                    occ.set(
                        x,
                        z,
                        if lot.kind == BuildingKind::Blacksmith {
                            CellKind::BarnYard
                        } else {
                            CellKind::Green
                        },
                        false,
                    );
                }
            }
        }
    }
}

pub(crate) fn try_lot(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    kind: BuildingKind,
    rect: Rect,
    facing: Dir,
) -> Option<u16> {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return None;
    }
    if !grid.rect_is_dry(rect) {
        return None;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Building
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::CastleKeep
                    | CellKind::CastleWall
                    | CellKind::CastleTower
                    | CellKind::CastleGate
                    | CellKind::WindSail
                    | CellKind::MineHead
                    | CellKind::MineRail
                    | CellKind::MineSpoil
                    | CellKind::Ship
                    | CellKind::TourneyList
                    | CellKind::TourneyStand
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Boat
                    | CellKind::MineAdit
                    | CellKind::Burnt
                    | CellKind::Breakwater
                    | CellKind::SiegeTrench
            ) {
                return None;
            }
        }
    }
    let id = lots.iter().map(|lot| lot.id).max().unwrap_or(0) + 1;
    let lot = Lot::new(id, kind, rect, facing, seed ^ (u64::from(id) << 8));
    occ.fill_building(rect, id, true);
    if !matches!(
        kind,
        BuildingKind::Watchtower
            | BuildingKind::Outpost
            | BuildingKind::BellTower
            | BuildingKind::Windmill
    ) {
        paint_eaves(occ, rect);
    }
    lots.push(lot);
    Some(id)
}

pub(crate) fn force_lot(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    kind: BuildingKind,
    rect: Rect,
    facing: Dir,
) -> Option<u16> {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return None;
    }
    if !grid.rect_is_dry(rect) {
        return None;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Building
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Well
                    | CellKind::CastleKeep
                    | CellKind::CastleWall
                    | CellKind::CastleTower
                    | CellKind::CastleGate
                    | CellKind::WindSail
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::MineAdit
                    | CellKind::MineHead
                    | CellKind::Burnt
                    | CellKind::Breakwater
                    | CellKind::SiegeTrench
            ) {
                return None;
            }
        }
    }
    let id = lots.iter().map(|lot| lot.id).max().unwrap_or(0) + 1;
    let lot = Lot::new(id, kind, rect, facing, seed ^ (u64::from(id) << 8));
    occ.fill_building(rect, id, true);
    if !matches!(
        kind,
        BuildingKind::Watchtower
            | BuildingKind::Outpost
            | BuildingKind::BellTower
            | BuildingKind::Windmill
            | BuildingKind::Stall
    ) {
        paint_eaves(occ, rect);
    }
    lots.push(lot);
    Some(id)
}

fn fill_green(occ: &mut Occupancy, rect: Rect, kind: CellKind) {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if occ.kind(x, z) == CellKind::Green {
                occ.set(x, z, kind, true);
            }
        }
    }
}

fn paint_eaves(occ: &mut Occupancy, rect: Rect) {
    for z in rect.min_z - 1..=rect.max_z + 1 {
        for x in rect.min_x - 1..=rect.max_x + 1 {
            if rect.contains(x, z) {
                continue;
            }
            if occ.kind(x, z) == CellKind::Empty {
                occ.set(x, z, CellKind::Eave, false);
            }
        }
    }
}

fn site_ok(occ: &Occupancy, grid: &HeightGrid, rect: Rect, max_relief: i32) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    if !occ.is_free(rect) {
        return false;
    }
    if !grid.rect_is_dry(rect) {
        return false;
    }
    let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
    hi - lo <= max_relief && lo > grid.sea_level + 1
}

fn find_cathedral_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
) -> Option<Rect> {
    for &(w, d) in &[(42, 34), (38, 30), (34, 26)] {
        if let Some(rect) = find_gentle_site(occ, grid, search, town, keep_out, w, d, 24, rng) {
            return Some(rect);
        }
    }
    None
}

fn find_gentle_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    max_relief: i32,
    rng: &mut TownRng,
) -> Option<Rect> {
    let (tx, tz) = town.center();
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 4;
    while z + d < search.max_z {
        let mut x = search.min_x + 4;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 8;
            if town.inflate(2).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, max_relief) {
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let (cx, cz) = rect.center();
            let dist = (cx - tx).abs() + (cz - tz).abs();
            let jitter = rng.gen_range(0, 4);
            let score = 800 - (hi - lo) * 14 - dist / 6 + jitter;
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 8;
    }
    best
}

fn find_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    prefer_high: bool,
    rng: &mut TownRng,
) -> Option<Rect> {
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 6;
    while z + d < search.max_z {
        let mut x = search.min_x + 6;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += w.max(10);
            if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 10) {
                continue;
            }
            let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let jitter = rng.gen_range(0, 6);
            let score = if prefer_high { mean } else { 80 - mean.abs() } + jitter;
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += d.max(8);
    }
    best
}

fn find_site_near_road(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    rng: &mut TownRng,
) -> Option<Rect> {
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + d < search.max_z {
        let mut x = search.min_x + 8;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 12;
            if town.inflate(10).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 7) {
                continue;
            }
            let road = near_road(occ, rect);
            let score = i32::from(road) * 40 + rng.gen_range(0, 5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 10;
    }
    best
}

fn find_peak_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    gap: i32,
    rng: &mut TownRng,
) -> Option<Rect> {
    let mut best = None;
    let mut best_h = i32::MIN;
    let mut z = search.min_z + 4;
    while z + d < search.max_z {
        let mut x = search.min_x + 4;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 8;
            if town.inflate(12).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if gap > 0 && !occ.is_free(rect.inflate(gap)) {
                continue;
            }
            if !site_ok(occ, grid, rect, 12) {
                continue;
            }
            let h = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z) + rng.gen_range(0, 3);
            if h > best_h {
                best_h = h;
                best = Some(rect);
            }
        }
        z += 8;
    }
    best
}

fn near_road(occ: &Occupancy, rect: Rect) -> bool {
    let ring = rect.inflate(4);
    for z in ring.min_z..=ring.max_z {
        for x in ring.min_x..=ring.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
                    | CellKind::Bridge
            ) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::{SpawnRole, TownInfo};
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::town::town_in_cell;

    #[test]
    fn hinterland_has_scenes_and_spawn_points() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            14,
            8,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert!(
            plan.scenes.len() >= 4,
            "expected hinterland scenes, got {}",
            plan.scenes.len()
        );
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Farmstead)
                || kinds.contains(&SceneKind::Orchard)
                || kinds.contains(&SceneKind::Pasture),
            "missing farms: {kinds:?}"
        );
        let info = TownInfo::from_plan(&plan);
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::VillagerBed));
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::Gate));
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::RallyMarket));
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::RallyKeep));
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::Workstation));
        assert!(info.spawns.iter().any(|s| s.role == SpawnRole::GuardPost));
        let json = info.to_json().unwrap();
        let back: TownInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.spawns.len(), info.spawns.len());
        assert_eq!(back.scenes.len(), info.scenes.len());
    }

    #[test]
    fn round8_cathedral_camp_and_sails() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            14,
            8,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        if let Some(camp) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == SceneKind::PillagerCamp)
        {
            let mut tents = 0;
            let mut war = 0;
            let mut treb = 0;
            let mut ram = 0;
            let mut palisade = 0;
            let mut walk = 0;
            let mut perimeter = 0;
            for z in camp.rect.min_z..=camp.rect.max_z {
                for x in camp.rect.min_x..=camp.rect.max_x {
                    if camp.rect.on_edge(x, z) {
                        perimeter += 1;
                    }
                    match plan.occ.kind(x, z) {
                        CellKind::Tent => tents += 1,
                        CellKind::WarTent => war += 1,
                        CellKind::SiegeTreb => treb += 1,
                        CellKind::SiegeRam => ram += 1,
                        CellKind::Palisade => palisade += 1,
                        CellKind::PalisadeWalk => walk += 1,
                        _ => {}
                    }
                }
            }
            assert!(tents >= 12, "camp tents {tents}");
            assert!(war >= 8, "war tent {war}");
            assert!(treb >= 8, "trebuchet {treb}");
            assert!(ram >= 6, "ram {ram}");
            assert!(walk >= 16, "palisade walk {walk}");
            assert!(
                palisade * 2 <= perimeter + 8,
                "palisade should be staked, palisade={palisade} perimeter={perimeter}"
            );
            assert!(
                plan.lots
                    .iter()
                    .any(|lot| lot.kind == BuildingKind::Outpost)
            );
        }
        let mills: Vec<_> = plan
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Windmill)
            .collect();
        if !mills.is_empty() {
            let mut sail_n = 0;
            let mut on_x = 0;
            for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
                for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                    if plan.occ.kind(x, z) != CellKind::WindSail {
                        continue;
                    }
                    sail_n += 1;
                    if mills.iter().any(|lot| {
                        let (cx, cz) = lot.rect.center();
                        let dx = x - cx;
                        let dz = z - cz;
                        dx != 0
                            && dz != 0
                            && (dx.abs() - dz.abs()).abs() <= 1
                            && dx.abs().max(dz.abs()) <= MILL_BLADE_LEN
                    }) {
                        on_x += 1;
                    }
                }
            }
            assert!(sail_n >= 12, "windmill sails {sail_n}");
            assert_eq!(on_x, sail_n, "sails should be a connected X");
        }
        if let Some(cat) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == SceneKind::Cathedral)
        {
            let mut nave = 0;
            let mut transept = 0;
            let mut choir = 0;
            let mut towers = 0;
            let mut buttress = 0;
            for z in cat.rect.min_z..=cat.rect.max_z {
                for x in cat.rect.min_x..=cat.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::CathedralNave => nave += 1,
                        CellKind::CathedralTransept => transept += 1,
                        CellKind::CathedralChoir => choir += 1,
                        CellKind::CathedralTower => towers += 1,
                        CellKind::CathedralButtress => buttress += 1,
                        _ => {}
                    }
                }
            }
            assert!(nave >= 40, "nave {nave}");
            assert!(transept >= 20, "transept {transept}");
            assert!(choir >= 12, "choir {choir}");
            assert!(towers >= 20, "towers {towers}");
            assert!(buttress >= 8, "buttresses {buttress}");
        }
        if kinds.contains(&SceneKind::Monastery) {
            let mut arcade = 0;
            for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
                for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                    if plan.occ.kind(x, z) == CellKind::Arcade {
                        arcade += 1;
                    }
                }
            }
            assert!(arcade >= 20, "cloister arcade {arcade}");
        }
        assert!(
            kinds.contains(&SceneKind::Cathedral)
                || kinds.contains(&SceneKind::MountainFortress)
                || kinds.contains(&SceneKind::Monastery)
                || kinds.contains(&SceneKind::PillagerCamp),
            "expected a round-7 scene, got {kinds:?}"
        );
    }

    #[test]
    fn round9_heroes_mines_ruins_and_biome_lots() {
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
        let oak = build_plan(20_261_025, origin, grid.clone(), PaletteKind::Oak);
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Cathedral) && kinds.contains(&SceneKind::MountainFortress),
            "cathedral and fortress should both place when sites exist, got {kinds:?}"
        );
        assert!(kinds.contains(&SceneKind::Mine), "missing mine: {kinds:?}");
        if let Some(ruin) = oak.scenes.iter().find(|s| s.kind == SceneKind::Ruin) {
            let mut houses = 0;
            for z in ruin.rect.min_z..=ruin.rect.max_z {
                for x in ruin.rect.min_x..=ruin.rect.max_x {
                    if oak.occ.kind(x, z) == CellKind::Ruin {
                        houses += 1;
                    }
                }
            }
            assert!(houses >= 40, "ruined village houses {houses}");
        }
        if let Some(mine) = oak.scenes.iter().find(|s| s.kind == SceneKind::Mine) {
            let mut head = 0;
            let mut rail = 0;
            let mut spoil = 0;
            for z in mine.rect.min_z..=mine.rect.max_z {
                for x in mine.rect.min_x..=mine.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::MineHead => head += 1,
                        CellKind::MineRail => rail += 1,
                        CellKind::MineSpoil => spoil += 1,
                        _ => {}
                    }
                }
            }
            assert!(head >= 16, "mine headframe {head}");
            assert!(rail >= 6, "mine rail {rail}");
            assert!(spoil >= 8, "mine spoil {spoil}");
        }
        let mills: Vec<_> = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Windmill)
            .collect();
        if let Some(mill) = mills.first() {
            let (cx, cz) = mill.rect.center();
            let mut far = 0;
            for z in oak.occ.bounds.min_z..=oak.occ.bounds.max_z {
                for x in oak.occ.bounds.min_x..=oak.occ.bounds.max_x {
                    if oak.occ.kind(x, z) != CellKind::WindSail {
                        continue;
                    }
                    let dist = (x - cx).abs().max((z - cz).abs());
                    if dist >= 8 {
                        far += 1;
                    }
                }
            }
            assert!(
                far >= 8,
                "sails should reach well outside the tower, far={far}"
            );
        }
        let spruce = build_plan(7, origin, grid.clone(), PaletteKind::Spruce);
        let spruce_long = spruce
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Longhouse)
            .count();
        let oak_long = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Longhouse)
            .count();
        assert!(
            spruce_long >= oak_long,
            "taiga should prefer longhouses, spruce={spruce_long} oak={oak_long}"
        );
        let desert = build_plan(314_159, origin, grid, PaletteKind::Sandstone);
        assert!(
            desert
                .lots
                .iter()
                .all(|lot| lot.kind != BuildingKind::Longhouse && lot.kind != BuildingKind::Timber),
            "desert towns should not place timber-frame longhouses"
        );
        assert!(
            desert
                .lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::Townhouse),
            "desert towns should keep sandstone townhouses"
        );
    }

    #[test]
    fn round10_harbour_tourney_and_connected_sails() {
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
        let oak = build_plan(20_261_025, origin, grid.clone(), PaletteKind::Oak);
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Tourney),
            "missing tourney: {kinds:?}"
        );
        if let Some(tourney) = oak.scenes.iter().find(|s| s.kind == SceneKind::Tourney) {
            let mut lists = 0;
            let mut stands = 0;
            for z in tourney.rect.min_z..=tourney.rect.max_z {
                for x in tourney.rect.min_x..=tourney.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::TourneyList => lists += 1,
                        CellKind::TourneyStand => stands += 1,
                        _ => {}
                    }
                }
            }
            assert!(lists >= 20, "jousting lists {lists}");
            assert!(stands >= 24, "tournament stands {stands}");
        }
        let desert = build_plan(314_159, origin, grid, PaletteKind::Sandstone);
        if desert
            .scenes
            .iter()
            .any(|scene| scene.kind == SceneKind::Harbour)
        {
            let mut ships = 0;
            for z in desert.occ.bounds.min_z..=desert.occ.bounds.max_z {
                for x in desert.occ.bounds.min_x..=desert.occ.bounds.max_x {
                    if desert.occ.kind(x, z) == CellKind::Ship {
                        ships += 1;
                    }
                }
            }
            assert!(ships >= 9, "harbour cog hulls {ships}");
        }
    }

    #[test]
    fn round11_sails_harbour_mine_civic_and_capital() {
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
        let oak = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert!(oak.capital, "cell (0,0) should be the royal capital");
        assert!(
            oak.castle.keep_masses.len() >= 5,
            "capital palace keep masses {}",
            oak.castle.keep_masses.len()
        );
        assert!(
            oak.lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::TownHall),
            "missing town hall"
        );
        assert!(
            oak.lots
                .iter()
                .filter(|lot| lot.kind == BuildingKind::Stall)
                .count()
                >= 4,
            "market should be dense with stalls"
        );
        let mills: Vec<_> = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Windmill)
            .collect();
        assert!(!mills.is_empty(), "edge mill should place");
        for mill in &mills {
            let (cx, cz) = mill.rect.center();
            let mut arms = 0;
            for (sx, sz) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let x = cx + sx * MILL_BLADE_LEN;
                let z = cz + sz * MILL_BLADE_LEN;
                if oak.occ.kind(x, z) == CellKind::WindSail {
                    arms += 1;
                }
            }
            assert_eq!(arms, 4, "all four mill arms should be full length");
            for dz in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
                for dx in -MILL_SAIL_RADIUS..=MILL_SAIL_RADIUS {
                    if dx * dx + dz * dz > MILL_SAIL_RADIUS * MILL_SAIL_RADIUS {
                        continue;
                    }
                    let x = cx + dx;
                    let z = cz + dz;
                    if mill.rect.contains(x, z) {
                        continue;
                    }
                    assert_ne!(
                        oak.occ.kind(x, z),
                        CellKind::Building,
                        "house inside mill sail disc at {x},{z}"
                    );
                }
            }
        }
        if let Some(mine) = oak.scenes.iter().find(|s| s.kind == SceneKind::Mine) {
            let mut adit = 0;
            let mut head = 0;
            let mut rail = 0;
            for z in mine.rect.min_z..=mine.rect.max_z {
                for x in mine.rect.min_x..=mine.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::MineAdit => adit += 1,
                        CellKind::MineHead => head += 1,
                        CellKind::MineRail => rail += 1,
                        _ => {}
                    }
                }
            }
            assert!(adit >= 8, "mine adit portal {adit}");
            assert!(head >= 24, "larger headframe {head}");
            assert!(rail >= 10, "adit cart track {rail}");
        }
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x - 40,
        );
        let harbour_plan = build_plan(7, origin, coast, PaletteKind::Oak);
        let mut ships = 0;
        let mut boats = 0;
        let mut quay = 0;
        for z in harbour_plan.occ.bounds.min_z..=harbour_plan.occ.bounds.max_z {
            for x in harbour_plan.occ.bounds.min_x..=harbour_plan.occ.bounds.max_x {
                match harbour_plan.occ.kind(x, z) {
                    CellKind::Ship => ships += 1,
                    CellKind::Boat => boats += 1,
                    CellKind::Quay | CellKind::Crane => quay += 1,
                    _ => {}
                }
            }
        }
        assert!(
            ships >= 9,
            "coastal seed should place a cog, ships={ships} boats={boats} quay={quay}"
        );
        assert!(boats >= 3, "fishing boats {boats}");
        assert!(quay >= 2, "stone quay/cranes {quay}");
    }

    #[test]
    fn round12_cog_market_frontier_and_clear_yards() {
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
        let oak = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Frontier),
            "missing frontier town: {kinds:?}"
        );
        let stalls = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Stall)
            .count();
        assert!(stalls >= 10, "packed market stalls {stalls}");
        assert!(
            oak.lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::TownHall),
            "clocktower hall facing the square"
        );
        let market = oak.market;
        let facing = |lot: &Lot| {
            matches!(
                lot.kind,
                BuildingKind::Timber | BuildingKind::Townhouse | BuildingKind::TownHall
            )
        };
        let north = oak
            .lots
            .iter()
            .filter(|lot| {
                facing(lot)
                    && lot.rect.min_z <= market.min_z + 6
                    && lot.rect.max_z >= market.min_z - 16
                    && lot.rect.max_z <= market.min_z + 8
                    && lot.rect.max_x >= market.min_x
                    && lot.rect.min_x <= market.max_x
            })
            .count();
        let south = oak
            .lots
            .iter()
            .filter(|lot| {
                facing(lot)
                    && lot.rect.max_z >= market.max_z - 6
                    && lot.rect.min_z <= market.max_z + 16
                    && lot.rect.min_z >= market.max_z - 8
                    && lot.rect.max_x >= market.min_x
                    && lot.rect.min_x <= market.max_x
            })
            .count();
        let west = oak
            .lots
            .iter()
            .filter(|lot| {
                facing(lot)
                    && lot.rect.min_x <= market.min_x + 6
                    && lot.rect.max_x >= market.min_x - 16
                    && lot.rect.max_x <= market.min_x + 8
                    && lot.rect.max_z >= market.min_z
                    && lot.rect.min_z <= market.max_z
            })
            .count();
        let east = oak
            .lots
            .iter()
            .filter(|lot| {
                facing(lot)
                    && lot.rect.max_x >= market.max_x - 6
                    && lot.rect.min_x <= market.max_x + 16
                    && lot.rect.min_x >= market.max_x - 8
                    && lot.rect.max_z >= market.min_z
                    && lot.rect.min_z <= market.max_z
            })
            .count();
        assert!(north >= 1, "market north frontage {north}");
        assert!(south >= 1, "market south frontage {south}");
        assert!(west >= 1, "market west frontage {west}");
        assert!(east >= 1, "market east frontage {east}");
        if let Some(front) = oak.scenes.iter().find(|s| s.kind == SceneKind::Frontier) {
            let mut burnt = 0;
            let mut treb = 0;
            let mut wall = 0;
            for z in front.rect.min_z..=front.rect.max_z {
                for x in front.rect.min_x..=front.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::Burnt => burnt += 1,
                        CellKind::SiegeTreb => treb += 1,
                        CellKind::TownWall | CellKind::TownTower | CellKind::Ruin => wall += 1,
                        CellKind::SiegeTrench => wall += 1,
                        _ => {}
                    }
                }
            }
            assert!(burnt >= 20, "burned houses {burnt}");
            assert!(treb >= 8, "trebuchet pits {treb}");
            assert!(wall >= 20, "damaged walls {wall}");
        }
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x - 40,
        );
        let harbour_plan = build_plan(7, origin, coast, PaletteKind::Oak);
        let mut ships = 0;
        let mut boats = 0;
        let mut quay = 0;
        let mut crane = 0;
        let mut hull: Option<Rect> = None;
        for z in harbour_plan.occ.bounds.min_z..=harbour_plan.occ.bounds.max_z {
            for x in harbour_plan.occ.bounds.min_x..=harbour_plan.occ.bounds.max_x {
                match harbour_plan.occ.kind(x, z) {
                    CellKind::Ship => {
                        ships += 1;
                        hull = Some(hull.map_or_else(
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
                    CellKind::Boat => boats += 1,
                    CellKind::Quay => quay += 1,
                    CellKind::Crane => crane += 1,
                    _ => {}
                }
            }
        }
        assert!(
            ships >= 40,
            "cog hull should be 15 long, ships={ships} boats={boats}"
        );
        let hull = hull.expect("harbour cog");
        assert!(
            hull.width().max(hull.depth()) >= COG_LEN,
            "cog keel {}x{}",
            hull.width(),
            hull.depth()
        );
        assert!(boats >= 9, "2–3 fishing boats {boats}");
        assert!(quay >= 2, "stone quay {quay}");
        assert!(crane >= 1, "harbour crane {crane}");
        assert!(
            harbour_plan
                .scenes
                .iter()
                .any(|scene| scene.kind == SceneKind::Harbour),
            "harbour scene missing"
        );
    }

    #[test]
    fn cathedral_terraces_on_world_seeds() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

        for seed in [42u64, 314_159, 20_261_025] {
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
            assert!(
                plan.scenes
                    .iter()
                    .any(|scene| scene.kind == SceneKind::Cathedral),
                "seed {seed} should get a cathedral, scenes={:?}",
                plan.scenes.iter().map(|s| s.kind).collect::<Vec<_>>()
            );
            for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
                for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                    if plan.occ.kind(x, z) != CellKind::Ship {
                        continue;
                    }
                    assert!(
                        water_body_ok(&plan.heights, plan.bounds, x, z),
                        "seed {seed} ship on a small pool at {x},{z}"
                    );
                }
            }
        }
    }

    #[test]
    fn round13_rose_harbour_lake_frontier_and_riverside() {
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
        let oak = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Monastery),
            "hilltop monastery: {kinds:?}"
        );
        if let Some(abbey) = oak.scenes.iter().find(|s| s.kind == SceneKind::Monastery) {
            let (ax, az) = abbey.rect.center();
            let mut road = 0;
            for z in az - 40..=az + 40 {
                for x in ax - 40..=ax + 40 {
                    if matches!(
                        oak.occ.kind(x, z),
                        CellKind::Road
                            | CellKind::RoadStairN
                            | CellKind::RoadStairS
                            | CellKind::RoadStairE
                            | CellKind::RoadStairW
                    ) {
                        road += 1;
                    }
                }
            }
            assert!(road >= 8, "monastery switchback {road}");
        }
        if let Some(front) = oak.scenes.iter().find(|s| s.kind == SceneKind::Frontier) {
            let mut stone = 0;
            let mut trench = 0;
            for z in front.rect.min_z..=front.rect.max_z {
                for x in front.rect.min_x..=front.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::TownWall | CellKind::TownTower => stone += 1,
                        CellKind::SiegeTrench => trench += 1,
                        _ => {}
                    }
                }
            }
            assert!(stone >= 20, "frontier stone curtain {stone}");
            assert!(trench >= 8, "siege trenches {trench}");
        }
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x - 40,
        );
        let harbour_plan = build_plan(7, origin, coast, PaletteKind::Oak);
        let mut breakwater = 0;
        let mut quay = 0;
        for z in harbour_plan.occ.bounds.min_z..=harbour_plan.occ.bounds.max_z {
            for x in harbour_plan.occ.bounds.min_x..=harbour_plan.occ.bounds.max_x {
                match harbour_plan.occ.kind(x, z) {
                    CellKind::Breakwater => breakwater += 1,
                    CellKind::Quay | CellKind::Crane => quay += 1,
                    _ => {}
                }
            }
        }
        assert!(breakwater >= 8, "harbour breakwater {breakwater}");
        assert!(quay >= 8, "bigger quay {quay}");
        let river = HeightGrid::synthetic_river(
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
        let river_plan = build_plan(11, origin, river, PaletteKind::Oak);
        let kinds: Vec<_> = river_plan.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::Riverside),
            "riverside town: {kinds:?}"
        );
        if let Some(scene) = river_plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Riverside)
        {
            let mut bridge = 0;
            let mut docks = 0;
            for z in scene.rect.min_z..=scene.rect.max_z {
                for x in scene.rect.min_x..=scene.rect.max_x {
                    match river_plan.occ.kind(x, z) {
                        CellKind::Bridge => bridge += 1,
                        CellKind::Dock => docks += 1,
                        _ => {}
                    }
                }
            }
            assert!(bridge >= 8, "stone bridge {bridge}");
            assert!(docks >= 2, "riverside docks {docks}");
            let mills = river_plan
                .lots
                .iter()
                .filter(|lot| {
                    matches!(lot.kind, BuildingKind::Windmill | BuildingKind::Barn)
                        && scene.rect.intersects(lot.rect)
                })
                .count();
            assert!(mills >= 1, "riverside mills {mills}");
        }
    }
}
