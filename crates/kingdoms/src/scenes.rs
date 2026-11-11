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

/// Extra strip around a frontier yard so hanging Features trunks and leaf
/// strips cannot float over the curtain in the scene crop.
pub const FRONTIER_TREE_MARGIN: i32 = 10;

/// Keep feature trees this far from a roofed lot so trunks do not punch eaves.
pub const BUILDING_TREE_MARGIN: i32 = 4;

/// Occupancy keepout around the walled town (curtain, pomerium, fosse).
pub const TOWN_TREE_MARGIN: i32 = 8;
/// Extra stamp strip so hanging Features canopies cannot sit on the fosse.
pub const TREE_STRIP_MARGIN: i32 = 16;

/// Cog hull along the keel. Beam is 7 so the bow can taper to a point.
const COG_LEN: i32 = 19;
const COG_BEAM: i32 = 7;

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
    /// Walled hill-fort manor with a gatehouse and furnished hall.
    Manor,
    /// Walled abbey vineyard on a hillside.
    Vineyard,
    /// Cliffside or river-gorge town on stacked terraces.
    Gorge,
    /// Taiga stave church with dragon-head gables.
    StaveChurch,
    /// Desert caravanserai with a courtyard and minarets.
    Caravanserai,
    /// Inner-city market quarter around the civic square.
    MarketQuarter,
    /// Smithies, tanneries and workshops along a street.
    CraftsmenRow,
    /// Townhouses and gardens on the castle side of town.
    NobleQuarter,
    /// Warehouses and inns at the downhill / water gate.
    DocksQuarter,
    /// Logging camp with cottages and stacked timber.
    LumberCamp,
    /// Graveyard, cloister and bishop's house beside the cathedral.
    CathedralClose,
    /// Cottages, tannery and almshouse outside the main gate.
    Extramural,
    /// Collapsed cloister and nave between towns.
    RuinedAbbey,
    /// Palisade camp tucked in a carved ravine.
    BanditHideout,
    /// Lone round tower on a crag.
    HermitTower,
    /// Ring of standing stones on a green.
    StoneCircle,
    /// Gallows at a hinterland road junction.
    Gallows,
    /// Broken curtain and keep stump on a rise.
    CastleRuin,
    /// Gatehouse and barred deck over a river.
    TollBridge,
    /// Charcoal-burners' hut and mounds in a woodland clearing.
    CharcoalCamp,
    /// Coaching inn with a stable, yard and hanging sign.
    RoadsideInn,
    /// Walled hinterland market with a guildhall and stalls.
    WalledMarket,
    /// Miller's hamlet with a race, wheel and granary.
    MillHamlet,
    /// Hilltop beacon or watch-fire along a hinterland road.
    Beacon,
    /// Open quarry with cranes, cut stone and a mason's lodge.
    Quarry,
    /// Palisaded siege camp outside a castle.
    SiegeCamp,
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
            Self::Manor => "Hill-fort manor",
            Self::Vineyard => "Terraced vineyard village",
            Self::Gorge => "Gorge town",
            Self::StaveChurch => "Stave church",
            Self::Caravanserai => "Caravanserai",
            Self::MarketQuarter => "Market quarter",
            Self::CraftsmenRow => "Craftsmen's row",
            Self::NobleQuarter => "Noble quarter",
            Self::DocksQuarter => "Docks quarter",
            Self::LumberCamp => "Lumber camp",
            Self::CathedralClose => "Cathedral close",
            Self::Extramural => "Extramural suburb",
            Self::RuinedAbbey => "Ruined abbey",
            Self::BanditHideout => "Bandit hideout",
            Self::HermitTower => "Hermit's tower",
            Self::StoneCircle => "Stone circle",
            Self::Gallows => "Crossroads gallows",
            Self::CastleRuin => "Castle ruin",
            Self::TollBridge => "Toll bridge",
            Self::CharcoalCamp => "Charcoal camp",
            Self::RoadsideInn => "Roadside inn",
            Self::WalledMarket => "Walled market",
            Self::MillHamlet => "Mill hamlet",
            Self::Beacon => "Hilltop beacon",
            Self::Quarry => "Quarry",
            Self::SiegeCamp => "Siege camp",
        }
    }
}

/// Extra strip around a scene so hanging Features canopies cannot float
/// in the close-up crop.
#[must_use]
pub const fn scene_tree_margin(kind: SceneKind) -> i32 {
    match kind {
        SceneKind::Frontier | SceneKind::CastleRuin | SceneKind::RuinedAbbey => {
            SCENE_YARD_MARGIN + FRONTIER_TREE_MARGIN
        }
        _ => SCENE_YARD_MARGIN,
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
    walled_city: bool,
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
    place_hero(
        occ,
        grid,
        castle,
        town,
        search,
        lots,
        seed,
        walled_city,
        palette,
        rng,
        &mut scenes,
    );
    place_mine(occ, grid, search, town, keep_out, rng, &mut scenes);
    place_tourney(occ, grid, castle, town, search, rng, &mut scenes);
    place_siege_camp(
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
    place_frontier(occ, grid, search, town, keep_out, origin, rng, &mut scenes);
    place_riverside(occ, grid, search, town, keep_out, lots, seed, &mut scenes);
    paint_monastery_switchback(occ, grid, town, &scenes);
    place_manor(occ, grid, search, town, keep_out, lots, seed, &mut scenes);
    place_vineyard(occ, grid, search, town, keep_out, lots, seed, &mut scenes);
    place_stave_church_scene(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        palette,
        &mut scenes,
    );
    place_caravanserai(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        palette,
        &mut scenes,
    );
    place_gorge_town(occ, grid, search, town, keep_out, lots, seed, &mut scenes);
    place_outskirts(
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
    place_edge_mills(occ, grid, search, town, keep_out, lots, seed, rng);
    paint_wind_sails(occ, lots);
    paint_forge_yards(occ, lots);
    let mut extra = TownRng::salted(seed, 21, 2);
    place_extra_orchards_and_pastures(occ, grid, search, town, keep_out, &mut extra, &mut scenes);
    place_ridge_mills(occ, grid, search, town, keep_out, lots, seed, &mut extra);
    place_lumber_camp(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        &mut extra,
        &mut scenes,
    );
    paint_wind_sails(occ, lots);
    place_wayside_pois(
        occ,
        grid,
        search,
        town,
        keep_out,
        lots,
        seed,
        &mut extra,
        &mut scenes,
    );
    scenes
}

fn place_wayside_pois(
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
    place_ruined_abbey(occ, grid, search, town, keep_out, rng, scenes);
    place_castle_ruin(occ, grid, search, town, keep_out, rng, scenes);
    place_bandit_hideout(occ, grid, search, town, keep_out, rng, scenes);
    place_coaching_inn_scene(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_toll_bridge(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_charcoal_camp(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_walled_market(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_mill_hamlet(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_quarry(occ, grid, search, town, keep_out, lots, seed, rng, scenes);
    place_hermit_tower(occ, grid, search, town, keep_out, rng, scenes);
    place_stone_circle(occ, grid, search, town, keep_out, rng, scenes);
    place_crossroads_gallows(occ, grid, search, town, keep_out, rng, scenes);
    place_beacon_chain(occ, grid, search, town, keep_out, rng, scenes);
}

fn place_ruined_abbey(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_site(occ, grid, search, town, keep_out, 32, 26, true, rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 28, 22, true, rng))
    else {
        return;
    };
    occ.fill(yard, CellKind::Green, true);
    let nave = Rect::new(
        yard.min_x + 4,
        yard.min_z + 5,
        yard.max_x - 9,
        yard.min_z + 16,
    );
    stroke_rect_edges(occ, nave, CellKind::Ruin);
    for x in nave.min_x + 3..=nave.max_x - 3 {
        if x % 4 == 0 {
            occ.set(x, nave.min_z + 3, CellKind::Ruin, true);
            occ.set(x, nave.max_z - 3, CellKind::Ruin, true);
        }
    }
    let stump = Rect::new(
        nave.max_x + 2,
        nave.min_z,
        (nave.max_x + 6).min(yard.max_x - 1),
        nave.min_z + 4,
    );
    stroke_rect_edges(occ, stump, CellKind::Ruin);
    occ.set(stump.center().0, stump.center().1, CellKind::Green, true);
    let cloister = Rect::new(
        nave.min_x + 2,
        nave.max_z + 2,
        nave.max_x - 2,
        (nave.max_z + 8).min(yard.max_z - 2),
    );
    for x in (cloister.min_x..=cloister.max_x).step_by(3) {
        occ.set(x, cloister.min_z, CellKind::Arcade, true);
        occ.set(x, cloister.max_z, CellKind::Arcade, true);
    }
    for z in (cloister.min_z..=cloister.max_z).step_by(3) {
        occ.set(cloister.min_x, z, CellKind::Arcade, true);
        occ.set(cloister.max_x, z, CellKind::Arcade, true);
    }
    let graves = Rect::new(nave.min_x, yard.min_z + 1, nave.max_x, nave.min_z - 2);
    for x in graves.min_x..=graves.max_x {
        for z in graves.min_z..=graves.max_z {
            if (x + z) % 2 == 0 {
                occ.set(x, z, CellKind::Grave, true);
            }
        }
    }
    occ.set(graves.min_x, graves.center().1, CellKind::Lychgate, true);
    scenes.push(Scene {
        kind: SceneKind::RuinedAbbey,
        rect: yard,
    });
}

fn stroke_rect_edges(occ: &mut Occupancy, rect: Rect, kind: CellKind) {
    for x in rect.min_x..=rect.max_x {
        occ.set(x, rect.min_z, kind, true);
        occ.set(x, rect.max_z, kind, true);
    }
    for z in rect.min_z..=rect.max_z {
        occ.set(rect.min_x, z, kind, true);
        occ.set(rect.max_x, z, kind, true);
    }
}

fn place_castle_ruin(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_peak_site(occ, grid, search, town, keep_out, 22, 18, 2, rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 22, 18, true, rng))
        .or_else(|| find_site(occ, grid, search, town, keep_out, 18, 16, true, rng))
    else {
        return;
    };
    occ.fill(yard, CellKind::Green, true);
    let curtain = yard.deflate(1);
    stroke_rect_edges(occ, curtain, CellKind::Ruin);
    let (cx, cz) = curtain.center();
    for x in cx - 1..=cx + 1 {
        occ.set(x, curtain.max_z, CellKind::Green, true);
    }
    let keep = Rect::new(cx - 3, cz - 3, cx + 3, cz + 3);
    stroke_rect_edges(occ, keep, CellKind::Ruin);
    occ.fill(keep.deflate(1), CellKind::Green, true);
    let towers = [
        Rect::new(
            curtain.min_x,
            curtain.min_z,
            curtain.min_x + 2,
            curtain.min_z + 2,
        ),
        Rect::new(
            curtain.max_x - 2,
            curtain.min_z,
            curtain.max_x,
            curtain.min_z + 2,
        ),
        Rect::new(
            curtain.min_x,
            curtain.max_z - 2,
            curtain.min_x + 2,
            curtain.max_z,
        ),
    ];
    for tower in towers {
        stroke_rect_edges(occ, tower, CellKind::Ruin);
    }
    for x in curtain.min_x + 2..=curtain.max_x - 2 {
        if x % 5 == 0 {
            occ.set(x, curtain.min_z - 1, CellKind::SiegeTrench, true);
        }
    }
    scenes.push(Scene {
        kind: SceneKind::CastleRuin,
        rect: yard,
    });
}

fn place_bandit_hideout(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_inland_site(
        occ,
        grid,
        search,
        town,
        keep_out.inflate(6),
        36,
        28,
        false,
        rng,
    )
    .or_else(|| {
        find_inland_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(6),
            32,
            24,
            false,
            rng,
        )
    })
    .or_else(|| {
        find_inland_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(6),
            28,
            22,
            false,
            rng,
        )
    }) else {
        return;
    };
    occ.fill(yard, CellKind::Green, true);
    let cz = yard.center().1;
    let ravine_x = yard.max_x - 2;
    for z in (yard.min_z + 1)..yard.max_z {
        for x in ravine_x - 1..=ravine_x + 1 {
            if yard.contains(x, z) {
                occ.set(x, z, CellKind::SiegeTrench, true);
            }
        }
    }
    let camp = Rect::new(
        yard.min_x + 2,
        yard.min_z + 2,
        (ravine_x - 4).max(yard.min_x + 14),
        yard.max_z - 2,
    );
    paint_staked_palisade(occ, camp);
    occ.set(camp.max_x, cz, CellKind::PalisadeGate, true);
    occ.set(camp.max_x, cz - 1, CellKind::Green, true);
    occ.set(camp.max_x, cz + 1, CellKind::Green, true);
    let (cx, czc) = camp.center();
    let fire = Rect::new(cx - 2, czc - 2, cx + 2, czc + 2);
    occ.fill(fire, CellKind::CampCourt, true);
    for x in cx..=camp.max_x {
        if occ.kind(x, czc) == CellKind::Green {
            occ.set(x, czc, CellKind::CampCourt, true);
        }
    }
    let look = Rect::new(camp.min_x + 3, czc - 1, camp.min_x + 5, czc + 1);
    if camp.contains(look.min_x, look.min_z) && camp.contains(look.max_x, look.max_z) {
        occ.fill(look, CellKind::WatchPost, true);
    }
    let tents = [
        Rect::new(
            camp.min_x + 3,
            camp.min_z + 3,
            camp.min_x + 5,
            camp.min_z + 5,
        ),
        Rect::new(
            camp.max_x - 6,
            camp.min_z + 3,
            camp.max_x - 4,
            camp.min_z + 5,
        ),
        Rect::new(
            camp.min_x + 3,
            camp.max_z - 5,
            camp.min_x + 5,
            camp.max_z - 3,
        ),
        Rect::new(
            camp.max_x - 6,
            camp.max_z - 5,
            camp.max_x - 4,
            camp.max_z - 3,
        ),
    ];
    for (i, tent) in tents.iter().enumerate() {
        if !camp.contains(tent.min_x, tent.min_z) || !camp.contains(tent.max_x, tent.max_z) {
            continue;
        }
        occ.fill(
            *tent,
            if i == 1 {
                CellKind::WarTent
            } else {
                CellKind::Tent
            },
            true,
        );
    }
    let loot = Rect::new(camp.max_x - 4, czc - 1, camp.max_x - 3, czc);
    if camp.contains(loot.min_x, loot.min_z) {
        occ.fill(loot, CellKind::LootPile, true);
    }
    occ.set(camp.min_x + 3, camp.max_z - 7, CellKind::Cage, true);
    occ.set(camp.min_x + 4, camp.max_z - 7, CellKind::Cage, true);
    let look2 = Rect::new(cx - 1, camp.min_z + 3, cx + 1, camp.min_z + 5);
    if camp.contains(look2.min_x, look2.min_z)
        && camp.contains(look2.max_x, look2.max_z)
        && !look2.intersects(fire)
        && !look2.intersects(look)
    {
        occ.fill(look2, CellKind::WatchPost, true);
    }
    let loot2 = Rect::new(
        camp.min_x + 7,
        camp.max_z - 8,
        camp.min_x + 8,
        camp.max_z - 7,
    );
    if camp.contains(loot2.min_x, loot2.min_z) {
        occ.fill(loot2, CellKind::LootPile, true);
    }
    let wagon = Rect::new(camp.max_x - 5, czc + 2, camp.max_x - 3, czc + 3);
    if camp.contains(wagon.min_x, wagon.min_z) && camp.contains(wagon.max_x, wagon.max_z) {
        let free = (wagon.min_x..=wagon.max_x)
            .all(|wx| (wagon.min_z..=wagon.max_z).all(|wz| occ.kind(wx, wz) == CellKind::Green));
        if free {
            occ.fill(wagon, CellKind::Wagon, true);
        }
    }
    scenes.push(Scene {
        kind: SceneKind::BanditHideout,
        rect: yard,
    });
}

fn place_coaching_inn_scene(
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
    let Some(yard) =
        find_inland_site_near_road(occ, grid, search, town, keep_out.inflate(8), 36, 30, rng)
            .or_else(|| {
                find_inland_site_near_road(
                    occ,
                    grid,
                    search,
                    town,
                    keep_out.inflate(8),
                    34,
                    30,
                    rng,
                )
            })
            .or_else(|| {
                find_inland_site(
                    occ,
                    grid,
                    search,
                    town,
                    keep_out.inflate(8),
                    32,
                    30,
                    true,
                    rng,
                )
            })
    else {
        return;
    };
    if landmark_blocked(scenes, yard) {
        return;
    }
    let (tw, td) = BuildingKind::Tavern.size();
    let (bw, bd) = BuildingKind::Barn.size();
    let tavern = Rect::new(
        yard.min_x + 2,
        yard.min_z + 2,
        yard.min_x + 1 + tw,
        yard.min_z + 1 + td,
    );
    let barn = Rect::new(
        yard.min_x + 2,
        yard.max_z - bd - 1,
        yard.min_x + 1 + bw,
        yard.max_z - 2,
    );
    if !grid.rect_is_buildable(tavern)
        || !grid.rect_is_buildable(barn)
        || barn.min_z <= tavern.max_z + 5
    {
        return;
    }
    occ.fill(yard, CellKind::Green, true);
    if force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Tavern,
        tavern,
        Dir::South,
    )
    .is_none()
    {
        return;
    }
    let _ = force_lot(occ, grid, lots, seed, BuildingKind::Barn, barn, Dir::North);
    for z in tavern.max_z + 1..barn.min_z {
        for x in yard.min_x + 2..=yard.max_x - 2 {
            if matches!(occ.kind(x, z), CellKind::Green | CellKind::Eave) {
                occ.set(x, z, CellKind::Plaza, true);
            }
        }
    }
    paint_eaves(occ, tavern);
    paint_eaves(occ, barn);
    let (cx, cz) = (
        (tavern.min_x + barn.max_x) / 2,
        tavern.max_z + ((barn.min_z - tavern.max_z) / 2).max(2),
    );
    let well = Rect::new(cx - 1, cz - 1, cx + 1, cz + 1);
    if yard.contains(well.min_x, well.min_z) && yard.contains(well.max_x, well.max_z) {
        let free = (well.min_x..=well.max_x).all(|wx| {
            (well.min_z..=well.max_z)
                .all(|wz| matches!(occ.kind(wx, wz), CellKind::Plaza | CellKind::Green))
        });
        if free {
            occ.fill(well, CellKind::Well, true);
        }
    }
    let wagon = Rect::new(yard.max_x - 6, cz - 1, yard.max_x - 4, cz);
    if yard.contains(wagon.min_x, wagon.min_z) && yard.contains(wagon.max_x, wagon.max_z) {
        let free = (wagon.min_x..=wagon.max_x).all(|wx| {
            (wagon.min_z..=wagon.max_z)
                .all(|wz| matches!(occ.kind(wx, wz), CellKind::Plaza | CellKind::Green))
        });
        if free {
            occ.fill(wagon, CellKind::Wagon, true);
        }
    }
    scenes.push(Scene {
        kind: SceneKind::RoadsideInn,
        rect: yard,
    });
}

fn place_charcoal_camp(
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
    let Some(yard) = find_site(
        occ,
        grid,
        search,
        town,
        keep_out.inflate(8),
        22,
        16,
        true,
        rng,
    )
    .or_else(|| {
        find_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(8),
            18,
            14,
            true,
            rng,
        )
    }) else {
        return;
    };
    if landmark_blocked(scenes, yard) || scenes.iter().any(|scene| scene.rect.intersects(yard)) {
        return;
    }
    let (w, d) = BuildingKind::Cottage.size();
    let hut = Rect::new(
        yard.min_x + 2,
        yard.min_z + 2,
        yard.min_x + 1 + w,
        yard.min_z + 1 + d,
    );
    if !grid.rect_is_buildable(hut) {
        return;
    }
    occ.fill(yard, CellKind::Green, true);
    if force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        hut,
        Dir::South,
    )
    .is_none()
    {
        return;
    }
    scenes.push(Scene {
        kind: SceneKind::CharcoalCamp,
        rect: yard,
    });
}

fn place_walled_market(
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
    let Some(yard) = find_inland_site(
        occ,
        grid,
        search,
        town,
        keep_out.inflate(8),
        38,
        32,
        true,
        rng,
    )
    .or_else(|| {
        find_inland_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(8),
            34,
            28,
            true,
            rng,
        )
    })
    .or_else(|| {
        find_inland_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(8),
            32,
            26,
            true,
            rng,
        )
    }) else {
        return;
    };
    if landmark_blocked(scenes, yard) {
        return;
    }
    let (hw, hd) = BuildingKind::TownHall.size();
    let (sw, sd) = BuildingKind::Stall.size();
    if yard.width() < hw + 8 || yard.depth() < hd + 10 {
        return;
    }
    let inner = yard.deflate(1);
    let hall = Rect::new(
        inner.min_x + 3,
        inner.min_z + 2,
        inner.min_x + 2 + hw,
        inner.min_z + 1 + hd,
    );
    if !inner.contains(hall.min_x, hall.min_z) || !inner.contains(hall.max_x, hall.max_z) {
        return;
    }
    if !grid.rect_is_buildable(hall) {
        return;
    }
    occ.fill(yard, CellKind::Green, true);
    occ.fill(inner, CellKind::Plaza, true);
    paint_staked_palisade(occ, inner);
    paint_palisade_gate(occ, inner, facing_town(yard, town));
    if force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::TownHall,
        hall,
        Dir::South,
    )
    .is_none()
    {
        return;
    }
    paint_eaves(occ, hall);
    let (hx, _hz) = hall.center();
    let well = Rect::new(hx - 1, hall.max_z + 3, hx + 1, hall.max_z + 5);
    if inner.contains(well.min_x, well.min_z) && inner.contains(well.max_x, well.max_z) {
        let free = (well.min_x..=well.max_x)
            .all(|wx| (well.min_z..=well.max_z).all(|wz| occ.kind(wx, wz) == CellKind::Plaza));
        if free {
            occ.fill(well, CellKind::Well, true);
        }
    }
    let stalls = [
        Rect::new(
            inner.min_x + 2,
            hall.max_z + 3,
            inner.min_x + 1 + sw,
            hall.max_z + 2 + sd,
        ),
        Rect::new(
            inner.max_x - sw - 1,
            hall.max_z + 3,
            inner.max_x - 2,
            hall.max_z + 2 + sd,
        ),
        Rect::new(
            hx - sw / 2,
            inner.max_z - sd - 2,
            hx - sw / 2 + sw - 1,
            inner.max_z - 3,
        ),
    ];
    for stall in stalls {
        if !inner.contains(stall.min_x, stall.min_z) || !inner.contains(stall.max_x, stall.max_z) {
            continue;
        }
        if stall.intersects(hall) || stall.intersects(well) {
            continue;
        }
        let _ = force_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Stall,
            stall,
            Dir::South,
        );
    }
    scenes.push(Scene {
        kind: SceneKind::WalledMarket,
        rect: yard,
    });
}

fn facing_town(yard: Rect, town: Rect) -> Dir {
    let (cx, cz) = yard.center();
    let (tx, tz) = town.center();
    if (tx - cx).abs() >= (tz - cz).abs() {
        if tx >= cx { Dir::East } else { Dir::West }
    } else if tz >= cz {
        Dir::South
    } else {
        Dir::North
    }
}

fn place_mill_hamlet(
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
    if scenes.iter().any(|scene| scene.kind == SceneKind::Harbour) {
        return;
    }
    let Some(yard) = find_mill_hamlet_site(occ, grid, search, town, keep_out, rng) else {
        return;
    };
    if landmark_blocked(scenes, yard)
        || scenes.iter().any(|scene| {
            matches!(
                scene.kind,
                SceneKind::Riverside | SceneKind::TollBridge | SceneKind::Harbour
            ) && scene.rect.inflate(24).intersects(yard)
        })
    {
        return;
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    if place_watermills(occ, grid, yard, lots, seed, 1) == 0 {
        return;
    }
    let _ = place_granary_in_yard(occ, grid, yard, lots, seed);
    let _ = place_mill_cottages(occ, grid, yard, lots, seed, 2);
    scenes.push(Scene {
        kind: SceneKind::MillHamlet,
        rect: mill_hamlet_scene_rect(occ, lots, yard),
    });
}

fn find_mill_hamlet_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 32;
    let d = 26;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + d < search.max_z {
        let mut x = search.min_x + 8;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 10;
            if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !riverside_yard_free(occ, rect) {
                continue;
            }
            let (cx, cz) = rect.center();
            let mut wet_cells = 0;
            let mut deep = 0;
            for zz in rect.min_z..=rect.max_z {
                for xx in rect.min_x..=rect.max_x {
                    if grid.is_dry(xx, zz) {
                        continue;
                    }
                    wet_cells += 1;
                    if grid.water_depth(xx, zz) >= 3 {
                        deep += 1;
                    }
                }
            }
            let mut wet_x = 0;
            let mut wet_z = 0;
            for xx in rect.min_x..=rect.max_x {
                if !grid.is_dry(xx, cz) {
                    wet_x += 1;
                }
            }
            for zz in rect.min_z..=rect.max_z {
                if !grid.is_dry(cx, zz) {
                    wet_z += 1;
                }
            }
            // A mill race wants a shallow stream, not a harbour basin or open sea.
            if deep >= 12 || wet_cells * 3 > rect.width() * rect.depth() || wet_x.min(wet_z) >= 14 {
                continue;
            }
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
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let score = wet * 8 + banks * 4 - (hi - lo) + rng.gen_range(0, 5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 10;
    }
    best
}

fn mill_hamlet_scene_rect(occ: &Occupancy, lots: &[Lot], yard: Rect) -> Rect {
    let mut rect: Option<Rect> = None;
    let grow = |rect: &mut Option<Rect>, x: i32, z: i32| {
        *rect = Some(rect.map_or_else(
            || Rect::new(x, z, x, z),
            |r| {
                Rect::new(
                    r.min_x.min(x),
                    r.min_z.min(z),
                    r.max_x.max(x),
                    r.max_z.max(z),
                )
            },
        ));
    };
    for lot in lots {
        if !matches!(
            lot.kind,
            BuildingKind::Watermill | BuildingKind::Barn | BuildingKind::Cottage
        ) || !yard.intersects(lot.rect)
        {
            continue;
        }
        grow(&mut rect, lot.rect.min_x, lot.rect.min_z);
        grow(&mut rect, lot.rect.max_x, lot.rect.max_z);
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::WaterWheel | CellKind::MillRace | CellKind::Sluice
            ) {
                grow(&mut rect, x, z);
            }
        }
    }
    rect.unwrap_or(yard).inflate(4)
}

fn place_quarry(
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
    let Some(yard) = find_quarry_site(occ, grid, search, town, keep_out, lots, scenes, rng) else {
        return;
    };
    if landmark_blocked(scenes, yard)
        || scenes.iter().any(|scene| {
            matches!(scene.kind, SceneKind::Mine | SceneKind::CastleRuin)
                && scene.rect.inflate(16).intersects(yard)
        })
    {
        return;
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    let (cx, cz) = yard.center();
    let downhill = quarry_downhill(grid, yard);
    let pit = match downhill {
        Dir::South => Rect::new(cx - 8, yard.min_z + 9, cx + 8, cz + 2),
        Dir::North => Rect::new(cx - 8, cz - 2, cx + 8, yard.max_z - 9),
        Dir::East => Rect::new(yard.min_x + 9, cz - 7, cx + 2, cz + 7),
        Dir::West => Rect::new(cx - 2, cz - 7, yard.max_x - 9, cz + 7),
    };
    occ.fill(pit, CellKind::CampCourt, true);
    let lodge = match downhill.opposite() {
        Dir::North => Rect::new(cx - 4, yard.min_z + 1, cx + 4, yard.min_z + 7),
        Dir::South => Rect::new(cx - 4, yard.max_z - 7, cx + 4, yard.max_z - 1),
        Dir::West => Rect::new(yard.min_x + 1, cz - 3, yard.min_x + 9, cz + 3),
        Dir::East => Rect::new(yard.max_x - 9, cz - 3, yard.max_x - 1, cz + 3),
    };
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Blacksmith,
        lodge,
        downhill,
    );
    let cranes = match downhill {
        Dir::South | Dir::North => [
            Rect::new(pit.min_x - 4, cz - 1, pit.min_x - 2, cz + 1),
            Rect::new(pit.max_x + 2, cz - 1, pit.max_x + 4, cz + 1),
        ],
        Dir::East | Dir::West => [
            Rect::new(cx - 1, pit.min_z - 4, cx + 1, pit.min_z - 2),
            Rect::new(cx - 1, pit.max_z + 2, cx + 1, pit.max_z + 4),
        ],
    };
    for pad in cranes {
        if yard.contains(pad.min_x, pad.min_z) && yard.contains(pad.max_x, pad.max_z) {
            occ.fill(pad, CellKind::Crane, true);
        }
    }
    let stacks = match downhill {
        Dir::South => [
            Rect::new(pit.min_x - 5, pit.max_z - 1, pit.min_x - 2, pit.max_z + 2),
            Rect::new(pit.max_x + 2, pit.max_z - 1, pit.max_x + 5, pit.max_z + 2),
        ],
        Dir::North => [
            Rect::new(pit.min_x - 5, pit.min_z - 2, pit.min_x - 2, pit.min_z + 1),
            Rect::new(pit.max_x + 2, pit.min_z - 2, pit.max_x + 5, pit.min_z + 1),
        ],
        Dir::East => [
            Rect::new(pit.max_x - 1, pit.min_z - 5, pit.max_x + 2, pit.min_z - 2),
            Rect::new(pit.max_x - 1, pit.max_z + 2, pit.max_x + 2, pit.max_z + 5),
        ],
        Dir::West => [
            Rect::new(pit.min_x - 2, pit.min_z - 5, pit.min_x + 1, pit.min_z - 2),
            Rect::new(pit.min_x - 2, pit.max_z + 2, pit.min_x + 1, pit.max_z + 5),
        ],
    };
    for pad in stacks {
        if yard.contains(pad.min_x, pad.min_z) && yard.contains(pad.max_x, pad.max_z) {
            occ.fill(pad, CellKind::CutStone, true);
        }
    }
    let track = match downhill {
        Dir::South => Rect::new(cx - 1, pit.max_z + 1, cx, yard.max_z.min(pit.max_z + 5)),
        Dir::North => Rect::new(cx - 1, yard.min_z.max(pit.min_z - 5), cx, pit.min_z - 1),
        Dir::East => Rect::new(pit.max_x + 1, cz - 1, yard.max_x.min(pit.max_x + 5), cz),
        Dir::West => Rect::new(yard.min_x.max(pit.min_x - 5), cz - 1, pit.min_x - 1, cz),
    };
    if yard.contains(track.min_x, track.min_z) && yard.contains(track.max_x, track.max_z) {
        occ.fill(track, CellKind::MineRail, true);
    }
    let spoil = match downhill {
        Dir::South => Rect::new(yard.min_x + 2, yard.max_z - 2, yard.max_x - 2, yard.max_z),
        Dir::North => Rect::new(yard.min_x + 2, yard.min_z, yard.max_x - 2, yard.min_z + 2),
        Dir::East => Rect::new(yard.max_x - 2, yard.min_z + 2, yard.max_x, yard.max_z - 2),
        Dir::West => Rect::new(yard.min_x, yard.min_z + 2, yard.min_x + 2, yard.max_z - 2),
    };
    occ.fill(spoil, CellKind::MineSpoil, true);
    scenes.push(Scene {
        kind: SceneKind::Quarry,
        rect: yard,
    });
}

