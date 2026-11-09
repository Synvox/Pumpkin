//! Hinterland scenes around a castle town: farms, hamlets, camps, ruins.
//!
//! Placement is a pure function of seed + cell through the same RNG stream
//! the planner already uses. Each scene is an axis-aligned rect of occupancy
//! cells; buildings are extra [`Lot`]s so the stamper and watertight check
//! reuse the existing house / church paths.

use crate::buildings::{BuildingKind, Lot};
use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect};
use crate::height::HeightGrid;
use crate::rng::TownRng;

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
            Self::Ruin => "Ruined fort",
            Self::Monastery => "Monastery",
            Self::PillagerCamp => "Pillager camp",
            Self::Graveyard => "Graveyard",
            Self::Harbour => "Harbour",
            Self::Plaza => "Plaza",
            Self::Cathedral => "Cathedral",
            Self::MountainFortress => "Mountain fortress",
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
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
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
    paint_wind_sails(occ, lots);
    paint_forge_yards(occ, lots);
    scenes
}

/// Records a harbour scene after wet cells have been remapped.
pub fn record_harbour(occ: &Occupancy, search: Rect, scenes: &mut Vec<Scene>) {
    let mut docks = Vec::new();
    for z in search.min_z..=search.max_z {
        for x in search.min_x..=search.max_x {
            if occ.kind(x, z) == CellKind::Dock {
                docks.push((x, z));
            }
        }
    }
    if docks.len() < 6 {
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
    let mut rect = Rect::new(min_x, min_z, max_x, max_z).inflate(6);
    if rect.width() > 48 {
        let (cx, _) = rect.center();
        rect = Rect::new(cx - 24, rect.min_z, cx + 23, rect.max_z);
    }
    if rect.depth() > 48 {
        let (_, cz) = rect.center();
        rect = Rect::new(rect.min_x, cz - 24, rect.max_x, cz + 23);
    }
    scenes.push(Scene {
        kind: SceneKind::Harbour,
        rect,
    });
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
            if placed % 2 == 0 {
                let wr = Rect::new(
                    rect.max_x - 8,
                    rect.min_z + 4,
                    rect.max_x - 2,
                    rect.min_z + 10,
                );
                let _ = try_lot(
                    occ,
                    grid,
                    lots,
                    seed,
                    BuildingKind::Windmill,
                    wr,
                    Dir::South,
                );
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
        Rect::new(
            yard.min_x + 2,
            yard.min_z + 12,
            yard.min_x + 11,
            yard.min_z + 19,
        ),
    ];
    let mut any = false;
    for (i, rect) in cottages.into_iter().enumerate() {
        let facing = [Dir::South, Dir::East, Dir::North][i];
        if try_lot(occ, grid, lots, seed, BuildingKind::Cottage, rect, facing).is_some() {
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
    let Some(rect) = find_peak_site(occ, grid, search, town, keep_out, 7, 7, rng) else {
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
    let Some(rect) = find_site(occ, grid, search, town, keep_out, 16, 14, true, rng) else {
        return;
    };
    occ.fill(rect, CellKind::Ruin, false);
    scenes.push(Scene {
        kind: SceneKind::Ruin,
        rect,
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

fn paint_wind_sails(occ: &mut Occupancy, lots: &[Lot]) {
    for lot in lots {
        if lot.kind != BuildingKind::Windmill {
            continue;
        }
        let (cx, cz) = lot.rect.center();
        for dist in 3..=7 {
            for (sx, sz) in [(1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let x = cx + sx * dist;
                let z = cz + sz * dist;
                if matches!(
                    occ.kind(x, z),
                    CellKind::Empty | CellKind::Farm | CellKind::FarmEdge | CellKind::Green
                ) {
                    occ.set(x, z, CellKind::WindSail, true);
                }
            }
        }
    }
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
    let town_h = grid.mean(town.min_x, town.min_z, town.max_x, town.max_z);
    let peak = find_peak_site(occ, grid, search, town, keep, 28, 24, rng);
    let peak_high = peak.is_some_and(|yard| {
        grid.mean(yard.min_x, yard.min_z, yard.max_x, yard.max_z) >= town_h + 12
    });
    if peak_high {
        if let Some(yard) = peak {
            paint_mountain_fortress(occ, yard, scenes);
        }
        return;
    }
    place_cathedral_district(occ, grid, town, keep, search, rng, scenes);
    if !scenes
        .iter()
        .any(|scene| scene.kind == SceneKind::Cathedral)
        && let Some(yard) = peak
    {
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
    let yard = find_site(occ, grid, search, town, keep_out, 42, 34, false, rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 38, 30, false, rng));
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
    let nave_h = if yard.depth() >= 34 { 5 } else { 4 };
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

fn try_lot(
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
        BuildingKind::Watchtower | BuildingKind::Outpost | BuildingKind::BellTower
    ) {
        paint_eaves(occ, rect);
    }
    lots.push(lot);
    Some(id)
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
            let mut diagonal = 0;
            for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
                for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                    if plan.occ.kind(x, z) != CellKind::WindSail {
                        continue;
                    }
                    sail_n += 1;
                    if mills.iter().any(|lot| {
                        let (cx, cz) = lot.rect.center();
                        (x - cx).abs() == (z - cz).abs()
                    }) {
                        diagonal += 1;
                    }
                }
            }
            assert!(sail_n >= 12, "windmill sails {sail_n}");
            assert_eq!(diagonal, sail_n, "sails should be a diagonal X");
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
}