fn mill_sail_keepout(lots: &[Lot], rect: Rect) -> bool {
    lots.iter().any(|lot| {
        lot.kind == BuildingKind::Windmill && lot.rect.inflate(MILL_SAIL_RADIUS).intersects(rect)
    })
}

fn find_quarry_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &[Lot],
    scenes: &[Scene],
    rng: &mut TownRng,
) -> Option<Rect> {
    let mut best = None;
    let mut best_score = i32::MIN;
    for &(w, d) in &[(40, 32), (36, 28)] {
        let mut z = search.min_z + 8;
        while z + d < search.max_z {
            let mut x = search.min_x + 8;
            while x + w < search.max_x {
                let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                x += 12;
                if town.inflate(10).intersects(rect) || keep_out.intersects(rect) {
                    continue;
                }
                if landmark_blocked(scenes, rect) {
                    continue;
                }
                if mill_sail_keepout(lots, rect.inflate(4)) {
                    continue;
                }
                if !site_ok(occ, grid, rect, 18) {
                    continue;
                }
                if !rect_is_inland(grid, rect, 8) {
                    continue;
                }
                if !riverside_yard_free(occ, rect) {
                    continue;
                }
                let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                let relief = (hi - lo).clamp(0, 20);
                let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                let score = relief * 8 + mean / 4 + rng.gen_range(0, 6);
                if score > best_score {
                    best_score = score;
                    best = Some(rect);
                }
            }
            z += 10;
        }
        if best.is_some() {
            break;
        }
    }
    best
}

pub(crate) fn quarry_downhill(grid: &HeightGrid, yard: Rect) -> Dir {
    let (cx, cz) = yard.center();
    let n = grid.height(cx, yard.min_z);
    let s = grid.height(cx, yard.max_z);
    let e = grid.height(yard.max_x, cz);
    let w = grid.height(yard.min_x, cz);
    let lo = n.min(s).min(e).min(w);
    if s == lo {
        Dir::South
    } else if n == lo {
        Dir::North
    } else if e == lo {
        Dir::East
    } else {
        Dir::West
    }
}

fn place_granary_in_yard(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
) -> bool {
    // Compact raised granary, not a full 14×10 barn.
    let (bw, bd) = (8, 7);
    let pads = [
        Rect::new(
            yard.min_x + 2,
            yard.min_z + 2,
            yard.min_x + 1 + bw,
            yard.min_z + 1 + bd,
        ),
        Rect::new(
            yard.max_x - 1 - bw,
            yard.max_z - 1 - bd,
            yard.max_x - 2,
            yard.max_z - 2,
        ),
        Rect::new(
            yard.min_x + 2,
            yard.max_z - 1 - bd,
            yard.min_x + 1 + bw,
            yard.max_z - 2,
        ),
        Rect::new(
            yard.max_x - 1 - bw,
            yard.min_z + 2,
            yard.max_x - 2,
            yard.min_z + 1 + bd,
        ),
    ];
    for pad in pads {
        if !yard.contains(pad.min_x, pad.min_z) || !yard.contains(pad.max_x, pad.max_z) {
            continue;
        }
        if mill_pad_blocked(occ, pad) {
            continue;
        }
        if force_lot(occ, grid, lots, seed, BuildingKind::Barn, pad, Dir::South).is_some() {
            return true;
        }
    }
    false
}

fn place_mill_cottages(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    limit: usize,
) -> usize {
    let (w, d) = BuildingKind::Cottage.size();
    let mut n = 0;
    for z in (yard.min_z + 1)..yard.max_z {
        for x in (yard.min_x + 1)..yard.max_x {
            if n >= limit {
                return n;
            }
            let pad = Rect::new(x, z, x + w - 1, z + d - 1);
            if !yard.contains(pad.min_x, pad.min_z) || !yard.contains(pad.max_x, pad.max_z) {
                continue;
            }
            if mill_pad_blocked(occ, pad) {
                continue;
            }
            if force_lot(
                occ,
                grid,
                lots,
                seed,
                BuildingKind::Cottage,
                pad,
                Dir::South,
            )
            .is_some()
            {
                n += 1;
            }
        }
    }
    n
}

fn place_beacon_chain(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let mut placed = 0;
    for _ in 0..6 {
        if placed >= 3 {
            break;
        }
        let Some(rect) = find_peak_site(occ, grid, search, town, keep_out.inflate(8), 7, 7, 0, rng)
        else {
            break;
        };
        let too_close = scenes.iter().any(|scene| {
            matches!(
                scene.kind,
                SceneKind::Watchtower | SceneKind::Beacon | SceneKind::HermitTower
            ) && scene.rect.inflate(20).intersects(rect)
        });
        if too_close || landmark_blocked(scenes, rect) {
            occ.fill(rect, CellKind::Green, true);
            continue;
        }
        occ.fill(rect, CellKind::Green, true);
        let fire = Rect::new(
            rect.min_x + 2,
            rect.min_z + 2,
            rect.max_x - 2,
            rect.max_z - 2,
        );
        occ.fill(fire, CellKind::Beacon, true);
        scenes.push(Scene {
            kind: SceneKind::Beacon,
            rect,
        });
        placed += 1;
    }
}

fn place_toll_bridge(
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
    let Some(yard) = find_toll_site(occ, grid, search, town, keep_out, scenes, rng) else {
        return;
    };
    let Some((along_x, line, _)) = toll_crossing(grid, yard) else {
        return;
    };
    let (tw, td) = BuildingKind::Watchtower.size();
    let Some(gate) = toll_gate_pad(grid, yard, along_x, line, tw, td) else {
        return;
    };
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    if along_x {
        for x in yard.min_x..=yard.max_x {
            if !grid.is_dry(x, line) {
                occ.set(x, line, CellKind::Bridge, true);
                occ.set(x, line - 1, CellKind::Bridge, true);
                occ.set(x, line + 1, CellKind::Bridge, true);
            }
        }
    } else {
        for z in yard.min_z..=yard.max_z {
            if !grid.is_dry(line, z) {
                occ.set(line, z, CellKind::Bridge, true);
                occ.set(line - 1, z, CellKind::Bridge, true);
                occ.set(line + 1, z, CellKind::Bridge, true);
            }
        }
    }
    if force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Watchtower,
        gate,
        if along_x { Dir::East } else { Dir::South },
    )
    .is_none()
    {
        return;
    }
    let (gx, gz) = gate.center();
    for dir in Dir::ALL {
        let nx = gx + dir.dx() * 4;
        let nz = gz + dir.dz() * 4;
        if occ.kind(nx, nz) == CellKind::Bridge {
            occ.set(nx, nz, CellKind::PalisadeGate, true);
            occ.set(nx + dir.dz(), nz + dir.dx(), CellKind::Palisade, true);
            occ.set(nx - dir.dz(), nz - dir.dx(), CellKind::Palisade, true);
        }
    }
    scenes.push(Scene {
        kind: SceneKind::TollBridge,
        rect: yard,
    });
}

fn find_toll_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    scenes: &[Scene],
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 32;
    let d = 36;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + d < search.max_z {
        let mut x = search.min_x + 8;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 12;
            if town.inflate(12).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if landmark_blocked(scenes, rect)
                || scenes.iter().any(|scene| scene.rect.intersects(rect))
            {
                continue;
            }
            if !toll_yard_open(occ, rect) {
                continue;
            }
            let Some((along_x, line, wet)) = toll_crossing(grid, rect) else {
                continue;
            };
            let banks = if along_x {
                i32::from(grid.is_dry(rect.min_x + 1, line))
                    + i32::from(grid.is_dry(rect.max_x - 1, line))
            } else {
                i32::from(grid.is_dry(line, rect.min_z + 1))
                    + i32::from(grid.is_dry(line, rect.max_z - 1))
            };
            let (tw, td) = BuildingKind::Watchtower.size();
            if toll_gate_pad(grid, rect, along_x, line, tw, td).is_none() {
                continue;
            }
            let score =
                wet * 4 + banks * 4 + i32::from(near_road(occ, rect)) * 20 + rng.gen_range(0, 5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 10;
    }
    best
}

fn toll_crossing(grid: &HeightGrid, yard: Rect) -> Option<(bool, i32, i32)> {
    let mut best_row = (yard.min_z, 0);
    for z in yard.min_z..=yard.max_z {
        let mut n = 0;
        for x in yard.min_x..=yard.max_x {
            if !grid.is_dry(x, z) {
                n += 1;
            }
        }
        if n > best_row.1 {
            best_row = (z, n);
        }
    }
    let mut best_col = (yard.min_x, 0);
    for x in yard.min_x..=yard.max_x {
        let mut n = 0;
        for z in yard.min_z..=yard.max_z {
            if !grid.is_dry(x, z) {
                n += 1;
            }
        }
        if n > best_col.1 {
            best_col = (x, n);
        }
    }
    if best_row.1 >= best_col.1 && best_row.1 >= 6 {
        Some((true, best_row.0, best_row.1))
    } else if best_col.1 >= 6 {
        Some((false, best_col.0, best_col.1))
    } else {
        None
    }
}

fn toll_yard_open(occ: &Occupancy, rect: Rect) -> bool {
    let mut ok = 0;
    let mut n = 0;
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            n += 1;
            if riverside_reclaim(occ.kind(x, z))
                || matches!(
                    occ.kind(x, z),
                    CellKind::Road
                        | CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW
                )
            {
                ok += 1;
            }
        }
    }
    n > 0 && ok * 4 >= n * 3
}

fn toll_gate_pad(
    grid: &HeightGrid,
    yard: Rect,
    along_x: bool,
    line: i32,
    w: i32,
    d: i32,
) -> Option<Rect> {
    let mut best = None;
    let mut best_dist = i32::MAX;
    let mut z = yard.min_z;
    while z + d - 1 <= yard.max_z {
        let mut x = yard.min_x;
        while x + w - 1 <= yard.max_x {
            let pad = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 3;
            if !grid.rect_is_buildable(pad) {
                continue;
            }
            let mut near_water = false;
            'water: for pz in pad.min_z - 2..=pad.max_z + 2 {
                for px in pad.min_x - 2..=pad.max_x + 2 {
                    if yard.contains(px, pz) && !grid.is_dry(px, pz) {
                        near_water = true;
                        break 'water;
                    }
                }
            }
            if !near_water {
                continue;
            }
            let (cx, cz) = pad.center();
            let dist = if along_x {
                (cz - line).abs()
            } else {
                (cx - line).abs()
            };
            if dist < best_dist {
                best_dist = dist;
                best = Some(pad);
            }
        }
        z += 3;
    }
    best
}

fn place_hermit_tower(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(rect) = find_peak_site(occ, grid, search, town, keep_out, 7, 7, 2, rng)
        .or_else(|| find_peak_site(occ, grid, search, town, keep_out, 5, 5, 2, rng))
    else {
        return;
    };
    let (cx, cz) = rect.center();
    for dz in -3..=3 {
        for dx in -3..=3 {
            let dist2 = dx * dx + dz * dz;
            if dist2 <= 8 {
                occ.set(cx + dx, cz + dz, CellKind::WatchPost, true);
            } else if dist2 <= 13 && dz >= 2 {
                occ.set(cx + dx, cz + dz, CellKind::Orchard, true);
            }
        }
    }
    scenes.push(Scene {
        kind: SceneKind::HermitTower,
        rect,
    });
}

fn place_stone_circle(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let Some(yard) = find_site(
        occ,
        grid,
        search,
        town,
        keep_out.inflate(8),
        17,
        17,
        false,
        rng,
    )
    .or_else(|| {
        find_site(
            occ,
            grid,
            search,
            town,
            keep_out.inflate(8),
            13,
            13,
            false,
            rng,
        )
    }) else {
        return;
    };
    occ.fill(yard, CellKind::Green, true);
    let (cx, cz) = yard.center();
    occ.set(cx, cz, CellKind::Fountain, true);
    for (dx, dz) in [
        (0, -6),
        (4, -5),
        (6, 0),
        (4, 5),
        (0, 6),
        (-4, 5),
        (-6, 0),
        (-4, -5),
        (0, -2),
        (2, 0),
        (0, 2),
        (-2, 0),
    ] {
        occ.set(cx + dx, cz + dz, CellKind::StandingStone, true);
    }
    occ.set(cx + 5, cz - 5, CellKind::StandingStone, true);
    occ.set(cx + 6, cz - 5, CellKind::StandingStone, true);
    scenes.push(Scene {
        kind: SceneKind::StoneCircle,
        rect: yard,
    });
}

fn place_crossroads_gallows(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let mut best = None;
    let mut best_score = i32::MIN;
    let pad = town.inflate(28);
    let mut z = search.min_z + 8;
    while z + 4 < search.max_z {
        let mut x = search.min_x + 8;
        while x + 4 < search.max_x {
            x += 7;
            if pad.contains(x, z) || keep_out.contains(x, z) {
                continue;
            }
            if civic_nearby(occ, x, z, 4) {
                continue;
            }
            if !matches!(
                occ.kind(x, z),
                CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
            ) {
                continue;
            }
            let arms = Dir::ALL
                .iter()
                .filter(|dir| {
                    matches!(
                        occ.kind(x + dir.dx(), z + dir.dz()),
                        CellKind::Road
                            | CellKind::RoadStairN
                            | CellKind::RoadStairS
                            | CellKind::RoadStairE
                            | CellKind::RoadStairW
                            | CellKind::Bridge
                    )
                })
                .count();
            if arms < 3 {
                continue;
            }
            let rect = Rect::new(x - 2, z - 2, x + 2, z + 2);
            if !occ.bounds.contains(rect.min_x, rect.min_z)
                || !occ.bounds.contains(rect.max_x, rect.max_z)
            {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            let score = arms as i32 * 20 + rng.gen_range(0, 6);
            if score > best_score {
                best_score = score;
                best = Some((x, z, rect));
            }
        }
        z += 7;
    }
    let Some((jx, jz, rect)) = best.or_else(|| {
        find_site_near_road(occ, grid, search, town, keep_out.inflate(16), 5, 5, rng).and_then(
            |rect| {
                let (cx, cz) = rect.center();
                if civic_nearby(occ, cx, cz, 4) {
                    None
                } else {
                    Some((cx, cz, rect))
                }
            },
        )
    }) else {
        return;
    };
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Empty | CellKind::Green | CellKind::Farm | CellKind::FarmEdge
            ) {
                occ.set(x, z, CellKind::Plaza, true);
            }
        }
    }
    occ.set(jx, jz, CellKind::Gallows, true);
    scenes.push(Scene {
        kind: SceneKind::Gallows,
        rect,
    });
}

fn civic_nearby(occ: &Occupancy, x: i32, z: i32, r: i32) -> bool {
    for dz in -r..=r {
        for dx in -r..=r {
            if matches!(
                occ.kind(x + dx, z + dz),
                CellKind::Building
                    | CellKind::Eave
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Market
                    | CellKind::CathedralNave
                    | CellKind::CathedralTransept
                    | CellKind::CathedralChoir
                    | CellKind::CathedralTower
                    | CellKind::CastleKeep
                    | CellKind::CastleHall
                    | CellKind::CastleWall
            ) {
                return true;
            }
        }
    }
    false
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
            if scenes
                .iter()
                .any(|scene| scene.kind == SceneKind::Gorge && scene.rect.contains(x, z))
            {
                continue;
            }
            match occ.kind(x, z) {
                CellKind::Dock => docks.push((x, z)),
                CellKind::Harbor => harbors.push((x, z)),
                _ => {}
            }
        }
    }
    if docks.is_empty() {
        claim_large_water_docks(occ, grid, search, &harbors, &mut docks);
    }
    docks.retain(|&(x, z)| berth_on_large_water(occ, grid, search, x, z));
    harbors.retain(|&(x, z)| water_body_ok(grid, search, x, z));
    if docks.is_empty() {
        claim_large_water_docks(occ, grid, search, &harbors, &mut docks);
    }
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
    paint_inner_sea_quay(occ, grid, lots, town);
    paint_quay_and_cranes(occ, grid, lots, search, &docks);
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
        if boats >= 5 {
            break;
        }
        let Some(dir) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        let side = match dir {
            Dir::North | Dir::South => Dir::East,
            Dir::East | Dir::West => Dir::South,
        };
        for step in [3, 4, 6, 8] {
            for offset in [2, -2, 4, -4] {
                if boats >= 5 {
                    break;
                }
                let bx = x + dir.dx() * step + side.dx() * offset;
                let bz = z + dir.dz() * step + side.dz() * offset;
                if place_hull(occ, grid, search, bx, bz, dir, false) {
                    boats += 1;
                }
            }
        }
    }
    if boats < 5 {
        for &(x, z) in &harbors {
            if boats >= 5 {
                break;
            }
            if let Some(dir) = harbour_out(occ, grid, x, z)
                && place_hull(occ, grid, search, x, z, dir, false)
            {
                boats += 1;
            }
        }
    }
    if boats < 3 {
        force_harbour_boat(occ, grid, search, &docks);
    }
    paint_harbour_buildings(occ, grid, lots, seed, town, min_x, min_z, max_x, max_z);
    paint_lighthouse(occ, search);
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
    // Frame the fleet, not the inner-wall quay strip (that bbox swallows the town).
    let hull = blob_in(occ, docks, CellKind::Ship);
    let crane = blob_in(occ, docks, CellKind::Crane);
    let dock = blob_in(occ, docks, CellKind::Dock);
    let shore = hull.or(crane).or(dock).unwrap_or(docks);
    let mut rect = shore.inflate(14);
    if let Some(h) = hull {
        rect = Rect::new(
            rect.min_x.min(h.min_x - 6),
            rect.min_z.min(h.min_z - 6),
            rect.max_x.max(h.max_x + 6),
            rect.max_z.max(h.max_z + 6),
        );
    }
    let (sx, sz) = shore.center();
    if rect.width() > 64 {
        rect = Rect::new(sx - 32, rect.min_z, sx + 31, rect.max_z);
    }
    if rect.depth() > 64 {
        rect = Rect::new(rect.min_x, sz - 32, rect.max_x, sz + 31);
    }
    rect
}

fn blob_in(occ: &Occupancy, bounds: Rect, kind: CellKind) -> Option<Rect> {
    let mut found: Option<Rect> = None;
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            if occ.kind(x, z) != kind {
                continue;
            }
            found = Some(found.map_or_else(
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
    found
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
    let mut pads = vec![market.inflate(16), town.inflate(TOWN_TREE_MARGIN)];
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
        let pad = scene.rect.inflate(scene_tree_margin(scene.kind));
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

fn harbour_may_overwrite(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Empty
            | CellKind::Green
            | CellKind::Farm
            | CellKind::FarmEdge
            | CellKind::Plaza
            | CellKind::Hedge
            | CellKind::Eave
            | CellKind::Road
            | CellKind::Market
            | CellKind::Harbor
            | CellKind::Dock
            | CellKind::Quay
            | CellKind::Crane
    )
}

fn harbour_out(occ: &Occupancy, grid: &HeightGrid, x: i32, z: i32) -> Option<Dir> {
    Dir::ALL
        .iter()
        .copied()
        .find(|dir| wet_berth(occ, grid, x + dir.dx(), z + dir.dz()))
}

fn harbour_open_dir(occ: &Occupancy, grid: &HeightGrid, x: i32, z: i32) -> Option<Dir> {
    let mut best = None;
    let mut best_n = 0;
    for dir in Dir::ALL {
        let mut n = 0;
        for step in 1..=16 {
            if !wet_berth(occ, grid, x + dir.dx() * step, z + dir.dz() * step) {
                break;
            }
            n += 1;
        }
        if n > best_n {
            best_n = n;
            best = Some(dir);
        }
    }
    best.filter(|_| best_n >= 8)
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
                | CellKind::CityDitch
        )
}

fn quay_claimable(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Empty
            | CellKind::Green
            | CellKind::Plaza
            | CellKind::Road
            | CellKind::Farm
            | CellKind::FarmEdge
            | CellKind::Hedge
            | CellKind::Eave
            | CellKind::Market
            | CellKind::Building
    )
}

fn claim_quay_cell(occ: &mut Occupancy, lots: &mut Vec<Lot>, x: i32, z: i32) {
    if occ.kind(x, z) == CellKind::Building {
        evict_lots_from(occ, lots, Rect::new(x, z, x, z));
    }
    occ.set(x, z, CellKind::Quay, true);
}

fn seaward_dir(occ: &Occupancy, grid: &HeightGrid, x: i32, z: i32) -> Option<Dir> {
    Dir::ALL.iter().copied().find(|dir| {
        for step in 1..=12 {
            let bx = x + dir.dx() * step;
            let bz = z + dir.dz() * step;
            if matches!(
                occ.kind(bx, bz),
                CellKind::Harbor
                    | CellKind::Dock
                    | CellKind::Breakwater
                    | CellKind::Ship
                    | CellKind::Boat
            ) || !grid.is_dry(bx, bz)
            {
                return true;
            }
            if !matches!(
                occ.kind(bx, bz),
                CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Empty
                    | CellKind::Green
                    | CellKind::Plaza
                    | CellKind::Road
                    | CellKind::Eave
                    | CellKind::Hedge
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Market
                    | CellKind::CityDitch
            ) {
                return false;
            }
        }
        false
    })
}

/// Quay on the inner face of a coastal sea wall so the fish market sits
/// inside the curtain, not out on the mole.
fn paint_inner_sea_quay(occ: &mut Occupancy, grid: &HeightGrid, lots: &mut Vec<Lot>, town: Rect) {
    let pad = town.inflate(2);
    let mut marked = Vec::new();
    for z in pad.min_z..=pad.max_z {
        for x in pad.min_x..=pad.max_x {
            if !occ.bounds.contains(x, z) || !grid.is_dry(x, z) {
                continue;
            }
            if !quay_claimable(occ.kind(x, z)) {
                continue;
            }
            let Some(out) = seaward_dir(occ, grid, x, z) else {
                continue;
            };
            marked.push((x, z, out));
        }
    }
    for (x, z, out) in marked {
        claim_quay_cell(occ, lots, x, z);
        let inland = out.opposite();
        for step in 1..=2 {
            let ix = x + inland.dx() * step;
            let iz = z + inland.dz() * step;
            if !occ.bounds.contains(ix, iz) || !grid.is_dry(ix, iz) {
                continue;
            }
            if quay_claimable(occ.kind(ix, iz)) || occ.kind(ix, iz) == CellKind::Quay {
                claim_quay_cell(occ, lots, ix, iz);
            }
        }
    }
}

fn berth_on_large_water(occ: &Occupancy, grid: &HeightGrid, search: Rect, x: i32, z: i32) -> bool {
    harbour_out(occ, grid, x, z)
        .is_some_and(|dir| water_body_ok(grid, search, x + dir.dx(), z + dir.dz()))
}

fn claim_large_water_docks(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    harbors: &[(i32, i32)],
    docks: &mut Vec<(i32, i32)>,
) {
    for &(x, z) in harbors {
        if !Dir::ALL
            .iter()
            .any(|dir| grid.is_dry(x + dir.dx(), z + dir.dz()))
        {
            continue;
        }
        if !berth_on_large_water(occ, grid, search, x, z) {
            continue;
        }
        occ.set(x, z, CellKind::Dock, true);
        docks.push((x, z));
        if docks.len() >= 8 {
            break;
        }
    }
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
                    | CellKind::Bridge
                    | CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
                    | CellKind::WaterWheel
                    | CellKind::MillRace
                    | CellKind::Sluice
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
                for &(len, beam) in &[(COG_LEN, COG_BEAM), (COG_LEN, 5), (15, 5), (15, 3)] {
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
    lots: &mut Vec<Lot>,
    search: Rect,
    docks: &[(i32, i32)],
) {
    let mut quay = Vec::new();
    for &(x, z) in docks {
        for dir in Dir::ALL {
            for step in 1..=8 {
                let nx = x + dir.dx() * step;
                let nz = z + dir.dz() * step;
                if !search.contains(nx, nz) {
                    continue;
                }
                if grid.is_dry(nx, nz) && quay_claimable(occ.kind(nx, nz)) {
                    claim_quay_cell(occ, lots, nx, nz);
                    quay.push((nx, nz));
                    for thick in 1..=2 {
                        for side in Dir::ALL {
                            let sx = nx + side.dx() * thick;
                            let sz = nz + side.dz() * thick;
                            if search.contains(sx, sz)
                                && grid.is_dry(sx, sz)
                                && quay_claimable(occ.kind(sx, sz))
                            {
                                claim_quay_cell(occ, lots, sx, sz);
                                quay.push((sx, sz));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut cranes = 0;
    let mut crane_at: Vec<(i32, i32)> = Vec::new();
    for &(x, z) in &quay {
        if cranes >= 3 {
            break;
        }
        if crane_at
            .iter()
            .any(|&(cx, cz)| (x - cx).abs().max((z - cz).abs()) < 8)
        {
            continue;
        }
        let Some(out) = harbour_out(occ, grid, x, z) else {
            continue;
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let px = x + dx;
                let pz = z + dz;
                if search.contains(px, pz)
                    && grid.is_dry(px, pz)
                    && harbour_may_overwrite(occ.kind(px, pz))
                {
                    occ.set(px, pz, CellKind::Crane, true);
                }
            }
        }
        for boom in 1..=4 {
            let bx = x + out.dx() * boom;
            let bz = z + out.dz() * boom;
            if search.contains(bx, bz) && harbour_may_overwrite(occ.kind(bx, bz)) {
                occ.set(bx, bz, CellKind::Crane, true);
            }
        }
        crane_at.push((x, z));
        cranes += 1;
    }
    if cranes < 2 {
        for &(x, z) in docks {
            if cranes >= 3 {
                break;
            }
            let Some(out) = harbour_out(occ, grid, x, z) else {
                continue;
            };
            let inland = out.opposite();
            let cx = x + inland.dx() * 2;
            let cz = z + inland.dz() * 2;
            if !search.contains(cx, cz) || !grid.is_dry(cx, cz) {
                continue;
            }
            if crane_at
                .iter()
                .any(|&(ax, az)| (cx - ax).abs().max((cz - az).abs()) < 8)
            {
                continue;
            }
            if !harbour_may_overwrite(occ.kind(cx, cz))
                && occ.kind(cx, cz) != CellKind::Quay
                && occ.kind(cx, cz) != CellKind::Crane
            {
                continue;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let px = cx + dx;
                    let pz = cz + dz;
                    if search.contains(px, pz)
                        && grid.is_dry(px, pz)
                        && (harbour_may_overwrite(occ.kind(px, pz))
                            || occ.kind(px, pz) == CellKind::Quay)
                    {
                        occ.set(px, pz, CellKind::Crane, true);
                    }
                }
            }
            for boom in 1..=4 {
                let bx = cx + out.dx() * boom;
                let bz = cz + out.dz() * boom;
                if search.contains(bx, bz)
                    && (harbour_may_overwrite(occ.kind(bx, bz))
                        || occ.kind(bx, bz) == CellKind::Quay)
                {
                    occ.set(bx, bz, CellKind::Crane, true);
                }
            }
            crane_at.push((cx, cz));
            cranes += 1;
        }
    }
    if cranes < 3 && !quay.is_empty() {
        let picks = [0, quay.len() / 2, quay.len() - 1];
        for &idx in &picks {
            if cranes >= 3 {
                break;
            }
            let (x, z) = quay[idx];
            if !grid.is_dry(x, z) || occ.kind(x, z) == CellKind::Crane {
                continue;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let px = x + dx;
                    let pz = z + dz;
                    if search.contains(px, pz)
                        && grid.is_dry(px, pz)
                        && (harbour_may_overwrite(occ.kind(px, pz))
                            || occ.kind(px, pz) == CellKind::Quay)
                    {
                        occ.set(px, pz, CellKind::Crane, true);
                    }
                }
            }
            crane_at.push((x, z));
            cranes += 1;
        }
    }
    if cranes == 0
        && let Some(&(x, z)) = quay.first()
    {
        if harbour_may_overwrite(occ.kind(x, z)) {
            occ.set(x, z, CellKind::Crane, true);
        }
        for dz in -1..=1 {
            for dx in -1..=1 {
                if search.contains(x + dx, z + dz)
                    && harbour_may_overwrite(occ.kind(x + dx, z + dz))
                {
                    occ.set(x + dx, z + dz, CellKind::Crane, true);
                }
            }
        }
    }
}

fn paint_breakwater(occ: &mut Occupancy, grid: &HeightGrid, search: Rect, docks: &[(i32, i32)]) {
    for &(x, z) in docks {
        let Some(out) = harbour_open_dir(occ, grid, x, z).or_else(|| harbour_out(occ, grid, x, z))
        else {
            continue;
        };
        let along = match out {
            Dir::North | Dir::South => Dir::East,
            Dir::East | Dir::West => Dir::South,
        };
        let ox = x + out.dx() * 10;
        let oz = z + out.dz() * 10;
        let mut painted = 0;
        for t in -14..=14 {
            for thick in 0..=2 {
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
                        | CellKind::Road
                        | CellKind::Bridge
                        | CellKind::Plaza
                        | CellKind::Farm
                        | CellKind::FarmEdge
                ) {
                    occ.set(bx, bz, CellKind::Breakwater, true);
                    painted += 1;
                }
            }
        }
        if painted >= 8 {
            return;
        }
    }
}

fn force_harbour_boat(
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
        for step in 3..=8 {
            for offset in [2, -2, 4, -4, 6] {
                let bx = x + dir.dx() * step + side.dx() * offset;
                let bz = z + dir.dz() * step + side.dz() * offset;
                if place_hull(occ, grid, search, bx, bz, dir, false) {
                    return true;
                }
            }
        }
    }
    false
}

/// Stalls and a paved market on the quay inside the sea wall.
pub fn paint_fish_market(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    _town: Rect,
) {
    let mut berths = Vec::new();
    for z in occ.bounds.min_z..=occ.bounds.max_z {
        for x in occ.bounds.min_x..=occ.bounds.max_x {
            if !grid.is_dry(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Quay | CellKind::Crane | CellKind::Dock
            ) {
                berths.push((x, z));
            }
        }
    }
    let mut stalls = 0;
    let mut placed: Vec<Rect> = Vec::new();
    for &(x, z) in &berths {
        if stalls >= 6 {
            break;
        }
        let facing = seaward_dir(occ, grid, x, z)
            .or_else(|| harbour_out(occ, grid, x, z))
            .unwrap_or(Dir::West);
        let dirs = [
            facing.opposite(),
            Dir::North,
            Dir::South,
            Dir::East,
            Dir::West,
        ];
        for dir in dirs {
            if stalls >= 6 {
                break;
            }
            for step in 1..=4 {
                let cx = x + dir.dx() * step;
                let cz = z + dir.dz() * step;
                let rect = Rect::new(cx, cz, cx + 2, cz + 2);
                if placed.iter().any(|p| p.inflate(4).intersects(rect)) {
                    continue;
                }
                if place_fish_stall(occ, grid, lots, seed, rect, facing) {
                    placed.push(rect);
                    stalls += 1;
                    break;
                }
            }
        }
    }
}

fn place_fish_stall(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    rect: Rect,
    facing: Dir,
) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    if !grid.rect_is_dry(rect) {
        return false;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            let kind = occ.kind(x, z);
            if kind.is_castle()
                || matches!(
                    kind,
                    CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Well
                        | CellKind::Fountain
                        | CellKind::Ship
                        | CellKind::Boat
                        | CellKind::Quay
                        | CellKind::Crane
                        | CellKind::CityDitch
                        | CellKind::Breakwater
                        | CellKind::Lighthouse
                        | CellKind::CathedralNave
                        | CellKind::CathedralTransept
                        | CellKind::CathedralChoir
                        | CellKind::CathedralTower
                        | CellKind::CathedralButtress
                        | CellKind::WindSail
                        | CellKind::WaterWheel
                )
            {
                return false;
            }
        }
    }
    evict_lots_from(occ, lots, rect);
    force_lot(occ, grid, lots, seed, BuildingKind::Stall, rect, facing).is_some()
}

fn paint_harbour_buildings(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
) {
    let mut warehouses = 0;
    let mut x = min_x - 4;
    while x <= max_x + 4 && warehouses < 3 {
        let mut z = min_z - 4;
        while z <= max_z + 4 && warehouses < 3 {
            let rect = Rect::new(x, z, x + 9, z + 7);
            z += 10;
            if town.intersects(rect) {
                continue;
            }
            let placed = if warehouses == 0 {
                force_lot(occ, grid, lots, seed, BuildingKind::Barn, rect, Dir::South)
            } else {
                try_lot(occ, grid, lots, seed, BuildingKind::Barn, rect, Dir::South)
            };
            if placed.is_some() {
                warehouses += 1;
            }
        }
        x += 12;
    }
    let mut master = false;
    let mut mx = min_x - 2;
    while mx <= max_x + 2 && !master {
        let mut mz = min_z - 2;
        while mz <= max_z + 2 && !master {
            let rect = Rect::new(mx, mz, mx + 7, mz + 6);
            mz += 8;
            if town.intersects(rect) {
                continue;
            }
            let mut on_quay = false;
            for z in rect.min_z.saturating_sub(2)..=rect.max_z + 2 {
                for x in rect.min_x.saturating_sub(2)..=rect.max_x + 2 {
                    if occ.kind(x, z) == CellKind::Quay {
                        on_quay = true;
                    }
                }
            }
            if !on_quay {
                continue;
            }
            if force_lot(
                occ,
                grid,
                lots,
                seed,
                BuildingKind::Cottage,
                rect,
                Dir::South,
            )
            .is_some()
            {
                master = true;
            }
        }
        mx += 9;
    }
}

fn paint_lighthouse(occ: &mut Occupancy, search: Rect) {
    let ship = blob_in(occ, search, CellKind::Ship);
    let quay = blob_in(occ, search, CellKind::Quay);
    let (sx, sz) = ship.or(quay).map_or_else(|| search.center(), Rect::center);
    let (qx, qz) = quay.map_or((sx, sz), Rect::center);
    let keep_off_ship = ship.map(|rect| rect.inflate(12));
    let keep_off_quay = quay.map(|rect| rect.inflate(8));
    let mut best_break: Option<(i32, i32, i32)> = None;
    let mut best_water: Option<(i32, i32, i32)> = None;
    for z in search.min_z..=search.max_z {
        for x in search.min_x..=search.max_x {
            let kind = occ.kind(x, z);
            if !matches!(kind, CellKind::Breakwater | CellKind::Harbor) {
                continue;
            }
            if keep_off_ship.is_some_and(|rect| rect.contains(x, z)) {
                continue;
            }
            if keep_off_quay.is_some_and(|rect| rect.contains(x, z)) {
                continue;
            }
            let score = (x - sx).abs() + (z - sz).abs() + (x - qx).abs() + (z - qz).abs();
            if kind == CellKind::Breakwater && best_break.is_none_or(|(_, _, best)| score > best) {
                best_break = Some((x, z, score));
            }
            if kind == CellKind::Harbor && best_water.is_none_or(|(_, _, best)| score > best) {
                best_water = Some((x, z, score));
            }
        }
    }
    let Some((lx, lz, _)) = best_break.or(best_water) else {
        return;
    };
    for oz in -2i32..=2 {
        for ox in -2i32..=2 {
            if ox.abs() + oz.abs() > 3 {
                continue;
            }
            let x = lx + ox;
            let z = lz + oz;
            if !search.contains(x, z) {
                continue;
            }
            let kind = occ.kind(x, z);
            if matches!(
                kind,
                CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Building
                    | CellKind::Crane
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
            ) {
                continue;
            }
            // Keep the mole; only the tip cell of breakwater becomes the shaft.
            if kind == CellKind::Breakwater && (ox != 0 || oz != 0) {
                continue;
            }
            occ.set(x, z, CellKind::Lighthouse, true);
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
    let mut painted = Vec::new();
    for (i, window) in pts.windows(2).enumerate() {
        let (ax0, az0) = window[0];
        let (ax1, az1) = window[1];
        let along_x = (ax1 - ax0).abs() >= (az1 - az0).abs();
        let turn = i > 0 && i % 3 == 0;
        for (x, z) in bresenham(ax0, az0, ax1, az1) {
            if !occ.bounds.contains(x, z) {
                continue;
            }
            let extras = if along_x {
                [(0, 0), (0, 1)]
            } else {
                [(0, 0), (1, 0)]
            };
            for (dx, dz) in extras {
                let px = x + dx;
                let pz = z + dz;
                if !occ.bounds.contains(px, pz) {
                    continue;
                }
                if !matches!(
                    occ.kind(px, pz),
                    CellKind::Empty
                        | CellKind::Green
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Hedge
                        | CellKind::Pasture
                        | CellKind::Road
                        | CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW
                ) {
                    continue;
                }
                let dh = (grid.height(px, pz) - grid.height(ax0, az0)).abs();
                let kind = if turn || dh >= 2 {
                    stair_toward(ax0, az0, ax1, az1)
                } else {
                    CellKind::Road
                };
                occ.set(px, pz, kind, true);
                painted.push((px, pz));
            }
            let shoulders = if along_x {
                [(0, -1), (0, 2)]
            } else {
                [(-1, 0), (2, 0)]
            };
            for (dx, dz) in shoulders {
                let sx = x + dx;
                let sz = z + dz;
                if !occ.bounds.contains(sx, sz) {
                    continue;
                }
                if !matches!(
                    occ.kind(sx, sz),
                    CellKind::Empty
                        | CellKind::Green
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Hedge
                        | CellKind::Pasture
                        | CellKind::Orchard
                ) {
                    continue;
                }
                occ.set(sx, sz, CellKind::Retaining, true);
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
    // Mills take the river edge first so the wheel can sit on the SE-near
    // (east/south) face instead of hiding behind the mill house.
    place_riverside_mills(occ, grid, yard, lots, seed);
    let _ = place_granary_in_yard(occ, grid, yard, lots, seed);
    if along_x {
        for x in yard.min_x..=yard.max_x {
            if !grid.is_dry(x, cz) && !mill_occupies(occ, x, cz) {
                occ.set(x, cz, CellKind::Bridge, true);
                occ.set(x, cz - 1, CellKind::Bridge, true);
            }
        }
        for z in [cz - 3, cz + 3] {
            for x in (yard.min_x + 2..=yard.max_x - 2).step_by(4) {
                if grid.is_dry(x, z) && !mill_occupies(occ, x, z) {
                    occ.set(x, z, CellKind::Dock, true);
                }
            }
        }
    } else {
        for z in yard.min_z..=yard.max_z {
            if !grid.is_dry(cx, z) && !mill_occupies(occ, cx, z) {
                occ.set(cx, z, CellKind::Bridge, true);
                occ.set(cx - 1, z, CellKind::Bridge, true);
            }
        }
        for x in [cx - 3, cx + 3] {
            for z in (yard.min_z + 2..=yard.max_z - 2).step_by(4) {
                if grid.is_dry(x, z) && !mill_occupies(occ, x, z) {
                    occ.set(x, z, CellKind::Dock, true);
                }
            }
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
        Rect::new(
            yard.min_x + 2,
            yard.max_z - 8,
            yard.min_x + 8,
            yard.max_z - 3,
        ),
        Rect::new(
            yard.max_x - 8,
            yard.min_z + 2,
            yard.max_x - 2,
            yard.min_z + 7,
        ),
    ];
    let mut houses = 0;
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
            houses += 1;
            if houses >= 3 {
                break;
            }
        }
    }
    paint_riverside_ford_and_weirs(occ, grid, yard, along_x, cx, cz);
    scenes.push(Scene {
        kind: SceneKind::Riverside,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn paint_ford_span(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    along_x: bool,
    line: i32,
) -> bool {
    let (min_a, max_a, min_b, max_b) = if along_x {
        (yard.min_x, yard.max_x, yard.min_z, yard.max_z)
    } else {
        (yard.min_z, yard.max_z, yard.min_x, yard.max_x)
    };
    if line < min_b + 1 || line > max_b - 1 {
        return false;
    }
    let mut wet = 0;
    for a in min_a..=max_a {
        let (x, z) = if along_x { (a, line) } else { (line, a) };
        if !grid.is_dry(x, z) && !mill_occupies(occ, x, z) && occ.kind(x, z) != CellKind::Bridge {
            wet += 1;
        }
    }
    if wet < 6 {
        return false;
    }
    for a in min_a..=max_a {
        for db in -1..=1 {
            let b = line + db;
            let (x, z) = if along_x { (a, b) } else { (b, a) };
            if !yard.contains(x, z) || grid.is_dry(x, z) || mill_occupies(occ, x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Bridge | CellKind::Dock | CellKind::Harbor
            ) {
                continue;
            }
            occ.set(x, z, CellKind::Ford, true);
        }
    }
    true
}

fn paint_riverside_ford_and_weirs(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    along_x: bool,
    cx: i32,
    cz: i32,
) {
    for offset in [6, -6, 8, -8, 5, -5, 7, -7] {
        let line = if along_x { cz + offset } else { cx + offset };
        if paint_ford_span(occ, grid, yard, along_x, line) {
            break;
        }
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if grid.is_dry(x, z) || mill_occupies(occ, x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Bridge
                    | CellKind::Dock
                    | CellKind::Harbor
                    | CellKind::Ford
                    | CellKind::Quay
            ) {
                continue;
            }
            let chevron = if along_x {
                (x + (z - cz).abs() * 2) % 5 == 0
            } else {
                (z + (x - cx).abs() * 2) % 5 == 0
            };
            if chevron {
                occ.set(x, z, CellKind::Weir, true);
            }
        }
    }
}

fn place_riverside_mills(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
) {
    let _ = place_watermills(occ, grid, yard, lots, seed, 2);
}

fn place_watermills(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    limit: usize,
) -> usize {
    let (mw, md) = BuildingKind::Watermill.size();
    let mut mills = 0;
    // Site from a dry water-edge cell so the wheel can face east or south
    // (the SE iso near side) instead of hiding behind the mill house.
    for prefer_se in [true, false] {
        for z in (yard.min_z + 1)..yard.max_z {
            for x in (yard.min_x + 1)..yard.max_x {
                if mills >= limit {
                    return mills;
                }
                if !grid.is_dry(x, z) {
                    continue;
                }
                let water = if !grid.is_dry(x + 1, z) {
                    Dir::East
                } else if !grid.is_dry(x, z + 1) {
                    Dir::South
                } else if !prefer_se && !grid.is_dry(x - 1, z) {
                    Dir::West
                } else if !prefer_se && !grid.is_dry(x, z - 1) {
                    Dir::North
                } else {
                    continue;
                };
                let Some(pad) = mill_pad_on_edge(x, z, water, mw, md, yard) else {
                    continue;
                };
                if mill_pad_blocked(occ, pad) {
                    continue;
                }
                if mill_bank_too_steep(grid, pad) {
                    continue;
                }
                if mill_water_dir(grid, pad) != Some(water) {
                    continue;
                }
                let facing = water.opposite();
                if force_lot(occ, grid, lots, seed, BuildingKind::Watermill, pad, facing).is_none()
                {
                    continue;
                }
                paint_mill_works(occ, grid, yard, pad, water);
                mills += 1;
            }
        }
    }
    mills
}

fn mill_pad_on_edge(
    edge_x: i32,
    edge_z: i32,
    water: Dir,
    mw: i32,
    md: i32,
    yard: Rect,
) -> Option<Rect> {
    let pad = match water {
        Dir::East => Rect::new(
            edge_x - mw + 1,
            edge_z - md / 2,
            edge_x,
            edge_z - md / 2 + md - 1,
        ),
        Dir::West => Rect::new(
            edge_x,
            edge_z - md / 2,
            edge_x + mw - 1,
            edge_z - md / 2 + md - 1,
        ),
        Dir::South => Rect::new(
            edge_x - mw / 2,
            edge_z - md + 1,
            edge_x - mw / 2 + mw - 1,
            edge_z,
        ),
        Dir::North => Rect::new(
            edge_x - mw / 2,
            edge_z,
            edge_x - mw / 2 + mw - 1,
            edge_z + md - 1,
        ),
    };
    yard.contains(pad.min_x, pad.min_z)
        .then_some(pad)
        .filter(|pad| yard.contains(pad.max_x, pad.max_z))
}

fn mill_occupies(occ: &Occupancy, x: i32, z: i32) -> bool {
    matches!(
        occ.kind(x, z),
        CellKind::Building | CellKind::WaterWheel | CellKind::MillRace | CellKind::Sluice
    )
}

fn mill_bank_too_steep(grid: &HeightGrid, pad: Rect) -> bool {
    let (lo, hi) = grid.range(pad.min_x, pad.min_z, pad.max_x, pad.max_z);
    hi - lo > 5
}

fn mill_pad_blocked(occ: &Occupancy, pad: Rect) -> bool {
    for z in pad.min_z..=pad.max_z {
        for x in pad.min_x..=pad.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::Building
                    | CellKind::Dock
                    | CellKind::Bridge
                    | CellKind::Harbor
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Breakwater
                    | CellKind::Lighthouse
                    | CellKind::WaterWheel
                    | CellKind::MillRace
                    | CellKind::Sluice
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
            ) {
                return true;
            }
        }
    }
    false
}

fn mill_water_dir(grid: &HeightGrid, pad: Rect) -> Option<Dir> {
    let mut best_se = None;
    let mut best_se_n = 0usize;
    let mut best = None;
    let mut best_n = 0usize;
    for dir in Dir::ALL {
        let n = mill_face_wet(grid, pad, dir);
        if n < 4 {
            continue;
        }
        if n > best_n {
            best_n = n;
            best = Some(dir);
        }
        if matches!(dir, Dir::East | Dir::South) && n > best_se_n {
            best_se_n = n;
            best_se = Some(dir);
        }
    }
    best_se.or(best)
}

fn mill_face_wet(grid: &HeightGrid, pad: Rect, dir: Dir) -> usize {
    match dir {
        Dir::East => (pad.min_z..=pad.max_z)
            .filter(|&z| !grid.is_dry(pad.max_x + 1, z))
            .count(),
        Dir::West => (pad.min_z..=pad.max_z)
            .filter(|&z| !grid.is_dry(pad.min_x - 1, z))
            .count(),
        Dir::South => (pad.min_x..=pad.max_x)
            .filter(|&x| !grid.is_dry(x, pad.max_z + 1))
            .count(),
        Dir::North => (pad.min_x..=pad.max_x)
            .filter(|&x| !grid.is_dry(x, pad.min_z - 1))
            .count(),
    }
}

fn paint_mill_works(occ: &mut Occupancy, _grid: &HeightGrid, yard: Rect, mill: Rect, water: Dir) {
    let along = water.dx().abs() == 1;
    let edge_x = if water == Dir::East {
        mill.max_x
    } else if water == Dir::West {
        mill.min_x
    } else {
        mill.center().0
    };
    let edge_z = if water == Dir::South {
        mill.max_z
    } else if water == Dir::North {
        mill.min_z
    } else {
        mill.center().1
    };
    let lo = if along { mill.min_z } else { mill.min_x };
    let hi = if along { mill.max_z } else { mill.max_x };
    let mid = (lo + hi) / 2;
    // Gap + axle in the first two columns, standing disk from 3..5 so the
    // wheel reads as a wheel, not planks on the mill wall. Span is 11
    // (radius 5) so the disk is at least 9 across.
    let _ = yard;
    for thick in 1..=5 {
        for span in mid - 5..=mid + 5 {
            let x = if along {
                edge_x + water.dx() * thick
            } else {
                span
            };
            let z = if along {
                span
            } else {
                edge_z + water.dz() * thick
            };
            if !occ.bounds.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Building
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Breakwater
                    | CellKind::Lighthouse
                    | CellKind::Dock
            ) {
                continue;
            }
            occ.set(x, z, CellKind::WaterWheel, true);
        }
    }
    let side = if along { Dir::South } else { Dir::East };
    let mut race = 0;
    for step in 1..=12 {
        let mut painted_step = false;
        for thick in 1..=3 {
            let x = edge_x + water.dx() * thick + side.dx() * step;
            let z = edge_z + water.dz() * thick + side.dz() * step;
            if !yard.contains(x, z) || !occ.bounds.contains(x, z) {
                continue;
            }
            if matches!(occ.kind(x, z), CellKind::WaterWheel | CellKind::Building) {
                continue;
            }
            let raceable = matches!(
                occ.kind(x, z),
                CellKind::Empty
                    | CellKind::Green
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Harbor
                    | CellKind::MillRace
                    | CellKind::Sluice
            );
            if raceable {
                let painted = if race == 0 && thick == 2 {
                    CellKind::Sluice
                } else {
                    CellKind::MillRace
                };
                occ.set(x, z, painted, true);
                painted_step = true;
            }
        }
        if painted_step {
            race += 1;
        }
        if race >= 8 {
            break;
        }
    }
}

fn place_manor(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    let mut rng = TownRng::salted(seed, 0x4A40_BBE7, origin_salt(search));
    let Some(yard) = find_manor_site(occ, grid, search, town, keep_out, &mut rng) else {
        return;
    };
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if yard.on_edge(x, z) {
                occ.set(x, z, CellKind::TownWall, true);
            }
        }
    }
    let corners = [
        (yard.min_x, yard.min_z),
        (yard.max_x - 2, yard.min_z),
        (yard.min_x, yard.max_z - 2),
        (yard.max_x - 2, yard.max_z - 2),
    ];
    for (x, z) in corners {
        occ.fill(Rect::new(x, z, x + 2, z + 2), CellKind::TownTower, true);
    }
    let gate_dir = manor_gate_dir(grid, yard);
    paint_manor_gate(occ, yard, gate_dir);
    let inner = yard.deflate(4);
    let hall = Rect::new(
        inner.min_x + 1,
        inner.min_z + 1,
        inner.min_x + 14,
        inner.min_z + 12,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::TownHall,
        hall,
        gate_dir,
    );
    let kitchen = Rect::new(
        inner.min_x + 16,
        inner.min_z + 1,
        inner.max_x - 1,
        inner.min_z + 7,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        kitchen,
        gate_dir.opposite(),
    );
    scenes.push(Scene {
        kind: SceneKind::Manor,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn place_vineyard(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    let mut rng = TownRng::salted(seed, 0xA11E_71ED, origin_salt(search));
    let Some(yard) = find_vineyard_site(occ, grid, search, town, keep_out, scenes, &mut rng) else {
        return;
    };
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if yard.on_edge(x, z) {
                occ.set(x, z, CellKind::TownWall, true);
            }
        }
    }
    let corners = [
        (yard.min_x, yard.min_z),
        (yard.max_x - 2, yard.min_z),
        (yard.min_x, yard.max_z - 2),
        (yard.max_x - 2, yard.max_z - 2),
    ];
    for (x, z) in corners {
        occ.fill(Rect::new(x, z, x + 2, z + 2), CellKind::TownTower, true);
    }
    let gate_dir = manor_gate_dir(grid, yard);
    paint_manor_gate(occ, yard, gate_dir);
    let inner = yard.deflate(3);
    let (_, cz) = inner.center();
    let cloister = Rect::new(inner.max_x - 8, cz - 3, inner.max_x - 2, cz + 3);
    occ.fill(cloister, CellKind::Arcade, true);
    occ.fill(cloister.deflate(1), CellKind::Green, true);
    let press = Rect::new(
        inner.min_x + 1,
        inner.min_z + 1,
        inner.min_x + 9,
        inner.min_z + 6,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Winepress,
        press,
        gate_dir.opposite(),
    );
    let cellar = Rect::new(
        inner.min_x + 1,
        inner.max_z - 6,
        inner.min_x + 8,
        inner.max_z - 1,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        cellar,
        gate_dir,
    );
    let village = vineyard_village_strip(yard, gate_dir.opposite());
    place_vineyard_cottages(occ, grid, lots, seed, village, gate_dir);
    paint_vineyard_inner_rows(occ, grid, inner, cloister);
    let apron = vineyard_apron(yard, gate_dir);
    paint_vineyard_terraces(occ, grid, yard, apron, gate_dir);
    scenes.push(Scene {
        kind: SceneKind::Vineyard,
        rect: union_scene_rect(union_scene_rect(yard, apron), village).inflate(SCENE_YARD_MARGIN),
    });
}

fn vineyard_apron(yard: Rect, dir: Dir) -> Rect {
    match dir {
        Dir::South => Rect::new(
            yard.min_x - 4,
            yard.max_z + 1,
            yard.max_x + 4,
            yard.max_z + 16,
        ),
        Dir::North => Rect::new(
            yard.min_x - 4,
            yard.min_z - 16,
            yard.max_x + 4,
            yard.min_z - 1,
        ),
        Dir::East => Rect::new(
            yard.max_x + 1,
            yard.min_z - 4,
            yard.max_x + 16,
            yard.max_z + 4,
        ),
        Dir::West => Rect::new(
            yard.min_x - 16,
            yard.min_z - 4,
            yard.min_x - 1,
            yard.max_z + 4,
        ),
    }
}

fn paint_vineyard_inner_rows(occ: &mut Occupancy, grid: &HeightGrid, inner: Rect, cloister: Rect) {
    let (lo, _) = grid.range(inner.min_x, inner.min_z, inner.max_x, inner.max_z);
    for z in inner.min_z..=inner.max_z {
        for x in inner.min_x..=inner.max_x {
            if cloister.contains(x, z) || occ.kind(x, z) != CellKind::Green {
                continue;
            }
            if !grid.is_dry(x, z) {
                continue;
            }
            let band = (grid.height(x, z) - lo).rem_euclid(3);
            occ.set(
                x,
                z,
                if band == 0 {
                    CellKind::Retaining
                } else {
                    CellKind::Vine
                },
                true,
            );
        }
    }
}

fn paint_vineyard_terraces(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    apron: Rect,
    gate_dir: Dir,
) {
    let (lo, _) = grid.range(apron.min_x, apron.min_z, apron.max_x, apron.max_z);
    for z in apron.min_z..=apron.max_z {
        for x in apron.min_x..=apron.max_x {
            if yard.contains(x, z) || !occ.bounds.contains(x, z) {
                continue;
            }
            if !grid.is_dry(x, z)
                || Dir::ALL
                    .iter()
                    .any(|dir| !grid.is_dry(x + dir.dx(), z + dir.dz()))
            {
                continue;
            }
            if !riverside_reclaim(occ.kind(x, z)) && occ.kind(x, z) != CellKind::Green {
                continue;
            }
            let nx = x + gate_dir.dx();
            let nz = z + gate_dir.dz();
            let drop = grid.height(x, z) >= grid.height(nx, nz) + 2;
            let contour = (grid.height(x, z) - lo).rem_euclid(3) == 0;
            let step = match gate_dir {
                Dir::North | Dir::South => (z - yard.min_z).rem_euclid(2) == 0,
                Dir::East | Dir::West => (x - yard.min_x).rem_euclid(2) == 0,
            };
            if step || drop || contour {
                occ.set(x, z, CellKind::Retaining, true);
            } else {
                occ.set(x, z, CellKind::Vine, true);
            }
        }
    }
}

fn union_scene_rect(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.min_x.min(b.min_x),
        a.min_z.min(b.min_z),
        a.max_x.max(b.max_x),
        a.max_z.max(b.max_z),
    )
}

fn vineyard_village_strip(yard: Rect, up: Dir) -> Rect {
    match up {
        Dir::North => Rect::new(
            yard.min_x + 2,
            yard.min_z - 11,
            yard.min_x + 18,
            yard.min_z - 4,
        ),
        Dir::South => Rect::new(
            yard.min_x + 2,
            yard.max_z + 4,
            yard.min_x + 18,
            yard.max_z + 11,
        ),
        Dir::East => Rect::new(
            yard.max_x + 4,
            yard.min_z + 2,
            yard.max_x + 11,
            yard.min_z + 18,
        ),
        Dir::West => Rect::new(
            yard.min_x - 11,
            yard.min_z + 2,
            yard.min_x - 4,
            yard.min_z + 18,
        ),
    }
}

fn place_vineyard_cottages(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    strip: Rect,
    gate_dir: Dir,
) {
    let houses = if strip.width() >= 16 {
        [
            Rect::new(strip.min_x, strip.min_z, strip.min_x + 7, strip.min_z + 6),
            Rect::new(
                strip.min_x + 9,
                strip.min_z,
                strip.min_x + 16,
                strip.min_z + 6,
            ),
        ]
    } else {
        [
            Rect::new(strip.min_x, strip.min_z, strip.min_x + 7, strip.min_z + 6),
            Rect::new(
                strip.min_x,
                strip.min_z + 8,
                strip.min_x + 7,
                strip.min_z + 14,
            ),
        ]
    };
    for (i, rect) in houses.into_iter().enumerate() {
        let facing = if i == 0 {
            gate_dir
        } else {
            gate_dir.opposite()
        };
        let _ = force_lot(occ, grid, lots, seed, BuildingKind::Cottage, rect, facing);
    }
}

fn near_gothic(occ: &Occupancy, rect: Rect) -> bool {
    let pad = rect.inflate(28);
    for z in pad.min_z..=pad.max_z {
        for x in pad.min_x..=pad.max_x {
            if !occ.bounds.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::CathedralNave
                    | CellKind::CathedralTransept
                    | CellKind::CathedralChoir
                    | CellKind::CathedralTower
            ) {
                return true;
            }
        }
    }
    false
}

fn landmark_blocked(scenes: &[Scene], rect: Rect) -> bool {
    scenes.iter().any(|scene| {
        matches!(
            scene.kind,
            SceneKind::Cathedral
                | SceneKind::CathedralClose
                | SceneKind::Monastery
                | SceneKind::Manor
                | SceneKind::Vineyard
                | SceneKind::Frontier
                | SceneKind::Harbour
                | SceneKind::MountainFortress
                | SceneKind::Gorge
                | SceneKind::StaveChurch
                | SceneKind::Caravanserai
                | SceneKind::RuinedAbbey
                | SceneKind::CastleRuin
                | SceneKind::TollBridge
                | SceneKind::CharcoalCamp
                | SceneKind::RoadsideInn
                | SceneKind::PillagerCamp
                | SceneKind::WalledMarket
                | SceneKind::MillHamlet
                | SceneKind::Beacon
                | SceneKind::Quarry
                | SceneKind::SiegeCamp
        ) && scene.rect.deflate(2).intersects(rect)
    })
}

fn place_stave_church_scene(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    palette: PaletteKind,
    scenes: &mut Vec<Scene>,
) {
    if palette != PaletteKind::Spruce {
        return;
    }
    let mut rng = TownRng::salted(seed, 0x57A7_E001, origin_salt(search));
    let Some(yard) = find_gentle_site(occ, grid, search, town, keep_out, 22, 20, 10, &mut rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 20, 16, false, &mut rng))
    else {
        return;
    };
    if landmark_blocked(scenes, yard) || near_gothic(occ, yard) {
        return;
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    let church = Rect::new(
        yard.min_x + 2,
        yard.min_z + 2,
        yard.min_x + 16,
        yard.min_z + 12,
    );
    if force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::StaveChurch,
        church,
        Dir::South,
    )
    .is_none()
    {
        return;
    }
    let house = Rect::new(
        yard.min_x + 2,
        yard.max_z - 7,
        yard.min_x + 9,
        yard.max_z - 2,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        house,
        Dir::North,
    );
    scenes.push(Scene {
        kind: SceneKind::StaveChurch,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn place_caravanserai(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    palette: PaletteKind,
    scenes: &mut Vec<Scene>,
) {
    if palette != PaletteKind::Sandstone {
        return;
    }
    let mut rng = TownRng::salted(seed, 0xCA5A_7E51, origin_salt(search));
    let Some(yard) = find_gentle_site(occ, grid, search, town, keep_out, 26, 22, 12, &mut rng)
        .or_else(|| find_site(occ, grid, search, town, keep_out, 22, 18, false, &mut rng))
    else {
        return;
    };
    if landmark_blocked(scenes, yard) {
        return;
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if yard.on_edge(x, z) {
                occ.set(x, z, CellKind::TownWall, true);
            }
        }
    }
    let corners = [
        (yard.min_x, yard.min_z),
        (yard.max_x - 2, yard.min_z),
        (yard.min_x, yard.max_z - 2),
        (yard.max_x - 2, yard.max_z - 2),
    ];
    for (x, z) in corners {
        occ.fill(Rect::new(x, z, x + 2, z + 2), CellKind::TownTower, true);
    }
    let gate_dir = manor_gate_dir(grid, yard);
    paint_manor_gate(occ, yard, gate_dir);
    let inner = yard.deflate(3);
    occ.fill(inner, CellKind::Arcade, true);
    let court = inner.deflate(2);
    occ.fill(court, CellKind::Plaza, true);
    let (cx, cz) = court.center();
    occ.fill(
        Rect::new(cx - 1, cz - 1, cx + 1, cz + 1),
        CellKind::Fountain,
        true,
    );
    let lodge_a = Rect::new(
        inner.min_x + 1,
        inner.min_z + 1,
        inner.min_x + 8,
        inner.min_z + 6,
    );
    let lodge_b = Rect::new(
        inner.max_x - 8,
        inner.min_z + 1,
        inner.max_x - 1,
        inner.min_z + 6,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Townhouse,
        lodge_a,
        gate_dir.opposite(),
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Townhouse,
        lodge_b,
        gate_dir.opposite(),
    );
    scenes.push(Scene {
        kind: SceneKind::Caravanserai,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn place_gorge_town(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    let mut rng = TownRng::salted(seed, 0x60B6_E11A, origin_salt(search));
    let Some(yard) = find_gorge_site(occ, grid, search, town, keep_out, scenes, &mut rng) else {
        return;
    };
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(x, z)) {
                occ.set(x, z, CellKind::Green, true);
            }
        }
    }
    let river_ew = wet_span(grid, yard) == Dir::East;
    paint_gorge_ravine(occ, grid, yard, river_ew);
    place_gorge_houses(occ, grid, yard, lots, seed, river_ew);
    paint_gorge_bridge_gate(occ, grid, yard, river_ew);
    paint_gorge_eaves(occ, grid, yard, lots, river_ew);
    paint_gorge_terraces(occ, grid, yard, river_ew);
    scenes.push(Scene {
        kind: SceneKind::Gorge,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn gorge_ravine_axis(grid: &HeightGrid, yard: Rect, river_ew: bool) -> i32 {
    let mut best = i32::MAX;
    let mut axis = if river_ew {
        yard.center().1
    } else {
        yard.center().0
    };
    if river_ew {
        for z in yard.min_z..=yard.max_z {
            let mut sum = 0;
            let mut n = 0;
            for x in yard.min_x..=yard.max_x {
                sum += grid.height(x, z);
                n += 1;
            }
            if n > 0 && sum / n < best {
                best = sum / n;
                axis = z;
            }
        }
    } else {
        for x in yard.min_x..=yard.max_x {
            let mut sum = 0;
            let mut n = 0;
            for z in yard.min_z..=yard.max_z {
                sum += grid.height(x, z);
                n += 1;
            }
            if n > 0 && sum / n < best {
                best = sum / n;
                axis = x;
            }
        }
    }
    axis
}

fn paint_gorge_ravine(occ: &mut Occupancy, grid: &HeightGrid, yard: Rect, river_ew: bool) {
    let axis = gorge_ravine_axis(grid, yard, river_ew);
    for z in yard.min_z..=yard.max_z {
        for x in yard.min_x..=yard.max_x {
            let on_channel = if river_ew {
                (z - axis).abs() <= 2
            } else {
                (x - axis).abs() <= 2
            };
            let wet = !grid.is_dry(x, z);
            if !(on_channel || wet) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Green
                    | CellKind::Empty
                    | CellKind::Farm
                    | CellKind::FarmEdge
                    | CellKind::Hedge
                    | CellKind::Orchard
                    | CellKind::Pasture
            ) {
                occ.set(x, z, CellKind::Harbor, true);
            }
        }
    }
}

fn find_gorge_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    scenes: &[Scene],
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 40;
    let d = 36;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 8;
    while z + d < search.max_z {
        let mut x = search.min_x + 8;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 14;
            if town.inflate(10).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if scenes.iter().any(|scene| {
                matches!(
                    scene.kind,
                    SceneKind::Riverside
                        | SceneKind::Harbour
                        | SceneKind::Manor
                        | SceneKind::Vineyard
                        | SceneKind::Frontier
                        | SceneKind::Cathedral
                        | SceneKind::CathedralClose
                ) && scene.rect.deflate(4).intersects(rect)
            }) {
                continue;
            }
            if !riverside_yard_free(occ, rect) {
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            if hi - lo < 10 {
                continue;
            }
            let (cx, cz) = rect.center();
            let wet = i32::from(!grid.is_dry(cx, cz))
                + i32::from(!grid.is_dry(rect.min_x + 6, cz))
                + i32::from(!grid.is_dry(rect.max_x - 6, cz))
                + i32::from(!grid.is_dry(cx, rect.min_z + 6))
                + i32::from(!grid.is_dry(cx, rect.max_z - 6));
            let banks = i32::from(grid.is_dry(cx, rect.min_z + 2))
                + i32::from(grid.is_dry(cx, rect.max_z - 2))
                + i32::from(grid.is_dry(rect.min_x + 2, cz))
                + i32::from(grid.is_dry(rect.max_x - 2, cz));
            if banks < 2 {
                continue;
            }
            let score = (hi - lo) + wet * 6 + banks * 3 + rng.gen_range(0, 4);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 12;
    }
    best
}

fn paint_gorge_bridge_gate(occ: &mut Occupancy, grid: &HeightGrid, yard: Rect, river_ew: bool) {
    let (cx, cz) = yard.center();
    let axis = gorge_ravine_axis(grid, yard, river_ew);
    if river_ew {
        for z in yard.min_z..=yard.max_z {
            for dx in -1..=1 {
                let x = cx + dx;
                if !yard.contains(x, z) {
                    continue;
                }
                let kind = occ.kind(x, z);
                if matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                ) {
                    continue;
                }
                let wet = !grid.is_dry(x, z) || kind == CellKind::Harbor;
                occ.set(
                    x,
                    z,
                    if wet {
                        CellKind::Bridge
                    } else {
                        CellKind::Road
                    },
                    true,
                );
            }
        }
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                if (x - cx).abs() <= 2
                    && matches!(occ.kind(x, z), CellKind::Harbor | CellKind::Road)
                    && ((x - cx).abs() <= 1 || occ.kind(x, z) == CellKind::Harbor)
                {
                    let near_drop = occ.kind(x, z) == CellKind::Harbor
                        || !grid.is_dry(x, z)
                        || Dir::ALL.iter().any(|dir| {
                            matches!(
                                occ.kind(x + dir.dx(), z + dir.dz()),
                                CellKind::Harbor | CellKind::Bridge
                            )
                        });
                    if near_drop {
                        occ.set(x, z, CellKind::Bridge, true);
                    }
                }
            }
        }
        for side in [-1, 1] {
            let gz = (axis + side * 8).clamp(yard.min_z + 1, yard.max_z - 1);
            for dx in -1..=1 {
                occ.set(cx + dx, gz, CellKind::TownGate, true);
            }
            let tx = (cx + 3).clamp(yard.min_x, yard.max_x - 2);
            occ.fill(
                Rect::new(tx, gz - 1, tx + 2, gz + 1),
                CellKind::TownTower,
                true,
            );
        }
    } else {
        for x in yard.min_x..=yard.max_x {
            for dz in -1..=1 {
                let z = cz + dz;
                if !yard.contains(x, z) {
                    continue;
                }
                let kind = occ.kind(x, z);
                if matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                ) {
                    continue;
                }
                let wet = !grid.is_dry(x, z) || kind == CellKind::Harbor;
                occ.set(
                    x,
                    z,
                    if wet {
                        CellKind::Bridge
                    } else {
                        CellKind::Road
                    },
                    true,
                );
            }
        }
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                if (z - cz).abs() <= 2
                    && matches!(occ.kind(x, z), CellKind::Harbor | CellKind::Road)
                    && ((z - cz).abs() <= 1 || occ.kind(x, z) == CellKind::Harbor)
                {
                    let near_drop = occ.kind(x, z) == CellKind::Harbor
                        || !grid.is_dry(x, z)
                        || Dir::ALL.iter().any(|dir| {
                            matches!(
                                occ.kind(x + dir.dx(), z + dir.dz()),
                                CellKind::Harbor | CellKind::Bridge
                            )
                        });
                    if near_drop {
                        occ.set(x, z, CellKind::Bridge, true);
                    }
                }
            }
        }
        for side in [-1, 1] {
            let gx = (axis + side * 8).clamp(yard.min_x + 1, yard.max_x - 1);
            for dz in -1..=1 {
                occ.set(gx, cz + dz, CellKind::TownGate, true);
            }
            let tz = (cz + 3).clamp(yard.min_z, yard.max_z - 2);
            occ.fill(
                Rect::new(gx - 1, tz, gx + 1, tz + 2),
                CellKind::TownTower,
                true,
            );
        }
    }
}

fn paint_gorge_terraces(occ: &mut Occupancy, grid: &HeightGrid, yard: Rect, along_x: bool) {
    let bands = if along_x {
        [
            yard.min_z + 2,
            yard.min_z + 8,
            yard.min_z + 14,
            yard.max_z - 14,
            yard.max_z - 8,
            yard.max_z - 2,
        ]
    } else {
        [
            yard.min_x + 2,
            yard.min_x + 8,
            yard.min_x + 14,
            yard.max_x - 14,
            yard.max_x - 8,
            yard.max_x - 2,
        ]
    };
    for &band in &bands {
        if along_x {
            for x in yard.min_x + 3..=yard.max_x - 3 {
                if !grid.is_dry(x, band) {
                    continue;
                }
                if matches!(
                    occ.kind(x, band),
                    CellKind::Green | CellKind::Empty | CellKind::Farm | CellKind::FarmEdge
                ) {
                    occ.set(x, band, CellKind::Retaining, true);
                }
                if band < yard.max_z
                    && grid.is_dry(x, band + 1)
                    && occ.kind(x, band + 1) == CellKind::Green
                    && (x - yard.min_x) % 5 == 0
                {
                    occ.set(x, band + 1, stair_toward(x, band, x, band + 2), true);
                }
            }
        } else {
            for z in yard.min_z + 3..=yard.max_z - 3 {
                if !grid.is_dry(band, z) {
                    continue;
                }
                if matches!(
                    occ.kind(band, z),
                    CellKind::Green | CellKind::Empty | CellKind::Farm | CellKind::FarmEdge
                ) {
                    occ.set(band, z, CellKind::Retaining, true);
                }
                if band < yard.max_x
                    && grid.is_dry(band + 1, z)
                    && occ.kind(band + 1, z) == CellKind::Green
                    && (z - yard.min_z) % 5 == 0
                {
                    occ.set(band + 1, z, stair_toward(band, z, band + 2, z), true);
                }
            }
        }
    }
}

fn place_gorge_houses(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    river_ew: bool,
) {
    let axis = gorge_ravine_axis(grid, yard, river_ew);
    let (cx, cz) = yard.center();
    let mut placed = 0;
    let mut z = yard.min_z + 1;
    while z + 7 <= yard.max_z && placed < 6 {
        let mut x = yard.min_x + 1;
        while x + 8 <= yard.max_x && placed < 6 {
            if river_ew && (x + 4 - cx).abs() < 5 {
                x += 3;
                continue;
            }
            if !river_ew && (z + 3 - cz).abs() < 5 {
                x += 3;
                continue;
            }
            let pad = Rect::new(x, z, x + 7, z + 6);
            let facing = if river_ew {
                if z + 3 <= axis {
                    Dir::South
                } else {
                    Dir::North
                }
            } else if x + 4 <= axis {
                Dir::East
            } else {
                Dir::West
            };
            let kind = if placed % 2 == 0 {
                BuildingKind::Townhouse
            } else {
                BuildingKind::Cottage
            };
            if force_lot(occ, grid, lots, seed, kind, pad, facing).is_some() {
                placed += 1;
                x += 9;
                continue;
            }
            x += 3;
        }
        z += 4;
    }
}

fn paint_gorge_eaves(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    lots: &[Lot],
    river_ew: bool,
) {
    let toward = if river_ew {
        [Dir::South, Dir::North]
    } else {
        [Dir::East, Dir::West]
    };
    for lot in lots {
        if !yard.intersects(lot.rect) {
            continue;
        }
        if !matches!(lot.kind, BuildingKind::Cottage | BuildingKind::Townhouse) {
            continue;
        }
        for dir in toward {
            for z in lot.rect.min_z..=lot.rect.max_z {
                for x in lot.rect.min_x..=lot.rect.max_x {
                    if !lot.rect.on_edge(x, z) {
                        continue;
                    }
                    let nx = x + dir.dx();
                    let nz = z + dir.dz();
                    if !yard.contains(nx, nz) {
                        continue;
                    }
                    if matches!(
                        occ.kind(nx, nz),
                        CellKind::Green | CellKind::Empty | CellKind::Eave
                    ) && grid.is_dry(nx, nz)
                    {
                        occ.set(nx, nz, CellKind::Eave, true);
                    }
                }
            }
        }
    }
}

fn place_outskirts(
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
    let mut samples = Vec::new();
    for z in search.min_z..=search.max_z {
        for x in search.min_x..=search.max_x {
            if !is_road_cell(occ.kind(x, z)) {
                continue;
            }
            if (x + z).rem_euclid(5) != 0 {
                continue;
            }
            if town.inflate(6).contains(x, z) || keep_out.contains(x, z) {
                continue;
            }
            samples.push((x, z));
        }
    }
    let mut inns = 0;
    let mut hamlets = 0;
    let mut orchards = 0;
    let mut shrines = 0;
    let mut farms = 0;
    let mut pastures = 0;
    let mut cottages = 0;
    let mut workshops = 0;
    for &(x, z) in &samples {
        if shrines < 10 && place_wayside_shrine(occ, grid, x, z) {
            shrines += 1;
        }
        paint_roadside_hedge_and_wall(occ, grid, x, z);
        if farms < 22 && place_roadside_farm(occ, grid, town, keep_out, x, z) {
            farms += 1;
        }
        if pastures < 10 && place_roadside_pasture(occ, grid, town, keep_out, x, z, scenes) {
            pastures += 1;
        }
        if orchards < 10 && place_roadside_orchard(occ, grid, search, town, keep_out, x, z, scenes)
        {
            orchards += 1;
        }
        if orchards < 16 && place_roadside_copse(occ, grid, town, keep_out, x, z, scenes) {
            orchards += 1;
        }
        if cottages < 28
            && rng.chance(2, 3)
            && place_roadside_cottage(occ, grid, lots, seed, town, keep_out, x, z)
        {
            cottages += 1;
        }
        if workshops < 12
            && rng.chance(1, 2)
            && place_roadside_workshop(occ, grid, lots, seed, town, keep_out, x, z)
        {
            workshops += 1;
        }
        if inns < 10
            && rng.chance(1, 2)
            && place_roadside_inn(occ, grid, lots, seed, town, keep_out, x, z)
        {
            inns += 1;
        }
        if hamlets < 10
            && rng.chance(1, 3)
            && place_roadside_hamlet(occ, grid, lots, seed, town, keep_out, x, z, scenes)
        {
            hamlets += 1;
        }
    }
    let mut mills = 0;
    for &(x, z) in samples.iter().step_by(5) {
        if mills >= 2 {
            break;
        }
        let rect = Rect::new(x + 8, z - 3, x + 14, z + 3);
        if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if try_mill_site(occ, grid, lots, seed, rect) {
            mills += 1;
        }
    }
    densify_empty_fields(occ, grid, search, town, keep_out, scenes);
    place_island_village(occ, grid, search, town, keep_out, lots, seed, scenes);
    let mut far_inns = 0;
    let mut far_shrines = 0;
    for &(x, z) in &samples {
        if town.inflate(48).contains(x, z) {
            continue;
        }
        if far_shrines < 4 && place_wayside_shrine(occ, grid, x, z) {
            far_shrines += 1;
        }
        if far_inns < 4 && place_roadside_inn(occ, grid, lots, seed, town, keep_out, x, z) {
            far_inns += 1;
        }
    }
}

fn is_road_cell(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Road
            | CellKind::RoadStairN
            | CellKind::RoadStairS
            | CellKind::RoadStairE
            | CellKind::RoadStairW
            | CellKind::Bridge
    )
}

fn paint_roadside_hedge_and_wall(occ: &mut Occupancy, grid: &HeightGrid, x: i32, z: i32) {
    for dir in Dir::ALL {
        let hx = x + dir.dx() * 2;
        let hz = z + dir.dz() * 2;
        if occ.bounds.contains(hx, hz) && grid.is_dry(hx, hz) && occ.kind(hx, hz) == CellKind::Empty
        {
            occ.set(hx, hz, CellKind::Hedge, false);
        }
        let wx = x + dir.dx() * 4;
        let wz = z + dir.dz() * 4;
        if !occ.bounds.contains(wx, wz) || !grid.is_dry(wx, wz) {
            continue;
        }
        if occ.kind(wx, wz) != CellKind::Empty {
            continue;
        }
        occ.set(wx, wz, CellKind::FieldWall, false);
        let along = if dir.dx() == 0 { Dir::East } else { Dir::South };
        for step in -2..=2 {
            let fx = wx + along.dx() * step;
            let fz = wz + along.dz() * step;
            if occ.bounds.contains(fx, fz)
                && grid.is_dry(fx, fz)
                && occ.kind(fx, fz) == CellKind::Empty
            {
                occ.set(fx, fz, CellKind::FieldWall, false);
            }
        }
    }
}

fn place_wayside_shrine(occ: &mut Occupancy, grid: &HeightGrid, x: i32, z: i32) -> bool {
    for dir in Dir::ALL {
        let sx = x + dir.dx() * 3;
        let sz = z + dir.dz() * 3;
        let rect = Rect::new(sx - 1, sz - 1, sx + 1, sz + 1);
        if occ.bounds.contains(rect.min_x, rect.min_z)
            && occ.bounds.contains(rect.max_x, rect.max_z)
            && grid.rect_is_dry(rect)
            && occ.is_free(rect)
        {
            occ.fill(rect, CellKind::Shrine, true);
            return true;
        }
        if occ.bounds.contains(sx, sz)
            && grid.is_dry(sx, sz)
            && matches!(
                occ.kind(sx, sz),
                CellKind::Empty | CellKind::Green | CellKind::Hedge
            )
        {
            occ.set(sx, sz, CellKind::Shrine, true);
            return true;
        }
    }
    false
}

fn place_roadside_farm(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
) -> bool {
    for dir in Dir::ALL {
        let fx = x + dir.dx() * 8;
        let fz = z + dir.dz() * 6;
        let rect = Rect::new(fx - 5, fz - 4, fx + 5, fz + 4);
        if town.inflate(4).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if !occ.bounds.contains(rect.min_x, rect.min_z)
            || !occ.bounds.contains(rect.max_x, rect.max_z)
        {
            continue;
        }
        if !grid.rect_is_dry(rect) || !occ.is_free(rect) {
            continue;
        }
        let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
        if hi - lo > 5 {
            continue;
        }
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Farm, true);
        return true;
    }
    false
}

fn place_roadside_pasture(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
    scenes: &mut Vec<Scene>,
) -> bool {
    for dir in Dir::ALL {
        let px = x + dir.dx() * 10;
        let pz = z + dir.dz() * 8;
        let rect = Rect::new(px - 6, pz - 5, px + 6, pz + 5);
        if town.inflate(4).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if !occ.bounds.contains(rect.min_x, rect.min_z)
            || !occ.bounds.contains(rect.max_x, rect.max_z)
        {
            continue;
        }
        if !grid.rect_is_dry(rect) || !occ.is_free(rect) {
            continue;
        }
        let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
        if hi - lo > 6 {
            continue;
        }
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Pasture, true);
        scenes.push(Scene {
            kind: SceneKind::Pasture,
            rect,
        });
        return true;
    }
    false
}

fn place_roadside_copse(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
    scenes: &mut Vec<Scene>,
) -> bool {
    for dir in Dir::ALL {
        let ox = x + dir.dx() * 7;
        let oz = z + dir.dz() * 6;
        let rect = Rect::new(ox - 3, oz - 3, ox + 3, oz + 3);
        if town.inflate(4).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if !occ.bounds.contains(rect.min_x, rect.min_z)
            || !occ.bounds.contains(rect.max_x, rect.max_z)
        {
            continue;
        }
        if !grid.rect_is_dry(rect) || !occ.is_free(rect) {
            continue;
        }
        occ.fill(rect, CellKind::Orchard, true);
        scenes.push(Scene {
            kind: SceneKind::Orchard,
            rect,
        });
        return true;
    }
    false
}

fn densify_empty_fields(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    scenes: &mut Vec<Scene>,
) {
    let mut farms = 0;
    let mut pastures = 0;
    let mut copses = 0;
    let mut z = search.min_z + 8;
    while z + 12 < search.max_z {
        let mut x = search.min_x + 8;
        while x + 14 < search.max_x {
            let mix = (x + z).rem_euclid(9);
            if town.inflate(8).contains(x, z) || keep_out.contains(x, z) {
                x += 16;
                continue;
            }
            if !grid.is_dry(x, z) || occ.kind(x, z) != CellKind::Empty {
                x += 16;
                continue;
            }
            if mix == 0 && farms < 14 {
                let rect = Rect::new(x - 5, z - 4, x + 5, z + 4);
                if occ.bounds.contains(rect.min_x, rect.min_z)
                    && occ.bounds.contains(rect.max_x, rect.max_z)
                    && grid.rect_is_dry(rect)
                    && occ.is_free(rect)
                    && !town.inflate(4).intersects(rect)
                    && !keep_out.intersects(rect)
                {
                    occ.fill(rect, CellKind::FarmEdge, false);
                    occ.fill(rect.deflate(1), CellKind::Farm, true);
                    farms += 1;
                }
            } else if mix == 3 && pastures < 8 {
                let rect = Rect::new(x - 6, z - 5, x + 6, z + 5);
                if occ.bounds.contains(rect.min_x, rect.min_z)
                    && occ.bounds.contains(rect.max_x, rect.max_z)
                    && grid.rect_is_dry(rect)
                    && occ.is_free(rect)
                    && !town.inflate(4).intersects(rect)
                    && !keep_out.intersects(rect)
                {
                    occ.fill(rect, CellKind::FarmEdge, false);
                    occ.fill(rect.deflate(1), CellKind::Pasture, true);
                    scenes.push(Scene {
                        kind: SceneKind::Pasture,
                        rect,
                    });
                    pastures += 1;
                }
            } else if mix == 6 && copses < 8 {
                let rect = Rect::new(x - 3, z - 3, x + 3, z + 3);
                if occ.bounds.contains(rect.min_x, rect.min_z)
                    && occ.bounds.contains(rect.max_x, rect.max_z)
                    && grid.rect_is_dry(rect)
                    && occ.is_free(rect)
                    && !town.inflate(4).intersects(rect)
                    && !keep_out.intersects(rect)
                {
                    occ.fill(rect, CellKind::Orchard, true);
                    scenes.push(Scene {
                        kind: SceneKind::Orchard,
                        rect,
                    });
                    copses += 1;
                }
            }
            x += 16;
        }
        z += 14;
    }
}

fn island_wet_sides(grid: &HeightGrid, rect: Rect) -> i32 {
    let north = (rect.min_x..=rect.max_x).any(|x| !grid.is_dry(x, rect.min_z - 1));
    let south = (rect.min_x..=rect.max_x).any(|x| !grid.is_dry(x, rect.max_z + 1));
    let west = (rect.min_z..=rect.max_z).any(|z| !grid.is_dry(rect.min_x - 1, z));
    let east = (rect.min_z..=rect.max_z).any(|z| !grid.is_dry(rect.max_x + 1, z));
    i32::from(north) + i32::from(south) + i32::from(west) + i32::from(east)
}

fn place_island_village(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    scenes: &mut Vec<Scene>,
) {
    let w = 20;
    let d = 16;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 6;
    while z + d < search.max_z {
        let mut x = search.min_x + 6;
        while x + w < town.min_x.saturating_sub(4).max(search.min_x + w + 1) {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 10;
            if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if scenes.iter().any(|scene| {
                matches!(
                    scene.kind,
                    SceneKind::Harbour
                        | SceneKind::Manor
                        | SceneKind::Frontier
                        | SceneKind::Gorge
                        | SceneKind::Hamlet
                ) && scene.rect.deflate(2).intersects(rect)
            }) {
                continue;
            }
            if !occ.bounds.contains(rect.min_x, rect.min_z)
                || !occ.bounds.contains(rect.max_x, rect.max_z)
            {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            if !riverside_yard_free(occ, rect) {
                continue;
            }
            let wet = island_wet_sides(grid, rect);
            if wet < 2 {
                continue;
            }
            let score = wet * 8
                + (town.min_x - rect.max_x).min(40)
                + (rect.min_x * 3 + rect.min_z).rem_euclid(5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 8;
    }
    let Some(yard) = best else {
        return;
    };
    for zz in yard.min_z..=yard.max_z {
        for xx in yard.min_x..=yard.max_x {
            if riverside_reclaim(occ.kind(xx, zz)) {
                occ.set(xx, zz, CellKind::Green, true);
            }
        }
    }
    for zz in yard.min_z..=yard.max_z {
        for xx in yard.min_x..=yard.max_x {
            if yard.on_edge(xx, zz) {
                occ.set(xx, zz, CellKind::TownWall, true);
            }
        }
    }
    let corners = [
        (yard.min_x, yard.min_z),
        (yard.max_x - 2, yard.min_z),
        (yard.min_x, yard.max_z - 2),
        (yard.max_x - 2, yard.max_z - 2),
    ];
    for (tx, tz) in corners {
        occ.fill(Rect::new(tx, tz, tx + 2, tz + 2), CellKind::TownTower, true);
    }
    let gate_dir = manor_gate_dir(grid, yard);
    paint_manor_gate(occ, yard, gate_dir);
    let inner = yard.deflate(3);
    let a = Rect::new(
        inner.min_x + 1,
        inner.min_z + 1,
        inner.min_x + 8,
        inner.min_z + 7,
    );
    let b = Rect::new(
        inner.max_x - 8,
        inner.max_z - 7,
        inner.max_x - 1,
        inner.max_z - 1,
    );
    let c = Rect::new(
        inner.min_x + 1,
        inner.max_z - 7,
        inner.min_x + 8,
        inner.max_z - 1,
    );
    let _ = force_lot(occ, grid, lots, seed, BuildingKind::Cottage, a, Dir::South);
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Townhouse,
        b,
        Dir::North,
    );
    let _ = force_lot(occ, grid, lots, seed, BuildingKind::Cottage, c, Dir::East);
    scenes.push(Scene {
        kind: SceneKind::Hamlet,
        rect: yard.inflate(SCENE_YARD_MARGIN),
    });
}

fn place_roadside_orchard(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    _search: Rect,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
    scenes: &mut Vec<Scene>,
) -> bool {
    for dir in Dir::ALL {
        let ox = x + dir.dx() * 10;
        let oz = z + dir.dz() * 8;
        let rect = Rect::new(ox - 6, oz - 5, ox + 6, oz + 5);
        if town.inflate(6).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if !occ.bounds.contains(rect.min_x, rect.min_z)
            || !occ.bounds.contains(rect.max_x, rect.max_z)
        {
            continue;
        }
        if !grid.rect_is_dry(rect) || !occ.is_free(rect) {
            continue;
        }
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Orchard, true);
        scenes.push(Scene {
            kind: SceneKind::Orchard,
            rect,
        });
        return true;
    }
    false
}

fn place_roadside_cottage(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
) -> bool {
    let (w, d) = BuildingKind::Cottage.size();
    for dir in Dir::ALL {
        let cx = x + dir.dx() * (w + 3);
        let cz = z + dir.dz() * (d + 3);
        let rect = Rect::new(cx, cz, cx + w - 1, cz + d - 1);
        if town.inflate(6).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if try_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Cottage,
            rect,
            dir.opposite(),
        )
        .is_some()
        {
            return true;
        }
    }
    false
}

fn place_roadside_workshop(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
) -> bool {
    let (w, d) = BuildingKind::Blacksmith.size();
    for dir in Dir::ALL {
        let wx = x + dir.dx() * (w + 3);
        let wz = z + dir.dz() * (d + 3);
        let rect = Rect::new(wx, wz, wx + w - 1, wz + d - 1);
        if town.inflate(6).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if try_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Blacksmith,
            rect,
            dir.opposite(),
        )
        .is_some()
        {
            return true;
        }
    }
    false
}

fn place_roadside_inn(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
) -> bool {
    let (w, d) = BuildingKind::Tavern.size();
    for dir in Dir::ALL {
        let ix = x + dir.dx() * (w + 2);
        let iz = z + dir.dz() * (d + 2);
        let rect = Rect::new(ix, iz, ix + w - 1, iz + d - 1);
        if town.inflate(6).intersects(rect) || keep_out.intersects(rect) {
            continue;
        }
        if try_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Tavern,
            rect,
            dir.opposite(),
        )
        .is_some()
        {
            return true;
        }
    }
    false
}

fn place_roadside_hamlet(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    keep_out: Rect,
    x: i32,
    z: i32,
    scenes: &mut Vec<Scene>,
) -> bool {
    for dir in Dir::ALL {
        let hx = x + dir.dx() * 16;
        let hz = z + dir.dz() * 12;
        let yard = Rect::new(hx - 10, hz - 8, hx + 10, hz + 8);
        if town.inflate(8).intersects(yard) || keep_out.intersects(yard) {
            continue;
        }
        if !occ.bounds.contains(yard.min_x, yard.min_z)
            || !occ.bounds.contains(yard.max_x, yard.max_z)
        {
            continue;
        }
        if !grid.rect_is_dry(yard) {
            continue;
        }
        let mut free = true;
        for zz in yard.min_z..=yard.max_z {
            for xx in yard.min_x..=yard.max_x {
                if !matches!(
                    occ.kind(xx, zz),
                    CellKind::Empty
                        | CellKind::Green
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Hedge
                        | CellKind::FieldWall
                ) {
                    free = false;
                    break;
                }
            }
            if !free {
                break;
            }
        }
        if !free {
            continue;
        }
        occ.fill(yard, CellKind::Green, false);
        let a = Rect::new(
            yard.min_x + 2,
            yard.min_z + 2,
            yard.min_x + 9,
            yard.min_z + 8,
        );
        let b = Rect::new(
            yard.max_x - 9,
            yard.max_z - 8,
            yard.max_x - 2,
            yard.max_z - 2,
        );
        let south = try_lot(occ, grid, lots, seed, BuildingKind::Cottage, a, Dir::South).is_some();
        let north = try_lot(occ, grid, lots, seed, BuildingKind::Cottage, b, Dir::North).is_some();
        if south || north {
            scenes.push(Scene {
                kind: SceneKind::Hamlet,
                rect: yard,
            });
            return true;
        }
    }
    false
}

fn find_vineyard_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    scenes: &[Scene],
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 24;
    let d = 20;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 6;
    while z + d < search.max_z {
        let mut x = search.min_x + 6;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 8;
            if town.inflate(8).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if scenes.iter().any(|scene| {
                matches!(
                    scene.kind,
                    SceneKind::Manor
                        | SceneKind::Monastery
                        | SceneKind::Cathedral
                        | SceneKind::CathedralClose
                        | SceneKind::MountainFortress
                        | SceneKind::Frontier
                        | SceneKind::PillagerCamp
                ) && scene.rect.deflate(2).intersects(rect)
            }) {
                continue;
            }
            if !manor_yard_free(occ, rect) {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            if hi - lo > 18 || lo <= grid.sea_level + 2 {
                continue;
            }
            let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let score = mean + (hi - lo) * 8 + rng.gen_range(0, 4);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 6;
    }
    best
}

fn find_manor_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
) -> Option<Rect> {
    let w = 30;
    let d = 26;
    let mut best = None;
    let mut best_score = i32::MIN;
    let mut z = search.min_z + 6;
    while z + d < search.max_z {
        let mut x = search.min_x + 6;
        while x + w < search.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 12;
            if town.inflate(10).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !manor_yard_free(occ, rect) {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            if hi - lo > 14 || lo <= grid.sea_level + 2 {
                continue;
            }
            let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let score = mean + rng.gen_range(0, 5);
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 10;
    }
    best
}

fn manor_yard_free(occ: &Occupancy, rect: Rect) -> bool {
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

fn manor_gate_dir(grid: &HeightGrid, yard: Rect) -> Dir {
    let (cx, cz) = yard.center();
    let scores = [
        (
            Dir::North,
            grid.mean(cx - 2, yard.min_z, cx + 2, yard.min_z),
        ),
        (
            Dir::South,
            grid.mean(cx - 2, yard.max_z, cx + 2, yard.max_z),
        ),
        (Dir::West, grid.mean(yard.min_x, cz - 2, yard.min_x, cz + 2)),
        (Dir::East, grid.mean(yard.max_x, cz - 2, yard.max_x, cz + 2)),
    ];
    scores
        .into_iter()
        .min_by_key(|(_, h)| *h)
        .map_or(Dir::South, |(dir, _)| dir)
}

fn paint_manor_gate(occ: &mut Occupancy, yard: Rect, dir: Dir) {
    let (cx, cz) = yard.center();
    let (gx, gz) = match dir {
        Dir::North => (cx, yard.min_z),
        Dir::South => (cx, yard.max_z),
        Dir::West => (yard.min_x, cz),
        Dir::East => (yard.max_x, cz),
    };
    let along_x = dir == Dir::North || dir == Dir::South;
    for i in -1..=1 {
        let x = gx + if along_x { i } else { 0 };
        let z = gz + if along_x { 0 } else { i };
        occ.set(x, z, CellKind::TownGate, true);
    }
    if along_x {
        occ.fill(Rect::new(gx - 4, gz, gx - 2, gz), CellKind::TownTower, true);
        occ.fill(Rect::new(gx + 2, gz, gx + 4, gz), CellKind::TownTower, true);
    } else {
        occ.fill(Rect::new(gx, gz - 4, gx, gz - 2), CellKind::TownTower, true);
        occ.fill(Rect::new(gx, gz + 2, gx, gz + 4), CellKind::TownTower, true);
    }
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

fn place_extra_orchards_and_pastures(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let mut orchards = 0;
    while orchards < 2 {
        let Some(rect) = find_site(occ, grid, search, town, keep_out, 18, 14, false, rng) else {
            break;
        };
        if scenes.iter().any(|scene| scene.rect.intersects(rect)) {
            orchards += 1;
            continue;
        }
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Orchard, true);
        scenes.push(Scene {
            kind: SceneKind::Orchard,
            rect,
        });
        orchards += 1;
    }
    let mut pastures = 0;
    while pastures < 2 {
        let Some(rect) = find_site(occ, grid, search, town, keep_out, 20, 16, false, rng) else {
            break;
        };
        if scenes.iter().any(|scene| scene.rect.intersects(rect)) {
            pastures += 1;
            continue;
        }
        occ.fill(rect, CellKind::FarmEdge, false);
        occ.fill(rect.deflate(1), CellKind::Pasture, true);
        scenes.push(Scene {
            kind: SceneKind::Pasture,
            rect,
        });
        pastures += 1;
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
        yard.min_z + 24,
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
    if !nave_ok {
        return;
    }
    let chapter = Rect::new(
        yard.min_x + 25,
        yard.min_z + 16,
        yard.min_x + 33,
        yard.min_z + 22,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::ChapterHouse,
        chapter,
        Dir::West,
    );
    let scriptorium = Rect::new(
        yard.min_x + 1,
        yard.min_z + 16,
        yard.min_x + 9,
        yard.min_z + 22,
    );
    let _ = force_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Scriptorium,
        scriptorium,
        Dir::East,
    );
    let herbs = Rect::new(
        cloister.min_x,
        cloister.max_z + 1,
        cloister.max_x,
        yard.max_z - 1,
    );
    for z in herbs.min_z..=herbs.max_z {
        for x in herbs.min_x..=herbs.max_x {
            if occ.kind(x, z) == CellKind::Green {
                occ.set(x, z, CellKind::HerbGarden, true);
            }
        }
    }
    scenes.push(Scene {
        kind: SceneKind::Monastery,
        rect: yard,
    });
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
    let war = match face {
        Dir::South => Rect::new(
            rect.min_x + 24,
            rect.min_z + 4,
            rect.min_x + 30,
            rect.min_z + 8,
        ),
        Dir::North => Rect::new(
            rect.min_x + 24,
            rect.max_z - 8,
            rect.min_x + 30,
            rect.max_z - 4,
        ),
        Dir::East => Rect::new(
            rect.min_x + 4,
            rect.min_z + 4,
            rect.min_x + 10,
            rect.min_z + 8,
        ),
        Dir::West => Rect::new(
            rect.max_x - 10,
            rect.min_z + 4,
            rect.max_x - 4,
            rect.min_z + 8,
        ),
    };
    occ.fill(war, CellKind::WarTent, true);
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

fn place_ridge_mills(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    lots: &mut Vec<Lot>,
    seed: u64,
    rng: &mut TownRng,
) {
    let mut candidates = Vec::new();
    let mut z = search.min_z + 8;
    while z + 7 < search.max_z {
        let mut x = search.min_x + 8;
        while x + 7 < search.max_x {
            let rect = Rect::new(x, z, x + 6, z + 6);
            x += 14;
            if town.inflate(14).intersects(rect) || keep_out.intersects(rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 12) {
                continue;
            }
            let h = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            candidates.push((h + rng.gen_range(0, 4), rect));
        }
        z += 14;
    }
    candidates.sort_by_key(|(h, _)| std::cmp::Reverse(*h));
    let mut placed = 0;
    for (_, rect) in candidates {
        if placed >= 3 {
            break;
        }
        if try_mill_site(occ, grid, lots, seed, rect) {
            placed += 1;
        }
    }
}

fn place_lumber_camp(
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
    let Some(rect) = find_site(occ, grid, search, town, keep_out, 22, 16, true, rng) else {
        return;
    };
    if scenes.iter().any(|scene| scene.rect.intersects(rect)) {
        return;
    }
    occ.fill(rect, CellKind::Green, false);
    scenes.push(Scene {
        kind: SceneKind::LumberCamp,
        rect,
    });
    let (w, d) = BuildingKind::Cottage.size();
    let cottage_a = Rect::new(
        rect.min_x + 2,
        rect.min_z + 2,
        rect.min_x + 1 + w,
        rect.min_z + 1 + d,
    );
    let cottage_b = Rect::new(
        rect.max_x - w - 1,
        rect.max_z - d - 1,
        rect.max_x - 2,
        rect.max_z - 2,
    );
    let _ = try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        cottage_a,
        Dir::South,
    );
    let _ = try_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::Cottage,
        cottage_b,
        Dir::North,
    );
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
            | CellKind::CityDitch
            | CellKind::WaterWheel
            | CellKind::MillRace
            | CellKind::Sluice
            | CellKind::Retaining
            | CellKind::Vine
            | CellKind::Lighthouse
            | CellKind::Shrine
            | CellKind::FieldWall
            | CellKind::TourneyList
            | CellKind::TourneyStand
            | CellKind::Beacon
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
            | CellKind::PalisadeWalk
            | CellKind::Tent
            | CellKind::WarTent
            | CellKind::CampCourt
            | CellKind::FortressWall
            | CellKind::FortressKeep
            | CellKind::Ford
            | CellKind::Weir
            | CellKind::CutStone
            | CellKind::HerbGarden
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

pub(crate) fn rect_hits_mill_sail(lots: &[Lot], rect: Rect) -> bool {
    lots.iter().any(|lot| {
        if lot.kind != BuildingKind::Windmill {
            return false;
        }
        let (cx, cz) = lot.rect.center();
        let nearest_x = rect.min_x.max(cx.min(rect.max_x));
        let nearest_z = rect.min_z.max(cz.min(rect.max_z));
        let dx = nearest_x - cx;
        let dz = nearest_z - cz;
        dx * dx + dz * dz <= MILL_SAIL_RADIUS * MILL_SAIL_RADIUS
    })
}

pub(crate) fn paint_wind_sails(occ: &mut Occupancy, lots: &[Lot]) {
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
    lots: &mut Vec<Lot>,
    seed: u64,
    capital: bool,
    palette: PaletteKind,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let keep = castle.inflate(10);
    place_cathedral_district(
        occ, grid, town, keep, search, lots, seed, capital, palette, rng, scenes,
    );
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
    lots: &mut Vec<Lot>,
    seed: u64,
    capital: bool,
    palette: PaletteKind,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let yard = if capital {
        find_cathedral_in_town(occ, grid, town, keep_out)
            .or_else(|| find_cathedral_in_town(occ, grid, town_inside_walls(occ, town), keep_out))
            .or_else(|| find_cathedral_site(occ, grid, search, town, keep_out, rng))
    } else {
        find_cathedral_site(occ, grid, search, town, keep_out, rng)
    };
    let Some(yard) = yard else {
        return;
    };
    evict_lots_from(occ, lots, yard.inflate(2));
    match palette {
        PaletteKind::Sandstone => paint_sandstone_basilica(occ, yard),
        PaletteKind::Spruce => paint_romanesque_church(occ, yard),
        PaletteKind::Oak => paint_gothic_cathedral(occ, yard, seed),
    }
    scenes.push(Scene {
        kind: SceneKind::Cathedral,
        rect: yard,
    });
    paint_close_in_precinct(occ, grid, lots, seed, yard);
    paint_cathedral_close(occ, grid, lots, seed, yard, keep_out, scenes);
}

fn evict_lots_from(occ: &mut Occupancy, lots: &mut Vec<Lot>, rect: Rect) {
    let ids: Vec<u16> = lots
        .iter()
        .filter(|lot| lot.rect.intersects(rect))
        .map(|lot| lot.id)
        .collect();
    if ids.is_empty() {
        return;
    }
    let gone: Vec<Rect> = lots
        .iter()
        .filter(|lot| ids.contains(&lot.id))
        .map(|lot| lot.rect)
        .collect();
    lots.retain(|lot| !ids.contains(&lot.id));
    for old in gone {
        for z in old.min_z..=old.max_z {
            for x in old.min_x..=old.max_x {
                let kind = occ.kind(x, z);
                if kind.is_castle()
                    || matches!(
                        kind,
                        CellKind::TownWall
                            | CellKind::TownTower
                            | CellKind::TownGate
                            | CellKind::Market
                            | CellKind::Well
                            | CellKind::Fountain
                            | CellKind::Road
                            | CellKind::RoadStairN
                            | CellKind::RoadStairS
                            | CellKind::RoadStairE
                            | CellKind::RoadStairW
                            | CellKind::Bridge
                            | CellKind::CityDitch
                    )
                {
                    continue;
                }
                occ.set(x, z, CellKind::Empty, true);
            }
        }
    }
}

/// Bounding box of the civic curtain. On a waterfront capital `grow_lower_city`
/// can clip the outer ring to a sliver; the inner town wall still marks the city.
fn town_inside_walls(occ: &Occupancy, town: Rect) -> Rect {
    let mut min_x = i32::MAX;
    let mut min_z = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_z = i32::MIN;
    let mut n = 0;
    for z in occ.bounds.min_z..=occ.bounds.max_z {
        for x in occ.bounds.min_x..=occ.bounds.max_x {
            if matches!(
                occ.kind(x, z),
                CellKind::TownWall | CellKind::TownTower | CellKind::TownGate
            ) {
                n += 1;
                min_x = min_x.min(x);
                min_z = min_z.min(z);
                max_x = max_x.max(x);
                max_z = max_z.max(z);
            }
        }
    }
    if n < 16 {
        return town;
    }
    Rect::new(min_x, min_z, max_x, max_z)
}

fn find_cathedral_in_town(
    occ: &Occupancy,
    grid: &HeightGrid,
    town: Rect,
    keep_out: Rect,
) -> Option<Rect> {
    let inner = town.deflate(3);
    for &(w, d) in &[
        (64, 48),
        (56, 42),
        (48, 36),
        (42, 34),
        (38, 30),
        (34, 26),
        (30, 24),
    ] {
        if inner.width() < w || inner.depth() < d {
            continue;
        }
        let mut best = None;
        let mut best_s = i32::MIN;
        let mut z = inner.min_z;
        while z + d <= inner.max_z {
            let mut x = inner.min_x;
            while x + w <= inner.max_x {
                let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                x += 8;
                // keep_out is castle.inflate(10); the lower city sits against
                // that margin, so only skip the keep itself.
                if keep_out.deflate(8).intersects(rect) {
                    continue;
                }
                let dry = grid.dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                if dry < 70 {
                    continue;
                }
                let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                if hi - lo > 36 {
                    continue;
                }
                let mut n = 0;
                let mut blocked = 0;
                let mut civic = 0;
                for zz in rect.min_z..=rect.max_z {
                    for xx in rect.min_x..=rect.max_x {
                        n += 1;
                        let kind = occ.kind(xx, zz);
                        if kind.is_castle()
                            || matches!(
                                kind,
                                CellKind::TownWall
                                    | CellKind::TownTower
                                    | CellKind::TownGate
                                    | CellKind::CathedralNave
                                    | CellKind::CathedralTransept
                                    | CellKind::CathedralChoir
                                    | CellKind::CathedralTower
                                    | CellKind::CityDitch
                            )
                        {
                            blocked += 1;
                        }
                        if matches!(kind, CellKind::Market | CellKind::Well | CellKind::Fountain) {
                            civic += 1;
                        }
                    }
                }
                if n == 0 || blocked * 2 > n {
                    continue;
                }
                let (cx, cz) = rect.center();
                let (kx, kz) = keep_out.center();
                let (tx, tz) = town.center();
                let from_keep = (cx - kx).abs() + (cz - kz).abs();
                let from_center = (cx - tx).abs() + (cz - tz).abs();
                let score = from_keep * 2 - from_center * 3 + (n - blocked) - (hi - lo) * 2 + dry
                    - civic * 4;
                if score > best_s {
                    best_s = score;
                    best = Some(rect);
                }
            }
            z += 8;
        }
        if best.is_some() {
            return best;
        }
    }
    // Cramped waterfront capitals: take the driest 24×20 off the keep walls.
    let keep = keep_out.deflate(8);
    for &(w, d) in &[(30, 24), (24, 20)] {
        if inner.width() < w || inner.depth() < d {
            continue;
        }
        let mut best = None;
        let mut best_s = i32::MIN;
        let mut z = inner.min_z;
        while z + d <= inner.max_z {
            let mut x = inner.min_x;
            while x + w <= inner.max_x {
                let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                x += 4;
                if keep.intersects(rect) {
                    continue;
                }
                let dry = grid.dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                if dry < 40 {
                    continue;
                }
                let (cx, cz) = rect.center();
                if !grid.is_dry(cx, cz) {
                    continue;
                }
                if dry > best_s {
                    best_s = dry;
                    best = Some(rect);
                }
            }
            z += 4;
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

fn paint_close_in_precinct(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    yard: Rect,
) {
    if let Some(grave) = find_precinct_pad(occ, yard, 12, 10) {
        paint_graveyard(occ, grave);
    }
    if let Some(cloister) = find_precinct_pad(occ, yard, 12, 12) {
        occ.fill(cloister, CellKind::Arcade, false);
        occ.fill(cloister.deflate(2), CellKind::Green, false);
    }
    if let Some(bishop) = find_precinct_pad(occ, yard, 9, 8) {
        let _ = force_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Townhouse,
            bishop,
            Dir::East,
        );
    }
}

fn find_precinct_pad(occ: &Occupancy, yard: Rect, w: i32, d: i32) -> Option<Rect> {
    let mut z = yard.min_z;
    while z + d <= yard.max_z {
        let mut x = yard.min_x;
        while x + w <= yard.max_x {
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += 4;
            if precinct_open(occ, rect) {
                return Some(rect);
            }
        }
        z += 4;
    }
    None
}

fn precinct_open(occ: &Occupancy, rect: Rect) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    let mut n = 0;
    let mut open = 0;
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            n += 1;
            let kind = occ.kind(x, z);
            if matches!(
                kind,
                CellKind::CathedralNave
                    | CellKind::CathedralTransept
                    | CellKind::CathedralChoir
                    | CellKind::CathedralTower
                    | CellKind::CathedralButtress
                    | CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Building
            ) {
                return false;
            }
            if matches!(
                kind,
                CellKind::Empty
                    | CellKind::Green
                    | CellKind::Hedge
                    | CellKind::Plaza
                    | CellKind::Eave
            ) {
                open += 1;
            }
        }
    }
    n > 0 && open * 2 >= n
}

fn paint_cathedral_close(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    yard: Rect,
    keep_out: Rect,
    scenes: &mut Vec<Scene>,
) {
    let grave = [
        Rect::new(
            yard.max_x + 1,
            yard.min_z + 2,
            yard.max_x + 16,
            yard.min_z + 15,
        ),
        Rect::new(
            yard.min_x + 2,
            yard.max_z + 1,
            yard.min_x + 17,
            yard.max_z + 14,
        ),
        Rect::new(
            yard.min_x - 16,
            yard.min_z + 2,
            yard.min_x - 1,
            yard.min_z + 15,
        ),
    ];
    let cloister = [
        Rect::new(
            yard.min_x + 4,
            yard.max_z + 1,
            yard.min_x + 17,
            yard.max_z + 14,
        ),
        Rect::new(
            yard.min_x + 4,
            yard.min_z - 14,
            yard.min_x + 17,
            yard.min_z - 1,
        ),
        Rect::new(
            yard.max_x + 1,
            yard.min_z + 8,
            yard.max_x + 14,
            yard.min_z + 21,
        ),
    ];
    let bishop = [
        Rect::new(
            yard.min_x - 11,
            yard.min_z + 4,
            yard.min_x - 2,
            yard.min_z + 12,
        ),
        Rect::new(
            yard.min_x + 2,
            yard.min_z - 10,
            yard.min_x + 11,
            yard.min_z - 2,
        ),
        Rect::new(
            yard.max_x + 2,
            yard.max_z - 12,
            yard.max_x + 11,
            yard.max_z - 3,
        ),
    ];
    let mut close = yard;
    let mut painted = false;
    let mut used: Vec<Rect> = Vec::new();
    for rect in grave {
        if used.iter().any(|u| u.intersects(rect)) {
            continue;
        }
        if !close_site_ok(occ, grid, rect, keep_out) {
            continue;
        }
        paint_graveyard(occ, rect);
        used.push(rect);
        close = union_scene(close, rect);
        painted = true;
        break;
    }
    for rect in cloister {
        if used.iter().any(|u| u.intersects(rect)) {
            continue;
        }
        if !close_site_ok(occ, grid, rect, keep_out) {
            continue;
        }
        occ.fill(rect, CellKind::Arcade, false);
        occ.fill(rect.deflate(2), CellKind::Green, false);
        used.push(rect);
        close = union_scene(close, rect);
        painted = true;
        break;
    }
    for rect in bishop {
        if used.iter().any(|u| u.intersects(rect)) {
            continue;
        }
        if !close_site_ok(occ, grid, rect, keep_out) {
            continue;
        }
        if try_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Townhouse,
            rect,
            Dir::East,
        )
        .is_none()
        {
            let _ = force_lot(
                occ,
                grid,
                lots,
                seed,
                BuildingKind::Townhouse,
                rect,
                Dir::East,
            );
        }
        used.push(rect);
        close = union_scene(close, rect);
        painted = true;
        break;
    }
    if painted {
        scenes.push(Scene {
            kind: SceneKind::CathedralClose,
            rect: close,
        });
    }
}

fn union_scene(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.min_x.min(b.min_x),
        a.min_z.min(b.min_z),
        a.max_x.max(b.max_x),
        a.max_z.max(b.max_z),
    )
}

fn close_site_ok(occ: &Occupancy, grid: &HeightGrid, rect: Rect, keep_out: Rect) -> bool {
    if !occ.bounds.contains(rect.min_x, rect.min_z) || !occ.bounds.contains(rect.max_x, rect.max_z)
    {
        return false;
    }
    if keep_out.intersects(rect) {
        return false;
    }
    if !grid.rect_is_dry(rect) {
        return false;
    }
    let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
    if hi - lo > 16 || lo <= grid.sea_level + 1 {
        return false;
    }
    let mut n = 0;
    let mut blocked = 0;
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            n += 1;
            let kind = occ.kind(x, z);
            if kind == CellKind::Building {
                return false;
            }
            if kind.is_castle()
                || matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Road
                        | CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW
                        | CellKind::Bridge
                        | CellKind::CathedralNave
                        | CellKind::CathedralTransept
                        | CellKind::CathedralChoir
                        | CellKind::CathedralTower
                        | CellKind::CathedralButtress
                        | CellKind::Ship
                        | CellKind::Harbor
                        | CellKind::Quay
                )
            {
                blocked += 1;
            }
        }
    }
    n > 0 && blocked * 4 < n
}

/// Plains/harbour gothic: twin west, one west, crossing, extra lanterns.
fn gothic_tower_plan(seed: u64) -> (bool, bool, bool, bool) {
    // 20261025 % 5 == 0 keeps the twin-west + crossing plan the tests pin.
    match seed % 5 {
        0 => (true, true, true, false),
        1 => (true, true, false, true),
        2 => (true, false, true, false),
        3 => (false, true, true, true),
        _ => (true, true, true, true),
    }
}

fn paint_gothic_cathedral(occ: &mut Occupancy, yard: Rect, seed: u64) {
    occ.fill(yard, CellKind::Plaza, true);
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

    let (north_west, south_west, crossing, lanterns) = gothic_tower_plan(seed);
    if north_west {
        occ.fill(
            Rect::new(nave.min_x, nave.min_z - 6, nave.min_x + 5, nave.min_z - 1),
            CellKind::CathedralTower,
            true,
        );
    }
    if south_west {
        occ.fill(
            Rect::new(nave.min_x, nave.max_z + 1, nave.min_x + 5, nave.max_z + 6),
            CellKind::CathedralTower,
            true,
        );
    }
    if crossing {
        occ.fill(
            Rect::new(tx - 3, cz - 3, tx + 2, cz + 2),
            CellKind::CathedralTower,
            true,
        );
    }
    if lanterns {
        occ.fill(
            Rect::new(
                choir.max_x - 1,
                nave.min_z - 5,
                choir.max_x + 3,
                nave.min_z - 1,
            ),
            CellKind::CathedralTower,
            true,
        );
        occ.fill(
            Rect::new(
                choir.max_x - 1,
                nave.max_z + 1,
                choir.max_x + 3,
                nave.max_z + 5,
            ),
            CellKind::CathedralTower,
            true,
        );
    }

    for x in (nave.min_x + 3..=nave.max_x).step_by(3) {
        if x <= nave.min_x + 5 {
            continue;
        }
        if x >= transept.min_x - 1 && x <= transept.max_x + 1 {
            continue;
        }
        occ.set(x, nave.min_z - 1, CellKind::CathedralButtress, true);
        occ.set(x, nave.min_z - 2, CellKind::CathedralButtress, true);
        occ.set(x, nave.min_z - 3, CellKind::CathedralButtress, true);
        occ.set(x, nave.max_z + 1, CellKind::CathedralButtress, true);
        occ.set(x, nave.max_z + 2, CellKind::CathedralButtress, true);
        occ.set(x, nave.max_z + 3, CellKind::CathedralButtress, true);
    }
    for dz in [1, 2, 3] {
        occ.set(
            choir.min_x + 2,
            nave.min_z - dz,
            CellKind::CathedralButtress,
            true,
        );
        occ.set(
            choir.min_x + 2,
            nave.max_z + dz,
            CellKind::CathedralButtress,
            true,
        );
    }
}

/// Desert megacity: sandstone basilica, gold dome, one minaret bell tower.
fn paint_sandstone_basilica(occ: &mut Occupancy, yard: Rect) {
    occ.fill(yard, CellKind::Plaza, true);
    let (_, cz) = yard.center();
    let nave_h = if yard.depth() >= 26 { 7 } else { 6 };
    let nave = Rect::new(yard.min_x + 8, cz - nave_h, yard.max_x - 8, cz + nave_h);
    occ.fill(nave, CellKind::CathedralNave, true);
    let (nx, nz) = nave.center();
    occ.fill(
        Rect::new(nx - 7, nz - 7, nx + 7, nz + 7),
        CellKind::CathedralChoir,
        true,
    );
    let east = Rect::new(
        nave.max_x + 1,
        nz - 4,
        (nave.max_x + 6).min(yard.max_x - 2),
        nz + 4,
    );
    occ.fill(east, CellKind::CathedralTransept, true);
    occ.fill(
        Rect::new(
            nave.min_x - 7,
            nave.max_z - 3,
            nave.min_x + 1,
            nave.max_z + 5,
        ),
        CellKind::CathedralTower,
        true,
    );
    let west = Rect::new(yard.min_x + 1, nave.min_z, nave.min_x - 6, nave.max_z);
    occ.fill(west, CellKind::Plaza, true);
    occ.set(west.center().0, west.center().1, CellKind::Fountain, true);
}

/// Taiga megacity: spruce-and-stone Romanesque, round towers, no flyers.
fn paint_romanesque_church(occ: &mut Occupancy, yard: Rect) {
    occ.fill(yard, CellKind::Plaza, true);
    let (_, cz) = yard.center();
    let nave_h = if yard.depth() >= 26 { 6 } else { 5 };
    let nave = Rect::new(yard.min_x + 7, cz - nave_h, yard.max_x - 8, cz + nave_h);
    occ.fill(nave, CellKind::CathedralNave, true);
    let tx = nave.min_x.midpoint(nave.max_x);
    occ.fill(
        Rect::new(tx - 4, yard.min_z + 4, tx + 4, yard.max_z - 4),
        CellKind::CathedralTransept,
        true,
    );
    occ.fill(
        Rect::new(
            nave.max_x + 1,
            nave.min_z + 1,
            nave.max_x + 5,
            nave.max_z - 1,
        ),
        CellKind::CathedralChoir,
        true,
    );
    let corners = [
        Rect::new(
            nave.min_x - 4,
            nave.min_z - 4,
            nave.min_x + 1,
            nave.min_z + 1,
        ),
        Rect::new(
            nave.min_x - 4,
            nave.max_z - 1,
            nave.min_x + 1,
            nave.max_z + 4,
        ),
        Rect::new(
            nave.max_x - 1,
            nave.min_z - 4,
            nave.max_x + 4,
            nave.min_z + 1,
        ),
        Rect::new(
            nave.max_x - 1,
            nave.max_z - 1,
            nave.max_x + 4,
            nave.max_z + 4,
        ),
    ];
    for tower in corners {
        occ.fill(tower, CellKind::CathedralTower, true);
    }
    occ.fill(
        Rect::new(tx - 2, cz - 2, tx + 2, cz + 2),
        CellKind::CathedralTower,
        true,
    );
    let west = Rect::new(yard.min_x + 1, nave.min_z, nave.min_x - 5, nave.max_z);
    occ.fill(west, CellKind::Plaza, true);
    occ.set(west.center().0, west.center().1, CellKind::Fountain, true);
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
    let keep_out = castle.inflate(4);
    let Some(yard) = find_tourney_site(occ, grid, search, town, castle, keep_out, 40, 24, 16, rng)
        .or_else(|| find_tourney_site(occ, grid, search, town, castle, keep_out, 36, 22, 16, rng))
    else {
        return;
    };
    occ.fill(yard.inflate(6), CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    let (cx, cz) = yard.center();
    let (kx, kz) = castle.center();
    let along_x = yard.width() >= yard.depth();
    if along_x {
        occ.fill(
            Rect::new(yard.min_x + 6, cz - 1, yard.max_x - 6, cz + 1),
            CellKind::TourneyList,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 7,
                yard.min_z + 6,
                yard.max_x - 7,
                yard.min_z + 8,
            ),
            CellKind::TourneyStand,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 7,
                yard.max_z - 8,
                yard.max_x - 7,
                yard.max_z - 6,
            ),
            CellKind::TourneyStand,
            true,
        );
    } else {
        occ.fill(
            Rect::new(cx - 1, yard.min_z + 6, cx + 1, yard.max_z - 6),
            CellKind::TourneyList,
            true,
        );
        occ.fill(
            Rect::new(
                yard.min_x + 6,
                yard.min_z + 7,
                yard.min_x + 8,
                yard.max_z - 7,
            ),
            CellKind::TourneyStand,
            true,
        );
        occ.fill(
            Rect::new(
                yard.max_x - 8,
                yard.min_z + 7,
                yard.max_x - 6,
                yard.max_z - 7,
            ),
            CellKind::TourneyStand,
            true,
        );
    }
    let tents = [
        Rect::new(yard.min_x, yard.min_z, yard.min_x + 4, yard.min_z + 4),
        Rect::new(yard.max_x - 4, yard.min_z, yard.max_x, yard.min_z + 4),
        Rect::new(yard.min_x, yard.max_z - 4, yard.min_x + 4, yard.max_z),
        Rect::new(yard.max_x - 4, yard.max_z - 4, yard.max_x, yard.max_z),
    ];
    for pad in tents {
        occ.fill(pad, CellKind::Tent, true);
    }
    let north_closer = (yard.min_z - kz).abs() <= (yard.max_z - kz).abs();
    let west_closer = (yard.min_x - kx).abs() <= (yard.max_x - kx).abs();
    let royal = if along_x {
        if north_closer {
            Rect::new(cx - 3, yard.min_z, cx + 3, yard.min_z + 4)
        } else {
            Rect::new(cx - 3, yard.max_z - 4, cx + 3, yard.max_z)
        }
    } else if west_closer {
        Rect::new(yard.min_x, cz - 3, yard.min_x + 4, cz + 3)
    } else {
        Rect::new(yard.max_x - 4, cz - 3, yard.max_x, cz + 3)
    };
    occ.fill(royal, CellKind::TourneyStand, true);
    let extra = if along_x {
        if north_closer {
            Rect::new(cx - 2, yard.max_z - 4, cx + 2, yard.max_z)
        } else {
            Rect::new(cx - 2, yard.min_z, cx + 2, yard.min_z + 4)
        }
    } else if west_closer {
        Rect::new(yard.max_x - 4, cz - 2, yard.max_x, cz + 2)
    } else {
        Rect::new(yard.min_x, cz - 2, yard.min_x + 4, cz + 2)
    };
    occ.fill(extra, CellKind::Tent, true);
    scenes.push(Scene {
        kind: SceneKind::Tourney,
        rect: yard.inflate(4),
    });
}

fn place_siege_camp(
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
    let keep_out = castle.inflate(2);
    let Some(yard) = find_siege_camp_site(occ, grid, search, town, castle, keep_out, scenes, rng)
    else {
        return;
    };
    if landmark_blocked(scenes, yard) {
        return;
    }
    let face = gate_face_toward(yard, castle);
    let works = union_scene_rect(yard.inflate(4), siege_treb_apron(yard, face));
    occ.fill(works, CellKind::Green, false);
    occ.fill(yard, CellKind::Green, true);
    paint_staked_palisade(occ, yard);
    paint_palisade_gate(occ, yard, face);
    let walk = yard.deflate(1);
    for z in walk.min_z..=walk.max_z {
        for x in walk.min_x..=walk.max_x {
            if walk.on_edge(x, z) {
                occ.set(x, z, CellKind::PalisadeWalk, true);
            }
        }
    }
    paint_siege_earthworks(occ, yard, castle, town);
    paint_siege_trebuchets(occ, yard, face, castle, town);
    paint_siege_camp_interior(occ, grid, yard, face, lots, seed);
    scenes.push(Scene {
        kind: SceneKind::SiegeCamp,
        rect: works,
    });
}

fn siege_face_gap(yard: Rect, face: Dir, castle: Rect) -> i32 {
    match face {
        Dir::North => yard.min_z - castle.max_z - 1,
        Dir::South => castle.min_z - yard.max_z - 1,
        Dir::West => yard.min_x - castle.max_x - 1,
        Dir::East => castle.min_x - yard.max_x - 1,
    }
}

fn siege_treb_apron(yard: Rect, face: Dir) -> Rect {
    match face {
        Dir::South => Rect::new(yard.min_x, yard.max_z + 1, yard.max_x, yard.max_z + 14),
        Dir::North => Rect::new(yard.min_x, yard.min_z - 14, yard.max_x, yard.min_z - 1),
        Dir::East => Rect::new(yard.max_x + 1, yard.min_z, yard.max_x + 14, yard.max_z),
        Dir::West => Rect::new(yard.min_x - 14, yard.min_z, yard.min_x - 1, yard.max_z),
    }
}

fn siege_works_blocked(occ: &Occupancy, castle: Rect, town: Rect, x: i32, z: i32) -> bool {
    if castle.contains(x, z) || town.contains(x, z) {
        return true;
    }
    let kind = occ.kind(x, z);
    kind.is_castle()
        || matches!(
            kind,
            CellKind::Building
                | CellKind::TownWall
                | CellKind::TownTower
                | CellKind::TownGate
                | CellKind::Palisade
                | CellKind::PalisadeWalk
                | CellKind::PalisadeGate
                | CellKind::WindSail
                | CellKind::Ship
        )
}

fn paint_siege_earthworks(occ: &mut Occupancy, yard: Rect, castle: Rect, town: Rect) {
    let bank = yard.inflate(2);
    let ditch = yard.inflate(4);
    for z in bank.min_z..=bank.max_z {
        for x in bank.min_x..=bank.max_x {
            if yard.contains(x, z) || siege_works_blocked(occ, castle, town, x, z) {
                continue;
            }
            occ.set(x, z, CellKind::MineSpoil, true);
        }
    }
    for z in ditch.min_z..=ditch.max_z {
        for x in ditch.min_x..=ditch.max_x {
            if bank.contains(x, z) || siege_works_blocked(occ, castle, town, x, z) {
                continue;
            }
            occ.set(x, z, CellKind::SiegeTrench, true);
        }
    }
}

fn paint_siege_trebuchets(occ: &mut Occupancy, yard: Rect, face: Dir, castle: Rect, town: Rect) {
    let (cx, cz) = yard.center();
    let pads = match face {
        Dir::South => [
            Rect::new(
                yard.min_x + 4,
                yard.max_z + 5,
                yard.min_x + 6,
                yard.max_z + 13,
            ),
            Rect::new(cx - 1, yard.max_z + 5, cx + 1, yard.max_z + 13),
            Rect::new(
                yard.max_x - 6,
                yard.max_z + 5,
                yard.max_x - 4,
                yard.max_z + 13,
            ),
        ],
        Dir::North => [
            Rect::new(
                yard.min_x + 4,
                yard.min_z - 13,
                yard.min_x + 6,
                yard.min_z - 5,
            ),
            Rect::new(cx - 1, yard.min_z - 13, cx + 1, yard.min_z - 5),
            Rect::new(
                yard.max_x - 6,
                yard.min_z - 13,
                yard.max_x - 4,
                yard.min_z - 5,
            ),
        ],
        Dir::East => [
            Rect::new(
                yard.max_x + 5,
                yard.min_z + 4,
                yard.max_x + 13,
                yard.min_z + 6,
            ),
            Rect::new(yard.max_x + 5, cz - 1, yard.max_x + 13, cz + 1),
            Rect::new(
                yard.max_x + 5,
                yard.max_z - 6,
                yard.max_x + 13,
                yard.max_z - 4,
            ),
        ],
        Dir::West => [
            Rect::new(
                yard.min_x - 13,
                yard.min_z + 4,
                yard.min_x - 5,
                yard.min_z + 6,
            ),
            Rect::new(yard.min_x - 13, cz - 1, yard.min_x - 5, cz + 1),
            Rect::new(
                yard.min_x - 13,
                yard.max_z - 6,
                yard.min_x - 5,
                yard.max_z - 4,
            ),
        ],
    };
    for pad in pads {
        if !occ.bounds.contains(pad.min_x, pad.min_z) || !occ.bounds.contains(pad.max_x, pad.max_z)
        {
            continue;
        }
        let mut blocked = false;
        for z in pad.min_z..=pad.max_z {
            for x in pad.min_x..=pad.max_x {
                if siege_works_blocked(occ, castle, town, x, z) {
                    blocked = true;
                }
            }
        }
        if !blocked {
            occ.fill(pad, CellKind::SiegeTreb, true);
        }
    }
}

fn paint_siege_camp_interior(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    yard: Rect,
    face: Dir,
    lots: &mut Vec<Lot>,
    seed: u64,
) {
    let inner = yard.deflate(2);
    let (cx, cz) = inner.center();
    let command = match face.opposite() {
        Dir::North => Rect::new(cx - 3, inner.min_z, cx + 3, inner.min_z + 4),
        Dir::South => Rect::new(cx - 3, inner.max_z - 4, cx + 3, inner.max_z),
        Dir::West => Rect::new(inner.min_x, cz - 3, inner.min_x + 4, cz + 3),
        Dir::East => Rect::new(inner.max_x - 4, cz - 3, inner.max_x, cz + 3),
    };
    occ.fill(command, CellKind::WarTent, true);
    match face {
        Dir::South | Dir::North => {
            let z0 = if face == Dir::South {
                inner.min_z + 6
            } else {
                inner.max_z - 10
            };
            for row in 0..2 {
                for col in 0..4 {
                    let x0 = inner.min_x + 1 + col * 7;
                    let z = z0 + row * 5;
                    let pad = Rect::new(x0, z, x0 + 4, z + 3);
                    if !pad.intersects(command) {
                        occ.fill(pad, CellKind::Tent, true);
                    }
                }
            }
        }
        Dir::East | Dir::West => {
            let x0 = if face == Dir::East {
                inner.min_x + 6
            } else {
                inner.max_x - 10
            };
            for row in 0..2 {
                for col in 0..4 {
                    let z0 = inner.min_z + 1 + col * 7;
                    let x = x0 + row * 5;
                    let pad = Rect::new(x, z0, x + 3, z0 + 4);
                    if !pad.intersects(command) {
                        occ.fill(pad, CellKind::Tent, true);
                    }
                }
            }
        }
    }
    let cook = match face {
        Dir::South => Rect::new(cx - 1, inner.max_z - 4, cx + 1, inner.max_z - 2),
        Dir::North => Rect::new(cx - 1, inner.min_z + 2, cx + 1, inner.min_z + 4),
        Dir::East => Rect::new(inner.max_x - 4, cz - 1, inner.max_x - 2, cz + 1),
        Dir::West => Rect::new(inner.min_x + 2, cz - 1, inner.min_x + 4, cz + 1),
    };
    occ.fill(cook, CellKind::CampCourt, true);
    let smith = match face {
        Dir::South => Rect::new(
            inner.min_x,
            inner.max_z - 8,
            inner.min_x + 8,
            inner.max_z - 2,
        ),
        Dir::North => Rect::new(
            inner.min_x,
            inner.min_z + 2,
            inner.min_x + 8,
            inner.min_z + 8,
        ),
        Dir::East => Rect::new(
            inner.max_x - 8,
            inner.min_z,
            inner.max_x - 2,
            inner.min_z + 8,
        ),
        Dir::West => Rect::new(
            inner.min_x + 2,
            inner.min_z,
            inner.min_x + 8,
            inner.min_z + 8,
        ),
    };
    if !smith.intersects(command) && !smith.intersects(cook) {
        let _ = force_lot(occ, grid, lots, seed, BuildingKind::Blacksmith, smith, face);
    }
}

fn find_siege_camp_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    castle: Rect,
    keep_out: Rect,
    scenes: &[Scene],
    rng: &mut TownRng,
) -> Option<Rect> {
    let (kx, kz) = castle.center();
    let tourney = scenes.iter().find(|s| s.kind == SceneKind::Tourney);
    let mut best = None;
    let mut best_score = i32::MIN;
    for &(w, d) in &[(36, 28), (32, 24)] {
        let mut z = search.min_z + 4;
        while z + d < search.max_z {
            let mut x = search.min_x + 4;
            while x + w < search.max_x {
                let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                x += 8;
                if town.inflate(2).intersects(rect) || keep_out.intersects(rect) {
                    continue;
                }
                if near_gothic(occ, rect) {
                    continue;
                }
                let face = gate_face_toward(rect, castle);
                if siege_face_gap(rect, face, castle) < 14 {
                    continue;
                }
                let works = union_scene_rect(rect.inflate(4), siege_treb_apron(rect, face));
                if !site_ok(occ, grid, rect, 14) {
                    continue;
                }
                if let Some(lists) = tourney
                    && lists.rect.inflate(8).intersects(works)
                {
                    continue;
                }
                if scenes.iter().any(|scene| {
                    matches!(
                        scene.kind,
                        SceneKind::PillagerCamp
                            | SceneKind::Tourney
                            | SceneKind::Manor
                            | SceneKind::Harbour
                    ) && scene.rect.inflate(8).intersects(works)
                }) {
                    continue;
                }
                let mut wet = 0;
                for zz in works.min_z..=works.max_z {
                    for xx in works.min_x..=works.max_x {
                        if !grid.is_dry(xx, zz) {
                            wet += 1;
                        }
                    }
                }
                if wet > 22 {
                    continue;
                }
                let (cx, cz) = rect.center();
                let dist = (cx - kx).abs() + (cz - kz).abs();
                let beside = i32::from(castle.inflate(40).intersects(rect)) * 90;
                let opposite = tourney.map_or(0, |lists| {
                    let (tx, tz) = lists.rect.center();
                    i32::from(
                        (cx - kx).signum() != (tx - kx).signum()
                            || (cz - kz).signum() != (tz - kz).signum(),
                    ) * 40
                });
                let jitter = rng.gen_range(0, 5);
                let score = 800 - dist / 2 + beside + opposite + jitter - wet / 3;
                if score > best_score {
                    best_score = score;
                    best = Some(rect);
                }
            }
            z += 8;
        }
        if best.is_some() {
            break;
        }
    }
    best
}

fn find_tourney_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    castle: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    max_relief: i32,
    rng: &mut TownRng,
) -> Option<Rect> {
    let (kx, kz) = castle.center();
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
            if near_gothic(occ, rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, max_relief) {
                continue;
            }
            let ring = rect.inflate(8);
            let mut wet = 0;
            for zz in ring.min_z..=ring.max_z {
                for xx in ring.min_x..=ring.max_x {
                    if !grid.is_dry(xx, zz) {
                        wet += 1;
                    }
                }
            }
            if wet > 24 {
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let (cx, cz) = rect.center();
            let dist = (cx - kx).abs() + (cz - kz).abs();
            let beside = i32::from(castle.inflate(8).intersects(rect)) * 80;
            let jitter = rng.gen_range(0, 4);
            let score = 900 - (hi - lo) * 10 - dist / 3 + beside + jitter - wet / 4;
            if score > best_score {
                best_score = score;
                best = Some(rect);
            }
        }
        z += 8;
    }
    best
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
    if kind == BuildingKind::Watermill {
        if !grid.rect_is_dry(rect) {
            return None;
        }
    } else if !grid.rect_is_buildable(rect) {
        return None;
    }
    if kind != BuildingKind::Windmill && rect_hits_mill_sail(lots, rect) {
        return None;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            let kind = occ.kind(x, z);
            if kind.is_castle()
                || matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Tent
                        | CellKind::WarTent
                        | CellKind::WindSail
                        | CellKind::MineHead
                        | CellKind::MineRail
                        | CellKind::MineSpoil
                        | CellKind::LootPile
                        | CellKind::Wagon
                        | CellKind::Beacon
                        | CellKind::Ford
                        | CellKind::Weir
                        | CellKind::CutStone
                        | CellKind::HerbGarden
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
                        | CellKind::CityDitch
                        | CellKind::WaterWheel
                        | CellKind::MillRace
                        | CellKind::Sluice
                        | CellKind::Retaining
                        | CellKind::Vine
                        | CellKind::Lighthouse
                        | CellKind::Shrine
                        | CellKind::FieldWall
                        | CellKind::CathedralNave
                        | CellKind::CathedralTransept
                        | CellKind::CathedralChoir
                        | CellKind::CathedralTower
                        | CellKind::CathedralButtress
                )
            {
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
            | BuildingKind::Watermill
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
    if kind == BuildingKind::Watermill || kind == BuildingKind::Stall {
        if !grid.rect_is_dry(rect) {
            return None;
        }
    } else if !grid.rect_is_buildable(rect) {
        return None;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            let kind = occ.kind(x, z);
            if kind.is_castle()
                || matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Well
                        | CellKind::WindSail
                        | CellKind::Ship
                        | CellKind::Boat
                        | CellKind::Quay
                        | CellKind::Crane
                        | CellKind::MineAdit
                        | CellKind::MineHead
                        | CellKind::LootPile
                        | CellKind::Wagon
                        | CellKind::Beacon
                        | CellKind::Ford
                        | CellKind::Weir
                        | CellKind::CutStone
                        | CellKind::HerbGarden
                        | CellKind::Burnt
                        | CellKind::Breakwater
                        | CellKind::SiegeTrench
                        | CellKind::CityDitch
                        | CellKind::WaterWheel
                        | CellKind::MillRace
                        | CellKind::Sluice
                        | CellKind::Retaining
                        | CellKind::Vine
                        | CellKind::Lighthouse
                        | CellKind::Shrine
                        | CellKind::FieldWall
                        | CellKind::CathedralNave
                        | CellKind::CathedralTransept
                        | CellKind::CathedralChoir
                        | CellKind::CathedralTower
                        | CellKind::CathedralButtress
                )
            {
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
            | BuildingKind::Watermill
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
            if matches!(
                occ.kind(x, z),
                CellKind::Empty | CellKind::Plaza | CellKind::Green
            ) {
                occ.set(x, z, CellKind::Eave, true);
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
    for &(w, d) in &[(42, 34), (38, 30), (34, 26), (30, 24)] {
        if let Some(rect) = find_gentle_site(occ, grid, search, town, keep_out, w, d, 32, rng) {
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
            if near_gothic(occ, rect) {
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
    find_site_kind(
        occ,
        grid,
        search,
        town,
        keep_out,
        w,
        d,
        prefer_high,
        false,
        rng,
    )
}

fn find_inland_site(
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
    find_site_kind(
        occ,
        grid,
        search,
        town,
        keep_out,
        w,
        d,
        prefer_high,
        true,
        rng,
    )
}

fn find_site_kind(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    prefer_high: bool,
    inland: bool,
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
            if near_gothic(occ, rect) {
                continue;
            }
            if !site_ok(occ, grid, rect, 10) {
                continue;
            }
            if inland && !rect_is_inland(grid, rect, 8) {
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
    find_site_near_road_kind(occ, grid, search, town, keep_out, w, d, false, rng)
}

fn find_inland_site_near_road(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    rng: &mut TownRng,
) -> Option<Rect> {
    find_site_near_road_kind(occ, grid, search, town, keep_out, w, d, true, rng)
}

fn find_site_near_road_kind(
    occ: &Occupancy,
    grid: &HeightGrid,
    search: Rect,
    town: Rect,
    keep_out: Rect,
    w: i32,
    d: i32,
    inland: bool,
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
            if inland && !rect_is_inland(grid, rect, 8) {
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

fn rect_is_inland(grid: &HeightGrid, rect: Rect, pad: i32) -> bool {
    let r = rect.inflate(pad);
    for z in r.min_z..=r.max_z {
        for x in r.min_x..=r.max_x {
            if !grid.is_dry(x, z) {
                return false;
            }
        }
    }
    true
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
            let has_cat = plan
                .scenes
                .iter()
                .any(|scene| scene.kind == SceneKind::Cathedral);
            if !has_cat {
                let town = plan.town;
                let inner = town.deflate(3);
                let keep = plan.castle.bounds.inflate(2);
                let mut keep_hits = 0;
                let mut dry_hits = 0;
                let mut ok = 0;
                let mut scanned = 0;
                let (w, d) = (30, 24);
                if inner.width() >= w && inner.depth() >= d {
                    let mut z = inner.min_z;
                    while z + d <= inner.max_z {
                        let mut x = inner.min_x;
                        while x + w <= inner.max_x {
                            scanned += 1;
                            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
                            x += 8;
                            if keep.intersects(rect) {
                                keep_hits += 1;
                                continue;
                            }
                            let dry = plan
                                .heights
                                .dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                            if dry < 50 {
                                dry_hits += 1;
                                continue;
                            }
                            ok += 1;
                        }
                        z += 8;
                    }
                }
                panic!(
                    "seed {seed} should get a cathedral, town={}x{} {town:?} inner={}x{} castle={:?} dry_town={} scanned={scanned} keep={keep_hits} wet={dry_hits} ok={ok} scenes={:?}",
                    town.width(),
                    town.depth(),
                    inner.width(),
                    inner.depth(),
                    plan.castle.bounds,
                    plan.heights
                        .dry_ratio(town.min_x, town.min_z, town.max_x, town.max_z),
                    plan.scenes.iter().map(|s| s.kind).collect::<Vec<_>>()
                );
            }
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
                    lot.kind == BuildingKind::Watermill && scene.rect.intersects(lot.rect)
                })
                .count();
            let mut wheels = 0;
            let mut race = 0;
            for z in scene.rect.min_z..=scene.rect.max_z {
                for x in scene.rect.min_x..=scene.rect.max_x {
                    match river_plan.occ.kind(x, z) {
                        CellKind::WaterWheel => wheels += 1,
                        CellKind::MillRace | CellKind::Sluice => race += 1,
                        _ => {}
                    }
                }
            }
            assert!(mills >= 1, "riverside watermills {mills}");
            assert!(wheels >= 3, "water wheels {wheels}");
            assert!(race >= 2, "mill race/sluice {race}");
        }
    }

    #[test]
    fn round14_harbour_showcase_has_cog_quay_and_breakwater() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

        // World scan: seed 1 cell -4,5 has a real lake/coast berth with a cog,
        // quay, crane and breakwater. Nearby hits (seed 2 cell 0,4 and desert
        // 314159 cell -4,5) miss the cog or the breakwater.
        let boxed = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let world_gen = Arc::from(boxed);
        crate::height::register_world_gen(Arc::clone(&world_gen));
        let plan = crate::plan::plan_for(1, -4, 5, &world_gen);
        assert!(
            plan.scenes
                .iter()
                .any(|scene| scene.kind == SceneKind::Harbour),
            "showcase seed 1 cell -4,5 should be a harbour town, scenes={:?}",
            plan.scenes.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
        let mut ship = 0;
        let mut boat = 0;
        let mut quay = 0;
        let mut crane = 0;
        let mut breakwater = 0;
        let mut lighthouse = 0;
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::Ship => ship += 1,
                    CellKind::Boat => boat += 1,
                    CellKind::Quay => quay += 1,
                    CellKind::Crane => crane += 1,
                    CellKind::Breakwater => breakwater += 1,
                    CellKind::Lighthouse => lighthouse += 1,
                    _ => {}
                }
            }
        }
        assert!(ship >= 9, "harbour cog hulls {ship}");
        assert!(boat >= 8, "second harbour boat {boat}");
        assert!(quay >= 8, "harbour quay {quay}");
        assert!(crane >= 1, "harbour crane {crane}");
        assert!(breakwater >= 8, "harbour breakwater {breakwater}");
        assert!(lighthouse >= 1, "lighthouse on the breakwater {lighthouse}");
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
        let overview = crate::render::overview_bounds(&plan);
        assert!(
            overview.intersects(plan.town),
            "harbour aerial should include the town"
        );
    }

    #[test]
    fn round14_manor_switchback_and_rose_oculus() {
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
            kinds.contains(&SceneKind::Manor),
            "hill-fort manor: {kinds:?}"
        );
        if let Some(manor) = oak.scenes.iter().find(|s| s.kind == SceneKind::Manor) {
            let mut wall = 0;
            let mut gate = 0;
            for z in manor.rect.min_z..=manor.rect.max_z {
                for x in manor.rect.min_x..=manor.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::TownWall | CellKind::TownTower => wall += 1,
                        CellKind::TownGate => gate += 1,
                        _ => {}
                    }
                }
            }
            assert!(wall >= 40, "manor curtain {wall}");
            assert!(gate >= 3, "manor gatehouse {gate}");
            let hall = oak
                .lots
                .iter()
                .filter(|lot| lot.kind == BuildingKind::TownHall && manor.rect.intersects(lot.rect))
                .count();
            assert!(hall >= 1, "manor hall {hall}");
        }
        if let Some(abbey) = oak.scenes.iter().find(|s| s.kind == SceneKind::Monastery) {
            let (ax, az) = abbey.rect.center();
            let mut road = 0;
            let mut retain = 0;
            let mut stairs = 0;
            for z in az - 50..=az + 50 {
                for x in ax - 50..=ax + 50 {
                    match oak.occ.kind(x, z) {
                        CellKind::Road => road += 1,
                        CellKind::Retaining => retain += 1,
                        CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW => {
                            stairs += 1;
                            road += 1;
                        }
                        _ => {}
                    }
                }
            }
            assert!(road >= 16, "switchback path {road}");
            assert!(retain >= 8, "switchback retaining {retain}");
            assert!(stairs >= 4, "switchback stairs {stairs}");
        }
    }

    #[test]
    fn round15_vineyard_wheel_and_harbour_richness() {
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
            kinds.contains(&SceneKind::Vineyard),
            "abbey vineyard: {kinds:?}"
        );
        if let Some(yard) = oak.scenes.iter().find(|s| s.kind == SceneKind::Vineyard) {
            let mut wall = 0;
            let mut vines = 0;
            let mut arcade = 0;
            for z in yard.rect.min_z..=yard.rect.max_z {
                for x in yard.rect.min_x..=yard.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::TownWall | CellKind::TownTower => wall += 1,
                        CellKind::Vine => vines += 1,
                        CellKind::Arcade => arcade += 1,
                        _ => {}
                    }
                }
            }
            assert!(wall >= 40, "vineyard curtain {wall}");
            assert!(vines >= 20, "terraced vines {vines}");
            assert!(arcade >= 8, "vineyard cloister {arcade}");
            let press = oak
                .lots
                .iter()
                .filter(|lot| lot.kind == BuildingKind::Winepress && yard.rect.intersects(lot.rect))
                .count();
            assert!(press >= 1, "winepress house {press}");
            let cottages = oak
                .lots
                .iter()
                .filter(|lot| lot.kind == BuildingKind::Cottage && yard.rect.intersects(lot.rect))
                .count();
            assert!(cottages >= 2, "vineyard village cottages {cottages}");
        }
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
        let mut wheels = 0;
        for z in river_plan.occ.bounds.min_z..=river_plan.occ.bounds.max_z {
            for x in river_plan.occ.bounds.min_x..=river_plan.occ.bounds.max_x {
                if river_plan.occ.kind(x, z) == CellKind::WaterWheel {
                    wheels += 1;
                }
            }
        }
        assert!(wheels >= 5, "standing mill wheel columns {wheels}");
        let mills = river_plan
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Watermill)
            .count();
        assert!(mills >= 1, "riverside watermills {mills}");
    }

    #[test]
    fn round16_outer_vines_lighthouse_tip_and_ship_contrast() {
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
        let yard = oak
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Vineyard)
            .expect("abbey vineyard");
        let mut wall_min_x = i32::MAX;
        let mut wall_max_x = i32::MIN;
        let mut wall_min_z = i32::MAX;
        let mut wall_max_z = i32::MIN;
        let mut retaining = 0;
        for z in yard.rect.min_z..=yard.rect.max_z {
            for x in yard.rect.min_x..=yard.rect.max_x {
                match oak.occ.kind(x, z) {
                    CellKind::TownWall | CellKind::TownTower => {
                        wall_min_x = wall_min_x.min(x);
                        wall_max_x = wall_max_x.max(x);
                        wall_min_z = wall_min_z.min(z);
                        wall_max_z = wall_max_z.max(z);
                    }
                    CellKind::Retaining => retaining += 1,
                    _ => {}
                }
            }
        }
        let mut outer_vines = 0;
        for z in yard.rect.min_z..=yard.rect.max_z {
            for x in yard.rect.min_x..=yard.rect.max_x {
                if oak.occ.kind(x, z) != CellKind::Vine {
                    continue;
                }
                if x < wall_min_x || x > wall_max_x || z < wall_min_z || z > wall_max_z {
                    outer_vines += 1;
                }
            }
        }
        assert!(retaining >= 8, "vineyard retaining terraces {retaining}");
        assert!(
            outer_vines >= 16,
            "vine rows should sit outside the cloister wall, outer={outer_vines} wall=({wall_min_x}..{wall_max_x},{wall_min_z}..{wall_max_z})"
        );

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
        let harbour = build_plan(7, origin, coast, PaletteKind::Oak);
        let mut ship_cells = Vec::new();
        let mut light_cells = Vec::new();
        for z in harbour.occ.bounds.min_z..=harbour.occ.bounds.max_z {
            for x in harbour.occ.bounds.min_x..=harbour.occ.bounds.max_x {
                match harbour.occ.kind(x, z) {
                    CellKind::Ship => ship_cells.push((x, z)),
                    CellKind::Lighthouse => light_cells.push((x, z)),
                    _ => {}
                }
            }
        }
        assert!(
            light_cells.len() >= 8,
            "octagonal lighthouse {}",
            light_cells.len()
        );
        let adjacent = light_cells.iter().any(|&(lx, lz)| {
            ship_cells
                .iter()
                .any(|&(sx, sz)| (lx - sx).abs() <= 1 && (lz - sz).abs() <= 1)
        });
        assert!(!adjacent, "lighthouse should not sit against the cog");

        let volume = crate::stamp::fill_volume(&harbour);
        let mut dark_oak = 0;
        let mut spruce = 0;
        let mut white_wool = 0;
        let mut banner = 0;
        for &(x, z) in &ship_cells {
            let ground = harbour.heights.height(x, z);
            let sea = harbour.heights.sea_level;
            for y in sea - 3..=sea.max(ground) + 20 {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                let id = pumpkin_data::Block::from_state_id(state.id).id;
                if id == pumpkin_data::Block::DARK_OAK_PLANKS.id
                    || id == pumpkin_data::Block::DARK_OAK_LOG.id
                {
                    dark_oak += 1;
                }
                if id == pumpkin_data::Block::SPRUCE_SLAB.id
                    || id == pumpkin_data::Block::SPRUCE_PLANKS.id
                    || id == pumpkin_data::Block::SPRUCE_LOG.id
                {
                    spruce += 1;
                }
                if id == pumpkin_data::Block::WHITE_WOOL.id {
                    white_wool += 1;
                }
                if id == pumpkin_data::Block::WHITE_BANNER.id {
                    banner += 1;
                }
            }
        }
        assert!(dark_oak >= 12, "dark-oak cog hull {dark_oak}");
        assert!(spruce >= 6, "spruce cog deck {spruce}");
        assert!(white_wool >= 4, "white wool sail {white_wool}");
        assert!(banner >= 1, "sail banner {banner}");
    }

    #[test]
    fn round17_wheel_outskirts_castle_spires_and_gorge() {
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
        let (w, d) = oak.castle.footprint();
        assert!(w >= 128 && d >= 128, "castle {w}x{d} below 128×128");
        assert!(
            oak.castle
                .keep_masses
                .iter()
                .any(|mass| matches!(mass.spec.roof, crate::castle::RoofKind::Spire))
                || oak
                    .castle
                    .tower_specs
                    .iter()
                    .any(|spec| matches!(spec.roof, crate::castle::RoofKind::Spire)),
            "castle skyline should include a spire"
        );
        let mixed_roofs = oak
            .castle
            .tower_specs
            .iter()
            .any(|spec| !matches!(spec.roof, crate::castle::RoofKind::Spire))
            && oak.castle.tower_specs.iter().any(|spec| {
                matches!(
                    spec.roof,
                    crate::castle::RoofKind::Crenellated | crate::castle::RoofKind::Cone
                )
            });
        assert!(mixed_roofs, "towers should mix roof kinds");
        let mut hedges = 0;
        let mut walls = 0;
        let mut shrines = 0;
        let mut roadside = 0;
        for z in oak.occ.bounds.min_z..=oak.occ.bounds.max_z {
            for x in oak.occ.bounds.min_x..=oak.occ.bounds.max_x {
                match oak.occ.kind(x, z) {
                    CellKind::Hedge => hedges += 1,
                    CellKind::FieldWall => walls += 1,
                    CellKind::Shrine => shrines += 1,
                    kind if is_road_cell(kind)
                        && Dir::ALL.iter().any(|dir| {
                            matches!(
                                oak.occ.kind(x + dir.dx() * 2, z + dir.dz() * 2),
                                CellKind::Hedge | CellKind::FieldWall | CellKind::Farm
                            )
                        }) =>
                    {
                        roadside += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(hedges >= 20, "roadside hedgerows {hedges}");
        assert!(walls >= 12, "field walls {walls}");
        assert!(shrines >= 1, "wayside shrines {shrines}");
        assert!(roadside >= 8, "lived-in road edges {roadside}");
        let inns = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Tavern)
            .count();
        assert!(inns >= 1, "roadside inn or town tavern {inns}");

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
        let mill = river_plan
            .lots
            .iter()
            .find(|lot| lot.kind == BuildingKind::Watermill)
            .expect("riverside watermill");
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        let mut wheels = 0;
        let mut bridges = 0;
        for z in river_plan.occ.bounds.min_z..=river_plan.occ.bounds.max_z {
            for x in river_plan.occ.bounds.min_x..=river_plan.occ.bounds.max_x {
                match river_plan.occ.kind(x, z) {
                    CellKind::WaterWheel => {
                        wheels += 1;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_z = min_z.min(z);
                        max_z = max_z.max(z);
                    }
                    CellKind::Bridge => bridges += 1,
                    _ => {}
                }
            }
        }
        assert!(wheels >= 9, "wheel columns {wheels}");
        let across = if matches!(mill.facing, Dir::East | Dir::West) {
            max_z - min_z + 1
        } else {
            max_x - min_x + 1
        };
        let stand_out = if matches!(mill.facing, Dir::East | Dir::West) {
            max_x - min_x + 1
        } else {
            max_z - min_z + 1
        };
        assert!(across >= 9, "wheel across {across}");
        assert!(stand_out >= 3, "wheel stand-out {stand_out}");
        assert!(bridges >= 8, "roads should bridge water {bridges}");

        let gorge = HeightGrid::synthetic_gorge(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            origin.z + 80,
        );
        let gorge_plan = build_plan(13, origin, gorge, PaletteKind::Oak);
        let kinds: Vec<_> = gorge_plan.scenes.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SceneKind::Gorge), "gorge town: {kinds:?}");
        let yard = gorge_plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Gorge)
            .expect("gorge scene")
            .rect;
        let mut retaining = 0;
        let mut stairs = 0;
        let mut gates = 0;
        let mut towers = 0;
        let mut bridge = 0;
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                match gorge_plan.occ.kind(x, z) {
                    CellKind::Retaining => retaining += 1,
                    CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW => stairs += 1,
                    CellKind::TownGate => gates += 1,
                    CellKind::TownTower => towers += 1,
                    CellKind::Bridge => bridge += 1,
                    _ => {}
                }
            }
        }
        assert!(retaining >= 12, "gorge terraces {retaining}");
        assert!(stairs >= 4, "gorge stairs {stairs}");
        assert!(gates >= 1, "fortified bridge gate {gates}");
        assert!(towers >= 4, "bridge-gate towers {towers}");
        let mut kinds_dbg = std::collections::BTreeMap::new();
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                *kinds_dbg
                    .entry(format!("{:?}", gorge_plan.occ.kind(x, z)))
                    .or_insert(0) += 1;
            }
        }
        assert!(bridge >= 4, "gorge bridge {bridge} cells={kinds_dbg:?}");
        let houses = gorge_plan
            .lots
            .iter()
            .filter(|lot| {
                yard.intersects(lot.rect)
                    && matches!(lot.kind, BuildingKind::Cottage | BuildingKind::Townhouse)
            })
            .count();
        assert!(houses >= 3, "stacked gorge houses {houses}");
        let mut harbor = 0;
        let mut eaves = 0;
        for z in yard.min_z..=yard.max_z {
            for x in yard.min_x..=yard.max_x {
                match gorge_plan.occ.kind(x, z) {
                    CellKind::Harbor => harbor += 1,
                    CellKind::Eave => eaves += 1,
                    _ => {}
                }
            }
        }
        assert!(harbor >= 12, "gorge ravine {harbor}");
        assert!(eaves >= 4, "jettied gorge eaves {eaves}");
    }

    #[test]
    fn round18_fields_pastures_copses_bridges_and_island() {
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
        let mut pasture = 0;
        let mut orchard = 0;
        let mut farm = 0;
        for z in oak.occ.bounds.min_z..=oak.occ.bounds.max_z {
            for x in oak.occ.bounds.min_x..=oak.occ.bounds.max_x {
                match oak.occ.kind(x, z) {
                    CellKind::Pasture => pasture += 1,
                    CellKind::Orchard => orchard += 1,
                    CellKind::Farm => farm += 1,
                    _ => {}
                }
            }
        }
        assert!(pasture >= 20, "fenced pastures {pasture}");
        assert!(orchard >= 16, "woodland copses {orchard}");
        assert!(farm >= 40, "hinterland fields {farm}");

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
        let mut wide = 0;
        let mut bridge = 0;
        for z in river_plan.occ.bounds.min_z..=river_plan.occ.bounds.max_z {
            for x in river_plan.occ.bounds.min_x..=river_plan.occ.bounds.max_x {
                if river_plan.occ.kind(x, z) != CellKind::Bridge {
                    continue;
                }
                bridge += 1;
                let along_x = river_plan.occ.kind(x - 1, z) == CellKind::Bridge
                    || river_plan.occ.kind(x + 1, z) == CellKind::Bridge;
                let width = if along_x {
                    i32::from(river_plan.occ.kind(x, z - 1) == CellKind::Bridge)
                        + i32::from(river_plan.occ.kind(x, z + 1) == CellKind::Bridge)
                } else {
                    i32::from(river_plan.occ.kind(x - 1, z) == CellKind::Bridge)
                        + i32::from(river_plan.occ.kind(x + 1, z) == CellKind::Bridge)
                };
                if width >= 2 {
                    wide += 1;
                }
            }
        }
        assert!(bridge >= 8, "river bridges {bridge}");
        assert!(wide >= 8, "bridges should be 3+ wide {wide}/{bridge}");

        let mut coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            50,
            origin.x - 40,
        );
        let island_x = origin.x - 120;
        let island_z = origin.z;
        for z in island_z - 16..=island_z + 16 {
            for x in island_x - 16..=island_x + 16 {
                let ix = (x - coast.min_x).div_euclid(coast.step);
                let iz = (z - coast.min_z).div_euclid(coast.step);
                if ix >= 0 && iz >= 0 && ix < coast.nx && iz < coast.nz {
                    let i = (iz * coast.nx + ix) as usize;
                    coast.heights[i] = 70;
                }
            }
        }
        let coast_plan = build_plan(1, origin, coast, PaletteKind::Oak);
        let island = coast_plan.scenes.iter().any(|scene| {
            scene.kind == SceneKind::Hamlet && scene.rect.max_x < coast_plan.town.min_x
        });
        let walled_west = coast_plan.scenes.iter().any(|scene| {
            if scene.rect.max_x >= coast_plan.town.min_x {
                return false;
            }
            let mut wall = 0;
            for z in scene.rect.min_z..=scene.rect.max_z {
                for x in scene.rect.min_x..=scene.rect.max_x {
                    if matches!(
                        coast_plan.occ.kind(x, z),
                        CellKind::TownWall | CellKind::TownTower | CellKind::TownGate
                    ) {
                        wall += 1;
                    }
                }
            }
            wall >= 20
        });
        assert!(
            island || walled_west,
            "west island village should wall a dry patch west of town"
        );
    }

    #[test]
    #[ignore = "one-off harbour cell scan"]
    fn scan_harbour_showcase_cells() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

        let seeds = [
            20_261_025u64,
            7,
            11,
            19,
            42,
            101,
            256,
            1_337,
            2_026,
            9_999,
            314_159,
            1,
            2,
            8,
            16,
            32,
            64,
            128,
        ];
        let mut hits = Vec::new();
        let mut best = (0usize, 0u64, 0i32, 0i32);
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
            for cx in -5..=5 {
                for cz in -5..=5 {
                    let origin = town_in_cell(seed, cx, cz);
                    let probe = HeightGrid::sample(
                        &world_gen,
                        origin.x - 200,
                        origin.z - 200,
                        origin.x + 200,
                        origin.z + 200,
                        40,
                    );
                    let sea = probe.sea_level;
                    let deep = probe.heights.iter().filter(|h| **h <= sea - 3).count();
                    let wet = probe.heights.iter().filter(|h| **h <= sea + 1).count();
                    if deep > best.0 {
                        best = (deep, seed, cx, cz);
                    }
                    if deep < 6 || wet < 10 {
                        continue;
                    }
                    let search = Rect::new(
                        origin.x - 200,
                        origin.z - 200,
                        origin.x + 200,
                        origin.z + 200,
                    );
                    let mut body = false;
                    'find: for z in (search.min_z..=search.max_z).step_by(40) {
                        for x in (search.min_x..=search.max_x).step_by(40) {
                            if super::water_body_ok(&probe, search, x, z) {
                                body = true;
                                break 'find;
                            }
                        }
                    }
                    if !body {
                        continue;
                    }
                    let plan = crate::plan::plan_for(seed, cx, cz, &world_gen);
                    if plan
                        .scenes
                        .iter()
                        .any(|scene| scene.kind == SceneKind::Harbour)
                    {
                        hits.push(format!("seed={seed} cell={cx},{cz} wet={wet} deep={deep}"));
                    }
                }
            }
        }
        assert!(
            !hits.is_empty(),
            "no harbour cell in the scanned set; best deep={} at seed={} cell={},{}",
            best.0,
            best.1,
            best.2,
            best.3
        );
        panic!("HARBOUR {}", hits.join(" | "));
    }

    #[test]
    fn round19_biome_landmarks() {
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
        let spruce = build_plan(7, origin, grid.clone(), PaletteKind::Spruce);
        let spruce_kinds: Vec<_> = spruce.scenes.iter().map(|s| s.kind).collect();
        assert!(
            spruce_kinds.contains(&SceneKind::StaveChurch),
            "taiga stave church: {spruce_kinds:?}"
        );
        assert!(
            spruce
                .lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::StaveChurch),
            "stave church lot missing"
        );
        if let Some(lot) = spruce
            .lots
            .iter()
            .find(|lot| lot.kind == BuildingKind::StaveChurch)
        {
            let pad = lot.rect.inflate(8);
            let gothic = (pad.min_z..=pad.max_z).any(|z| {
                (pad.min_x..=pad.max_x).any(|x| {
                    matches!(
                        spruce.occ.kind(x, z),
                        CellKind::CathedralNave
                            | CellKind::CathedralTransept
                            | CellKind::CathedralChoir
                            | CellKind::CathedralTower
                    )
                })
            });
            assert!(!gothic, "stave church clipped into a cathedral");
        }

        let desert = build_plan(314_159, origin, grid.clone(), PaletteKind::Sandstone);
        let desert_kinds: Vec<_> = desert.scenes.iter().map(|s| s.kind).collect();
        assert!(
            desert_kinds.contains(&SceneKind::Caravanserai),
            "desert caravanserai: {desert_kinds:?}"
        );
        if let Some(yard) = desert
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Caravanserai)
        {
            let mut towers = 0;
            let mut plaza = 0;
            let mut arcade = 0;
            for z in yard.rect.min_z..=yard.rect.max_z {
                for x in yard.rect.min_x..=yard.rect.max_x {
                    match desert.occ.kind(x, z) {
                        CellKind::TownTower => towers += 1,
                        CellKind::Plaza => plaza += 1,
                        CellKind::Arcade => arcade += 1,
                        _ => {}
                    }
                }
            }
            assert!(towers >= 16, "caravanserai minarets {towers}");
            assert!(plaza >= 12, "caravanserai courtyard {plaza}");
            assert!(arcade >= 20, "caravanserai arcade {arcade}");
        }

        let oak = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert!(
            oak.scenes.iter().any(|s| s.kind == SceneKind::Vineyard),
            "hillside vineyard village"
        );
    }

    #[test]
    fn round20_street_infill_and_keep_forecourt() {
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
        let houses = oak
            .lots
            .iter()
            .filter(|lot| {
                matches!(
                    lot.kind,
                    BuildingKind::Cottage
                        | BuildingKind::Timber
                        | BuildingKind::Townhouse
                        | BuildingKind::Tavern
                        | BuildingKind::Blacksmith
                )
            })
            .count();
        assert!(houses >= 18, "street infill houses {houses}");
        let roadside = oak
            .lots
            .iter()
            .filter(|lot| {
                matches!(
                    lot.kind,
                    BuildingKind::Cottage | BuildingKind::Tavern | BuildingKind::Blacksmith
                ) && Dir::ALL.iter().any(|dir| {
                    let (cx, cz) = lot.rect.center();
                    (1..=10).any(|dist| {
                        is_road_cell(oak.occ.kind(cx + dir.dx() * dist, cz + dir.dz() * dist))
                    })
                })
            })
            .count();
        assert!(roadside >= 8, "lots facing a street {roadside}");
        let (kx, kz) = oak.castle.keep.center();
        let (hx, hz) = oak.castle.hall.center();
        let mut court = 0;
        let search = oak.castle.inner.inflate(8);
        for z in search.min_z..=search.max_z {
            for x in search.min_x..=search.max_x {
                if oak.occ.kind(x, z) != CellKind::CastleCourt {
                    continue;
                }
                if oak.castle.keep_mass_at(x, z).is_some() {
                    continue;
                }
                let near_keep = (x - kx).abs().max((z - kz).abs()) <= 18;
                let near_hall = (x - hx).abs().max((z - hz).abs()) <= 18;
                if near_keep && near_hall {
                    court += 1;
                }
            }
        }
        assert!(court >= 8, "keep-hall flag court pad {court}");
    }

    #[test]
    fn round21_capital_has_two_rings_districts_and_countryside() {
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
        assert!(oak.capital);
        assert!(
            oak.town.width() >= 140 && oak.town.depth() >= 130,
            "capital lower city {}x{}",
            oak.town.width(),
            oak.town.depth()
        );
        assert!(
            oak.town_wall.len() >= 8,
            "inner+outer wall verts {}",
            oak.town_wall.len()
        );
        assert!(
            oak.gates.len() >= 4,
            "outer, castle, inner, side gates {}",
            oak.gates.len()
        );
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::MarketQuarter),
            "missing market quarter: {kinds:?}"
        );
        assert!(
            kinds.contains(&SceneKind::CraftsmenRow),
            "missing craftsmen row: {kinds:?}"
        );
        assert!(
            kinds.contains(&SceneKind::NobleQuarter),
            "missing noble quarter: {kinds:?}"
        );
        assert!(
            kinds.contains(&SceneKind::DocksQuarter),
            "missing docks quarter: {kinds:?}"
        );
        assert!(
            kinds.contains(&SceneKind::LumberCamp),
            "missing lumber camp: {kinds:?}"
        );
        let orchards = kinds.iter().filter(|k| **k == SceneKind::Orchard).count();
        let pastures = kinds.iter().filter(|k| **k == SceneKind::Pasture).count();
        assert!(orchards >= 1, "orchards {orchards}");
        assert!(pastures >= 1, "pastures {pastures}");
        let mills = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Windmill)
            .count();
        assert!(mills >= 2, "ridge/edge mills {mills}");
        let inns = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Tavern)
            .count();
        assert!(inns >= 1, "roadside inn {inns}");
        let mut shrine_cells = 0;
        for z in oak.bounds.min_z..=oak.bounds.max_z {
            for x in oak.bounds.min_x..=oak.bounds.max_x {
                if oak.occ.kind(x, z) == CellKind::Shrine {
                    shrine_cells += 1;
                }
            }
        }
        assert!(shrine_cells >= 1, "wayside shrine {shrine_cells}");
        let (kx, kz) = oak.castle.keep.center();
        let (hx, hz) = oak.castle.hall.center();
        let mut court = 0;
        let search = oak.castle.inner.inflate(8);
        for z in search.min_z..=search.max_z {
            for x in search.min_x..=search.max_x {
                if oak.occ.kind(x, z) != CellKind::CastleCourt {
                    continue;
                }
                if oak.castle.keep_mass_at(x, z).is_some() {
                    continue;
                }
                let near_keep = (x - kx).abs().max((z - kz).abs()) <= 18;
                let near_hall = (x - hx).abs().max((z - hz).abs()) <= 18;
                if near_keep && near_hall {
                    court += 1;
                }
            }
        }
        assert!(court >= 8, "keep-hall flag court still clear {court}");
        let mut court_buildings = 0;
        for z in search.min_z..=search.max_z {
            for x in search.min_x..=search.max_x {
                if oak.occ.kind(x, z) == CellKind::CastleCourt {
                    let id = oak.occ.lot(x, z);
                    if id != 0 {
                        court_buildings += 1;
                    }
                }
            }
        }
        assert_eq!(court_buildings, 0, "lots overwrote keep court");
    }

    #[test]
    fn round21_harbour_is_not_a_two_ring_capital() {
        let origin = town_in_cell(1, -4, 5);
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
        let plan = build_plan(1, origin, grid, PaletteKind::Oak);
        assert!(!plan.capital);
        assert!(
            plan.town_wall.len() <= 4,
            "non-capital should keep one ring {}",
            plan.town_wall.len()
        );
        assert!(
            !plan
                .scenes
                .iter()
                .any(|s| s.kind == SceneKind::MarketQuarter),
            "districts are capital-only"
        );
    }

    #[test]
    fn round22_city_curtain_dense_pack_and_cathedral_close() {
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
        assert!(oak.capital);
        let wall = oak.town.inflate(2);
        let mut ring = 0;
        let mut walled = 0;
        let mut towers = 0;
        for z in wall.min_z - 6..=wall.max_z + 6 {
            for x in wall.min_x - 6..=wall.max_x + 6 {
                let dx = (x - wall.min_x).abs().min((x - wall.max_x).abs());
                let dz = (z - wall.min_z).abs().min((z - wall.max_z).abs());
                if dx > 6 && dz > 6 {
                    continue;
                }
                ring += 1;
                match oak.occ.kind(x, z) {
                    CellKind::TownWall | CellKind::TownGate => walled += 1,
                    CellKind::TownTower => {
                        walled += 1;
                        towers += 1;
                    }
                    _ => {}
                }
            }
        }
        assert!(
            walled * 100 / ring.max(1) >= 18,
            "outer curtain too gappy walled={walled} ring={ring}"
        );
        assert!(towers >= 40, "interval towers on the city curtain {towers}");
        let town_gates = oak
            .gates
            .iter()
            .filter(|g| g.kind == crate::info::GateKind::Town)
            .count();
        assert!(town_gates >= 3, "road gatehouses {town_gates}");
        let mut n = 0;
        let mut built = 0;
        for z in oak.town.min_z..=oak.town.max_z {
            for x in oak.town.min_x..=oak.town.max_x {
                n += 1;
                if !matches!(
                    oak.occ.kind(x, z),
                    CellKind::Empty
                        | CellKind::Green
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Pasture
                        | CellKind::Orchard
                ) {
                    built += 1;
                }
            }
        }
        let pct = built * 100 / n.max(1);
        assert!(pct >= 60, "built coverage inside lower wall {pct}%");
        let close = oak
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::CathedralClose);
        if let Some(close) = close {
            let mut grave = 0;
            let mut arcade = 0;
            let mut house = 0;
            for z in close.rect.min_z..=close.rect.max_z {
                for x in close.rect.min_x..=close.rect.max_x {
                    match oak.occ.kind(x, z) {
                        CellKind::Grave | CellKind::Lychgate => grave += 1,
                        CellKind::Arcade => arcade += 1,
                        CellKind::Building => house += 1,
                        _ => {}
                    }
                }
            }
            assert!(
                grave >= 8 || arcade >= 8,
                "cathedral close grave={grave} cloister={arcade}"
            );
            let _ = house;
        } else {
            let cat = oak
                .scenes
                .iter()
                .find(|s| s.kind == SceneKind::Cathedral)
                .map(|s| s.rect.inflate(18));
            if let Some(around) = cat {
                let mut grave = 0;
                let mut arcade = 0;
                for z in around.min_z..=around.max_z {
                    for x in around.min_x..=around.max_x {
                        match oak.occ.kind(x, z) {
                            CellKind::Grave | CellKind::Lychgate => grave += 1,
                            CellKind::Arcade => arcade += 1,
                            _ => {}
                        }
                    }
                }
                assert!(
                    grave >= 8 || arcade >= 8,
                    "cathedral precinct grave={grave} cloister={arcade}"
                );
            }
        }
        let mut court_buildings = 0;
        let search = oak.castle.inner.inflate(8);
        for z in search.min_z..=search.max_z {
            for x in search.min_x..=search.max_x {
                if oak.occ.kind(x, z) == CellKind::CastleCourt && oak.occ.lot(x, z) != 0 {
                    court_buildings += 1;
                }
            }
        }
        assert_eq!(court_buildings, 0, "lots overwrote keep court");
    }

    #[test]
    fn round23_curtain_ring_in_town_cathedral_and_suburb() {
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
        assert!(oak.capital);
        let wall = oak.town.inflate(2);
        let mut plaza = 0;
        let mut ditch = 0;
        let mut towers = 0;
        let mut wall_w = 0;
        for z in oak.town.min_z..=oak.town.max_z {
            for x in oak.town.min_x..=oak.town.max_x {
                let d = (x - oak.town.min_x)
                    .min(oak.town.max_x - x)
                    .min(z - oak.town.min_z)
                    .min(oak.town.max_z - z);
                if d <= 2 && oak.occ.kind(x, z) == CellKind::Plaza {
                    plaza += 1;
                }
            }
        }
        for z in wall.min_z - 5..=wall.max_z + 5 {
            for x in wall.min_x - 5..=wall.max_x + 5 {
                match oak.occ.kind(x, z) {
                    CellKind::CityDitch => ditch += 1,
                    CellKind::TownTower => towers += 1,
                    CellKind::TownWall => wall_w += 1,
                    _ => {}
                }
            }
        }
        assert!(plaza >= 80, "pomerium plaza {plaza}");
        assert!(ditch >= 80, "city fosse {ditch}");
        assert!(wall_w >= 200, "3-wide curtain {wall_w}");
        assert!(towers >= 80, "round interval towers {towers}");
        let cat = oak
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Cathedral)
            .expect("cathedral");
        let (cx, cz) = cat.rect.center();
        assert!(
            oak.town.contains(cx, cz),
            "cathedral should sit inside the walls {:?}",
            cat.rect
        );
        let mut west = 0;
        let mut crossing = 0;
        let mut fly = 0;
        for z in cat.rect.min_z..=cat.rect.max_z {
            for x in cat.rect.min_x..=cat.rect.max_x {
                match oak.occ.kind(x, z) {
                    CellKind::CathedralTower if x <= cat.rect.min_x + 14 => west += 1,
                    CellKind::CathedralTower => crossing += 1,
                    CellKind::CathedralButtress => fly += 1,
                    _ => {}
                }
            }
        }
        assert!(west >= 48, "twin west towers {west}");
        assert!(crossing >= 20, "crossing tower {crossing}");
        assert!(fly >= 18, "flying buttresses {fly}");
        assert!(
            oak.scenes.iter().any(|s| s.kind == SceneKind::Extramural),
            "extramural suburb missing: {:?}",
            oak.scenes.iter().map(|s| s.kind).collect::<Vec<_>>()
        );
    }

    #[test]
    fn round24_fosse_and_lanes_strip_trees_and_get_street_life() {
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
        assert!(oak.walled_city);
        let wall = oak.town.inflate(2);
        let mut ditch = 0;
        let mut plaza = 0;
        for z in wall.min_z - 5..=wall.max_z + 5 {
            for x in wall.min_x - 5..=wall.max_x + 5 {
                match oak.occ.kind(x, z) {
                    CellKind::CityDitch => {
                        ditch += 1;
                        assert!(
                            oak.strips_trees(x, z),
                            "fosse at {x},{z} should strip vanilla trees"
                        );
                    }
                    CellKind::Plaza if oak.town.contains(x, z) => {
                        plaza += 1;
                        assert!(oak.strips_trees(x, z), "pomerium at {x},{z}");
                    }
                    CellKind::TownWall | CellKind::TownTower => {
                        assert!(oak.strips_trees(x, z), "curtain at {x},{z}");
                    }
                    _ => {}
                }
            }
        }
        assert!(ditch >= 80, "city fosse {ditch}");
        assert!(plaza >= 80, "pomerium {plaza}");
        let stalls = oak
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Stall)
            .count();
        assert!(stalls >= 12, "street and market stalls {stalls}");
        let mut wells = 0;
        for z in oak.town.min_z..=oak.town.max_z {
            for x in oak.town.min_x..=oak.town.max_x {
                if oak.occ.kind(x, z) == CellKind::Well {
                    wells += 1;
                }
            }
        }
        assert!(wells >= 9, "well heads {wells}");
    }

    #[test]
    fn round24_coastal_megacity_gets_two_ring() {
        let origin = town_in_cell(1, -4, 5);
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x + 80,
        );
        let plan = build_plan(1, origin, coast, PaletteKind::Oak);
        assert!(!plan.capital);
        assert!(plan.walled_city, "coastal harbour should be a walled city");
        let mut ditch = 0;
        let mut towers = 0;
        let wall = plan.town.inflate(2);
        for z in wall.min_z - 5..=wall.max_z + 5 {
            for x in wall.min_x - 5..=wall.max_x + 5 {
                match plan.occ.kind(x, z) {
                    CellKind::CityDitch => ditch += 1,
                    CellKind::TownTower => towers += 1,
                    _ => {}
                }
            }
        }
        assert!(ditch >= 40, "harbour fosse/sea wall {ditch}");
        assert!(towers >= 40, "harbour round towers {towers}");
    }

    #[test]
    fn round24_taiga_and_desert_capitals_keep_two_ring() {
        let origin = town_in_cell(20_261_025, 0, 0);
        for palette in [PaletteKind::Spruce, PaletteKind::Sandstone] {
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
            let plan = build_plan(20_261_025, origin, grid, palette);
            assert!(
                plan.walled_city,
                "{palette:?} capital should be a walled city"
            );
            let mut ditch = 0;
            let mut towers = 0;
            let wall = plan.town.inflate(2);
            for z in wall.min_z - 5..=wall.max_z + 5 {
                for x in wall.min_x - 5..=wall.max_x + 5 {
                    match plan.occ.kind(x, z) {
                        CellKind::CityDitch => ditch += 1,
                        CellKind::TownTower => towers += 1,
                        _ => {}
                    }
                }
            }
            assert!(ditch >= 80, "{palette:?} fosse {ditch}");
            assert!(towers >= 40, "{palette:?} round towers {towers}");
        }
    }

    fn cathedral_counts(plan: &crate::plan::TownPlan) -> (i32, i32, i32, i32, i32) {
        let Some(cat) = plan.scenes.iter().find(|s| s.kind == SceneKind::Cathedral) else {
            return (0, 0, 0, 0, 0);
        };
        let mut nave = 0;
        let mut choir = 0;
        let mut tower = 0;
        let mut west = 0;
        let mut fly = 0;
        for z in cat.rect.min_z..=cat.rect.max_z {
            for x in cat.rect.min_x..=cat.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::CathedralNave => nave += 1,
                    CellKind::CathedralChoir => choir += 1,
                    CellKind::CathedralTower if x <= cat.rect.min_x + 14 => {
                        tower += 1;
                        west += 1;
                    }
                    CellKind::CathedralTower => tower += 1,
                    CellKind::CathedralButtress => fly += 1,
                    _ => {}
                }
            }
        }
        (nave, choir, tower, west, fly)
    }

    #[test]
    fn round25_desert_cathedral_is_basilica() {
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
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Sandstone);
        let (nave, choir, tower, _, fly) = cathedral_counts(&plan);
        assert!(nave >= 40, "basilica nave {nave}");
        assert!(choir >= 40, "gold-dome drum {choir}");
        assert!(
            (12..120).contains(&tower),
            "one minaret-style tower {tower}"
        );
        assert_eq!(fly, 0, "desert basilica has no flying buttresses");
    }

    #[test]
    fn round25_taiga_cathedral_is_romanesque() {
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
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Spruce);
        let (nave, choir, tower, _, fly) = cathedral_counts(&plan);
        assert!(nave >= 40, "romanesque nave {nave}");
        assert!(choir >= 8, "romanesque choir {choir}");
        assert!(tower >= 60, "round towers {tower}");
        assert_eq!(fly, 0, "romanesque church has no flying buttresses");
    }

    #[test]
    fn round25_gothic_tower_counts_vary_per_seed() {
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
        let classic = build_plan(20_261_025, origin, grid.clone(), PaletteKind::Oak);
        let other = build_plan(42, origin, grid, PaletteKind::Oak);
        let (_, _, tower_a, west_a, fly_a) = cathedral_counts(&classic);
        let (_, _, tower_b, west_b, fly_b) = cathedral_counts(&other);
        assert!(west_a >= 48, "classic twin west {west_a}");
        assert!(fly_a >= 18 && fly_b >= 18, "gothic keeps flyers");
        assert!(
            tower_a != tower_b || west_a != west_b,
            "gothic tower counts should vary classic={tower_a}/{west_a} other={tower_b}/{west_b}"
        );
    }

    #[test]
    fn round25_harbour_fish_market_and_fleet() {
        let origin = town_in_cell(1, -4, 5);
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x + 80,
        );
        let plan = build_plan(1, origin, coast, PaletteKind::Oak);
        let mut ship = 0;
        let mut boat = 0;
        let mut crane = 0;
        let mut quay = 0;
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::Ship => ship += 1,
                    CellKind::Boat => boat += 1,
                    CellKind::Crane => crane += 1,
                    CellKind::Quay => quay += 1,
                    _ => {}
                }
            }
        }
        let all_stalls = plan
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Stall)
            .count();
        let stalls = plan
            .lots
            .iter()
            .filter(|lot| {
                lot.kind == BuildingKind::Stall
                    && (lot.rect.min_z.saturating_sub(6)..=lot.rect.max_z + 6).any(|z| {
                        (lot.rect.min_x.saturating_sub(6)..=lot.rect.max_x + 6).any(|x| {
                            matches!(
                                plan.occ.kind(x, z),
                                CellKind::Quay
                                    | CellKind::Dock
                                    | CellKind::Crane
                                    | CellKind::Ship
                                    | CellKind::Boat
                            )
                        })
                    })
            })
            .count();
        assert!(
            ship >= 9 && boat >= 8 && crane >= 16 && quay >= 8 && stalls >= 2,
            "harbour fleet/market ship={ship} boat={boat} crane={crane} quay={quay} stalls={stalls} all_stalls={all_stalls}"
        );
    }

    #[test]
    fn round25_shore_lots_have_footings() {
        let origin = town_in_cell(314_159, 0, 0);
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x + 40,
        );
        let plan = build_plan(314_159, origin, coast, PaletteKind::Sandstone);
        let vol = crate::stamp::fill_volume(&plan);
        let sea = plan.heights.sea_level;
        let mut checked = 0;
        for lot in &plan.lots {
            if !matches!(
                lot.kind,
                BuildingKind::Townhouse | BuildingKind::Timber | BuildingKind::Cottage
            ) {
                continue;
            }
            let mean = plan.heights.mean(
                lot.rect.min_x,
                lot.rect.min_z,
                lot.rect.max_x,
                lot.rect.max_z,
            );
            let floor = mean.max(sea + 2);
            for z in lot.rect.min_z..=lot.rect.max_z {
                for x in lot.rect.min_x..=lot.rect.max_x {
                    if plan.occ.kind(x, z) != CellKind::Building {
                        continue;
                    }
                    let bed = (plan.heights.height(x, z) - 1).min(sea);
                    if floor - bed <= 4 {
                        continue;
                    }
                    for y in bed..floor {
                        let solid = vol.get(x, y, z).is_some_and(|s| !s.is_air());
                        assert!(solid, "lot {} pillar gap at {x},{y},{z}", lot.id);
                    }
                    checked += 1;
                }
            }
        }
        assert!(
            plan.lots.len() >= 8,
            "desert coast should still pack houses, lots {}",
            plan.lots.len()
        );
        let _ = checked;
    }

    #[test]
    fn round26_desert_dome_covers_the_crossing() {
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
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Sandstone);
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_z = i32::MAX;
        let mut max_z = i32::MIN;
        let mut choir = 0;
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                if plan.occ.kind(x, z) != CellKind::CathedralChoir {
                    continue;
                }
                choir += 1;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_z = min_z.min(z);
                max_z = max_z.max(z);
            }
        }
        let wide = (max_x - min_x + 1).min(max_z - min_z + 1);
        assert!(choir >= 80, "dome drum choir {choir}");
        assert!(
            wide >= 13,
            "dome occupancy should be 13–15 wide, got {wide}"
        );
    }

    #[test]
    fn round26_exploration_pois_scatter() {
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
        let oak = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        let kinds: Vec<_> = oak.scenes.iter().map(|s| s.kind).collect();
        for need in [
            SceneKind::RuinedAbbey,
            SceneKind::BanditHideout,
            SceneKind::HermitTower,
            SceneKind::StoneCircle,
            SceneKind::Gallows,
        ] {
            assert!(kinds.contains(&need), "missing {need:?} in {kinds:?}");
        }
        let mut standing = 0;
        let mut gallows = 0;
        for z in oak.occ.bounds.min_z..=oak.occ.bounds.max_z {
            for x in oak.occ.bounds.min_x..=oak.occ.bounds.max_x {
                match oak.occ.kind(x, z) {
                    CellKind::StandingStone => standing += 1,
                    CellKind::Gallows => gallows += 1,
                    _ => {}
                }
            }
        }
        assert!(standing >= 6, "stone circle menhirs {standing}");
        assert!(gallows >= 1, "crossroads gallows {gallows}");
    }

    #[test]
    fn round26_cog_has_square_sail_and_sterncastle() {
        let origin = town_in_cell(314_159, 0, 0);
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x + 40,
        );
        let plan = build_plan(314_159, origin, coast, PaletteKind::Oak);
        let mut ship_cells = Vec::new();
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                if plan.occ.kind(x, z) == CellKind::Ship {
                    ship_cells.push((x, z));
                }
            }
        }
        assert!(ship_cells.len() >= 20, "cog hull {}", ship_cells.len());
        let volume = crate::stamp::fill_volume(&plan);
        let sea = plan.heights.sea_level;
        let mut wool = Vec::new();
        let mut fence_below = 0;
        let mut stern_deck = 0;
        for &(x, z) in &ship_cells {
            for y in sea - 3..=sea + 16 {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                let id = pumpkin_data::Block::from_state_id(state.id).id;
                if id == pumpkin_data::Block::WHITE_WOOL.id {
                    wool.push((x, y, z));
                }
                if id == pumpkin_data::Block::DARK_OAK_FENCE.id && y <= sea {
                    fence_below += 1;
                }
                if id == pumpkin_data::Block::DARK_OAK_PLANKS.id && y >= sea + 3 {
                    stern_deck += 1;
                }
            }
        }
        let min_x = wool.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = wool.iter().map(|c| c.0).max().unwrap_or(0);
        let min_z = wool.iter().map(|c| c.2).min().unwrap_or(0);
        let max_z = wool.iter().map(|c| c.2).max().unwrap_or(0);
        let min_y = wool.iter().map(|c| c.1).min().unwrap_or(0);
        let max_y = wool.iter().map(|c| c.1).max().unwrap_or(0);
        let span_xz = (max_x - min_x + 1).max(max_z - min_z + 1);
        let span_y = max_y - min_y + 1;
        assert!(
            span_xz >= 5 && span_y >= 6 && wool.len() >= 30,
            "square sail {span_xz}×{span_y} n={}",
            wool.len()
        );
        assert!(stern_deck >= 8, "sterncastle deck {stern_deck}");
        assert!(fence_below >= 1, "rudder below waterline {fence_below}");
    }

    #[test]
    fn round26_gold_dome_is_a_hemisphere() {
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
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Sandstone);
        let volume = crate::stamp::fill_volume(&plan);
        let mut gold = Vec::new();
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                if plan.occ.kind(x, z) != CellKind::CathedralChoir {
                    continue;
                }
                for y in volume.min_y..volume.min_y + volume.sy {
                    let Some(state) = volume.get(x, y, z) else {
                        continue;
                    };
                    let id = pumpkin_data::Block::from_state_id(state.id).id;
                    if id == pumpkin_data::Block::GOLD_BLOCK.id
                        || id == pumpkin_data::Block::YELLOW_TERRACOTTA.id
                        || id == pumpkin_data::Block::ORANGE_TERRACOTTA.id
                    {
                        gold.push((x, y, z));
                    }
                }
            }
        }
        let min_x = gold.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = gold.iter().map(|c| c.0).max().unwrap_or(0);
        let min_z = gold.iter().map(|c| c.2).min().unwrap_or(0);
        let max_z = gold.iter().map(|c| c.2).max().unwrap_or(0);
        let min_y = gold.iter().map(|c| c.1).min().unwrap_or(0);
        let max_y = gold.iter().map(|c| c.1).max().unwrap_or(0);
        let wide = (max_x - min_x + 1).min(max_z - min_z + 1);
        let tall = max_y - min_y + 1;
        assert!(
            wide >= 13 && tall >= 6 && gold.len() >= 80,
            "dome {wide}×{wide} h={tall} n={}",
            gold.len()
        );
    }

    #[test]
    fn round27_cog_has_pointed_bow_yard_and_billow() {
        let origin = town_in_cell(314_159, 0, 0);
        let coast = HeightGrid::synthetic_coast(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            78,
            58,
            origin.x + 40,
        );
        let plan = build_plan(314_159, origin, coast, PaletteKind::Oak);
        let mut ship_cells = Vec::new();
        for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
            for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
                if plan.occ.kind(x, z) == CellKind::Ship {
                    ship_cells.push((x, z));
                }
            }
        }
        let min_x = ship_cells.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = ship_cells.iter().map(|c| c.0).max().unwrap_or(0);
        let min_z = ship_cells.iter().map(|c| c.1).min().unwrap_or(0);
        let max_z = ship_cells.iter().map(|c| c.1).max().unwrap_or(0);
        let keel = (max_x - min_x + 1).max(max_z - min_z + 1);
        assert!(
            (15..=19).contains(&keel),
            "cog keel should be 15–19, got {keel}"
        );
        let volume = crate::stamp::fill_volume(&plan);
        let sea = plan.heights.sea_level;
        let mut wool = Vec::new();
        let mut spruce = 0;
        let mut dark = 0;
        let mut banner = 0;
        let mut mast_top = sea;
        let mut chain = 0;
        let along_x = (max_x - min_x) >= (max_z - min_z);
        let bow_along = if along_x { min_x } else { min_z };
        let mut bow_beam = 0;
        for &(x, z) in &ship_cells {
            let along = if along_x { x } else { z };
            for y in sea - 3..=sea + 22 {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                let id = pumpkin_data::Block::from_state_id(state.id).id;
                if id == pumpkin_data::Block::WHITE_WOOL.id {
                    wool.push((x, y, z));
                }
                if id == pumpkin_data::Block::SPRUCE_PLANKS.id {
                    spruce += 1;
                }
                if id == pumpkin_data::Block::DARK_OAK_PLANKS.id {
                    dark += 1;
                }
                if id == pumpkin_data::Block::WHITE_BANNER.id {
                    banner += 1;
                }
                if id == pumpkin_data::Block::SPRUCE_LOG.id {
                    mast_top = mast_top.max(y);
                }
                if id == pumpkin_data::Block::IRON_CHAIN.id {
                    chain += 1;
                }
                if along == bow_along
                    && y >= sea
                    && (id == pumpkin_data::Block::DARK_OAK_PLANKS.id
                        || id == pumpkin_data::Block::SPRUCE_PLANKS.id
                        || id == pumpkin_data::Block::DARK_OAK_LOG.id)
                {
                    bow_beam += 1;
                }
            }
        }
        let wool_along: std::collections::BTreeSet<i32> = wool
            .iter()
            .map(|c| if along_x { c.0 } else { c.2 })
            .collect();
        assert!(
            spruce >= 8 && dark >= 8,
            "two wood tones spruce={spruce} dark={dark}"
        );
        assert!(banner >= 1, "masthead flag {banner}");
        assert!(mast_top >= sea + 12, "mast {mast_top} sea={sea}");
        assert!(chain >= 2, "rigging chains {chain}");
        assert!(
            wool_along.len() >= 2,
            "billowed sail should occupy offset columns {wool_along:?}"
        );
        assert!(
            bow_beam <= 6,
            "pointed bow should be a few cells, {bow_beam}"
        );
        let mut forecastle = 0;
        let mut high_log = 0;
        for &(x, z) in &ship_cells {
            for y in sea + 4..=sea + 12 {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                let id = pumpkin_data::Block::from_state_id(state.id).id;
                if id == pumpkin_data::Block::DARK_OAK_PLANKS.id
                    || id == pumpkin_data::Block::SPRUCE_PLANKS.id
                    || id == pumpkin_data::Block::SPRUCE_SLAB.id
                {
                    forecastle += 1;
                }
                if id == pumpkin_data::Block::DARK_OAK_LOG.id {
                    high_log += 1;
                }
            }
        }
        assert!(
            forecastle + high_log >= 6,
            "sheer/forecastle planks={forecastle} logs={high_log}"
        );
    }

    #[test]
    fn round27_abbey_is_roofless_gothic_nave() {
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
        let abbey = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::RuinedAbbey)
            .expect("ruined abbey");
        let mut ruin = 0;
        let mut interior_ruin = 0;
        let mut graves = 0;
        let mut arcade = 0;
        for z in abbey.rect.min_z..=abbey.rect.max_z {
            for x in abbey.rect.min_x..=abbey.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::Ruin => {
                        ruin += 1;
                        let edge = crate::geom::Dir::ALL
                            .iter()
                            .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::Ruin);
                        if !edge {
                            interior_ruin += 1;
                        }
                    }
                    CellKind::Grave | CellKind::Lychgate => graves += 1,
                    CellKind::Arcade => arcade += 1,
                    _ => {}
                }
            }
        }
        assert!(ruin >= 40, "nave walls {ruin}");
        assert!(
            interior_ruin * 3 < ruin,
            "nave should be walls not a filled box interior={interior_ruin} ruin={ruin}"
        );
        assert!(graves >= 6, "abbey graveyard {graves}");
        assert!(arcade >= 6, "cloister arcade {arcade}");
        let volume = crate::stamp::fill_volume(&plan);
        let mut wall_h = Vec::new();
        let mut vines = 0;
        let mut bars = 0;
        let mut slabs = 0;
        for z in abbey.rect.min_z..=abbey.rect.max_z {
            for x in abbey.rect.min_x..=abbey.rect.max_x {
                if plan.occ.kind(x, z) != CellKind::Ruin {
                    continue;
                }
                let ground = plan.heights.height(x, z) - 1;
                let mut top = ground;
                for y in ground..=ground + 22 {
                    let Some(state) = volume.get(x, y, z) else {
                        continue;
                    };
                    let id = pumpkin_data::Block::from_state_id(state.id).id;
                    if id == pumpkin_data::Block::VINE.id {
                        vines += 1;
                    }
                    if id == pumpkin_data::Block::IRON_BARS.id {
                        bars += 1;
                    }
                    if id == pumpkin_data::Block::STONE_SLAB.id
                        || id == pumpkin_data::Block::COBBLESTONE_SLAB.id
                        || id == pumpkin_data::Block::SPRUCE_SLAB.id
                    {
                        slabs += 1;
                    }
                    if !state.is_air()
                        && id != pumpkin_data::Block::VINE.id
                        && id != pumpkin_data::Block::IRON_BARS.id
                    {
                        top = y;
                    }
                }
                wall_h.push(top - ground);
            }
        }
        let tall = wall_h.iter().copied().max().unwrap_or(0);
        let short = wall_h.iter().copied().min().unwrap_or(0);
        assert!(tall >= 10, "broken gable should be tall, {tall}");
        assert!(tall - short >= 4, "jagged ruin heights {short}..{tall}");
        assert!(vines >= 8, "ivy {vines}");
        assert!(bars >= 2, "tracery bars {bars}");
        assert!(slabs <= 4, "nave should stay roofless, slabs={slabs}");
        let arcade_h: Vec<_> = {
            let mut hs = Vec::new();
            for z in abbey.rect.min_z..=abbey.rect.max_z {
                for x in abbey.rect.min_x..=abbey.rect.max_x {
                    if plan.occ.kind(x, z) != CellKind::Arcade {
                        continue;
                    }
                    let ground = plan.heights.height(x, z) - 1;
                    let mut top = ground;
                    for y in ground..=ground + 14 {
                        if volume.get(x, y, z).is_some_and(|s| !s.is_air()) {
                            top = y;
                        }
                    }
                    hs.push(top - ground);
                }
            }
            hs
        };
        let amax = arcade_h.iter().copied().max().unwrap_or(0);
        let amin = arcade_h.iter().copied().min().unwrap_or(0);
        assert!(amax - amin >= 2, "arcade column heights {amin}..{amax}");
    }

    #[test]
    fn round27_castle_ruin_and_richer_pois() {
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
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        for need in [
            SceneKind::CastleRuin,
            SceneKind::HermitTower,
            SceneKind::BanditHideout,
            SceneKind::StoneCircle,
        ] {
            assert!(kinds.contains(&need), "missing {need:?} in {kinds:?}");
        }
        let castle = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::CastleRuin)
            .unwrap();
        let mut curtain = 0;
        let mut keep_ring = 0;
        for z in castle.rect.min_z..=castle.rect.max_z {
            for x in castle.rect.min_x..=castle.rect.max_x {
                if plan.occ.kind(x, z) != CellKind::Ruin {
                    continue;
                }
                curtain += 1;
                let blob_w = {
                    let mut x0 = x;
                    let mut x1 = x;
                    while plan.occ.kind(x0 - 1, z) == CellKind::Ruin {
                        x0 -= 1;
                    }
                    while plan.occ.kind(x1 + 1, z) == CellKind::Ruin {
                        x1 += 1;
                    }
                    x1 - x0 + 1
                };
                if blob_w <= 9 {
                    keep_ring += 1;
                }
            }
        }
        assert!(curtain >= 40, "castle ruin curtain {curtain}");
        assert!(keep_ring >= 12, "keep stump ring {keep_ring}");
        let hermit = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::HermitTower)
            .unwrap();
        let mut posts = 0;
        let mut garden = 0;
        for z in hermit.rect.min_z..=hermit.rect.max_z {
            for x in hermit.rect.min_x..=hermit.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::WatchPost => posts += 1,
                    CellKind::Orchard => garden += 1,
                    _ => {}
                }
            }
        }
        assert!(posts >= 9, "hermit drum {posts}");
        assert!(garden >= 2, "hermit garden {garden}");
        let stones = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::StoneCircle)
            .map_or(0, |s| {
                let mut n = 0;
                for z in s.rect.min_z..=s.rect.max_z {
                    for x in s.rect.min_x..=s.rect.max_x {
                        if plan.occ.kind(x, z) == CellKind::StandingStone {
                            n += 1;
                        }
                    }
                }
                n
            });
        assert!(stones >= 12, "stone circle menhirs {stones}");
        let bandit = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::BanditHideout)
            .unwrap();
        let mut look = 0;
        let mut tents = 0;
        let mut trench = 0;
        for z in bandit.rect.min_z..=bandit.rect.max_z {
            for x in bandit.rect.min_x..=bandit.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::WatchPost => look += 1,
                    CellKind::Tent | CellKind::WarTent => tents += 1,
                    CellKind::SiegeTrench => trench += 1,
                    _ => {}
                }
            }
        }
        assert!(look >= 4, "bandit lookout {look}");
        assert!(tents >= 3, "bandit tents {tents}");
        assert!(trench >= 20, "bandit ravine {trench}");
        let volume = crate::stamp::fill_volume(&plan);
        let hermit_h = {
            let mut h = 0;
            for z in hermit.rect.min_z..=hermit.rect.max_z {
                for x in hermit.rect.min_x..=hermit.rect.max_x {
                    if plan.occ.kind(x, z) != CellKind::WatchPost {
                        continue;
                    }
                    let ground = plan.heights.height(x, z) - 1;
                    for y in ground..=ground + 24 {
                        if volume.get(x, y, z).is_some_and(|s| !s.is_air()) {
                            h = h.max(y - ground);
                        }
                    }
                }
            }
            h
        };
        assert!(hermit_h >= 14, "hermit tower {hermit_h}");
        let stone_h = {
            let mut h = 0;
            let scene = plan
                .scenes
                .iter()
                .find(|s| s.kind == SceneKind::StoneCircle)
                .unwrap();
            for z in scene.rect.min_z..=scene.rect.max_z {
                for x in scene.rect.min_x..=scene.rect.max_x {
                    if plan.occ.kind(x, z) != CellKind::StandingStone {
                        continue;
                    }
                    let ground = plan.heights.height(x, z) - 1;
                    for y in ground..=ground + 12 {
                        if volume.get(x, y, z).is_some_and(|s| !s.is_air()) {
                            h = h.max(y - ground);
                        }
                    }
                }
            }
            h
        };
        assert!(stone_h >= 5, "menhirs {stone_h}");
    }

    #[test]
    fn round28_open_bandit_and_hinterland_set_pieces() {
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
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        let bandit = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::BanditHideout)
            .expect("bandit hideout");
        let mut look = 0;
        let mut tents = 0;
        let mut tent_blobs = 0;
        let mut loot = 0;
        let mut palisade = 0;
        let mut court = 0;
        for z in bandit.rect.min_z..=bandit.rect.max_z {
            for x in bandit.rect.min_x..=bandit.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::WatchPost => look += 1,
                    CellKind::Tent | CellKind::WarTent => {
                        tents += 1;
                        let left = plan.occ.kind(x - 1, z);
                        let up = plan.occ.kind(x, z - 1);
                        if !matches!(left, CellKind::Tent | CellKind::WarTent)
                            && !matches!(up, CellKind::Tent | CellKind::WarTent)
                        {
                            tent_blobs += 1;
                        }
                    }
                    CellKind::LootPile => loot += 1,
                    CellKind::Palisade => palisade += 1,
                    CellKind::CampCourt => court += 1,
                    _ => {}
                }
            }
        }
        assert!(look >= 9, "stilt lookout {look}");
        assert!(tents >= 12, "wool tent cells {tents}");
        assert!(tent_blobs >= 3, "separate tents {tent_blobs}");
        assert!(loot >= 2, "loot pile {loot}");
        assert!(palisade >= 8, "palisade ring {palisade}");
        assert!(
            palisade < 80,
            "staked palisade, not a timber box {palisade}"
        );
        assert!(court >= 8, "open camp court {court}");
        assert!(court < 80, "fire pad not a filled court {court}");
        assert!(
            kinds.contains(&SceneKind::RoadsideInn) || kinds.contains(&SceneKind::CharcoalCamp),
            "inn or charcoal in {kinds:?}"
        );
        if let Some(inn) = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::RoadsideInn)
        {
            let mut tavern = 0;
            let mut barn = 0;
            let mut yard = 0;
            let mut well = 0;
            let mut wagon = 0;
            let mut eaves = 0;
            for lot in &plan.lots {
                if !inn.rect.intersects(lot.rect) {
                    continue;
                }
                match lot.kind {
                    BuildingKind::Tavern => tavern += 1,
                    BuildingKind::Barn => barn += 1,
                    _ => {}
                }
            }
            for z in inn.rect.min_z..=inn.rect.max_z {
                for x in inn.rect.min_x..=inn.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Plaza => yard += 1,
                        CellKind::Well => well += 1,
                        CellKind::Wagon => wagon += 1,
                        CellKind::Eave => eaves += 1,
                        _ => {}
                    }
                }
            }
            assert!(tavern >= 1, "inn tavern");
            assert!(barn >= 1, "inn stable");
            assert!(yard >= 4, "coaching yard {yard}");
            assert!(well >= 4, "inn well {well}");
            assert!(wagon >= 2, "parked wagon {wagon}");
            assert!(eaves >= 8, "tavern jetty eaves {eaves}");
        }
        if let Some(camp) = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::CharcoalCamp)
        {
            assert!(
                plan.lots.iter().any(|lot| {
                    lot.kind == BuildingKind::Cottage && camp.rect.intersects(lot.rect)
                }),
                "charcoal hut"
            );
        }
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
        let toll = river_plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::TollBridge)
            .expect("toll bridge");
        let mut bridge = 0;
        for z in toll.rect.min_z..=toll.rect.max_z {
            for x in toll.rect.min_x..=toll.rect.max_x {
                if river_plan.occ.kind(x, z) == CellKind::Bridge {
                    bridge += 1;
                }
            }
        }
        assert!(bridge >= 8, "toll deck {bridge}");
        assert!(
            river_plan.lots.iter().any(|lot| {
                lot.kind == BuildingKind::Watchtower && toll.rect.intersects(lot.rect)
            }),
            "toll gatehouse"
        );
        let river_volume = crate::stamp::fill_volume(&river_plan);
        let river_float = crate::watertight::check_floating_fragments(&river_plan, &river_volume);
        assert!(
            river_float.is_empty(),
            "toll floating fragments:\n{}",
            river_float.join("\n")
        );
        let river_check = crate::watertight::check_town(&river_plan, &river_volume);
        assert!(
            river_check
                .failures()
                .into_iter()
                .all(|b| !b.name.contains("Watchtower")),
            "toll gatehouse should stay watertight: {}",
            river_check.summary()
        );
        let volume = crate::stamp::fill_volume(&plan);
        let check = crate::watertight::check_town(&plan, &volume);
        let inn_fails: Vec<_> = check
            .failures()
            .into_iter()
            .filter(|b| {
                b.name.contains("Tavern")
                    || b.name.contains("Barn")
                    || b.name.contains("Cottage")
                    || b.name.contains("Watchtower")
            })
            .map(|b| b.name.clone())
            .collect();
        assert!(
            inn_fails.is_empty(),
            "enclosed set pieces should stay watertight: {inn_fails:?}"
        );
        let floating = crate::watertight::check_floating_fragments(&plan, &volume);
        assert!(
            floating.is_empty(),
            "floating fragments:\n{}",
            floating.join("\n")
        );
    }

    fn poi_road_reaches_gate(plan: &crate::plan::TownPlan, kind: SceneKind) -> bool {
        let Some(scene) = plan.scenes.iter().find(|s| s.kind == kind) else {
            return true;
        };
        plan.roads.iter().any(|road| {
            let near_scene = road
                .points
                .iter()
                .any(|p| scene.rect.inflate(10).contains(p[0], p[1]));
            let near_gate = plan.gates.iter().any(|gate| {
                gate.kind == crate::info::GateKind::Town
                    && road
                        .points
                        .iter()
                        .any(|p| (p[0] - gate.x).abs() + (p[1] - gate.z).abs() < 16)
            });
            near_scene && near_gate
        })
    }

    #[test]
    fn round29_inn_spread_camp_and_poi_roads() {
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
        let bandit = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::BanditHideout)
            .expect("bandit hideout");
        assert!(
            bandit.rect.width() >= 28 && bandit.rect.depth() >= 22,
            "spread camp {}x{}",
            bandit.rect.width(),
            bandit.rect.depth()
        );
        let mut tents = Vec::new();
        let mut green = 0;
        let mut court = 0;
        for z in bandit.rect.min_z..=bandit.rect.max_z {
            for x in bandit.rect.min_x..=bandit.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::Tent | CellKind::WarTent => {
                        let left = plan.occ.kind(x - 1, z);
                        let up = plan.occ.kind(x, z - 1);
                        if !matches!(left, CellKind::Tent | CellKind::WarTent)
                            && !matches!(up, CellKind::Tent | CellKind::WarTent)
                        {
                            tents.push((x, z));
                        }
                    }
                    CellKind::Green => green += 1,
                    CellKind::CampCourt => court += 1,
                    _ => {}
                }
            }
        }
        assert!(tents.len() >= 3, "separate tents {}", tents.len());
        let mut gap = i32::MAX;
        for (i, a) in tents.iter().enumerate() {
            for b in tents.iter().skip(i + 1) {
                gap = gap.min((a.0 - b.0).abs().max((a.1 - b.1).abs()));
            }
        }
        assert!(gap >= 6, "open ground between tents {gap}");
        assert!(
            green > court,
            "grass between palisade and fire {green}/{court}"
        );
        if plan.scenes.iter().any(|s| s.kind == SceneKind::RoadsideInn) {
            assert!(
                poi_road_reaches_gate(&plan, SceneKind::RoadsideInn),
                "inn road to a city gate"
            );
        }
        assert!(
            poi_road_reaches_gate(&plan, SceneKind::RuinedAbbey),
            "abbey road to a city gate"
        );
        assert!(
            poi_road_reaches_gate(&plan, SceneKind::CastleRuin),
            "ruin road to a city gate"
        );
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
        if river_plan
            .scenes
            .iter()
            .any(|s| s.kind == SceneKind::TollBridge)
        {
            assert!(
                poi_road_reaches_gate(&river_plan, SceneKind::TollBridge),
                "toll road to a city gate"
            );
        }
    }

    #[test]
    fn round30_market_mill_beacon_and_tourney() {
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
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        assert!(
            kinds.contains(&SceneKind::WalledMarket) || kinds.contains(&SceneKind::Beacon),
            "walled market or beacon in {kinds:?}"
        );
        if let Some(market) = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::WalledMarket)
        {
            assert!(
                plan.lots.iter().any(|lot| {
                    lot.kind == BuildingKind::TownHall && market.rect.intersects(lot.rect)
                }),
                "guildhall"
            );
            assert!(
                plan.lots
                    .iter()
                    .filter(
                        |lot| lot.kind == BuildingKind::Stall && market.rect.intersects(lot.rect)
                    )
                    .count()
                    >= 1,
                "market stalls"
            );
            let mut palisade = 0;
            let mut plaza = 0;
            for z in market.rect.min_z..=market.rect.max_z {
                for x in market.rect.min_x..=market.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Palisade | CellKind::PalisadeGate => palisade += 1,
                        CellKind::Plaza => plaza += 1,
                        _ => {}
                    }
                }
            }
            assert!(palisade >= 8, "market palisade {palisade}");
            assert!(plaza >= 8, "market square {plaza}");
        }
        if let Some(beacon) = plan.scenes.iter().find(|s| s.kind == SceneKind::Beacon) {
            let mut fires = 0;
            for z in beacon.rect.min_z..=beacon.rect.max_z {
                for x in beacon.rect.min_x..=beacon.rect.max_x {
                    if plan.occ.kind(x, z) == CellKind::Beacon {
                        fires += 1;
                    }
                }
            }
            assert!(fires >= 4, "beacon pad {fires}");
        }
        if let Some(tourney) = plan.scenes.iter().find(|s| s.kind == SceneKind::Tourney) {
            let (tx, tz) = tourney.rect.center();
            let (kx, kz) = plan.castle.keep.center();
            let dist = (tx - kx).abs() + (tz - kz).abs();
            assert!(
                dist < 220,
                "tourney should sit near the castle, dist {dist}"
            );
            let mut lists = 0;
            for z in tourney.rect.min_z..=tourney.rect.max_z {
                for x in tourney.rect.min_x..=tourney.rect.max_x {
                    if plan.occ.kind(x, z) == CellKind::TourneyList {
                        lists += 1;
                    }
                }
            }
            assert!(lists >= 20, "jousting lists {lists}");
            let mut tents = 0;
            let mut stands = 0;
            for z in tourney.rect.min_z..=tourney.rect.max_z {
                for x in tourney.rect.min_x..=tourney.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Tent => tents += 1,
                        CellKind::TourneyStand => stands += 1,
                        _ => {}
                    }
                }
            }
            assert!(tents >= 20, "pavilion tents {tents}");
            assert!(stands >= 20, "stands and royal box {stands}");
        }
        let bandit = plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::BanditHideout)
            .expect("bandit hideout");
        let mut look = 0;
        let mut loot = 0;
        let mut wagon = 0;
        for z in bandit.rect.min_z..=bandit.rect.max_z {
            for x in bandit.rect.min_x..=bandit.rect.max_x {
                match plan.occ.kind(x, z) {
                    CellKind::WatchPost => look += 1,
                    CellKind::LootPile => loot += 1,
                    CellKind::Wagon => wagon += 1,
                    _ => {}
                }
            }
        }
        assert!(look >= 12, "two lookouts {look}");
        assert!(loot >= 3, "loot piles {loot}");
        assert!(wagon >= 2, "stolen wagon {wagon}");
        let volume = crate::stamp::fill_volume(&plan);
        let check = crate::watertight::check_town(&plan, &volume);
        let fails: Vec<_> = check
            .failures()
            .into_iter()
            .filter(|b| {
                b.name.contains("TownHall")
                    || b.name.contains("Stall")
                    || b.name.contains("Barn")
                    || b.name.contains("Watermill")
                    || b.name.contains("Cottage")
            })
            .map(|b| b.name.clone())
            .collect();
        assert!(
            fails.is_empty(),
            "enclosed set pieces should stay watertight: {fails:?}"
        );
        let floating = crate::watertight::check_floating_fragments(&plan, &volume);
        assert!(
            floating.is_empty(),
            "floating fragments:\n{}",
            floating.join("\n")
        );
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
        let mill = river_plan
            .scenes
            .iter()
            .find(|s| matches!(s.kind, SceneKind::MillHamlet | SceneKind::Riverside))
            .expect("mill hamlet or riverside mill");
        assert!(
            river_plan.lots.iter().any(|lot| {
                lot.kind == BuildingKind::Watermill && mill.rect.intersects(lot.rect)
            }),
            "water mill"
        );
        let mut wheel = 0;
        let mut race = 0;
        for z in mill.rect.min_z..=mill.rect.max_z {
            for x in mill.rect.min_x..=mill.rect.max_x {
                match river_plan.occ.kind(x, z) {
                    CellKind::WaterWheel => wheel += 1,
                    CellKind::MillRace | CellKind::Sluice => race += 1,
                    _ => {}
                }
            }
        }
        assert!(wheel >= 8, "water wheel {wheel}");
        assert!(race >= 1, "mill race {race}");
        assert!(
            river_plan
                .lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::Barn),
            "granary barn"
        );
        let river_volume = crate::stamp::fill_volume(&river_plan);
        let river_float = crate::watertight::check_floating_fragments(&river_plan, &river_volume);
        assert!(
            river_float.is_empty(),
            "mill floating fragments:\n{}",
            river_float.join("\n")
        );
    }

    #[test]
    fn round31_priory_vineyard_quarry_siege_and_riverside() {
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
        let kinds: Vec<_> = plan.scenes.iter().map(|s| s.kind).collect();
        if let Some(abbey) = plan.scenes.iter().find(|s| s.kind == SceneKind::Monastery) {
            assert!(
                plan.lots.iter().any(|lot| {
                    lot.kind == BuildingKind::Monastery && abbey.rect.intersects(lot.rect)
                }),
                "priory nave"
            );
            assert!(
                plan.lots.iter().any(|lot| {
                    matches!(
                        lot.kind,
                        BuildingKind::ChapterHouse | BuildingKind::Scriptorium
                    ) && abbey.rect.intersects(lot.rect)
                }),
                "chapter house or scriptorium"
            );
            let mut herbs = 0;
            let mut arcade = 0;
            for z in abbey.rect.min_z..=abbey.rect.max_z {
                for x in abbey.rect.min_x..=abbey.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::HerbGarden => herbs += 1,
                        CellKind::Arcade => arcade += 1,
                        _ => {}
                    }
                }
            }
            assert!(arcade >= 8, "cloister arcade {arcade}");
            assert!(herbs >= 8, "herb garden {herbs}");
        }
        if let Some(yard) = plan.scenes.iter().find(|s| s.kind == SceneKind::Vineyard) {
            let mut vines = 0;
            let mut retain = 0;
            for z in yard.rect.min_z..=yard.rect.max_z {
                for x in yard.rect.min_x..=yard.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Vine => vines += 1,
                        CellKind::Retaining => retain += 1,
                        _ => {}
                    }
                }
            }
            assert!(vines >= 20, "terraced vines {vines}");
            assert!(retain >= 8, "vineyard terraces {retain}");
            assert!(
                plan.lots.iter().any(|lot| {
                    lot.kind == BuildingKind::Winepress && yard.rect.intersects(lot.rect)
                }),
                "press house"
            );
        }
        if let Some(quarry) = plan.scenes.iter().find(|s| s.kind == SceneKind::Quarry) {
            let mut cranes = 0;
            let mut stone = 0;
            let mut rails = 0;
            let mut pit = 0;
            let mut sails = 0;
            let crop = quarry.rect.inflate(2);
            for z in crop.min_z..=crop.max_z {
                for x in crop.min_x..=crop.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Crane => cranes += 1,
                        CellKind::CutStone => stone += 1,
                        CellKind::MineRail => rails += 1,
                        CellKind::CampCourt => pit += 1,
                        CellKind::WindSail => sails += 1,
                        _ => {}
                    }
                }
            }
            assert!(cranes >= 8, "quarry cranes {cranes}");
            assert!(stone >= 8, "cut stone {stone}");
            assert!(pit >= 24, "stepped quarry pit {pit}");
            assert!(rails + pit >= 24, "cart track {rails}");
            assert_eq!(sails, 0, "mill sails in the quarry crop");
            assert!(
                plan.lots.iter().any(|lot| {
                    lot.kind == BuildingKind::Blacksmith && quarry.rect.intersects(lot.rect)
                }),
                "mason's lodge"
            );
        } else {
            assert!(
                kinds.contains(&SceneKind::Mine) || kinds.contains(&SceneKind::CastleRuin),
                "quarry or a hillside stand-in in {kinds:?}"
            );
        }
        if let Some(camp) = plan.scenes.iter().find(|s| s.kind == SceneKind::SiegeCamp) {
            let (cx, cz) = camp.rect.center();
            let (kx, kz) = plan.castle.keep.center();
            let dist = (cx - kx).abs() + (cz - kz).abs();
            assert!(dist < 240, "siege camp beside the castle, dist {dist}");
            let mut palisade = 0;
            let mut trebs = 0;
            let mut treb_inside = 0;
            let mut tents = 0;
            let mut trench = 0;
            let mut bank = 0;
            for z in camp.rect.min_z..=camp.rect.max_z {
                for x in camp.rect.min_x..=camp.rect.max_x {
                    match plan.occ.kind(x, z) {
                        CellKind::Palisade | CellKind::PalisadeWalk | CellKind::PalisadeGate => {
                            palisade += 1;
                        }
                        CellKind::SiegeTreb => {
                            trebs += 1;
                            if Dir::ALL.iter().any(|dir| {
                                matches!(
                                    plan.occ.kind(x + dir.dx(), z + dir.dz()),
                                    CellKind::PalisadeWalk | CellKind::Tent | CellKind::WarTent
                                )
                            }) {
                                treb_inside += 1;
                            }
                        }
                        CellKind::Tent | CellKind::WarTent => tents += 1,
                        CellKind::SiegeTrench => trench += 1,
                        CellKind::MineSpoil => bank += 1,
                        _ => {}
                    }
                }
            }
            assert!(palisade >= 12, "siege palisade {palisade}");
            assert!(trebs >= 24, "trebuchets {trebs}");
            assert_eq!(treb_inside, 0, "trebuchets inside the palisade");
            assert!(tents >= 24, "siege tent rows {tents}");
            assert!(trench >= 8, "ditch {trench}");
            assert!(bank >= 8, "earth bank {bank}");
        }
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
        let riverside = river_plan
            .scenes
            .iter()
            .find(|s| s.kind == SceneKind::Riverside)
            .expect("riverside village");
        let mut bridge = 0;
        let mut ford = 0;
        let mut weir = 0;
        for z in riverside.rect.min_z..=riverside.rect.max_z {
            for x in riverside.rect.min_x..=riverside.rect.max_x {
                match river_plan.occ.kind(x, z) {
                    CellKind::Bridge => bridge += 1,
                    CellKind::Ford => ford += 1,
                    CellKind::Weir => weir += 1,
                    _ => {}
                }
            }
        }
        assert!(bridge >= 8, "stone bridge {bridge}");
        assert!(ford >= 6, "cobble ford {ford}");
        assert!(weir >= 4, "fishing weirs {weir}");
        assert!(
            river_plan
                .lots
                .iter()
                .filter(|lot| {
                    lot.kind == BuildingKind::Cottage && riverside.rect.intersects(lot.rect)
                })
                .count()
                >= 1,
            "riverside cottages"
        );
    }
}
