//! Per-column stamping. Each chunk writes only its own columns.

use std::collections::BTreeMap;

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::BlockStateId;
use pumpkin_data::biome::Biome;
use pumpkin_data::block_properties::WheatLikeProperties;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::generation::hooks::ChunkDecorationContext;

use crate::buildings::{BuildingKind, Lot};
use crate::castle::{RoofKind, TowerShape, TowerSpec};
use crate::cells::CellKind;
use crate::geom::{Dir, Rect, isqrt};
use crate::interior;
use crate::palette::{Palette, PaletteKind, stair_state};
use crate::plan::TownPlan;

/// Distinctive well-floor block used by tests and `/locatetown` checks.
pub const MARKER_FLOOR: &Block = &Block::MOSSY_COBBLESTONE;

const MAX_FOUNDATION: i32 = 8;
const MAX_CUT: i32 = 1;

/// Destination for town blocks. Chunk decoration and the offline renderer both
/// implement this so a column is produced the same way either path.
pub trait BlockSink {
    /// Writes `state` at world `(x, y, z)`.
    fn put(&mut self, x: i32, y: i32, z: i32, state: &BlockState);
    /// Lowest writable Y.
    fn floor_min(&self) -> i32;
}

impl BlockSink for ChunkDecorationContext<'_> {
    fn put(&mut self, x: i32, y: i32, z: i32, state: &BlockState) {
        self.set_block_state(x, y, z, state);
    }

    fn floor_min(&self) -> i32 {
        self.min_y()
    }
}

/// Writes town blocks for columns owned by `ctx`'s center chunk.
pub fn stamp_plan_in_chunk(ctx: &mut ChunkDecorationContext<'_>, plan: &TownPlan) {
    let cx = ctx.chunk_x();
    let cz = ctx.chunk_z();
    let min_x = cx * 16;
    let min_z = cz * 16;
    for z in min_z..min_z + 16 {
        for x in min_x..min_x + 16 {
            if !plan.bounds.contains(x, z) && !plan.border_roads.contains_key(&(x, z)) {
                if plan.strips_trees(x, z) {
                    let surface = ctx.surface_height_exclusive(x, z);
                    strip_feature_vegetation(ctx, plan, x, z, surface);
                }
                continue;
            }
            let surface = ctx.surface_height_exclusive(x, z);
            let biome = ctx.get_biome(x, surface, z);
            stamp_column(ctx, plan, x, z, surface, Some(biome));
            strip_feature_vegetation(ctx, plan, x, z, surface);
        }
    }
}

/// Features can hang leaves into a column after the surface snapshot. Air out
/// leftover vanilla trees on tree-strip columns that are not planned timber.
fn strip_feature_vegetation(
    ctx: &mut ChunkDecorationContext<'_>,
    plan: &TownPlan,
    x: i32,
    z: i32,
    surface: i32,
) {
    let kind = column_kind(plan, x, z);
    let wet_use = matches!(
        kind,
        CellKind::Harbor
            | CellKind::Dock
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Breakwater
            | CellKind::Lighthouse
            | CellKind::Bridge
            | CellKind::MillRace
            | CellKind::Sluice
            | CellKind::WaterWheel
            | CellKind::Shrine
            | CellKind::FieldWall
            | CellKind::Ford
            | CellKind::Weir
    );
    let flowing = !plan.heights.is_dry(x, z);
    if !plan.strips_trees(x, z) && !wet_use && !flowing {
        return;
    }
    let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
    // Fancy oak / 2×2 spruce hang side logs well above ground on neighbour
    // columns, where `surface` is dirt not canopy. Match TREE_STRIP height.
    let top = surface.max(planned) + 32;
    if plan.strips_trees(x, z) && keeps_planned_timber(kind) {
        let ruin_yard = plan.scene_at(x, z).is_some_and(|scene| {
            matches!(
                scene.kind,
                crate::scenes::SceneKind::CastleRuin | crate::scenes::SceneKind::RuinedAbbey
            )
        });
        let open_camp = plan
            .scene_at(x, z)
            .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::BanditHideout);
        if !matches!(kind, CellKind::Green | CellKind::Orchard | CellKind::Ruin) {
            return;
        }
        // Garden / orchard / palm timber is at most 6. Ruin shells are stone;
        // leftover Features oaks on the broken curtain still need airing.
        // Bandit open grass must stay grass, not a Features thicket.
        let from = if kind == CellKind::Ruin || ruin_yard || open_camp {
            planned + 1
        } else {
            planned + 7
        };
        let air = Block::AIR.default_state;
        for y in from..=top {
            let id = Block::from_state_id(ctx.get_block_state(x, y, z)).id;
            if is_vanilla_tree_part(id) {
                ctx.set_block_state(x, y, z, air);
            }
        }
        return;
    }
    let air = Block::AIR.default_state;
    for y in planned.saturating_sub(4)..=top {
        let id = Block::from_state_id(ctx.get_block_state(x, y, z)).id;
        if (wet_use || flowing) && !plan.strips_trees(x, z) {
            if id == Block::LILY_PAD.id {
                ctx.set_block_state(x, y, z, air);
            }
            continue;
        }
        if is_vanilla_tree_part(id) {
            ctx.set_block_state(x, y, z, air);
        }
    }
}

pub(crate) fn keeps_planned_timber(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Building
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Crane
            | CellKind::Dock
            | CellKind::MineHead
            | CellKind::MineAdit
            | CellKind::MineRail
            | CellKind::Palisade
            | CellKind::PalisadeWalk
            | CellKind::PalisadeGate
            | CellKind::WindSail
            | CellKind::WatchPost
            | CellKind::Orchard
            | CellKind::Grave
            | CellKind::Lychgate
            | CellKind::Arcade
            | CellKind::Hedge
            | CellKind::Tent
            | CellKind::WarTent
            | CellKind::SiegeTreb
            | CellKind::SiegeRam
            | CellKind::SiegeLadder
            | CellKind::Burnt
            | CellKind::Eave
            | CellKind::WaterWheel
            | CellKind::Sluice
            | CellKind::Vine
            | CellKind::Shrine
            | CellKind::FieldWall
            | CellKind::Green
            | CellKind::Gallows
            | CellKind::StandingStone
            | CellKind::LootPile
            | CellKind::Wagon
            | CellKind::Beacon
            | CellKind::Weir
            | CellKind::HerbGarden
            | CellKind::Maypole
            | CellKind::MarketCross
            | CellKind::Pillory
            | CellKind::Ropewalk
            | CellKind::Slipway
            | CellKind::Sawpit
    )
}

pub(crate) fn is_vanilla_tree_part(id: pumpkin_data::BlockId) -> bool {
    id == Block::OAK_LEAVES.id
        || id == Block::SPRUCE_LEAVES.id
        || id == Block::BIRCH_LEAVES.id
        || id == Block::JUNGLE_LEAVES.id
        || id == Block::ACACIA_LEAVES.id
        || id == Block::DARK_OAK_LEAVES.id
        || id == Block::CHERRY_LEAVES.id
        || id == Block::MANGROVE_LEAVES.id
        || id == Block::AZALEA_LEAVES.id
        || id == Block::FLOWERING_AZALEA_LEAVES.id
        || id == Block::OAK_LOG.id
        || id == Block::SPRUCE_LOG.id
        || id == Block::BIRCH_LOG.id
        || id == Block::JUNGLE_LOG.id
        || id == Block::ACACIA_LOG.id
        || id == Block::DARK_OAK_LOG.id
        || id == Block::CHERRY_LOG.id
        || id == Block::MANGROVE_LOG.id
        || id == Block::OAK_SAPLING.id
        || id == Block::BIRCH_SAPLING.id
        || id == Block::SPRUCE_SAPLING.id
        || id == Block::PALE_OAK_LEAVES.id
        || id == Block::PALE_OAK_LOG.id
        || id == Block::PALE_OAK_SAPLING.id
        || id == Block::LILY_PAD.id
}

/// Places one planned column into `sink`.
pub fn stamp_column(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    z: i32,
    surface: i32,
    biome: Option<&Biome>,
) {
    let kind = column_kind(plan, x, z);
    let palette = Palette::new(
        if matches!(
            kind,
            CellKind::TownWall | CellKind::TownTower | CellKind::TownGate | CellKind::CityDitch
        ) {
            plan.palette
        } else {
            biome.map_or(plan.palette, PaletteKind::from_biome)
        },
    );
    let mut ground = surface - 1;
    if ground <= sink.floor_min() + 2 {
        return;
    }
    // Features trees make `surface` the canopy. Snap tree-strip columns
    // (scene yards, walled town, market crop, building keepout) back to
    // the planner height so trunks and hanging leaves are cleared.
    // fill_volume has `ground + 1 == planned`; only then skip the
    // tree-height strip, or halls get a cave the watertight flood follows.
    // Desert mesa is not a canopy: carving 16 down leaves floating
    // sand/terracotta on the strip edge, so only air out bushes there.
    if plan.strips_trees(x, z) {
        let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
        let sea = plan.heights.sea_level;
        let top = ground.max(planned);
        let desert = plan.palette == PaletteKind::Sandstone
            || biome.is_some_and(|b| PaletteKind::from_biome(b) == PaletteKind::Sandstone);
        if desert {
            if plan.heights.is_dry(x, z) {
                clear_above(sink, x, ground + 1, ground + 8, z);
            } else {
                clear_above(sink, x, sea.saturating_sub(4), top.max(sea) + 8, z);
                ground = sea.min(ground).max(sink.floor_min() + 3);
            }
        } else if plan.heights.is_dry(x, z) {
            const TREE_STRIP: i32 = 16;
            let tree = ground + 1 != planned;
            let surface = if tree { planned } else { planned.min(ground) };
            let from = if tree {
                planned.min(ground).saturating_sub(TREE_STRIP)
            } else {
                surface
            };
            clear_above(sink, x, from, top + 56, z);
            if tree {
                for y in from..surface {
                    sink.put(x, y, z, Block::DIRT.default_state);
                }
            }
            ground = surface.max(sink.floor_min() + 3);
        } else {
            clear_above(sink, x, sea.saturating_sub(4), top.max(sea) + 24, z);
            ground = sea.min(ground).max(sink.floor_min() + 3);
        }
    }
    if occupy_raises(kind) && ground < plan.heights.sea_level + 1 {
        raise_out_of_water(sink, x, ground, z, plan.heights.sea_level, &palette);
        ground = plan.heights.sea_level + 1;
    }
    let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
    if occupy_raises(kind) && ground > planned + 3 {
        clear_above(sink, x, planned + 1, ground + 8, z);
        ground = planned;
    }
    let planned = planned.max(ground);
    let floor = terrace_floor(ground, planned);
    if occupy_raises(kind) {
        let cap = if is_cathedral_mass(kind) || kind == CellKind::CathedralButtress {
            ground + 56
        } else if kind == CellKind::WindSail {
            ground + 36
        } else if matches!(
            kind,
            CellKind::MineHead
                | CellKind::MineRail
                | CellKind::MineSpoil
                | CellKind::MineAdit
                | CellKind::Ruin
                | CellKind::Burnt
                | CellKind::TourneyList
                | CellKind::TourneyStand
                | CellKind::Quay
                | CellKind::Crane
                | CellKind::Breakwater
                | CellKind::SiegeTrench
                | CellKind::CityDitch
        ) || plan.strips_trees(x, z)
        {
            ground + 40
        } else {
            ground + 28
        };
        clear_above(sink, x, ground + 1, cap, z);
    }
    if matches!(
        kind,
        CellKind::Harbor
            | CellKind::Dock
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Breakwater
            | CellKind::Lighthouse
            | CellKind::Bridge
            | CellKind::MillRace
            | CellKind::Sluice
            | CellKind::WaterWheel
            | CellKind::Ford
            | CellKind::Weir
    ) {
        let planned = plan.heights.height(x, z);
        let sea = plan.heights.sea_level;
        if planned <= sea + 1 && ground > sea + 2 {
            clear_above(sink, x, sea.min(planned), ground + 8, z);
            ground = planned.min(sea).max(sink.floor_min() + 3);
        }
        // Lily pads sit on the water block, including highland rivers.
        let water_y = planned.min(sea);
        clear_above(sink, x, water_y, water_y + 24, z);
    }
    if plan.strips_trees(x, z) {
        let sea = plan.heights.sea_level;
        if !plan.heights.is_dry(x, z)
            || matches!(
                kind,
                CellKind::Harbor
                    | CellKind::Dock
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Breakwater
                    | CellKind::Bridge
                    | CellKind::MillRace
                    | CellKind::Sluice
                    | CellKind::Ford
                    | CellKind::Weir
            )
        {
            clear_above(sink, x, sea.saturating_sub(4), sea.max(ground) + 24, z);
        } else if !occupy_raises(kind) {
            clear_above(sink, x, ground + 1, ground + 40, z);
        }
    }
    // Hall, kitchen, chapel, and curtain towers fill after keep masses and
    // overwrite occupancy. Stamp the keep anyway so the spire's near faces
    // are not replaced by a lower L-roof (harbour great-keep SE V).
    let kind = if plan.castle.keep_mass_at(x, z).is_some()
        && matches!(
            kind,
            CellKind::CastleWall
                | CellKind::CastleInnerWall
                | CellKind::CastleTower
                | CellKind::CastleHall
                | CellKind::CastleChapel
                | CellKind::CastleKitchen
                | CellKind::CastleStables
                | CellKind::CastleBarracks
        ) {
        CellKind::CastleKeep
    } else {
        kind
    };
    match kind {
        CellKind::Empty => {}
        CellKind::Road => place_road(sink, plan, x, ground, z, &palette, None),
        CellKind::BarnYard => place_forge_yard(sink, x, ground, z, &palette),
        CellKind::RoadStairN => {
            place_road(sink, plan, x, ground, z, &palette, Some(Dir::North));
        }
        CellKind::RoadStairS => {
            place_road(sink, plan, x, ground, z, &palette, Some(Dir::South));
        }
        CellKind::RoadStairE => {
            place_road(sink, plan, x, ground, z, &palette, Some(Dir::East));
        }
        CellKind::RoadStairW => {
            place_road(sink, plan, x, ground, z, &palette, Some(Dir::West));
        }
        CellKind::Market => place_market(sink, x, ground, z, &palette),
        CellKind::Plaza => place_plaza(sink, plan, x, ground, z, &palette),
        CellKind::Fountain => place_fountain(sink, plan, x, ground, z, &palette),
        CellKind::Farm => place_farm(sink, plan, x, ground, z, false),
        CellKind::FarmEdge => place_farm(sink, plan, x, ground, z, true),
        CellKind::Orchard => place_orchard(sink, plan, x, ground, z, &palette),
        CellKind::Vine => place_vine(sink, plan, x, ground, z, &palette),
        CellKind::Pasture => place_pasture(sink, plan, x, ground, z, &palette),
        CellKind::Grave => place_grave(sink, x, ground, z, &palette),
        CellKind::Palisade => place_palisade(sink, plan, x, ground, z, &palette),
        CellKind::PalisadeWalk => place_palisade_walk(sink, x, ground, z, &palette),
        CellKind::Tent => place_tent(sink, plan, x, ground, z, &palette),
        CellKind::CampCourt => place_camp_court(sink, plan, x, ground, z, &palette),
        CellKind::Ruin => match plan.scene_at(x, z).map(|s| s.kind) {
            Some(crate::scenes::SceneKind::RuinedAbbey) => {
                place_abbey_ruin(sink, plan, x, ground, z, &palette);
            }
            Some(crate::scenes::SceneKind::CastleRuin) => {
                place_castle_ruin_mass(sink, plan, x, ground, z, &palette);
            }
            _ => place_ruin(sink, plan, x, ground, z, &palette),
        },
        CellKind::WatchPost => match plan.scene_at(x, z).map(|s| s.kind) {
            Some(crate::scenes::SceneKind::HermitTower) => {
                place_hermit_tower(sink, plan, x, floor, ground, z, &palette);
            }
            Some(crate::scenes::SceneKind::BanditHideout) => {
                place_bandit_lookout(sink, plan, x, floor, ground, z, &palette);
            }
            _ if plan.has_scene(x, z, crate::scenes::SceneKind::Manor) => {
                place_motte_bank(sink, plan, x, ground, z, &palette);
            }
            _ => place_watch_post(sink, x, floor, ground, z, &palette),
        },
        CellKind::Well => place_well(sink, plan, x, ground, z, &palette),
        CellKind::TownWall => {
            if in_frontier(plan, x, z) {
                place_scarred_curtain(sink, plan, x, floor, ground, z, &palette);
            } else {
                let height = if plan.walled_city { 10 } else { 6 };
                place_curtain(sink, plan, x, floor, ground, z, &palette, height, false);
            }
        }
        CellKind::TownTower => {
            if in_frontier(plan, x, z) {
                place_collapsed_tower(sink, plan, x, floor, ground, z, &palette);
            } else if in_caravanserai(plan, x, z) {
                place_minaret(sink, plan, x, floor, ground, z, &palette);
            } else if plan.walled_city {
                place_round_city_tower(sink, plan, x, floor, ground, z, &palette);
            } else {
                place_tower(sink, plan, x, floor, ground, z, &palette, 5, 14);
            }
        }
        CellKind::TownGate => {
            let height = if plan.walled_city { 12 } else { 8 };
            place_gate(sink, plan, x, floor, ground, z, &palette, height);
        }
        CellKind::Building => {
            if let Some(lot) = plan.lots.iter().find(|l| l.id == plan.occ.lot(x, z)) {
                place_building(sink, plan, lot, x, ground, z, &palette);
            }
        }
        CellKind::CastleWall => place_curtain(
            sink,
            plan,
            x,
            floor,
            ground,
            z,
            &palette,
            plan.castle.outer_ring.height,
            true,
        ),
        CellKind::CastleInnerWall => place_curtain(
            sink,
            plan,
            x,
            floor,
            ground,
            z,
            &palette,
            plan.castle.inner_ring.height,
            true,
        ),
        CellKind::CastleTower => {
            if let Some(spec) = plan.castle.tower_at(x, z) {
                place_shaped_tower(sink, plan, spec, x, ground, z, &palette);
            } else {
                place_tower(sink, plan, x, floor, ground, z, &palette, 7, 18);
            }
        }
        CellKind::CastleGate => place_gatehouse(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleBarbican => place_barbican(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleKeep => place_keep(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleHall => place_hall(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleChapel => place_chapel(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleStables => {
            place_stables(
                sink,
                plan.castle.stables,
                x,
                ridge_floor(plan, plan.castle.stables),
                ground,
                z,
                &palette,
            );
        }
        CellKind::CastleBarracks => {
            let floor = ridge_floor(plan, plan.castle.barracks);
            place_gabled(
                sink,
                plan.castle.barracks,
                x,
                floor,
                ground,
                z,
                &palette,
                6,
                true,
            );
            let (roof_y, _) = gable_roof_y(plan.castle.barracks, x, z, floor + 6, 1);
            interior::furnish_barracks(sink, plan.castle.barracks, x, floor, z, roof_y, &palette);
        }
        CellKind::CastleKitchen => {
            let floor = ridge_floor(plan, plan.castle.kitchen);
            place_gabled(
                sink,
                plan.castle.kitchen,
                x,
                floor,
                ground,
                z,
                &palette,
                5,
                false,
            );
            let (roof_y, _) = gable_roof_y(plan.castle.kitchen, x, z, floor + 5, 1);
            interior::furnish_kitchen(sink, plan.castle.kitchen, x, floor, z, roof_y, &palette);
        }
        CellKind::CastleCourt => place_court(sink, x, ground, z, &palette),
        CellKind::CastleMoat => place_moat(sink, plan, x, ground, z),
        CellKind::Green => place_green(sink, plan, x, ground, z, &palette),
        CellKind::Eave => place_eave(sink, plan, x, floor, ground, z, &palette),
        CellKind::Hedge => {
            sink.put(x, ground, z, palette.grass);
            sink.put(x, ground + 1, z, palette.leaves);
            sink.put(x, ground + 2, z, palette.leaves);
        }
        CellKind::Harbor => place_harbor(sink, plan, x, ground, z, plan.heights.sea_level),
        CellKind::Dock => place_dock(sink, x, ground, z, plan.heights.sea_level, &palette),
        CellKind::Bridge => {
            place_bridge(sink, plan, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::WarTent => place_ridge_tent(sink, plan, x, ground, z, &palette, true),
        CellKind::Cage => place_cage(sink, plan, x, ground, z, &palette),
        CellKind::PalisadeGate => place_palisade_gate(sink, plan, x, ground, z, &palette),
        CellKind::SiegeTreb => place_trebuchet(sink, plan, x, ground, z, &palette),
        CellKind::SiegeRam => place_ram(sink, plan, x, ground, z, &palette),
        CellKind::SiegeLadder => place_siege_ladder(sink, x, ground, z, &palette),
        CellKind::WindSail => place_wind_sail(sink, plan, x, ground, z, &palette),
        CellKind::Arcade => {
            if plan
                .scene_at(x, z)
                .is_some_and(|s| s.kind == crate::scenes::SceneKind::RuinedAbbey)
            {
                place_abbey_arcade(sink, x, ground, z, &palette);
            } else {
                place_arcade(sink, plan, x, ground, z, &palette);
            }
        }
        CellKind::Lychgate => place_lychgate(sink, plan, x, ground, z, &palette),
        CellKind::FortressWall => {
            place_curtain(sink, plan, x, floor, ground, z, &palette, 8, true);
        }
        CellKind::FortressKeep => {
            let rect = blob_of(plan, x, z, CellKind::FortressKeep);
            let mut keep_floor = ridge_floor(plan, rect);
            if plan.has_scene(x, z, crate::scenes::SceneKind::Manor) {
                keep_floor += 5;
            }
            place_gabled(sink, rect, x, keep_floor, ground, z, &palette, 10, true);
            let (roof_y, _) = gable_roof_y(rect, x, z, keep_floor + 10, 1);
            interior::furnish_hall(sink, rect, x, keep_floor, z, roof_y, &palette);
        }
        CellKind::FortressTower => {
            place_fortress_tower(sink, plan, x, floor, ground, z, &palette);
        }
        CellKind::FortressCourt => place_court(sink, x, ground, z, &palette),
        CellKind::CathedralNave
        | CellKind::CathedralTransept
        | CellKind::CathedralChoir
        | CellKind::CathedralTower => {
            place_church_mass(sink, plan, x, floor, ground, z, &palette, kind);
        }
        CellKind::CathedralButtress => {
            place_flying_buttress(sink, plan, x, floor, ground, z, &palette);
        }
        CellKind::MineHead => place_mine_head(sink, plan, x, ground, z, &palette),
        CellKind::MineRail => place_mine_rail(sink, x, ground, z, &palette),
        CellKind::MineSpoil => place_mine_spoil(sink, plan, x, ground, z, &palette),
        CellKind::MineAdit => place_mine_adit(sink, plan, x, ground, z, &palette),
        CellKind::Ship => place_ship(sink, plan, x, ground, z, &palette),
        CellKind::Boat => place_fishing_boat(sink, plan, x, ground, z, &palette),
        CellKind::Quay => place_quay(sink, x, ground, z, plan.heights.sea_level, &palette),
        CellKind::Crane => place_crane(sink, plan, x, ground, z, plan.heights.sea_level, &palette),
        CellKind::TourneyList => place_tourney_list(sink, plan, x, ground, z, &palette),
        CellKind::TourneyStand => place_tourney_stand(sink, plan, x, ground, z, &palette),
        CellKind::Burnt => place_burnt(sink, plan, x, ground, z, &palette),
        CellKind::Breakwater => {
            place_breakwater(sink, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::SiegeTrench => {
            if plan.scenes.iter().any(|scene| {
                scene.kind == crate::scenes::SceneKind::BanditHideout && scene.rect.contains(x, z)
            }) {
                place_bandit_ravine(sink, plan, x, ground, z, &palette);
            } else {
                place_siege_trench(sink, x, ground, z, &palette);
            }
        }
        CellKind::WaterWheel => place_water_wheel(sink, plan, x, ground, z, &palette),
        CellKind::MillRace => {
            place_mill_race(sink, plan, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::Sluice => {
            place_sluice(sink, plan, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::Retaining => place_retaining(sink, plan, x, ground, z, &palette),
        CellKind::Lighthouse => {
            place_lighthouse(sink, plan, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::Shrine => place_shrine(sink, plan, x, ground, z, &palette),
        CellKind::FieldWall => place_field_wall(sink, plan, x, ground, z, &palette),
        CellKind::FlagPlinth => place_flag_plinth(sink, plan, x, ground, z),
        CellKind::CityDitch => place_city_ditch(sink, plan, x, ground, z, &palette),
        CellKind::StandingStone => place_standing_stone(sink, plan, x, ground, z, &palette),
        CellKind::Gallows => place_gallows(sink, x, ground, z, &palette),
        CellKind::LootPile => place_loot_pile(sink, x, ground, z, &palette),
        CellKind::Wagon => place_wagon(sink, plan, x, ground, z, &palette),
        CellKind::Beacon => place_beacon(sink, plan, x, ground, z, &palette),
        CellKind::Ford => place_ford(sink, plan, x, ground, z, &palette),
        CellKind::Weir => place_weir(sink, plan, x, ground, z, &palette),
        CellKind::CutStone => place_cut_stone(sink, plan, x, ground, z, &palette),
        CellKind::HerbGarden => place_herb_garden(sink, x, ground, z, &palette),
        CellKind::Maypole => place_maypole(sink, plan, x, ground, z, &palette),
        CellKind::MarketCross => place_market_cross(sink, plan, x, ground, z, &palette),
        CellKind::Pillory => place_pillory(sink, plan, x, ground, z, &palette),
        CellKind::Ropewalk => place_ropewalk(sink, plan, x, ground, z, &palette),
        CellKind::Slipway => place_slipway(sink, plan, x, ground, z, &palette),
        CellKind::Sawpit => place_sawpit(sink, plan, x, ground, z, &palette),
    }
    apply_column_damage(sink, plan, x, z, kind, floor, ground, &palette);
    // Harbour cranes used to punch mill walls. Restamp the mill envelope on
    // any lot cell that occupancy overwrote, except the wheel itself and
    // harbour hulls that later claimed the same water.
    if !matches!(
        kind,
        CellKind::Building
            | CellKind::WaterWheel
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Harbor
            | CellKind::Dock
            | CellKind::Quay
            | CellKind::Crane
            | CellKind::Breakwater
            | CellKind::Lighthouse
            | CellKind::MillRace
            | CellKind::Sluice
    ) && let Some(mill) = plan
        .lots
        .iter()
        .find(|lot| lot.kind == BuildingKind::Watermill && lot.rect.contains(x, z))
    {
        place_building(sink, plan, mill, x, ground, z, &palette);
    }
    for mill in plan
        .lots
        .iter()
        .filter(|lot| lot.kind == BuildingKind::Windmill)
    {
        let (cx, cz) = mill.rect.center();
        let on_hub = x == cx && z == cz;
        let on_blade = on_mill_blade(x - cx, z - cz);
        if !on_hub && !on_blade {
            continue;
        }
        if mill.rect.contains(x, z)
            || (kind == CellKind::WindSail && mill_owns_sail(&plan.lots, mill, x, z))
        {
            place_mill_blades(sink, x, z, cx, cz, mill_floor(plan, mill));
        }
    }
}

fn column_kind(plan: &TownPlan, x: i32, z: i32) -> CellKind {
    if plan.occ.bounds.contains(x, z) {
        let kind = plan.occ.kind(x, z);
        if kind == CellKind::Empty {
            plan.border_roads
                .get(&(x, z))
                .copied()
                .unwrap_or(CellKind::Empty)
        } else {
            kind
        }
    } else {
        plan.border_roads
            .get(&(x, z))
            .copied()
            .unwrap_or(CellKind::Empty)
    }
}

fn occupy_raises(kind: CellKind) -> bool {
    !matches!(
        kind,
        CellKind::Empty
            | CellKind::Harbor
            | CellKind::Dock
            | CellKind::CastleMoat
            | CellKind::Bridge
            | CellKind::Ship
            | CellKind::Boat
            | CellKind::Breakwater
            | CellKind::Lighthouse
            | CellKind::SiegeTrench
            | CellKind::CityDitch
            | CellKind::WaterWheel
            | CellKind::MillRace
            | CellKind::Sluice
            | CellKind::Ford
            | CellKind::Weir
            | CellKind::Slipway
    )
}

fn raise_out_of_water(
    sink: &mut impl BlockSink,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    for y in ground + 1..=sea {
        sink.put(x, y, z, palette.cobble);
    }
    sink.put(x, sea + 1, z, palette.grass);
}

fn terrace_floor(ground: i32, planned: i32) -> i32 {
    let raised = planned.max(ground - MAX_CUT);
    if ground - raised > MAX_FOUNDATION {
        ground - 2
    } else {
        raised
    }
}

/// One floor Y for a castle building so walls and roofs do not smear down a slope.
pub(crate) fn ridge_floor(plan: &TownPlan, rect: crate::geom::Rect) -> i32 {
    let mean = plan
        .heights
        .mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
    let (_, _, peak) = plan
        .heights
        .peak(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
    ((mean + peak) / 2 - 1).max(plan.heights.sea_level + 2)
}

fn foundation(
    sink: &mut impl BlockSink,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let start = ground.min(floor) - 2;
    let end = floor - 1;
    if end < start {
        return;
    }
    for y in start..=end {
        let mossy = (x + z + y) % 8 == 0;
        sink.put(x, y, z, palette.foundation(mossy));
    }
    if ground > floor + MAX_CUT {
        sink.put(x, ground, z, palette.cobble);
    }
}

/// Wider stone footing when a shoreline column would otherwise be a 1-wide pillar.
fn widen_piling(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let bed = ground.min(plan.heights.height(x, z) - 1).min(sea);
    if floor - bed <= 4 {
        return;
    }
    let foot = bed.max(sink.floor_min() + 2);
    for y in foot..floor {
        sink.put(x, y, z, palette.foundation(y <= sea));
    }
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = x + dx;
            let nz = z + dz;
            if !plan.bounds.contains(nx, nz) {
                continue;
            }
            let ng = plan.heights.height(nx, nz) - 1;
            if ng >= floor - 2 {
                continue;
            }
            let nkind = plan.occ.kind(nx, nz);
            if occupy_raises(nkind)
                && nkind == CellKind::Building
                && plan.occ.lot(nx, nz) != plan.occ.lot(x, z)
            {
                continue;
            }
            if nkind.is_castle()
                || matches!(
                    nkind,
                    CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Ship
                        | CellKind::Boat
                )
            {
                continue;
            }
            let nbed = ng.min(sea).max(sink.floor_min() + 2);
            for y in nbed..=(sea + 1).min(floor - 1) {
                sink.put(nx, y, nz, palette.cobble);
            }
        }
    }
}

fn terrace_edge(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    z: i32,
    palette: &Palette,
) {
    let lower = Dir::ALL
        .iter()
        .any(|dir| plan.heights.height(x + dir.dx(), z + dir.dz()) < floor - 1);
    if lower {
        sink.put(x, floor, z, palette.cobble);
        sink.put(x, floor + 1, z, palette.cobble_wall);
    }
}

fn clear_above(sink: &mut impl BlockSink, x: i32, from: i32, to: i32, z: i32) {
    let air = Block::AIR.default_state;
    for y in from..=to {
        sink.put(x, y, z, air);
    }
}

fn place_road(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    stair: Option<Dir>,
) {
    let mix = (x * 3 + z * 7).unsigned_abs();
    let country = !plan.town.inflate(6).contains(x, z);
    let block = if country {
        match mix % 5 {
            0 | 1 => palette.path,
            2 => Block::COARSE_DIRT.default_state,
            _ => palette.gravel,
        }
    } else {
        match mix % 5 {
            0 | 1 => palette.path,
            2 => palette.gravel,
            _ => palette.cobble,
        }
    };
    sink.put(x, ground, z, block);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 3, z);
    if let Some(dir) = stair {
        sink.put(x, ground + 1, z, stair_state(palette.cobble_stairs, dir));
    }
    if country {
        if mix.is_multiple_of(41) {
            sink.put(x, ground + 1, z, palette.cobble);
            sink.put(x, ground + 2, z, palette.cobble);
            sink.put(x, ground + 3, z, palette.slab);
        }
        return;
    }
    if mix.is_multiple_of(9) {
        sink.put(x, ground + 1, z, palette.cobble);
        sink.put(x, ground + 2, z, palette.fence);
        sink.put(x, ground + 3, z, Block::LANTERN.default_state);
    } else if mix.is_multiple_of(17) {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
        if mix.is_multiple_of(34) {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.slab);
        }
    } else if mix.is_multiple_of(23) {
        sink.put(x, ground + 1, z, Block::BARREL.default_state);
    } else if mix.is_multiple_of(29) {
        sink.put(x, ground + 1, z, palette.planks);
        sink.put(x, ground + 2, z, Block::CHEST.default_state);
    }
}

fn place_market(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.gravel);
    clear_above(sink, x, ground + 1, ground + 3, z);
    if (x + z) % 7 == 0 {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, Block::WHITE_WOOL.default_state);
        sink.put(x, ground + 3, z, Block::LANTERN.default_state);
    }
    if (x % 8 == 0) && (z % 8 == 0) {
        sink.put(x, ground + 1, z, palette.bricks);
        sink.put(x, ground + 2, z, palette.brick_wall);
        sink.put(x, ground + 3, z, Block::LANTERN.default_state);
    }
}

fn place_plaza(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::RoadsideInn)
    {
        place_inn_yard(sink, plan, x, ground, z, palette);
        return;
    }
    let mix = (x.wrapping_mul(3) + z).unsigned_abs() % 5;
    sink.put(
        x,
        ground,
        z,
        if mix == 0 {
            palette.cobble
        } else {
            palette.gravel
        },
    );
    clear_above(sink, x, ground + 1, ground + 3, z);
    if (x + z) % 6 == 0 {
        sink.put(x, ground + 1, z, palette.cobble);
        sink.put(x, ground + 2, z, palette.fence);
        sink.put(x, ground + 3, z, Block::LANTERN.default_state);
    } else if (x.wrapping_mul(5) + z) % 15 == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    }
}

fn place_fountain(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let (wx, wz) = plan.well;
    let dx = x - wx;
    let dz = z - wz;
    sink.put(x, ground, z, palette.bricks);
    let town_ring = dx.abs() <= 5 && dz.abs() <= 5;
    let cluster_edge = Dir::ALL.iter().any(|dir| {
        !matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::Fountain | CellKind::Well | CellKind::Market
        )
    });
    if (town_ring && dx.abs() <= 1 && dz.abs() <= 1) || (!town_ring && !cluster_edge) {
        sink.put(x, ground + 1, z, Block::WATER.default_state);
        return;
    }
    let face = if dx.abs() >= dz.abs() {
        if dx < 0 { Dir::East } else { Dir::West }
    } else if dz < 0 {
        Dir::South
    } else {
        Dir::North
    };
    sink.put(x, ground + 1, z, stair_state(palette.brick_stairs, face));
    if town_ring && dx == 0 && dz.abs() == 3 {
        sink.put(x, ground + 2, z, palette.bricks);
        sink.put(x, ground + 3, z, palette.brick_wall);
        sink.put(x, ground + 4, z, Block::LANTERN.default_state);
    }
}

fn place_orchard(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 6, z);
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::HermitTower)
    {
        let edge = Dir::ALL
            .iter()
            .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::Orchard);
        if edge {
            sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        } else {
            match (x + z * 3).unsigned_abs() % 5 {
                0 => sink.put(x, ground + 1, z, Block::POPPY.default_state),
                1 => sink.put(x, ground + 1, z, Block::DANDELION.default_state),
                2 => sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state),
                3 => {
                    sink.put(x, ground + 1, z, palette.log);
                    sink.put(x, ground + 2, z, Block::LANTERN.default_state);
                }
                _ => sink.put(x, ground + 1, z, Block::FERN.default_state),
            }
        }
        return;
    }
    if plan.has_scene(x, z, crate::scenes::SceneKind::HuntingLodge) {
        place_deer_park_column(sink, x, ground, z, palette);
        return;
    }
    let grid = (x.div_euclid(4) + z.div_euclid(4) + plan.seed as i32).unsigned_abs();
    if x % 4 == 0 && z % 4 == 0 {
        sink.put(x, ground + 1, z, palette.log);
        sink.put(x, ground + 2, z, palette.log);
        sink.put(x, ground + 3, z, palette.leaves);
        sink.put(x, ground + 4, z, palette.leaves);
    } else if grid % 7 == 0 {
        sink.put(x, ground + 1, z, Block::POPPY.default_state);
    }
}

fn park_oak_at(x: i32, z: i32) -> bool {
    x.rem_euclid(8) == 1 && z.rem_euclid(8) == 1
}

fn place_deer_park_column(
    sink: &mut impl BlockSink,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if park_oak_at(x, z) {
        sink.put(x, ground + 1, z, palette.log);
        sink.put(x, ground + 2, z, palette.log);
        sink.put(x, ground + 3, z, palette.leaves);
        sink.put(x, ground + 4, z, palette.leaves);
        sink.put(x, ground + 5, z, palette.leaves);
    }
}

fn place_pasture(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 3, z);
    let edge = Dir::ALL.iter().any(|dir| {
        !matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::Pasture | CellKind::FarmEdge
        )
    });
    if plan.has_scene(x, z, crate::scenes::SceneKind::HuntingLodge) {
        let near_kennel = (0..=4).any(|step| {
            Dir::ALL.iter().any(|dir| {
                plan.occ.kind(x + dir.dx() * step, z + dir.dz() * step) == CellKind::Cage
            })
        });
        if near_kennel {
            sink.put(x, ground, z, palette.path);
            let run_edge = Dir::ALL.iter().any(|dir| {
                !matches!(
                    plan.occ.kind(x + dir.dx(), z + dir.dz()),
                    CellKind::Pasture
                        | CellKind::FarmEdge
                        | CellKind::FieldWall
                        | CellKind::Cage
                        | CellKind::Orchard
                )
            });
            if run_edge {
                sink.put(x, ground + 1, z, palette.fence);
                sink.put(x, ground + 2, z, palette.fence);
            }
            return;
        }
        let edge = Dir::ALL.iter().any(|dir| {
            !matches!(
                plan.occ.kind(x + dir.dx(), z + dir.dz()),
                CellKind::Pasture
                    | CellKind::FarmEdge
                    | CellKind::FieldWall
                    | CellKind::Orchard
                    | CellKind::Cage
            )
        });
        if edge {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.fence);
            return;
        }
        let rect = blob_of(plan, x, z, CellKind::Pasture);
        let (cx, cz) = rect.center();
        if z == cz && (x - cx).abs() <= 1 {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, Block::HAY_BLOCK.default_state);
            sink.put(x, ground + 3, z, palette.fence);
            return;
        }
        if x == cx && z == cz + 2 {
            sink.put(x, ground + 1, z, Block::CAULDRON.default_state);
        }
        return;
    }
    if edge {
        sink.put(x, ground + 1, z, palette.fence);
        if (x + z) % 4 == 0 {
            sink.put(x, ground + 2, z, palette.fence);
        }
    } else if (x + z) % 13 == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    } else if (x.wrapping_mul(3) + z).rem_euclid(17) == 0 {
        sink.put(x, ground + 1, z, Block::WHITE_WOOL.default_state);
    }
}

fn place_grave(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(
        x,
        ground,
        z,
        if (x + z) % 2 == 0 {
            palette.path
        } else {
            palette.grass
        },
    );
    clear_above(sink, x, ground + 1, ground + 5, z);
    let mix = (x.wrapping_mul(19) + z.wrapping_mul(11)).unsigned_abs();
    if mix % 9 == 0 {
        sink.put(x, ground + 1, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, ground + 2, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, ground + 3, z, Block::DARK_OAK_LEAVES.default_state);
        sink.put(x, ground + 4, z, Block::DARK_OAK_LEAVES.default_state);
    } else if x % 2 == 0 && z % 2 == 0 {
        sink.put(x, ground + 1, z, palette.cobble);
        sink.put(x, ground + 2, z, palette.brick_wall);
        if mix % 5 == 0 {
            sink.put(x, ground + 3, z, palette.slab);
        }
    } else if mix % 7 == 0 {
        sink.put(x, ground + 1, z, Block::POPPY.default_state);
    }
}

fn place_palisade(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let bandit = plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::BanditHideout);
    let top = if bandit { 3 } else { 5 };
    sink.put(x, ground, z, palette.cobble);
    for y in ground + 1..=ground + top {
        sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
    }
    sink.put(x, ground + top + 1, z, Block::DARK_OAK_FENCE.default_state);
    if bandit {
        if (x + z) % 6 == 0 {
            sink.put(x, ground + top + 2, z, Block::BLACK_BANNER.default_state);
        }
        return;
    }
    sink.put(x, ground + 7, z, Block::DARK_OAK_FENCE.default_state);
    if (x + z) % 8 == 0 {
        sink.put(x, ground + 8, z, Block::BLACK_BANNER.default_state);
    }
}

fn place_palisade_walk(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.path);
    clear_above(sink, x, ground + 1, ground + 3, z);
    sink.put(x, ground + 3, z, Block::DARK_OAK_FENCE.default_state);
    sink.put(x, ground + 4, z, Block::DARK_OAK_SLAB.default_state);
    if (x + z) % 3 == 0 {
        sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, Block::DARK_OAK_FENCE.default_state);
    }
    let _ = palette;
}

fn place_tent(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if plan.scene_at(x, z).is_some_and(|scene| {
        matches!(
            scene.kind,
            crate::scenes::SceneKind::Tourney | crate::scenes::SceneKind::SiegeCamp
        )
    }) {
        place_tourney_pavilion(sink, plan, x, ground, z, palette);
        return;
    }
    place_ridge_tent(sink, plan, x, ground, z, palette, false);
}

fn place_camp_court(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let over_ravine = Dir::ALL.iter().any(|dir| {
        matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::SiegeTrench
        )
    });
    if over_ravine {
        sink.put(x, ground, z, Block::SPRUCE_LOG.default_state);
        sink.put(x, ground + 1, z, Block::SPRUCE_FENCE.default_state);
        clear_above(sink, x, ground + 2, ground + 6, z);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::Quarry)
    {
        place_quarry_pit(sink, plan, x, ground, z, palette);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::SiegeCamp)
    {
        place_siege_cook(sink, plan, x, ground, z, palette);
        return;
    }
    sink.put(x, ground, z, palette.path);
    clear_above(sink, x, ground + 1, ground + 6, z);
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::BanditHideout)
    {
        let blob = blob_of(plan, x, z, CellKind::CampCourt);
        let (cx, cz) = blob.center();
        let cheb = (x - cx).abs().max((z - cz).abs());
        if cheb == 0 {
            sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
            return;
        }
        if cheb == 1 {
            sink.put(x, ground + 1, z, palette.cobble);
            sink.put(x, ground + 2, z, Block::SPRUCE_SLAB.default_state);
            return;
        }
        if cheb == 2 && (x == cx || z == cz) {
            sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE.default_state);
            sink.put(x, ground + 2, z, Block::IRON_BARS.default_state);
            return;
        }
    }
    let mix = (x.wrapping_mul(13) + z.wrapping_mul(7)).unsigned_abs();
    if mix % 28 == 0 {
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
    } else if mix % 22 == 0 {
        sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 3, z, Block::BLACK_BANNER.default_state);
    }
}

fn place_quarry_pit(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let pit = blob_of(plan, x, z, CellKind::CampCourt);
    let downhill = crate::scenes::quarry_downhill(&plan.heights, pit);
    let (cx, cz) = pit.center();
    let cheb = (x - cx).abs().max((z - cz).abs());
    let max_c = (pit.width().max(pit.depth()) / 2).max(1);
    let bench = ((max_c - cheb) * 4 / max_c).clamp(0, 4);
    let on_ramp = match downhill {
        Dir::South | Dir::North => (x - cx).abs() <= 1,
        Dir::East | Dir::West => (z - cz).abs() <= 1,
    };
    let cut = if on_ramp {
        let span = match downhill {
            Dir::South | Dir::North => pit.depth(),
            Dir::East | Dir::West => pit.width(),
        }
        .max(1);
        let from_uphill = match downhill {
            Dir::South => z - pit.min_z,
            Dir::North => pit.max_z - z,
            Dir::East => x - pit.min_x,
            Dir::West => pit.max_x - x,
        };
        (from_uphill * 10 / span).clamp(2, 10)
    } else {
        bench * 2 + 2
    };
    let floor = ground - cut;
    let stone = match bench {
        0 => palette.cobble,
        1 => palette.andesite,
        2 => palette.bricks,
        3 => palette.mossy_bricks,
        _ => Block::STONE.default_state,
    };
    sink.put(x, floor.saturating_sub(1), z, palette.andesite);
    sink.put(x, floor, z, stone);
    if on_ramp {
        if cut > 0 && (x + z) & 1 == 0 {
            sink.put(
                x,
                floor + 1,
                z,
                stair_state(palette.cobble_stairs, downhill),
            );
            sink.put(x, floor + 2, z, Block::RAIL.default_state);
        } else {
            sink.put(x, floor + 1, z, Block::RAIL.default_state);
            if (x + z).rem_euclid(6) == 0 {
                sink.put(x, floor + 2, z, Block::CHEST.default_state);
            }
        }
        clear_above(sink, x, floor + 3, ground + 8, z);
    } else {
        clear_above(sink, x, floor + 1, ground + 8, z);
    }
}

fn place_siege_cook(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let blob = blob_of(plan, x, z, CellKind::CampCourt);
    let (cx, cz) = blob.center();
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 6, z);
    if x == cx && z == cz {
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
        return;
    }
    if (x - cx).abs() + (z - cz).abs() == 1 && x == cx {
        sink.put(x, ground + 1, z, Block::CAULDRON.default_state);
        return;
    }
    if blob.is_corner(x, z) {
        sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, Block::OAK_TRAPDOOR.default_state);
        return;
    }
    if (x + z) & 1 == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    } else {
        sink.put(x, ground + 1, z, Block::BARREL.default_state);
    }
}

fn place_loot_pile(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 4, z);
    let mix = (x.wrapping_mul(5) + z).unsigned_abs() % 4;
    sink.put(
        x,
        ground + 1,
        z,
        match mix {
            0 => Block::CHEST.default_state,
            1 => Block::BARREL.default_state,
            2 => Block::CHEST.default_state,
            _ => Block::BARREL.default_state,
        },
    );
    if mix == 0 {
        sink.put(x, ground + 2, z, Block::BARREL.default_state);
    }
}

fn place_beacon(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Beacon);
    foundation(sink, x, ground, ground, z, palette);
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 14, z);
    let (cx, cz) = rect.center();
    let centre = x == cx && z == cz;
    if rect.width() <= 1 && rect.depth() <= 1 {
        for y in ground + 1..=ground + 7 {
            sink.put(x, y, z, palette.cobble);
        }
        sink.put(x, ground + 8, z, Block::CAMPFIRE.default_state);
        return;
    }
    if centre {
        for y in ground + 1..=ground + 8 {
            sink.put(
                x,
                y,
                z,
                if y % 2 == 0 {
                    palette.bricks
                } else {
                    palette.cobble
                },
            );
        }
        sink.put(x, ground + 9, z, Block::GLOWSTONE.default_state);
        sink.put(x, ground + 10, z, Block::CAMPFIRE.default_state);
        return;
    }
    if rect.is_corner(x, z) {
        for y in ground + 1..=ground + 4 {
            sink.put(x, y, z, palette.cobble_wall);
        }
        sink.put(x, ground + 5, z, Block::OAK_FENCE.default_state);
        sink.put(x, ground + 6, z, Block::RED_BANNER.default_state);
        return;
    }
    for y in ground + 1..=ground + 8 {
        sink.put(x, y, z, palette.cobble);
    }
    sink.put(x, ground + 9, z, Block::IRON_BARS.default_state);
    if x == rect.min_x {
        for y in ground + 1..=ground + 8 {
            sink.put(x, y, z, Block::LADDER.default_state);
        }
    }
}

fn place_ford(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let water_y = plan.heights.height(x, z).min(sea);
    let depth = plan.heights.water_depth(x, z).max(1);
    let bed = (water_y - depth).max(sink.floor_min() + 2);
    for y in bed..=water_y {
        sink.put(
            x,
            y,
            z,
            if y == water_y {
                palette.path
            } else if (x + z + y) % 3 == 0 {
                palette.mossy
            } else {
                palette.cobble
            },
        );
    }
    if (x + z) % 4 == 0 {
        sink.put(x, water_y + 1, z, palette.cobble_wall);
    }
    clear_above(sink, x, water_y + 2, water_y + 8, z);
}

fn place_weir(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let water_y = plan.heights.height(x, z).min(sea);
    let depth = plan.heights.water_depth(x, z).max(1);
    let bed = (water_y - depth).max(sink.floor_min() + 2);
    sink.put(x, bed, z, palette.cobble);
    let post_lo = (water_y - 2).max(bed + 1);
    for y in post_lo..=water_y + 1 {
        sink.put(x, y, z, palette.fence);
    }
    if (x + z) % 2 == 0 {
        sink.put(x, water_y + 1, z, Block::OAK_TRAPDOOR.default_state);
    }
    if (x + z) % 5 == 0 {
        sink.put(x, water_y + 2, z, Block::LANTERN.default_state);
    }
    clear_above(sink, x, water_y + 3, water_y + 8, z);
}

fn place_cut_stone(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if plan.has_scene(x, z, crate::scenes::SceneKind::Shipyard) {
        place_timber_stack(sink, plan, x, ground, z, palette);
        return;
    }
    let mix = (x.wrapping_mul(7) + z.wrapping_mul(11)).unsigned_abs();
    let h = 1 + (mix % 4) as i32;
    for y in ground..=ground + h {
        sink.put(
            x,
            y,
            z,
            match mix % 4 {
                0 => palette.bricks,
                1 => palette.mossy_bricks,
                2 => palette.cracked_bricks,
                _ => palette.cobble,
            },
        );
    }
    if mix % 3 == 0 {
        sink.put(
            x,
            ground + h + 1,
            z,
            stair_state(palette.brick_stairs, Dir::South),
        );
        clear_above(sink, x, ground + h + 2, ground + h + 4, z);
    } else {
        clear_above(sink, x, ground + h + 1, ground + h + 4, z);
    }
}

fn place_herb_garden(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    let mix = (x + z * 3).rem_euclid(6);
    if mix == 0 {
        sink.put(x, ground, z, palette.path);
        clear_above(sink, x, ground + 1, ground + 4, z);
        return;
    }
    sink.put(x, ground, z, Block::FARMLAND.default_state);
    match mix {
        1 => sink.put(x, ground + 1, z, Block::OAK_TRAPDOOR.default_state),
        2 => sink.put(x, ground + 1, z, Block::SWEET_BERRY_BUSH.default_state),
        3 => sink.put(x, ground + 1, z, Block::POPPY.default_state),
        4 => sink.put(x, ground + 1, z, Block::DANDELION.default_state),
        _ => sink.put(x, ground + 1, z, Block::FERN.default_state),
    }
    clear_above(sink, x, ground + 2, ground + 6, z);
}

fn place_timber_stack(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::CutStone);
    let y = shipyard_bench_y(plan, x, z).max(ground);
    let (cx, cz) = rect.center();
    let dx = (x - cx).abs();
    let dz = (z - cz).abs();
    let h = if dx + dz == 0 {
        3
    } else if rect.on_edge(x, z) && !rect.is_corner(x, z) {
        2
    } else if rect.is_corner(x, z) {
        1
    } else {
        3
    };
    let stripped = rect.min_x.wrapping_add(rect.min_z) & 1 == 0;
    let log = if stripped {
        Block::STRIPPED_OAK_LOG.default_state
    } else {
        palette.log
    };
    let lo = ground.min(y).max(sink.floor_min() + 2);
    for yy in lo..=y {
        sink.put(x, yy, z, Block::DIRT.default_state);
    }
    clear_above(sink, x, y + 1, y + 8, z);
    for n in 1..=h {
        sink.put(x, y + n, z, log);
    }
}

fn place_maypole(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Maypole);
    let (cx, cz) = rect.center();
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 10, z);
    if x == cx && z == cz {
        for y in ground + 1..=ground + 8 {
            sink.put(x, y, z, palette.log);
        }
        sink.put(x, ground + 9, z, Block::YELLOW_WOOL.default_state);
        return;
    }
    sink.put(x, ground + 1, z, palette.fence);
    let ribbon = match (x + z).rem_euclid(4) {
        0 => Block::RED_WOOL.default_state,
        1 => Block::WHITE_WOOL.default_state,
        2 => Block::YELLOW_WOOL.default_state,
        _ => Block::BLUE_WOOL.default_state,
    };
    sink.put(x, ground + 2, z, ribbon);
    if rect.on_edge(x, z) {
        sink.put(x, ground + 3, z, Block::OAK_FENCE.default_state);
    }
}

fn place_market_cross(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::MarketCross);
    let (cx, cz) = rect.center();
    sink.put(x, ground, z, palette.bricks);
    clear_above(sink, x, ground + 1, ground + 8, z);
    sink.put(x, ground + 1, z, palette.slab);
    if x == cx && z == cz {
        for y in ground + 2..=ground + 5 {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, ground + 6, z, palette.brick_wall);
        sink.put(x, ground + 7, z, Block::LANTERN.default_state);
        return;
    }
    if x == cx || z == cz {
        sink.put(x, ground + 2, z, palette.bricks);
        sink.put(
            x,
            ground + 3,
            z,
            stair_state(palette.brick_stairs, interior::outward(rect, x, z)),
        );
    }
}

fn place_pillory(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Pillory);
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 5, z);
    if rect.is_corner(x, z) {
        sink.put(x, ground + 1, z, palette.log);
        sink.put(x, ground + 2, z, palette.log);
        return;
    }
    sink.put(x, ground + 1, z, palette.slab);
    sink.put(x, ground + 2, z, palette.fence);
    if rect.on_edge(x, z) {
        sink.put(x, ground + 3, z, palette.log);
    }
}

fn ropewalk_floor(plan: &TownPlan, rect: Rect) -> i32 {
    let mut y = plan.heights.sea_level + 2;
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if plan.occ.kind(x, z) == CellKind::Ropewalk {
                y = y.max(shipyard_bench_y(plan, x, z));
            }
        }
    }
    y
}

fn place_ropewalk(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Ropewalk);
    let y = ropewalk_floor(plan, rect);
    let eaves = y + 4;
    let roof_y = eaves + 1;
    let (cx, cz) = rect.center();
    let along_x = rect.width() >= rect.depth();
    let door = if along_x {
        x == rect.min_x && z == cz
    } else {
        z == rect.min_z && x == cx
    };
    sink.put(x, y, z, palette.planks);
    foundation(sink, x, y, ground, z, palette);
    clear_above(sink, x, y + 1, roof_y + 2, z);
    if rect.on_edge(x, z) {
        if rect.is_corner(x, z) {
            for yy in y + 1..=eaves {
                sink.put(x, yy, z, palette.log);
            }
        } else if door {
            interior::place_door(sink, x, y + 1, z, interior::outward(rect, x, z), false);
            for yy in y + 3..=eaves {
                sink.put(x, yy, z, palette.planks);
            }
        } else {
            for yy in y + 1..=eaves {
                if yy == y + 2 && (x + z) % 5 == 0 {
                    sink.put(x, yy, z, Block::GLASS_PANE.default_state);
                } else {
                    sink.put(x, yy, z, palette.planks);
                }
            }
        }
        sink.put(x, roof_y, z, palette.planks);
        return;
    }
    if (x + z) % 3 == 0 {
        sink.put(x, y + 1, z, palette.fence);
        sink.put(x, y + 2, z, Block::WHITE_WOOL.default_state);
    } else if (x + z) % 5 == 0 {
        sink.put(x, y + 1, z, Block::OAK_FENCE.default_state);
    }
    sink.put(x, roof_y, z, palette.planks);
    if (x + z) % 4 == 0 {
        sink.put(x, roof_y - 1, z, Block::LANTERN.default_state);
    }
}

fn place_slipway(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let rect = blob_of(plan, x, z, CellKind::Slipway);
    let deck = slip_deck_y(plan, &rect, x, z, ground, sea);
    let lo = ground.min(deck).max(sink.floor_min() + 2);
    for yy in lo..=deck {
        sink.put(
            x,
            yy,
            z,
            if yy == deck {
                palette.planks
            } else {
                palette.bricks
            },
        );
    }
    clear_above(sink, x, deck + 1, deck + 12, z);
    let out = slip_water_dir(plan, rect);
    let long_side = match out {
        Dir::North | Dir::South => x == rect.min_x || x == rect.max_x,
        Dir::East | Dir::West => z == rect.min_z || z == rect.max_z,
    };
    let short_end = match out {
        Dir::North | Dir::South => z == rect.min_z || z == rect.max_z,
        Dir::East | Dir::West => x == rect.min_x || x == rect.max_x,
    };
    if long_side && !short_end {
        sink.put(x, deck, z, palette.log);
        sink.put(x, deck + 1, z, palette.log);
    }
}

fn slip_deck_y(plan: &TownPlan, rect: &Rect, x: i32, z: i32, ground: i32, sea: i32) -> i32 {
    let out = slip_water_dir(plan, *rect);
    let span = match out {
        Dir::North | Dir::South => rect.depth(),
        Dir::East | Dir::West => rect.width(),
    }
    .max(1);
    let from_inland = match out {
        Dir::South => z - rect.min_z,
        Dir::North => rect.max_z - z,
        Dir::East => x - rect.min_x,
        Dir::West => rect.max_x - x,
    }
    .clamp(0, span);
    let inland_h = match out {
        Dir::South => (rect.min_x..=rect.max_x)
            .map(|xx| shipyard_bench_y(plan, xx, rect.min_z))
            .max()
            .unwrap_or(ground),
        Dir::North => (rect.min_x..=rect.max_x)
            .map(|xx| shipyard_bench_y(plan, xx, rect.max_z))
            .max()
            .unwrap_or(ground),
        Dir::East => (rect.min_z..=rect.max_z)
            .map(|zz| shipyard_bench_y(plan, rect.min_x, zz))
            .max()
            .unwrap_or(ground),
        Dir::West => (rect.min_z..=rect.max_z)
            .map(|zz| shipyard_bench_y(plan, rect.max_x, zz))
            .max()
            .unwrap_or(ground),
    }
    .max(sea + 2)
    .min(sea + 6);
    sea + (inland_h - sea) * (span - from_inland) / span
}

fn slip_water_dir(plan: &TownPlan, rect: Rect) -> Dir {
    Dir::ALL
        .iter()
        .copied()
        .max_by_key(|dir| {
            let (cx, cz) = rect.center();
            (1..=6)
                .filter(|step| {
                    let nx = cx + dir.dx() * step;
                    let nz = cz + dir.dz() * step;
                    !plan.heights.is_dry(nx, nz)
                        || matches!(
                            plan.occ.kind(nx, nz),
                            CellKind::Harbor | CellKind::Dock | CellKind::Breakwater
                        )
                })
                .count()
        })
        .unwrap_or(Dir::South)
}

fn slip_rect_near(plan: &TownPlan, x: i32, z: i32) -> Option<Rect> {
    if plan.occ.kind(x, z) == CellKind::Slipway {
        return Some(blob_of(plan, x, z, CellKind::Slipway));
    }
    for step in 1..=8 {
        for dir in Dir::ALL {
            let nx = x + dir.dx() * step;
            let nz = z + dir.dz() * step;
            if plan.occ.kind(nx, nz) == CellKind::Slipway {
                return Some(blob_of(plan, nx, nz, CellKind::Slipway));
            }
        }
    }
    None
}

fn place_sawpit(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Sawpit);
    let (cx, cz) = rect.center();
    let y = shipyard_bench_y(plan, x, z).max(ground);
    let floor = y - 2;
    foundation(sink, x, floor, ground, z, palette);
    for yy in floor..=y {
        sink.put(
            x,
            yy,
            z,
            if rect.on_edge(x, z) {
                palette.log
            } else {
                palette.cobble
            },
        );
    }
    clear_above(sink, x, y + 1, y + 6, z);
    if x == cx && z == cz {
        sink.put(x, y + 1, z, palette.log);
        sink.put(x, y + 2, z, Block::OAK_TRAPDOOR.default_state);
        return;
    }
    if !rect.on_edge(x, z) && (x == cx || z == cz) {
        sink.put(x, y + 1, z, palette.fence);
    }
}

fn place_motte_bank(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::WatchPost);
    let (cx, cz) = rect.center();
    let dist = (x - cx).abs().max((z - cz).abs());
    let rise = (8 - dist / 2).clamp(4, 8);
    foundation(sink, x, ground + rise, ground, z, palette);
    for y in ground..=ground + rise {
        sink.put(
            x,
            y,
            z,
            if y == ground + rise {
                palette.grass
            } else if (x + z + y) % 3 == 0 {
                palette.cobble
            } else {
                Block::DIRT.default_state
            },
        );
    }
    clear_above(sink, x, ground + rise + 1, ground + rise + 4, z);
    if rect.on_edge(x, z) {
        sink.put(x, ground + rise + 1, z, palette.cobble);
        if (x + z) % 2 == 0 {
            sink.put(x, ground + rise + 2, z, palette.cobble_wall);
        }
    }
}

fn place_wagon(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let blob = blob_of(plan, x, z, CellKind::Wagon);
    sink.put(x, ground, z, palette.path);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 4, z);
    if blob.is_corner(x, z) {
        sink.put(x, ground + 1, z, palette.fence);
        return;
    }
    sink.put(x, ground + 1, z, Block::OAK_TRAPDOOR.default_state);
    if x == blob.min_x {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, palette.fence);
    }
}

fn blob_of(plan: &TownPlan, x: i32, z: i32, kind: CellKind) -> Rect {
    let mut min_x = x;
    let mut max_x = x;
    let mut min_z = z;
    let mut max_z = z;
    while plan.occ.kind(min_x - 1, z) == kind {
        min_x -= 1;
    }
    while plan.occ.kind(max_x + 1, z) == kind {
        max_x += 1;
    }
    while plan.occ.kind(x, min_z - 1) == kind {
        min_z -= 1;
    }
    while plan.occ.kind(x, max_z + 1) == kind {
        max_z += 1;
    }
    for zz in min_z..=max_z {
        while plan.occ.kind(min_x - 1, zz) == kind {
            min_x -= 1;
        }
        while plan.occ.kind(max_x + 1, zz) == kind {
            max_x += 1;
        }
    }
    for xx in min_x..=max_x {
        while plan.occ.kind(xx, min_z - 1) == kind {
            min_z -= 1;
        }
        while plan.occ.kind(xx, max_z + 1) == kind {
            max_z += 1;
        }
    }
    Rect::new(min_x, min_z, max_x, max_z)
}

fn place_ridge_tent(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    war: bool,
) {
    sink.put(x, ground, z, palette.path);
    let kind = if war {
        CellKind::WarTent
    } else {
        CellKind::Tent
    };
    let rect = blob_of(plan, x, z, kind);
    let along_x = rect.width() >= rect.depth();
    let (cx, cz) = rect.center();
    let ridge_dist = if along_x {
        (z - cz).abs()
    } else {
        (x - cx).abs()
    };
    let peak = if war { 5 } else { 4 };
    let h = (peak - ridge_dist).max(2);
    let stripe = if along_x { x } else { z };
    let wool = if war {
        if stripe % 2 == 0 {
            Block::RED_WOOL.default_state
        } else {
            Block::BLACK_WOOL.default_state
        }
    } else if stripe % 2 == 0 {
        Block::WHITE_WOOL.default_state
    } else {
        Block::GRAY_WOOL.default_state
    };
    let pole = rect.is_corner(x, z)
        || (along_x && z == cz && (x == rect.min_x || x == rect.max_x))
        || (!along_x && x == cx && (z == rect.min_z || z == rect.max_z));
    if pole {
        for y in ground + 1..=ground + h {
            sink.put(x, y, z, Block::DARK_OAK_FENCE.default_state);
        }
        if war && rect.is_corner(x, z) {
            sink.put(x, ground + h + 1, z, Block::BLACK_BANNER.default_state);
            if plan
                .scene_at(x, z)
                .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::SiegeCamp)
            {
                sink.put(x, ground + h + 2, z, Block::RED_BANNER.default_state);
            }
        }
    }
    for y in ground + 1..=ground + h {
        if pole && y < ground + h {
            continue;
        }
        sink.put(x, y, z, wool);
    }
    let interior = ridge_dist == 0 && !rect.on_edge(x, z);
    if interior {
        if war {
            sink.put(x, ground + 1, z, Block::CHEST.default_state);
        } else {
            sink.put(x, ground + 1, z, Block::WHITE_BED.default_state);
        }
        sink.put(x, ground + 2, z, wool);
    }
}

fn place_cage(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.cobble);
    let rect = blob_of(plan, x, z, CellKind::Cage);
    if plan.has_scene(x, z, crate::scenes::SceneKind::HuntingLodge) {
        place_kennel(sink, rect, x, ground, z, palette);
        return;
    }
    if rect.is_corner(x, z) {
        for y in ground + 1..=ground + 3 {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
        }
    } else if rect.on_edge(x, z) {
        for y in ground + 1..=ground + 3 {
            sink.put(x, y, z, Block::IRON_BARS.default_state);
        }
    } else {
        clear_above(sink, x, ground + 1, ground + 2, z);
        sink.put(x, ground + 1, z, Block::IRON_CHAIN.default_state);
    }
    sink.put(x, ground + 4, z, Block::IRON_BARS.default_state);
}

fn place_kennel(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    clear_above(sink, x, ground + 1, ground + 5, z);
    if rect.is_corner(x, z) {
        sink.put(x, ground + 1, z, palette.log);
        sink.put(x, ground + 2, z, palette.log);
        sink.put(x, ground + 3, z, palette.slab);
        return;
    }
    if rect.on_edge(x, z) {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, palette.fence);
        sink.put(x, ground + 3, z, palette.slab);
        return;
    }
    sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    if (x + z) % 2 == 0 {
        sink.put(x, ground + 2, z, palette.slab);
    }
}

fn place_palisade_gate(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    let rect = blob_of(plan, x, z, CellKind::PalisadeGate);
    let (cx, cz) = rect.center();
    if x == cx && z == cz {
        sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE_GATE.default_state);
        clear_above(sink, x, ground + 2, ground + 5, z);
    } else {
        for y in ground + 1..=ground + 6 {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
        }
        sink.put(x, ground + 7, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 8, z, Block::BLACK_BANNER.default_state);
    }
}

fn treb_counterweight_at_min(plan: &TownPlan, rect: Rect, along_x: bool) -> bool {
    let (ax, az) = if along_x {
        (rect.min_x, (rect.min_z + rect.max_z) / 2)
    } else {
        ((rect.min_x + rect.max_x) / 2, rect.min_z)
    };
    let (bx, bz) = if along_x {
        (rect.max_x, (rect.min_z + rect.max_z) / 2)
    } else {
        ((rect.min_x + rect.max_x) / 2, rect.max_z)
    };
    treb_palisade_dist(plan, ax, az) <= treb_palisade_dist(plan, bx, bz)
}

fn treb_palisade_dist(plan: &TownPlan, x: i32, z: i32) -> i32 {
    let mut best = 64;
    for dz in -16..=16 {
        for dx in -16..=16 {
            if matches!(
                plan.occ.kind(x + dx, z + dz),
                CellKind::Palisade | CellKind::PalisadeGate | CellKind::PalisadeWalk
            ) {
                best = best.min(dx.abs().max(dz.abs()));
            }
        }
    }
    best
}

fn place_trebuchet(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 12, z);
    let rect = blob_of(plan, x, z, CellKind::SiegeTreb);
    let along_x = rect.width() >= rect.depth();
    let span = if along_x { rect.width() } else { rect.depth() };
    let across = if along_x { rect.depth() } else { rect.width() };
    let t_from_min = if along_x {
        x - rect.min_x
    } else {
        z - rect.min_z
    };
    let t = if treb_counterweight_at_min(plan, rect, along_x) {
        t_from_min
    } else {
        span - 1 - t_from_min
    };
    let a = if along_x {
        z - rect.min_z
    } else {
        x - rect.min_x
    };
    let a_mid = across / 2;
    let pivot = (span / 3).clamp(2, (span - 4).max(2));
    let mast = ground + 8;
    let log = Block::DARK_OAK_LOG.default_state;
    let on_leg = a == 0 || a == across - 1;
    let on_keel = a == a_mid;
    if on_leg && (t - pivot).abs() <= 1 {
        let h = mast - (t - pivot).abs() * 2;
        for y in ground + 1..=h {
            sink.put(x, y, z, log);
        }
    }
    if on_keel {
        sink.put(x, mast, z, log);
        if t == pivot {
            for y in ground + 1..=mast + 2 {
                sink.put(x, y, z, log);
            }
            sink.put(x, mast + 3, z, Block::LANTERN.default_state);
        } else if t <= 1 {
            for y in (mast - 3)..mast {
                sink.put(x, y, z, palette.cobble);
            }
            sink.put(x, mast - 4, z, palette.planks);
            sink.put(x, mast + 1, z, palette.planks);
        } else if t >= span - 2 {
            sink.put(x, mast + 1, z, Block::DARK_OAK_FENCE.default_state);
            sink.put(x, mast + 2, z, Block::DARK_OAK_FENCE.default_state);
            for dy in 1..=4 {
                sink.put(x, mast - dy, z, Block::DARK_OAK_FENCE.default_state);
            }
            sink.put(x, mast - 5, z, palette.cobble);
        } else if t > pivot {
            sink.put(x, mast + 1, z, log);
        } else {
            sink.put(x, mast - 1, z, Block::DARK_OAK_FENCE.default_state);
            sink.put(x, mast - 2, z, Block::DARK_OAK_FENCE.default_state);
        }
    }
}

fn place_ram(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    let rect = blob_of(plan, x, z, CellKind::SiegeRam);
    let along_x = rect.width() >= rect.depth();
    let (cx, cz) = rect.center();
    if rect.is_corner(x, z) {
        for y in ground + 1..=ground + 4 {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
        }
        sink.put(x, ground + 5, z, Block::DARK_OAK_SLAB.default_state);
    } else if rect.on_edge(x, z) {
        sink.put(x, ground + 4, z, Block::DARK_OAK_SLAB.default_state);
        sink.put(x, ground + 5, z, Block::DARK_OAK_PLANKS.default_state);
    }
    let on_beam = if along_x { z == cz } else { x == cx };
    if on_beam && !rect.on_edge(x, z) {
        sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, Block::STRIPPED_DARK_OAK_LOG.default_state);
        sink.put(x, ground + 3, z, Block::DARK_OAK_SLAB.default_state);
    }
}

fn place_siege_ladder(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.path);
    sink.put(x, ground + 1, z, Block::DARK_OAK_LOG.default_state);
    for y in ground + 2..=ground + 6 {
        sink.put(x, y, z, Block::LADDER.default_state);
    }
    sink.put(x, ground + 7, z, Block::DARK_OAK_FENCE.default_state);
}

const MILL_BLADE_LEN: i32 = 10;

fn mill_floor(plan: &TownPlan, mill: &Lot) -> i32 {
    plan.heights
        .mean(
            mill.rect.min_x,
            mill.rect.min_z,
            mill.rect.max_x,
            mill.rect.max_z,
        )
        .max(plan.heights.sea_level + 2)
}

fn mill_hub_y(floor: i32) -> i32 {
    floor + 18
}

fn on_mill_blade(dx: i32, dz: i32) -> bool {
    if dx == 0 || dz == 0 {
        return false;
    }
    let dist = dx.abs().max(dz.abs());
    (1..=MILL_BLADE_LEN).contains(&dist) && (dx.abs() - dz.abs()).abs() <= 1
}

pub(crate) fn mill_owns_sail(lots: &[Lot], mill: &Lot, x: i32, z: i32) -> bool {
    let (cx, cz) = mill.rect.center();
    let dist = (x - cx).abs() + (z - cz).abs();
    lots.iter().all(|lot| {
        if lot.id == mill.id || lot.kind != BuildingKind::Windmill {
            true
        } else {
            let (ox, oz) = lot.rect.center();
            (x - ox).abs() + (z - oz).abs() > dist
        }
    })
}

fn in_mill_disc(plan: &TownPlan, x: i32, z: i32) -> bool {
    plan.lots.iter().any(|lot| {
        if lot.kind != BuildingKind::Windmill {
            return false;
        }
        let (cx, cz) = lot.rect.center();
        let dx = x - cx;
        let dz = z - cz;
        dx * dx + dz * dz <= crate::scenes::MILL_SAIL_RADIUS * crate::scenes::MILL_SAIL_RADIUS
    })
}

pub(crate) fn nearest_mill(plan: &TownPlan, x: i32, z: i32) -> Option<&Lot> {
    plan.lots
        .iter()
        .filter(|lot| lot.kind == BuildingKind::Windmill)
        .min_by_key(|lot| {
            let (cx, cz) = lot.rect.center();
            (cx - x).abs() + (cz - z).abs()
        })
}

fn place_mill_blades(sink: &mut impl BlockSink, x: i32, z: i32, cx: i32, cz: i32, floor: i32) {
    let dx = x - cx;
    let dz = z - cz;
    let hub_y = mill_hub_y(floor);
    if dx == 0 && dz == 0 {
        sink.put(x, hub_y - 1, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, hub_y, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, hub_y + 1, z, Block::DARK_OAK_LOG.default_state);
        return;
    }
    if !on_mill_blade(dx, dz) {
        return;
    }
    sink.put(x, hub_y, z, Block::OAK_LOG.default_state);
    if dx.abs() != dz.abs() {
        sink.put(x, hub_y - 1, z, Block::OAK_LOG.default_state);
        sink.put(x, hub_y + 1, z, Block::OAK_LOG.default_state);
        return;
    }
    let dist = dx.abs();
    for dy in -3i32..=3 {
        if dy == 0 {
            continue;
        }
        let frame = dy.abs() == 3 || dy.abs() == 1;
        sink.put(
            x,
            hub_y + dy,
            z,
            if frame {
                Block::OAK_TRAPDOOR.default_state
            } else {
                Block::WHITE_WOOL.default_state
            },
        );
    }
    if dist >= 9 {
        sink.put(x, hub_y + 4, z, Block::WHITE_BANNER.default_state);
    }
}

fn place_wind_sail(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 32, z);
    let Some(mill) = nearest_mill(plan, x, z) else {
        return;
    };
    let (cx, cz) = mill.rect.center();
    place_mill_blades(sink, x, z, cx, cz, mill_floor(plan, mill));
}

fn place_arcade(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    let court = Dir::ALL.iter().any(|dir| {
        matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::Green | CellKind::Plaza | CellKind::Fountain | CellKind::Well
        )
    });
    let pillar = (x + z) % 2 == 0;
    if court {
        if pillar {
            for y in ground + 1..=ground + 3 {
                sink.put(x, y, z, palette.bricks);
            }
        } else {
            clear_above(sink, x, ground + 1, ground + 2, z);
            sink.put(
                x,
                ground + 3,
                z,
                stair_state(palette.brick_stairs, Dir::South),
            );
        }
    } else {
        for y in ground + 1..=ground + 3 {
            sink.put(x, y, z, palette.bricks);
        }
    }
    sink.put(x, ground + 4, z, palette.brick_wall);
    sink.put(x, ground + 5, z, palette.slab);
}

fn place_lychgate(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    let rect = blob_of(plan, x, z, CellKind::Lychgate);
    if rect.is_corner(x, z) || (rect.on_edge(x, z) && (x == rect.min_x || x == rect.max_x)) {
        for y in ground + 1..=ground + 3 {
            sink.put(x, y, z, palette.log);
        }
        sink.put(x, ground + 4, z, stair_state(palette.roof, Dir::South));
    } else {
        sink.put(x, ground + 1, z, Block::OAK_FENCE_GATE.default_state);
        clear_above(sink, x, ground + 2, ground + 3, z);
        sink.put(x, ground + 4, z, palette.planks);
    }
    sink.put(x, ground + 5, z, palette.roof_slab);
}

fn place_fortress_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let rect = blob_of(plan, x, z, CellKind::FortressTower);
    let top = floor + 14;
    if rect.on_edge(x, z) {
        for y in floor..=top {
            sink.put(
                x,
                y,
                z,
                if y <= floor + 2 {
                    palette.cobble
                } else {
                    palette.masonry(((x + y) as u8) & 15)
                },
            );
        }
        if !rect.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 5, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 9, z, Block::IRON_BARS.default_state);
        }
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.masonry(2));
            sink.put(x, top + 2, z, palette.brick_wall);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, floor + 4, z, palette.planks);
        sink.put(x, floor + 8, z, palette.planks);
        sink.put(x, top, z, palette.path);
        if x == rect.center().0 && z == rect.center().1 {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 5, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
        }
    }
}

fn place_ruin(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, ground, ground, z, palette);
    let mix = (x.wrapping_mul(13) + z.wrapping_mul(7) + plan.seed as i32).unsigned_abs();
    let edge = Dir::ALL
        .iter()
        .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::Ruin);
    if edge && mix % 5 != 0 {
        let h = 2 + (mix % 4) as i32;
        for y in ground..=ground + h {
            sink.put(
                x,
                y,
                z,
                if mix % 3 == 0 {
                    palette.mossy
                } else {
                    palette.cobble
                },
            );
        }
        if mix % 7 == 0 {
            sink.put(x, ground + h + 1, z, Block::VINE.default_state);
        }
    } else {
        sink.put(x, ground, z, palette.grass);
        if mix % 11 == 0 {
            sink.put(x, ground + 1, z, palette.cobble);
        }
    }
}

fn ruin_mix_block(palette: &Palette, mix: u32) -> &'static BlockState {
    match mix % 5 {
        0 => palette.mossy,
        1 => palette.mossy_bricks,
        2 => palette.andesite,
        _ => palette.cobble,
    }
}

fn place_abbey_ruin(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, ground, ground, z, palette);
    let blob = blob_of(plan, x, z, CellKind::Ruin);
    let mix = (x.wrapping_mul(13) + z.wrapping_mul(7) + plan.seed as i32).unsigned_abs();
    let stone = ruin_mix_block(palette, mix);
    let wide = blob.width();
    let deep = blob.depth();
    if wide <= 2 && deep <= 2 {
        let h = 3 + (mix % 6) as i32;
        for y in ground..=ground + h {
            sink.put(x, y, z, stone);
        }
        if mix % 3 == 0 {
            sink.put(x, ground + h + 1, z, Block::VINE.default_state);
        }
        clear_above(sink, x, ground + h + 2, ground + 20, z);
        return;
    }
    let tower = wide <= 6 && deep <= 6;
    let gable_x = wide >= deep;
    let west = x == blob.min_x;
    let north = z == blob.min_z;
    let gable = if gable_x {
        west || x == blob.max_x
    } else {
        north || z == blob.max_z
    };
    let along = if gable_x {
        z - blob.min_z
    } else {
        x - blob.min_x
    };
    let span = if gable_x { deep } else { wide };
    let mid = span / 2;
    let from_mid = (along - mid).abs();
    let intact = (gable_x && west) || (!gable_x && north);
    let peak = if tower {
        9
    } else if intact {
        16
    } else {
        11
    };
    let h = if tower {
        (6 + (mix % 4) as i32).max((peak - from_mid * 2).clamp(6, 9))
    } else if gable {
        (peak - from_mid * 2).max(5)
    } else {
        4 + (mix % 5) as i32
    };
    if !gable && !tower && mix % 7 == 0 {
        sink.put(x, ground, z, palette.mossy);
        if mix % 2 == 0 {
            sink.put(x, ground + 1, z, palette.cobble);
        }
        clear_above(sink, x, ground + 2, ground + 20, z);
        return;
    }
    for y in ground..=ground + h {
        let window = gable && !tower && intact && from_mid <= 2;
        let opening = window
            && match from_mid {
                0 => y >= ground + 4 && y <= ground + h - 3,
                1 => y >= ground + 5 && y <= ground + h - 5,
                _ => y >= ground + 8 && y <= ground + h - 7,
            };
        if opening {
            if from_mid == 0 && (y - ground) % 3 == 0 {
                sink.put(x, y, z, Block::IRON_BARS.default_state);
            } else {
                sink.put(x, y, z, Block::AIR.default_state);
            }
        } else {
            sink.put(x, y, z, stone);
        }
    }
    if mix % 2 == 0 {
        sink.put(x, ground + h + 1, z, Block::VINE.default_state);
    } else if mix % 5 == 0 {
        sink.put(x, ground + h + 1, z, palette.brick_wall);
    }
    clear_above(sink, x, ground + h + 2, ground + 22, z);
}

fn place_castle_ruin_mass(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, ground, ground, z, palette);
    let blob = blob_of(plan, x, z, CellKind::Ruin);
    let mix = (x.wrapping_mul(11) + z.wrapping_mul(17) + plan.seed as i32).unsigned_abs();
    let stone = ruin_mix_block(palette, mix);
    let keep = blob.width() <= 9 && blob.depth() <= 9;
    let corner = blob.is_corner(x, z);
    let h = if !keep && mix % 8 == 0 {
        3
    } else if keep || corner {
        8 + (mix % 4) as i32
    } else {
        5 + (mix % 4) as i32
    };
    for y in ground..=ground + h {
        sink.put(x, y, z, stone);
    }
    if (x + z) % 2 == 0 {
        sink.put(x, ground + h + 1, z, palette.brick_wall);
    }
    if mix % 3 == 0 {
        sink.put(x, ground + h + 1, z, Block::VINE.default_state);
    }
    if mix % 11 == 0 {
        sink.put(x, ground + 1, z, Block::IRON_BARS.default_state);
    }
    clear_above(sink, x, ground + h + 2, ground + 18, z);
}

fn place_abbey_arcade(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.path);
    let mix = (x.wrapping_mul(9) + z.wrapping_mul(5)).unsigned_abs();
    let h = 3 + (mix % 6) as i32;
    let stone = ruin_mix_block(palette, mix);
    for y in ground + 1..=ground + h {
        sink.put(x, y, z, stone);
    }
    if mix % 2 == 0 {
        sink.put(x, ground + h + 1, z, Block::VINE.default_state);
    }
    clear_above(sink, x, ground + h + 2, ground + 12, z);
}

fn place_abbey_floor(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    let mix = (x.wrapping_mul(17) + z.wrapping_mul(31)).unsigned_abs();
    sink.put(
        x,
        ground,
        z,
        match mix % 7 {
            0 | 1 => palette.mossy,
            2 => palette.cobble,
            3 => Block::COARSE_DIRT.default_state,
            _ => palette.grass,
        },
    );
    clear_above(sink, x, ground + 1, ground + 16, z);
    match mix % 11 {
        0 => sink.put(x, ground + 1, z, palette.cobble),
        1 => sink.put(x, ground + 1, z, palette.mossy),
        2 => {
            sink.put(x, ground + 1, z, Block::VINE.default_state);
            sink.put(x, ground + 2, z, Block::VINE.default_state);
        }
        3 => sink.put(x, ground + 1, z, Block::FERN.default_state),
        _ => {}
    }
}

fn place_castle_ruin_yard(
    sink: &mut impl BlockSink,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let mix = (x.wrapping_mul(19) + z.wrapping_mul(13)).unsigned_abs();
    sink.put(
        x,
        ground,
        z,
        match mix % 6 {
            0 => palette.cobble,
            1 => palette.mossy,
            2 => Block::COARSE_DIRT.default_state,
            _ => palette.grass,
        },
    );
    clear_above(sink, x, ground + 1, ground + 10, z);
    if mix % 9 == 0 {
        sink.put(x, ground + 1, z, palette.cobble);
        sink.put(x, ground + 2, z, palette.andesite);
    } else if mix % 7 == 0 {
        sink.put(x, ground + 1, z, Block::VINE.default_state);
    }
}

fn place_bandit_lookout(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::WatchPost);
    let mut base = ground;
    for zz in rect.min_z..=rect.max_z {
        for xx in rect.min_x..=rect.max_x {
            base = base.max(plan.heights.height(xx, zz) - 1);
        }
    }
    let loft = base + 5;
    let post = rect.is_corner(x, z);
    sink.put(x, ground, z, palette.path);
    if post {
        for y in ground + 1..=loft {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
        }
        sink.put(x, loft + 1, z, Block::DARK_OAK_FENCE.default_state);
        if x == rect.min_x {
            sink.put(x, loft + 2, z, Block::BLACK_BANNER.default_state);
        } else {
            sink.put(x, loft + 2, z, Block::LANTERN.default_state);
        }
        return;
    }
    for y in ground + 1..loft {
        sink.put(x, y, z, Block::AIR.default_state);
    }
    sink.put(x, loft, z, Block::DARK_OAK_PLANKS.default_state);
    sink.put(x, loft + 1, z, Block::DARK_OAK_FENCE.default_state);
    if x == rect.min_x {
        for y in ground + 1..=loft {
            sink.put(x, y, z, Block::LADDER.default_state);
        }
    }
}

fn place_burnt(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, ground, ground, z, palette);
    let mix = (x.wrapping_mul(19) + z.wrapping_mul(11) + plan.seed as i32).unsigned_abs();
    let edge = Dir::ALL
        .iter()
        .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::Burnt);
    sink.put(x, ground, z, Block::COAL_BLOCK.default_state);
    if edge && mix % 4 != 0 {
        let h = 1 + (mix % 3) as i32;
        for y in ground + 1..=ground + h {
            sink.put(
                x,
                y,
                z,
                if mix % 2 == 0 {
                    Block::DARK_OAK_PLANKS.default_state
                } else {
                    palette.cobble
                },
            );
        }
        if mix % 5 == 0 {
            sink.put(x, ground + h + 1, z, Block::CAMPFIRE.default_state);
        }
    } else if mix % 7 == 0 {
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
    } else if mix % 5 == 0 {
        sink.put(x, ground + 1, z, Block::COBBLESTONE.default_state);
    }
    clear_above(sink, x, ground + 4, ground + 12, z);
}

fn place_mine_head(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::MineHead);
    let (cx, cz) = rect.center();
    let top = ground + 12;
    foundation(sink, x, ground, ground, z, palette);
    if rect.is_corner(x, z) {
        for y in ground + 1..=top {
            sink.put(x, y, z, Block::OAK_LOG.default_state);
        }
        sink.put(x, top + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, top + 2, z, Block::LANTERN.default_state);
        return;
    }
    if rect.on_edge(x, z) {
        sink.put(x, ground, z, palette.path);
        clear_above(sink, x, ground + 1, top - 1, z);
        if x == cx || z == cz {
            for y in top - 1..=top {
                sink.put(x, y, z, Block::OAK_LOG.default_state);
            }
            sink.put(x, top + 1, z, Block::OAK_FENCE.default_state);
        }
        return;
    }
    for y in (ground - 2).max(sink.floor_min() + 1)..ground {
        sink.put(x, y, z, palette.cobble);
    }
    clear_above(sink, x, ground, top - 1, z);
    if x == cx || z == cz {
        sink.put(x, top, z, Block::OAK_LOG.default_state);
    }
    if x == cx && z == cz {
        sink.put(x, ground - 1, z, Block::AIR.default_state);
        sink.put(x, ground, z, Block::AIR.default_state);
        for y in (ground - 1)..top {
            sink.put(x, y, z, Block::LADDER.default_state);
        }
        sink.put(x, top + 1, z, Block::OAK_LOG.default_state);
        sink.put(x, top + 2, z, Block::OAK_FENCE.default_state);
        sink.put(x, top + 3, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, top + 4, z, Block::LANTERN.default_state);
        return;
    }
    let manh = (x - cx).abs() + (z - cz).abs();
    if manh == 1 {
        sink.put(x, top + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, top + 2, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, top + 3, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, top + 4, z, Block::OAK_FENCE.default_state);
    }
    if manh == 2 {
        sink.put(x, top + 2, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, top + 3, z, Block::OAK_FENCE.default_state);
    }
}

fn place_mine_adit(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::MineAdit);
    let (cx, cz) = rect.center();
    foundation(sink, x, ground, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 6, z);
    if rect.is_corner(x, z) {
        for y in ground + 1..=ground + 4 {
            sink.put(x, y, z, Block::OAK_LOG.default_state);
        }
        sink.put(x, ground + 5, z, Block::OAK_LOG.default_state);
        return;
    }
    if rect.on_edge(x, z) {
        sink.put(x, ground, z, Block::OAK_PLANKS.default_state);
        sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, ground + 4, z, Block::OAK_LOG.default_state);
        sink.put(x, ground + 5, z, Block::OAK_SLAB.default_state);
        if x == cx || z == cz {
            interior::place_door(sink, x, ground + 1, z, interior::outward(rect, x, z), false);
        }
        return;
    }
    sink.put(x, ground, z, palette.cobble);
    sink.put(x, ground + 1, z, Block::RAIL.default_state);
    clear_above(sink, x, ground + 2, ground + 4, z);
    if x == cx && z == cz {
        sink.put(x, ground + 1, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, ground + 2, z, Block::CHEST.default_state);
    }
}

fn place_mine_rail(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.cobble);
    sink.put(x, ground + 1, z, Block::RAIL.default_state);
    clear_above(sink, x, ground + 2, ground + 5, z);
    if (x + z).rem_euclid(4) == 0 {
        sink.put(x, ground + 1, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, ground + 2, z, Block::OAK_SLAB.default_state);
        sink.put(x, ground + 3, z, Block::CHEST.default_state);
    }
}

fn place_mine_spoil(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::SiegeCamp)
    {
        sink.put(x, ground, z, Block::COARSE_DIRT.default_state);
        sink.put(x, ground + 1, z, Block::DIRT.default_state);
        sink.put(x, ground + 2, z, palette.grass);
        if (x + z) % 4 == 0 {
            sink.put(x, ground + 2, z, palette.cobble);
        }
        clear_above(sink, x, ground + 3, ground + 6, z);
        return;
    }
    let mix = (x.wrapping_mul(9) + z.wrapping_mul(5) + plan.seed as i32).unsigned_abs();
    let h = 1 + (mix % 3) as i32;
    for y in ground..=ground + h {
        sink.put(
            x,
            y,
            z,
            if mix % 2 == 0 {
                Block::GRAVEL.default_state
            } else {
                palette.cobble
            },
        );
    }
    clear_above(sink, x, ground + h + 1, ground + h + 2, z);
}

fn place_hermit_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::WatchPost);
    let (cx, cz) = rect.center();
    let dx = x - cx;
    let dz = z - cz;
    let dist2 = dx * dx + dz * dz;
    let top = floor + 16;
    foundation(sink, x, floor, ground, z, palette);
    if dist2 > 5 {
        sink.put(x, floor, z, palette.cobble);
        if dist2 <= 8 {
            sink.put(x, floor + 1, z, palette.brick_wall);
            if dz >= 1 {
                sink.put(x, floor + 1, z, Block::OAK_FENCE.default_state);
            }
        }
        return;
    }
    let wall = dist2 >= 4;
    let mix = (x.wrapping_mul(7) + z.wrapping_mul(11)).unsigned_abs();
    if wall {
        let stone = if mix % 3 == 0 {
            palette.mossy
        } else {
            palette.cobble
        };
        for y in ground.min(floor)..=top {
            sink.put(x, y, z, stone);
        }
        let door = dx == 0 && dz == 2;
        if door {
            interior::place_door(sink, x, floor + 1, z, Dir::South, true);
            sink.put(x, floor + 3, z, stone);
        } else if (x + z) % 2 == 0 {
            sink.put(x, floor + 8, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 12, z, Block::GLASS_PANE.default_state);
        }
        if dx == 1 && dz == 2 {
            for y in top + 1..=top + 4 {
                sink.put(x, y, z, palette.bricks);
            }
            sink.put(x, top + 5, z, Block::CAMPFIRE.default_state);
        } else {
            sink.put(x, top + 1, z, palette.brick_wall);
            sink.put(x, top + 2, z, Block::SPRUCE_SLAB.default_state);
        }
        if dz == 2 && dx == -1 {
            sink.put(x, top + 1, z, Block::SPRUCE_PLANKS.default_state);
            sink.put(x, top + 2, z, Block::SPRUCE_FENCE.default_state);
            sink.put(x, top + 3, z, Block::LANTERN.default_state);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, floor + 6, z, palette.planks);
        sink.put(x, floor + 11, z, palette.planks);
        sink.put(x, top, z, palette.planks);
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 7, z, Block::LADDER.default_state);
            sink.put(x, floor + 12, z, Block::LADDER.default_state);
            sink.put(x, top + 1, z, Block::LANTERN.default_state);
        } else if dx == 1 && dz == 0 {
            sink.put(x, floor + 2, z, Block::RED_BED.default_state);
        } else if dx == -1 && dz == 0 {
            sink.put(x, floor + 8, z, Block::BOOKSHELF.default_state);
            sink.put(x, floor + 9, z, Block::BOOKSHELF.default_state);
        } else if dx == 0 && dz == -1 {
            sink.put(x, floor + 12, z, Block::CRAFTING_TABLE.default_state);
        }
    }
}

fn place_standing_stone(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let fallen = matches!(plan.occ.kind(x + 1, z), CellKind::StandingStone)
        || matches!(plan.occ.kind(x - 1, z), CellKind::StandingStone);
    if fallen {
        sink.put(x, ground, z, palette.cobble);
        sink.put(x, ground + 1, z, palette.andesite);
        sink.put(x, ground + 2, z, Block::STONE_SLAB.default_state);
        clear_above(sink, x, ground + 3, ground + 8, z);
        return;
    }
    let inner = (-2i32..=2).any(|dx| {
        (-2i32..=2).any(|dz| {
            dx.abs() + dz.abs() <= 2 && matches!(plan.occ.kind(x + dx, z + dz), CellKind::Fountain)
        })
    });
    let mix = (x.wrapping_mul(5) + z.wrapping_mul(9) + plan.seed as i32).unsigned_abs();
    let h = if inner {
        3 + (mix % 2) as i32
    } else {
        4 + (mix % 4) as i32
    };
    for y in ground..=ground + h {
        sink.put(
            x,
            y,
            z,
            if y == ground {
                palette.mossy
            } else if (x + z + y) % 2 == 0 {
                palette.andesite
            } else {
                Block::STONE.default_state
            },
        );
    }
    if mix % 3 == 0 {
        sink.put(x, ground + h + 1, z, Block::VINE.default_state);
    }
}

fn place_gallows(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    for y in ground..=ground + 4 {
        sink.put(x, y, z, palette.log);
    }
    sink.put(x, ground + 5, z, palette.log);
    sink.put(x + 1, ground + 5, z, palette.log);
    sink.put(x + 1, ground + 4, z, Block::IRON_CHAIN.default_state);
    sink.put(x + 1, ground + 3, z, Block::OAK_FENCE.default_state);
    sink.put(x, ground + 1, z, palette.cobble);
}

fn place_watch_post(
    sink: &mut impl BlockSink,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    for y in floor..=floor + 8 {
        sink.put(x, y, z, palette.cobble);
    }
    sink.put(x, floor + 9, z, palette.brick_wall);
    sink.put(x, floor + 10, z, Block::LANTERN.default_state);
}

fn place_forge_yard(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 4, z);
    match (x + z * 3).unsigned_abs() % 11 {
        0 => sink.put(x, ground + 1, z, Block::ANVIL.default_state),
        1 => sink.put(x, ground + 1, z, Block::BLAST_FURNACE.default_state),
        2 => {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, Block::LANTERN.default_state);
        }
        3 => sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state),
        _ => {}
    }
}

fn place_farm(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32, edge: bool) {
    if edge {
        sink.put(x, ground, z, palette_dirt());
        let pasture = Dir::ALL
            .iter()
            .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) == CellKind::Pasture);
        if pasture {
            sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
            return;
        }
        let hedge = (x + z) % 3 != 0;
        if hedge {
            sink.put(x, ground + 1, z, Block::OAK_LEAVES.default_state);
            if (x + z) % 5 == 0 {
                sink.put(x, ground + 2, z, Block::OAK_LEAVES.default_state);
            }
        } else {
            sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        }
        return;
    }
    if (x + z * 3) % 9 == 0 {
        sink.put(x, ground, z, Block::WATER.default_state);
        return;
    }
    if (x % 11 == 0) && (z % 11 == 0) {
        place_scarecrow(sink, x, ground, z);
        return;
    }
    sink.put(x, ground, z, Block::FARMLAND.default_state);
    let mix = ((plan.seed as i32) ^ x.wrapping_mul(13) ^ z.wrapping_mul(29)).unsigned_abs();
    let age = (mix % 8) as u8;
    let crop = match mix % 4 {
        0 => &Block::CARROTS,
        1 => &Block::POTATOES,
        _ => &Block::WHEAT,
    };
    let props = WheatLikeProperties { r#age: age.min(7) };
    sink.put(
        x,
        ground + 1,
        z,
        BlockState::from_id(props.to_state_id(crop)),
    );
    if mix % 23 == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    }
}

fn palette_dirt() -> &'static BlockState {
    Block::DIRT.default_state
}

fn place_scarecrow(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32) {
    sink.put(x, ground, z, Block::FARMLAND.default_state);
    sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
    sink.put(x, ground + 2, z, Block::HAY_BLOCK.default_state);
    sink.put(x, ground + 3, z, Block::CARVED_PUMPKIN.default_state);
}

fn place_well(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let blob = blob_of(plan, x, z, CellKind::Well);
    let (wx, wz) = blob.center();
    let dx = x - wx;
    let dz = z - wz;
    sink.put(x, ground, z, palette.cobble);
    if dx == 0 && dz == 0 {
        sink.put(x, ground, z, MARKER_FLOOR.default_state);
        sink.put(x, ground + 1, z, Block::WATER.default_state);
    } else if dx.abs() <= 1 && dz.abs() <= 1 {
        sink.put(x, ground + 1, z, palette.cobble);
        sink.put(x, ground + 2, z, palette.cobble);
    }
    if dx.abs() == 2 && dz.abs() == 2 {
        sink.put(x, ground + 1, z, palette.log);
        sink.put(x, ground + 2, z, palette.log);
        sink.put(x, ground + 3, z, palette.log);
        sink.put(x, ground + 4, z, palette.slab);
    }
}

fn place_curtain(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    height: i32,
    castle: bool,
) {
    foundation(sink, x, floor, ground, z, palette);
    terrace_edge(sink, plan, x, floor, z, palette);
    let top = floor + height;
    for y in floor..=top {
        let v = ((x.wrapping_mul(3) + z + y) as u8) & 15;
        sink.put(
            x,
            y,
            z,
            if castle {
                palette.masonry(v)
            } else if height >= 8 {
                palette.city_masonry(v)
            } else {
                palette.foundation(v % 5 == 0)
            },
        );
    }
    if castle {
        // Darker plinth: cobble / andesite / tuff, mossy near grade.
        for y in floor..=floor + 2 {
            let mix = ((x.wrapping_mul(5) + z.wrapping_mul(11) + y) as u8) % 5;
            let block = match mix {
                0 | 1 => palette.cobble,
                2 => palette.andesite,
                3 => Block::TUFF.default_state,
                _ => palette.mossy,
            };
            sink.put(x, y, z, block);
        }
        if top >= floor + 4 {
            sink.put(x, floor + 4, z, palette.andesite);
        }
        if top >= floor + 8 {
            sink.put(x, floor + 8, z, palette.andesite);
        }
        if (x + z) % 4 == 0 {
            sink.put(x, floor + 5, z, Block::IRON_BARS.default_state);
        }
        if outer_wall_face(plan, x, z) && (x.wrapping_mul(7) + z) % 11 == 0 {
            sink.put(x, floor + 3, z, Block::VINE.default_state);
        }
    }
    let outer = wall_is_outer(plan, x, z);
    if outer {
        if ((x + z) & 1) == 0 {
            let cap = if castle {
                palette.masonry(1)
            } else {
                palette.city_masonry(1)
            };
            sink.put(x, top + 1, z, cap);
            if height >= 8 {
                sink.put(
                    x,
                    top + 2,
                    z,
                    if castle {
                        palette.masonry(2)
                    } else {
                        palette.city_masonry(2)
                    },
                );
            } else {
                sink.put(x, top + 2, z, palette.brick_wall);
            }
        } else if castle {
            // Same-column corbel: an outward stair standing in for a machicolation.
            if let Some(face) = outer_facing(plan, x, z) {
                sink.put(x, top, z, stair_state(palette.brick_stairs, face));
            }
            sink.put(x, top + 1, z, palette.fence);
            sink.put(x, top + 2, z, palette.slab);
        } else if height >= 8 {
            sink.put(x, top + 1, z, palette.slab);
        }
        if castle && r_banner(x, z) {
            sink.put(
                x,
                top + 3,
                z,
                if (x + z) % 3 == 0 {
                    Block::BLACK_BANNER.default_state
                } else {
                    Block::RED_BANNER.default_state
                },
            );
        }
        if castle && (x + z) % 9 == 0 {
            sink.put(x, top - 1, z, Block::LANTERN.default_state);
        }
    } else {
        sink.put(x, top + 1, z, palette.path);
        if (x + z) % 4 == 0 {
            sink.put(x, top + 2, z, Block::LANTERN.default_state);
        }
    }
}

fn r_banner(x: i32, z: i32) -> bool {
    (x + z * 3) % 13 == 0
}

fn outer_wall_face(plan: &TownPlan, x: i32, z: i32) -> bool {
    wall_is_outer(plan, x, z)
}

fn wall_is_outer(plan: &TownPlan, x: i32, z: i32) -> bool {
    Dir::ALL.iter().any(|dir| {
        matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::Empty
                | CellKind::Harbor
                | CellKind::Dock
                | CellKind::CastleMoat
                | CellKind::Farm
                | CellKind::FarmEdge
                | CellKind::Orchard
                | CellKind::Pasture
                | CellKind::CityDitch
                | CellKind::Grave
                | CellKind::Ruin
                | CellKind::Palisade
                | CellKind::Tent
                | CellKind::WarTent
                | CellKind::CampCourt
                | CellKind::Arcade
                | CellKind::Lychgate
                | CellKind::FortressCourt
                | CellKind::WindSail
                | CellKind::MineHead
                | CellKind::MineRail
                | CellKind::MineSpoil
                | CellKind::Ship
                | CellKind::TourneyList
                | CellKind::TourneyStand
                | CellKind::Burnt
        )
    })
}

fn outer_facing(plan: &TownPlan, x: i32, z: i32) -> Option<Dir> {
    Dir::ALL.iter().copied().find(|dir| {
        matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::Empty
                | CellKind::Harbor
                | CellKind::Dock
                | CellKind::CastleMoat
                | CellKind::Farm
                | CellKind::FarmEdge
                | CellKind::Orchard
                | CellKind::Pasture
                | CellKind::CityDitch
        )
    })
}

fn place_round_city_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let top = floor + 22;
    for y in floor..=top {
        sink.put(
            x,
            y,
            z,
            palette.city_masonry(((x.wrapping_mul(5) + y) as u8) & 15),
        );
    }
    let rim = Dir::ALL
        .iter()
        .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::TownTower);
    if rim {
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.city_masonry(2));
            sink.put(x, top + 2, z, palette.brick_wall);
        }
        if (x + z) % 3 == 0 {
            sink.put(x, floor + 8, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 14, z, Block::IRON_BARS.default_state);
        }
        if (x + z) % 11 == 0 {
            sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
        }
    } else {
        sink.put(x, top + 1, z, palette.path);
        let (cx, cz) = blob_of(plan, x, z, CellKind::TownTower).center();
        if x == cx && z == cz {
            sink.put(x, top + 2, z, Block::LANTERN.default_state);
        }
    }
}

fn place_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    half: i32,
    height: i32,
) {
    foundation(sink, x, floor, ground, z, palette);
    let top = floor + height;
    for y in floor..=top {
        sink.put(
            x,
            y,
            z,
            palette.masonry(((x.wrapping_mul(5) + y) as u8) & 15),
        );
    }
    let lx = (x & 7) - 3;
    let lz = (z & 7) - 3;
    let ring = lx.abs().max(lz.abs());
    if ring <= half / 2 {
        sink.put(x, top + 1, z, palette.bricks);
        if ring <= 1 {
            sink.put(x, top + 2, z, stair_state(palette.brick_stairs, Dir::South));
        }
        if ring == 0 {
            sink.put(x, top + 3, z, Block::LANTERN.default_state);
        }
    }
    if ((x + z) & 1) == 0 {
        sink.put(x, top + 1, z, palette.masonry(2));
        sink.put(x, top + 2, z, palette.brick_wall);
    }
    if (x + z) % 3 == 0 {
        sink.put(x, floor + 5, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 10, z, Block::IRON_BARS.default_state);
    }
    if (x + z) % 11 == 0 {
        sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
    }
    let _ = plan;
}

fn place_shaped_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    spec: TowerSpec,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let floor = ridge_floor(plan, spec.bounds());
    foundation(sink, x, floor, ground, z, palette);
    let top = floor + spec.height;
    let rim = on_shell(spec, x, z);
    let (roof_y, _) = mass_roof_y(spec, x, z, top);
    let cellar = spec.inner && spec.height >= 16 && floor >= plan.heights.sea_level + 8;
    if cellar && rim {
        for y in floor - 6..floor {
            sink.put(
                x,
                y,
                z,
                palette.masonry(((x.wrapping_mul(5) + y) as u8) & 15),
            );
        }
    }
    if rim {
        let wall_top = roof_y.saturating_sub(1).max(top);
        for y in floor..=wall_top {
            let block = if y <= floor + 2 {
                match ((x + y) as u8) % 4 {
                    0 => palette.andesite,
                    1 => Block::TUFF.default_state,
                    _ => palette.cobble,
                }
            } else if y == floor + 4 || y == floor + 8 {
                palette.andesite
            } else {
                palette.masonry(((x.wrapping_mul(5) + y) as u8) & 15)
            };
            sink.put(x, y, z, block);
        }
        if (x + z) % 3 == 0 {
            sink.put(
                x,
                floor + spec.height / 3,
                z,
                Block::IRON_BARS.default_state,
            );
            sink.put(
                x,
                floor + spec.height * 2 / 3,
                z,
                Block::IRON_BARS.default_state,
            );
        }
        if spec.inner && spec.radius >= 3 && (x + z) % 5 == 0 {
            sink.put(x, floor + 6, z, Block::GLASS_PANE.default_state);
        }
        if keep_door_at(plan, spec, x, z) {
            interior::place_door(sink, x, floor + 1, z, keep_door_facing(plan, spec), true);
        }
        interior::rim_sconce(sink, x, floor + 3, z);
    } else {
        if cellar {
            sink.put(x, floor - 6, z, palette.cobble);
            clear_above(sink, x, floor - 5, floor - 1, z);
        }
        clear_above(sink, x, floor + 1, top - 1, z);
        for s in 0..=spec.height / 4 {
            let y = floor + s * 4;
            if y < top {
                sink.put(x, y, z, palette.planks);
            }
        }
        sink.put(x, top, z, palette.planks);
        if matches!(spec.roof, RoofKind::Crenellated) {
            sink.put(x, top, z, palette.path);
        }
        interior::furnish_keep_mass(sink, spec, x, floor, z, top, palette, cellar);
    }
    place_mass_roof(sink, spec, x, z, top, palette);
    if rim && matches!(spec.roof, RoofKind::Crenellated) && ((x + z) & 1) == 0 {
        sink.put(x, top + 1, z, palette.masonry(2));
        sink.put(x, top + 2, z, palette.brick_wall);
    }
    if rim && spec.radius >= 4 && (x - spec.cx).abs() + (z - spec.cz).abs() == spec.radius {
        sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
    }
}

fn on_shell(spec: TowerSpec, x: i32, z: i32) -> bool {
    spec.contains(x, z)
        && Dir::ALL
            .iter()
            .any(|dir| !spec.contains(x + dir.dx(), z + dir.dz()))
}

fn keep_door_at(plan: &TownPlan, spec: TowerSpec, x: i32, z: i32) -> bool {
    if !on_shell(spec, x, z) {
        return false;
    }
    let (hx, hz) = plan.castle.hall.center();
    let tx = (hx - spec.cx).signum();
    let tz = (hz - spec.cz).signum();
    if tz.abs() >= tx.abs() {
        x == spec.cx && z == spec.cz + tz * spec.radius
    } else {
        z == spec.cz && x == spec.cx + tx * spec.radius
    }
}

fn keep_door_facing(plan: &TownPlan, spec: TowerSpec) -> Dir {
    let (hx, hz) = plan.castle.hall.center();
    let dx = hx - spec.cx;
    let dz = hz - spec.cz;
    if dz.abs() >= dx.abs() {
        if dz >= 0 { Dir::South } else { Dir::North }
    } else if dx >= 0 {
        Dir::East
    } else {
        Dir::West
    }
}

fn mass_roof_y(spec: TowerSpec, x: i32, z: i32, top: i32) -> (i32, Dir) {
    let dx = x - spec.cx;
    let dz = z - spec.cz;
    let dist = isqrt(dx, dz);
    // Chebyshev keeps the four square-pyramid faces the same height.
    // Euclidean dist made diagonal eaves sit lower than cardinal ones, so a
    // SE view showed two tall far faces behind a stepped near half (a V).
    let cheb = dx.abs().max(dz.abs());
    let out = if dx.abs() >= dz.abs() {
        if dx < 0 { Dir::West } else { Dir::East }
    } else if dz < 0 {
        Dir::North
    } else {
        Dir::South
    };
    match spec.roof {
        RoofKind::Crenellated => (top, out),
        RoofKind::Cone => (top + 1 + (spec.radius - dist).max(0), out),
        RoofKind::Spire => (top + 1 + (spec.radius - cheb).max(0) * 2, out),
        RoofKind::Hip => (top + 1 + (spec.radius - dist).clamp(0, 8), out),
        RoofKind::Gable { along_x } => {
            let ridge = if along_x { spec.cz } else { spec.cx };
            let d = if along_x {
                (z - ridge).abs()
            } else {
                (x - ridge).abs()
            };
            let dir = if along_x {
                if z < ridge { Dir::North } else { Dir::South }
            } else if x < ridge {
                Dir::West
            } else {
                Dir::East
            };
            (top + 1 + (spec.radius - d).clamp(0, 8), dir)
        }
    }
}

fn roof_cap_block(palette: &Palette) -> &'static BlockState {
    match palette.kind {
        PaletteKind::Oak => Block::DARK_OAK_PLANKS.default_state,
        PaletteKind::Spruce => Block::SPRUCE_PLANKS.default_state,
        PaletteKind::Sandstone => Block::SMOOTH_SANDSTONE.default_state,
    }
}

fn mass_roof_needs_cap(spec: TowerSpec, x: i32, z: i32) -> bool {
    let dx = x - spec.cx;
    let dz = z - spec.cz;
    let dist = isqrt(dx, dz);
    match spec.roof {
        RoofKind::Crenellated => false,
        RoofKind::Hip => {
            dist <= spec.radius.saturating_sub(6).max(3)
                || (dx.abs().min(dz.abs()) == 0 && dist <= spec.radius.min(6))
        }
        RoofKind::Gable { along_x } => {
            let d = if along_x {
                (z - spec.cz).abs()
            } else {
                (x - spec.cx).abs()
            };
            d <= 2
        }
        RoofKind::Cone => dist <= 2,
        RoofKind::Spire => dx.abs().max(dz.abs()) <= 1,
    }
}

fn place_mass_roof(
    sink: &mut impl BlockSink,
    spec: TowerSpec,
    x: i32,
    z: i32,
    top: i32,
    palette: &Palette,
) {
    let (roof_y, dir) = mass_roof_y(spec, x, z, top);
    match spec.roof {
        RoofKind::Crenellated => {}
        RoofKind::Spire => place_spire_pyramid(sink, spec, x, z, top, roof_y, dir, palette),
        RoofKind::Cone | RoofKind::Hip | RoofKind::Gable { .. } => {
            let cap = roof_cap_block(palette);
            if on_shell(spec, x, z) {
                if mass_roof_needs_cap(spec, x, z) {
                    sink.put(x, roof_y, z, cap);
                } else {
                    sink.put(x, roof_y, z, stair_state(palette.roof, dir));
                }
            } else {
                // Fill the pyramid so a hip cannot leave a sky well with
                // oak rafters showing between the two slopes.
                for y in top + 1..=roof_y {
                    sink.put(x, y, z, cap);
                }
            }
        }
    }
}

/// Square-pyramid keep roof: stairs on every Chebyshev ring, equal faces, a
/// short apex finial. Interior fill under the stairs so the mass stays sealed.
fn place_spire_pyramid(
    sink: &mut impl BlockSink,
    spec: TowerSpec,
    x: i32,
    z: i32,
    top: i32,
    roof_y: i32,
    dir: Dir,
    palette: &Palette,
) {
    let dx = x - spec.cx;
    let dz = z - spec.cz;
    let dist = dx.abs().max(dz.abs());
    if dist > spec.radius {
        return;
    }
    let cap = roof_cap_block(palette);
    for y in top + 1..roof_y {
        sink.put(x, y, z, cap);
    }
    if dist <= 1 {
        // A 3×3 cap under the finial so the peak is a ridge block, not a
        // slot of outward stairs facing each other (open-ridge check).
        sink.put(x, roof_y, z, cap);
        if dist == 0 {
            sink.put(x, roof_y + 1, z, palette.bricks);
            sink.put(x, roof_y + 2, z, palette.brick_wall);
            sink.put(x, roof_y + 3, z, Block::LANTERN.default_state);
        }
    } else {
        sink.put(x, roof_y, z, stair_state(palette.roof, dir));
    }
}

fn place_gate(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    height: i32,
) {
    foundation(sink, x, floor, ground, z, palette);
    let opening = (x + z) % 3 != 0;
    if opening {
        sink.put(x, floor, z, palette.path);
        clear_above(sink, x, floor + 1, floor + 3, z);
        sink.put(x, floor + 1, z, Block::IRON_BARS.default_state);
        for y in floor + 4..=floor + height {
            sink.put(x, y, z, palette.masonry(1));
        }
    } else {
        place_curtain(sink, plan, x, floor, ground, z, palette, height, false);
    }
}

fn place_gatehouse(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _column_floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let r = plan.castle.gatehouse;
    let floor = ridge_floor(plan, r);
    foundation(sink, x, floor, ground, z, palette);
    let (cx, cz) = r.center();
    let passage = match plan.castle.gate_dir {
        Dir::North | Dir::South => (x - cx).abs() <= 2,
        Dir::East | Dir::West => (z - cz).abs() <= 2,
    };
    if passage {
        sink.put(x, floor, z, palette.path);
        clear_above(sink, x, floor + 1, floor + 4, z);
        sink.put(x, floor + 1, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 2, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 3, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 4, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 5, z, palette.masonry(2));
        let enclosed = Dir::ALL
            .iter()
            .all(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()).is_shell());
        let core = enclosed;
        sink.put(x, floor + 6, z, palette.planks);
        if core {
            clear_above(sink, x, floor + 7, floor + 10, z);
            sink.put(x, floor + 6, z, palette.planks);
            interior::furnish_winch(sink, x, floor, z, enclosed);
            sink.put(x, floor + 11, z, palette.planks);
            sink.put(x, floor + 12, z, palette.masonry(2));
        } else {
            for y in floor + 7..=floor + 12 {
                sink.put(x, y, z, palette.masonry(2));
            }
        }
        sink.put(x, floor + 13, z, palette.brick_wall);
        if (x - cx).abs() + (z - cz).abs() == 0 {
            sink.put(
                x,
                floor.max(plan.heights.sea_level),
                z,
                Block::SPRUCE_PLANKS.default_state,
            );
        }
        return;
    }
    for y in floor..=floor + 12 {
        sink.put(x, y, z, palette.masonry(((x + y) as u8) & 15));
    }
    if ((x + z) & 1) == 0 {
        sink.put(x, floor + 13, z, palette.masonry(1));
        sink.put(x, floor + 14, z, palette.brick_wall);
    }
}

fn place_barbican(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let spec = TowerSpec {
        cx: plan.castle.barbican.center().0,
        cz: plan.castle.barbican.center().1,
        radius: 5,
        height: 10,
        shape: TowerShape::DShape {
            flat: plan.castle.gate_dir.opposite(),
        },
        roof: RoofKind::Crenellated,
        inner: false,
    };
    if spec.contains(x, z) {
        place_shaped_tower(sink, plan, spec, x, ground, z, palette);
    } else {
        foundation(sink, x, floor, ground, z, palette);
        for y in floor..=floor + 7 {
            sink.put(x, y, z, palette.masonry(3));
        }
        if ((x + z) & 1) == 0 {
            sink.put(x, floor + 8, z, palette.brick_wall);
        }
    }
}

fn place_keep(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _column_floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if let Some(mass) = plan.castle.keep_mass_at(x, z) {
        place_shaped_tower(sink, plan, mass.spec, x, ground, z, palette);
        return;
    }
    let r = plan.castle.keep;
    let floor = ridge_floor(plan, r);
    foundation(sink, x, floor, ground, z, palette);
    let edge = r.on_edge(x, z);
    let top = floor + 16;
    if edge {
        let (roof_y, _) = gable_roof_y(r, x, z, top, 1);
        for y in floor..roof_y {
            sink.put(
                x,
                y,
                z,
                palette.masonry(((x.wrapping_mul(3) + y) as u8) & 15),
            );
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), true);
        }
        place_roof(sink, r, x, top, z, palette, 1);
        return;
    }
    let (roof_y, _) = gable_roof_y(r, x, z, top, 1);
    clear_above(sink, x, floor + 1, roof_y - 1, z);
    for s in 0..=4 {
        let y = floor + s * 4;
        if y < roof_y {
            sink.put(x, y, z, palette.planks);
        }
    }
    place_roof(sink, r, x, top, z, palette, 1);
}

fn place_hall(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _column_floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let hall = plan.castle.hall;
    let floor = ridge_floor(plan, hall);
    if !hall.contains(x, z) {
        foundation(sink, x, floor, ground, z, palette);
        let (roof_y, _) = gable_roof_y(hall, x, z, floor + 8, 1);
        for y in floor..roof_y {
            sink.put(x, y, z, palette.bricks);
        }
        place_roof(sink, hall, x, floor + 8, z, palette, 1);
        return;
    }
    place_gabled(sink, hall, x, floor, ground, z, palette, 8, true);
    let (roof_y, _) = gable_roof_y(hall, x, z, floor + 8, 1);
    interior::furnish_hall(sink, hall, x, floor, z, roof_y, palette);
}

fn place_chapel(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _column_floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let r = plan.castle.chapel;
    let floor = ridge_floor(plan, r);
    if r.contains(x, z) {
        place_gabled(sink, r, x, floor, ground, z, palette, 8, true);
        let (roof_y, _) = gable_roof_y(r, x, z, floor + 8, 1);
        interior::furnish_chapel(sink, r, x, floor, z, roof_y, palette, true);
    } else {
        foundation(sink, x, floor, ground, z, palette);
        let (cx, cz) = r.center();
        let dist = isqrt(x - cx, z - cz);
        let roof_y = floor + 8 + (4 - dist).max(0);
        let apse_edge = Dir::ALL.iter().any(|dir| {
            let nx = x + dir.dx() - cx;
            let nz = z + dir.dz() - cz;
            nx * nx + nz * nz > 16
        });
        if apse_edge {
            for y in floor..roof_y {
                sink.put(x, y, z, palette.bricks);
            }
            if (x + z) % 3 == 0 {
                sink.put(
                    x,
                    floor + 3,
                    z,
                    Block::YELLOW_STAINED_GLASS_PANE.default_state,
                );
            }
        } else {
            sink.put(x, floor, z, palette.planks);
            clear_above(sink, x, floor + 1, roof_y - 1, z);
            if dist == 0 {
                sink.put(x, floor + 1, z, palette.slab);
                sink.put(x, floor + 2, z, Block::CANDLE.default_state);
            }
        }
        sink.put(x, roof_y, z, stair_state(palette.roof, Dir::South));
    }
    let (cx, _cz) = r.center();
    let steeple = (x - cx).abs() <= 1 && (z - r.min_z).abs() <= 1;
    if steeple {
        for y in floor + 8..=floor + 20 {
            sink.put(x, y, z, palette.bricks);
        }
        if x == cx && z == r.min_z {
            sink.put(
                x,
                floor + 21,
                z,
                stair_state(palette.brick_stairs, Dir::South),
            );
            sink.put(x, floor + 22, z, Block::LANTERN.default_state);
        }
    }
}

fn place_stables(
    sink: &mut impl BlockSink,
    rect: crate::geom::Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let eaves = floor + 5;
    let (roof_y, _) = gable_roof_y(rect, x, z, eaves, 1);
    let edge = rect.on_edge(x, z);
    let open_front = z == rect.max_z && !rect.is_corner(x, z);
    if edge {
        if open_front {
            sink.put(x, floor, z, palette.path);
            for y in floor + 1..roof_y {
                sink.put(x, y, z, Block::OAK_FENCE.default_state);
            }
            if interior::gable_door_at(rect, x, z) {
                interior::place_door(sink, x, floor + 1, z, Dir::South, false);
            }
        } else {
            for y in floor..roof_y {
                sink.put(x, y, z, palette.planks);
            }
            if (x + z) % 3 == 0 {
                sink.put(x, floor + 2, z, Block::OAK_FENCE.default_state);
            }
        }
    } else {
        sink.put(x, floor, z, palette.path);
        clear_above(sink, x, floor + 1, roof_y - 1, z);
        interior::furnish_stables(sink, rect, x, floor, z, roof_y, palette);
    }
    place_roof(sink, rect, x, eaves, z, palette, 1);
}

fn place_gabled(
    sink: &mut impl BlockSink,
    rect: crate::geom::Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    wall_h: i32,
    stone: bool,
) {
    foundation(sink, x, floor, ground, z, palette);
    let eaves = floor + wall_h;
    let pitch = roof_pitch(palette);
    let (roof_y, _) = gable_roof_y_pitch(rect, x, z, eaves, 1, pitch);
    let edge = rect.on_edge(x, z);
    let wall = if stone {
        palette.bricks
    } else {
        palette.planks
    };
    if edge {
        for y in floor..roof_y {
            sink.put(x, y, z, wall);
        }
        if interior::gable_door_at(rect, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(rect, x, z), stone);
        } else if !rect.is_corner(x, z) && (x + z) % 4 == 0 {
            sink.put(x, floor + 2, z, Block::GLASS_PANE.default_state);
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        clear_above(sink, x, floor + 1, roof_y - 1, z);
    }
    place_roof_pitched(sink, rect, x, eaves, z, palette, 1, pitch);
    clear_hill_above_roof(sink, x, roof_y, ground, z, false);
}

/// Roof Y and stair facing for a gable over `rect`.
#[must_use]
pub fn gable_roof_y(rect: Rect, x: i32, z: i32, eaves_y: i32, overhang: i32) -> (i32, Dir) {
    gable_roof_y_pitch(rect, x, z, eaves_y, overhang, 1)
}

fn roof_pitch(palette: &Palette) -> i32 {
    match palette.kind {
        PaletteKind::Spruce => 2,
        _ => 1,
    }
}

fn gable_roof_y_pitch(
    rect: Rect,
    x: i32,
    z: i32,
    eaves_y: i32,
    overhang: i32,
    pitch: i32,
) -> (i32, Dir) {
    let along_x = rect.width() >= rect.depth();
    let ridge = if along_x {
        rect.min_z.midpoint(rect.max_z)
    } else {
        rect.min_x.midpoint(rect.max_x)
    };
    let dist = if along_x {
        (z - ridge).abs()
    } else {
        (x - ridge).abs()
    };
    let span = if along_x { rect.depth() } else { rect.width() };
    let half = span / 2 + overhang;
    let roof_y = eaves_y + 1 + (half - dist).clamp(0, 10) * pitch.max(1);
    let roof_dir = if along_x {
        if z < ridge { Dir::North } else { Dir::South }
    } else if x < ridge {
        Dir::West
    } else {
        Dir::East
    };
    (roof_y, roof_dir)
}

fn place_roof(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    overhang: i32,
) {
    place_roof_pitched(sink, rect, x, eaves_y, z, palette, overhang, 1);
}

fn place_roof_pitched(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    overhang: i32,
    pitch: i32,
) {
    write_pitched_roof(
        sink,
        rect,
        x,
        eaves_y,
        z,
        palette,
        overhang,
        pitch,
        palette.roof,
        palette.roof_slab,
        roof_cap_block(palette),
    );
}

fn write_pitched_roof(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    overhang: i32,
    pitch: i32,
    roof: &Block,
    roof_slab: &'static BlockState,
    cap: &'static BlockState,
) {
    let along_x = rect.width() >= rect.depth();
    let ridge = if along_x {
        rect.min_z.midpoint(rect.max_z)
    } else {
        rect.min_x.midpoint(rect.max_x)
    };
    let dist = if along_x {
        (z - ridge).abs()
    } else {
        (x - ridge).abs()
    };
    let (roof_y, roof_dir) = gable_roof_y_pitch(rect, x, z, eaves_y, overhang, pitch);
    // Interior ceiling only. Overwriting the shell (or the eave overhang)
    // with planks hid timber framing and turned houses into brown prisms.
    if rect.contains(x, z) && !rect.on_edge(x, z) {
        sink.put(x, roof_y - 1, z, palette.planks);
    }
    if dist <= 1 {
        sink.put(x, roof_y, z, cap);
        sink.put(x, roof_y + 1, z, roof_slab);
    } else {
        sink.put(x, roof_y, z, stair_state(roof, roof_dir));
    }
    if palette.kind == PaletteKind::Spruce {
        sink.put(x, roof_y + 1, z, Block::SNOW.default_state);
        if dist == 0 {
            sink.put(x, roof_y + 2, z, Block::SNOW.default_state);
        }
    }
}

fn place_tavern_dormer(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    pitch: i32,
) {
    if palette.kind == PaletteKind::Sandstone || !rect.on_edge(x, z) || rect.is_corner(x, z) {
        return;
    }
    let along_x = rect.width() >= rect.depth();
    let on_eave = if along_x {
        z == rect.min_z || z == rect.max_z
    } else {
        x == rect.min_x || x == rect.max_x
    };
    if !on_eave {
        return;
    }
    let along = if along_x {
        x - rect.min_x
    } else {
        z - rect.min_z
    };
    if along % 4 != 2 {
        return;
    }
    let (roof_y, roof_dir) = gable_roof_y_pitch(rect, x, z, eaves_y, 1, pitch);
    sink.put(x, roof_y, z, Block::GLASS_PANE.default_state);
    sink.put(x, roof_y + 1, z, stair_state(palette.roof, roof_dir));
}

fn house_stone_ground(lot: &Lot) -> bool {
    matches!(
        lot.kind,
        BuildingKind::Townhouse
            | BuildingKind::Timber
            | BuildingKind::Tavern
            | BuildingKind::Cottage
    ) && lot.seed % 3 == 0
}

fn house_red_tile(lot: &Lot, palette: &Palette) -> bool {
    palette.kind == PaletteKind::Oak
        && matches!(
            lot.kind,
            BuildingKind::Townhouse | BuildingKind::Timber | BuildingKind::Tavern
        )
        && lot.seed % 5 == 0
}

fn house_plaster(lot: &Lot, palette: &Palette) -> &'static BlockState {
    match (lot.seed % 7) as u8 {
        0 => Block::WHITE_TERRACOTTA.default_state,
        1 => Block::LIGHT_GRAY_TERRACOTTA.default_state,
        2 => Block::YELLOW_TERRACOTTA.default_state,
        3 => Block::ORANGE_TERRACOTTA.default_state,
        4 => Block::BROWN_TERRACOTTA.default_state,
        5 => Block::RED_TERRACOTTA.default_state,
        _ => palette.infill((lot.seed as u8) & 7),
    }
}

fn place_flat_roof(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    kind: BuildingKind,
) {
    if rect.contains(x, z) && !rect.on_edge(x, z) {
        sink.put(x, eaves_y - 1, z, palette.planks);
    }
    sink.put(x, eaves_y, z, palette.planks);
    if matches!(
        kind,
        BuildingKind::Church | BuildingKind::Townhouse | BuildingKind::Tavern
    ) {
        place_dome(sink, rect, x, eaves_y, z, palette);
    }
}

fn place_dome(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    base_y: i32,
    z: i32,
    palette: &Palette,
) {
    let (cx, cz) = rect.center();
    let r = (rect.width().min(rect.depth()) / 3).clamp(3, 5);
    let dx = x - cx;
    let dz = z - cz;
    let d2 = dx * dx + dz * dz;
    if d2 > r * r {
        return;
    }
    let h = ((r * r - d2) as f64).sqrt() as i32;
    let shell = h.max(1);
    if d2 >= (r - 1) * (r - 1) || shell <= 2 {
        for y in shell..=shell + 1 {
            sink.put(x, base_y + y, z, palette.bricks);
        }
    } else {
        sink.put(x, base_y + shell, z, palette.bricks);
        sink.put(x, base_y + shell + 1, z, palette.slab);
    }
}

fn place_rose_window(sink: &mut impl BlockSink, x: i32, floor: i32, z: i32, cz: i32) {
    let cy = floor + 13;
    let radius = 7;
    for y in cy - radius..=cy + radius {
        let dy = y - cy;
        let dz = z - cz;
        let d2 = dy * dy + dz * dz;
        if d2 > radius * radius {
            continue;
        }
        // Recessed stone surround, calcite tracery, 16 petals, graded glass.
        let surround = d2 >= 36;
        let outer = !surround && d2 >= 25;
        let oculus_ring = (4..=8).contains(&d2);
        let oculus = d2 <= 3;
        let ady = dy.abs();
        let adz = dz.abs();
        let spoke = !oculus
            && !outer
            && !surround
            && (ady == 0
                || adz == 0
                || ady == adz
                || ady * 2 == adz
                || adz * 2 == ady
                || ady * 3 == adz * 2
                || adz * 3 == ady * 2);
        let glass = if oculus || (dy >= 0 && dz >= 0) {
            Block::PURPLE_STAINED_GLASS.default_state
        } else if dy >= 0 && dz < 0 {
            Block::BLUE_STAINED_GLASS.default_state
        } else if dy < 0 && dz < 0 {
            Block::YELLOW_STAINED_GLASS.default_state
        } else {
            Block::RED_STAINED_GLASS.default_state
        };
        sink.put(
            x,
            y,
            z,
            if surround {
                Block::STONE_BRICKS.default_state
            } else if outer || oculus_ring || spoke {
                Block::CALCITE.default_state
            } else {
                glass
            },
        );
    }
}

fn clear_hill_above_roof(
    sink: &mut impl BlockSink,
    x: i32,
    roof_y: i32,
    ground: i32,
    z: i32,
    keep_chimney: bool,
) {
    if ground <= roof_y || keep_chimney {
        return;
    }
    clear_above(sink, x, roof_y + 1, ground + 2, z);
}

fn place_flag_plinth(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32) {
    let (fx, fz) = plan.castle.flag;
    sink.put(x, ground, z, Block::STONE_BRICKS.default_state);
    clear_above(sink, x, ground + 1, ground + 12, z);
    if x == fx && z == fz {
        for y in 1..=crate::siege::FLAG_MAST_FENCES {
            sink.put(x, ground + y, z, Block::OAK_FENCE.default_state);
        }
        let owner = crate::siege::TownOwner::kingdom(&crate::faction::KingdomId::for_cell(
            plan.origin.cell_x,
            plan.origin.cell_z,
        ));
        sink.put(
            x,
            ground + crate::siege::FLAG_MAST_FENCES + 1,
            z,
            crate::siege::owner_banner(&owner).default_state,
        );
        place_flag_panel(sink, plan, x, ground, z, &owner);
        place_flag_braziers(sink, x, ground, z);
    } else {
        sink.put(x, ground + 1, z, Block::STONE_PRESSURE_PLATE.default_state);
    }
}

fn flag_panel_axes(plan: &TownPlan, fx: i32, fz: i32) -> (i32, i32, i32, i32) {
    let (kx, kz) = plan.castle.keep.center();
    let dx = (fx - kx).signum();
    let dz = (fz - kz).signum();
    if dx.abs() >= dz.abs() {
        (if dx == 0 { 1 } else { dx }, 0, 0, 1)
    } else {
        (0, dz, 1, 0)
    }
}

fn place_flag_panel(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    fx: i32,
    ground: i32,
    fz: i32,
    owner: &crate::siege::TownOwner,
) {
    let (dx, dz, px, pz) = flag_panel_axes(plan, fx, fz);
    let wool = crate::siege::owner_wool(owner).default_state;
    let base_y = ground + crate::siege::FLAG_MAST_FENCES;
    let half = crate::siege::FLAG_PANEL_WIDTH / 2;
    for row in 0..crate::siege::FLAG_PANEL_HEIGHT {
        for col in -half..=half {
            sink.put(fx + dx + col * px, base_y + row, fz + dz + col * pz, wool);
        }
    }
}

fn place_flag_braziers(sink: &mut impl BlockSink, fx: i32, ground: i32, fz: i32) {
    let off = crate::siege::FLAG_BRAZIER_OFFSET;
    for (dx, dz) in [(-off, -off), (-off, off), (off, -off), (off, off)] {
        let x = fx + dx;
        let z = fz + dz;
        sink.put(x, ground, z, Block::STONE_BRICKS.default_state);
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
        clear_above(sink, x, ground + 2, ground + 4, z);
    }
}

fn place_court(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    let mix = (x * 5 + z * 2).unsigned_abs() % 5;
    let block = match mix {
        0 => palette.gravel,
        1 => palette.cobble,
        _ => palette.path,
    };
    sink.put(x, ground, z, block);
    clear_above(sink, x, ground + 1, ground + 3, z);
    if mix == 4 && (x + z) % 17 == 0 {
        sink.put(x, ground + 1, z, Block::LANTERN.default_state);
    }
}

fn place_moat(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32) {
    let (gx, gz) = plan.castle.gatehouse.center();
    let toward_gate = (x - gx).abs() <= 4 && (z - gz).abs() <= 10;
    if toward_gate {
        sink.put(
            x,
            ground.max(plan.heights.sea_level),
            z,
            Block::SPRUCE_PLANKS.default_state,
        );
        clear_above(sink, x, ground + 1, ground + 3, z);
        return;
    }
    let water_y = ground.min(plan.heights.sea_level);
    sink.put(x, water_y - 1, z, Block::DIRT.default_state);
    sink.put(x, water_y, z, Block::WATER.default_state);
    if ground < plan.heights.sea_level {
        for y in ground + 1..=plan.heights.sea_level {
            sink.put(x, y, z, Block::WATER.default_state);
        }
    }
    clear_above(
        sink,
        x,
        plan.heights.sea_level.max(ground) + 1,
        ground + 4,
        z,
    );
}

fn place_building(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    lot: &Lot,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let mean = plan.heights.mean(
        lot.rect.min_x,
        lot.rect.min_z,
        lot.rect.max_x,
        lot.rect.max_z,
    );
    let floor = if lot.kind == BuildingKind::Watermill {
        mill_house_floor(plan, lot)
    } else {
        mean.max(plan.heights.sea_level + 2)
    };
    match lot.kind {
        BuildingKind::Watermill => place_watermill(sink, plan, lot, x, floor, ground, z, palette),
        BuildingKind::Barn if lot.rect.width() <= 9 && lot.rect.depth() <= 8 => {
            place_staddle_granary(sink, lot, x, floor, ground, z, palette);
        }
        BuildingKind::Winepress => place_winepress(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Windmill => place_windmill(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Church => place_town_church(sink, lot, x, floor, ground, z, palette),
        BuildingKind::StaveChurch => place_stave_church(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Monastery => place_monastery(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Watchtower => place_watchtower(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Outpost => place_outpost(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Cathedral => place_cathedral(sink, lot, x, floor, ground, z, palette),
        BuildingKind::BellTower => place_bell_tower(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Stall => place_stall(sink, lot, x, ground, z, palette),
        BuildingKind::TownHall => place_town_hall(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Blacksmith => {
            place_house(sink, lot, x, floor, ground, z, palette);
            if x == lot.rect.center().0 && z == lot.rect.center().1 {
                sink.put(x, floor + 1, z, Block::ANVIL.default_state);
                sink.put(x, floor + 2, z, Block::CAMPFIRE.default_state);
            }
        }
        _ => place_house(sink, lot, x, floor, ground, z, palette),
    }
    widen_piling(sink, plan, x, floor, ground, z, palette);
}

fn mill_house_floor(plan: &TownPlan, lot: &Lot) -> i32 {
    let (_, hi) = plan.heights.range(
        lot.rect.min_x,
        lot.rect.min_z,
        lot.rect.max_x,
        lot.rect.max_z,
    );
    hi.max(plan.heights.sea_level + 2)
}

fn place_staddle_granary(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let floor = floor + 2;
    place_house(sink, lot, x, floor, ground, z, palette);
    let r = lot.rect;
    if ground >= floor {
        return;
    }
    let staddle = r.is_corner(x, z) || (r.on_edge(x, z) && (x + z).rem_euclid(3) == 0);
    if staddle {
        for y in ground..floor {
            sink.put(x, y, z, palette.cobble);
        }
        return;
    }
    // Fence, not air: watertight treats the crawl-space as closed, and the
    // cobble corner posts still read as staddle stones from the outside.
    for y in ground..floor {
        sink.put(x, y, z, palette.fence);
    }
}

fn place_house(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let wall_h = lot.kind.wall_h();
    let eaves = floor + wall_h;
    let desert = palette.kind == PaletteKind::Sandstone;
    let pitch = roof_pitch(palette);
    let (roof_y, _) = if desert {
        (eaves, Dir::South)
    } else {
        gable_roof_y_pitch(r, x, z, eaves, 1, pitch)
    };
    let timber = lot.kind.timber_frame() && !desert;
    let stone_ground = house_stone_ground(lot);
    let plaster = house_plaster(lot, palette);
    let edge = r.on_edge(x, z);
    let front = is_front(lot, x, z);
    let door = front && x == door_x(lot) && z == door_z(lot);
    let wall_top = if desert { eaves } else { roof_y };
    if edge {
        sink.put(x, floor, z, palette.cobble);
        sink.put(
            x,
            floor + 1,
            z,
            if stone_ground {
                palette.bricks
            } else {
                palette.foundation((x + z) % 5 == 0)
            },
        );
        for y in floor + 2..=wall_top {
            let storey_seam = !desert && (y - floor) % 4 == 0;
            let stud = match lot.facing {
                Dir::North | Dir::South => (x - r.min_x) % 3 == 0,
                Dir::East | Dir::West => (z - r.min_z) % 3 == 0,
            };
            let b = if desert {
                if window_here(lot, x, z, y - floor) && y < eaves {
                    Block::GLASS_PANE.default_state
                } else {
                    palette.bricks
                }
            } else if r.is_corner(x, z) || storey_seam || (timber && stud) {
                palette.log
            } else if window_here(lot, x, z, y - floor) {
                Block::GLASS_PANE.default_state
            } else if timber {
                plaster
            } else if lot.kind == BuildingKind::Blacksmith
                || lot.kind == BuildingKind::Barn
                || lot.kind == BuildingKind::Watermill
                || lot.kind == BuildingKind::Winepress
                || lot.kind == BuildingKind::Monastery
                || lot.kind == BuildingKind::Cathedral
                || lot.kind == BuildingKind::ChapterHouse
            {
                palette.cobble
            } else {
                palette.planks
            };
            sink.put(x, y, z, b);
        }
        if door {
            interior::place_door(sink, x, floor + 1, z, lot.facing, false);
            sink.put(
                x,
                floor + 3,
                z,
                if desert { palette.bricks } else { palette.log },
            );
            if lot.kind == BuildingKind::Tavern {
                let sx = x + lot.facing.dx();
                let sz = z + lot.facing.dz();
                sink.put(sx, floor + 4, sz, palette.fence);
                sink.put(sx, floor + 3, sz, Block::OAK_HANGING_SIGN.default_state);
            }
        } else if front && !r.is_corner(x, z) && (x.wrapping_mul(7) + z + lot.id as i32) % 8 == 0 {
            let sx = x + lot.facing.dx();
            let sz = z + lot.facing.dz();
            sink.put(sx, floor + 3, sz, palette.fence);
            sink.put(sx, floor + 2, sz, Block::OAK_HANGING_SIGN.default_state);
        }
        if front && !door && (x + z) % 4 == 1 {
            sink.put(x, floor + 2, z, Block::OAK_TRAPDOOR.default_state);
        }
        if r.is_corner(x, z) {
            for y in floor..=wall_top {
                sink.put(x, y, z, if desert { palette.bricks } else { palette.log });
            }
        }
        if desert {
            sink.put(x, eaves + 1, z, palette.brick_wall);
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 1, z);
        sink.put(x, floor, z, palette.planks);
        for s in 1..lot.kind.stories() {
            let y = floor + s * 4;
            if y < roof_y {
                sink.put(x, y, z, palette.planks);
            }
        }
        interior::furnish_house(
            sink,
            lot.kind,
            r,
            x,
            floor,
            z,
            roof_y,
            lot.kind.stories(),
            palette,
        );
    }
    if desert {
        place_flat_roof(sink, r, x, eaves, z, palette, lot.kind);
    } else {
        place_roof_pitched(sink, r, x, eaves, z, palette, 1, pitch);
        if lot.kind == BuildingKind::Tavern {
            place_tavern_dormer(sink, r, x, eaves, z, palette, pitch);
        }
        if house_red_tile(lot, palette) && r.on_edge(x, z) {
            let (roof_y, roof_dir) = gable_roof_y_pitch(r, x, z, eaves, 1, pitch);
            let along_x = r.width() >= r.depth();
            let ridge = if along_x {
                r.min_z.midpoint(r.max_z)
            } else {
                r.min_x.midpoint(r.max_x)
            };
            let dist = if along_x {
                (z - ridge).abs()
            } else {
                (x - ridge).abs()
            };
            if dist <= 1 {
                sink.put(x, roof_y, z, Block::BRICKS.default_state);
                sink.put(x, roof_y + 1, z, Block::BRICK_SLAB.default_state);
            } else {
                sink.put(x, roof_y, z, stair_state(&Block::BRICK_STAIRS, roof_dir));
            }
        }
    }
    clear_hill_above_roof(sink, x, roof_y, ground, z, r.is_corner(x, z) && !front);
    if r.is_corner(x, z) && !front {
        for y in roof_y..=roof_y + 3 {
            sink.put(x, y, z, palette.cobble);
        }
        sink.put(x, roof_y + 4, z, Block::CAMPFIRE.default_state);
    }
    if door {
        sink.put(x, floor + 4, z, Block::LANTERN.default_state);
    }
}

fn door_x(lot: &Lot) -> i32 {
    match lot.facing {
        Dir::North | Dir::South => lot.rect.center().0,
        Dir::West => lot.rect.min_x,
        Dir::East => lot.rect.max_x,
    }
}

fn door_z(lot: &Lot) -> i32 {
    match lot.facing {
        Dir::East | Dir::West => lot.rect.center().1,
        Dir::North => lot.rect.min_z,
        Dir::South => lot.rect.max_z,
    }
}

fn window_here(lot: &Lot, x: i32, z: i32, dy: i32) -> bool {
    if lot.rect.is_corner(x, z) {
        return false;
    }
    let along = match lot.facing {
        Dir::North | Dir::South => x - lot.rect.min_x,
        Dir::East | Dir::West => z - lot.rect.min_z,
    };
    (dy == 3 || dy == 7 || dy == 10) && along % 3 == 1
}

fn place_monastery(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(sink, lot.rect, x, floor, ground, z, palette, 9, true);
    let (roof_y, _) = gable_roof_y(lot.rect, x, z, floor + 9, 1);
    interior::furnish_chapel(sink, lot.rect, x, floor, z, roof_y, palette, true);
    let (cx, _) = lot.rect.center();
    let bellcote = (x - cx).abs() <= 1 && (z - lot.rect.min_z).abs() <= 1;
    if bellcote {
        for y in floor + 9..=floor + 14 {
            sink.put(x, y, z, palette.bricks);
        }
        if x == cx && z == lot.rect.min_z {
            sink.put(x, floor + 15, z, Block::LANTERN.default_state);
        }
    }
}

fn place_watchtower(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let top = floor + lot.kind.wall_h();
    let edge = r.on_edge(x, z);
    if edge {
        for y in floor..=top {
            let mix = ((x.wrapping_mul(5) + y) as u8) & 15;
            sink.put(
                x,
                y,
                z,
                if y <= floor + 2 {
                    palette.cobble
                } else {
                    palette.masonry(mix)
                },
            );
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), true);
        } else if !r.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 5, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 9, z, Block::IRON_BARS.default_state);
        }
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.masonry(2));
            sink.put(x, top + 2, z, palette.brick_wall);
        }
        if r.is_corner(x, z) {
            sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, floor + 4, z, palette.planks);
        sink.put(x, floor + 8, z, palette.planks);
        sink.put(x, top, z, palette.path);
        if x == r.center().0 && z == r.center().1 {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 5, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, top + 1, z, Block::LANTERN.default_state);
        } else if (x + z) % 4 == 0 {
            sink.put(x, floor + 1, z, Block::CHEST.default_state);
        }
    }
}

fn place_town_church(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if palette.kind == PaletteKind::Sandstone {
        place_desert_church(sink, lot, x, floor, ground, z, palette);
        return;
    }
    place_gabled(sink, lot.rect, x, floor, ground, z, palette, 8, true);
    let pitch = roof_pitch(palette);
    let (roof_y, _) = gable_roof_y_pitch(lot.rect, x, z, floor + 8, 1, pitch);
    interior::furnish_chapel(sink, lot.rect, x, floor, z, roof_y, palette, true);
    let (cx, cz) = lot.rect.center();
    let tower = (x - cx).abs() <= 1 && (z - lot.rect.min_z).abs() <= 2;
    if tower {
        for y in floor + 8..=floor + 18 {
            sink.put(x, y, z, palette.bricks);
        }
        if x == cx && z == cz {
            sink.put(
                x,
                floor + 19,
                z,
                stair_state(palette.brick_stairs, Dir::South),
            );
            sink.put(x, floor + 20, z, Block::LANTERN.default_state);
        }
        if (x + z) % 2 == 0 {
            sink.put(x, floor + 12, z, Block::GLASS_PANE.default_state);
        }
    }
}

/// Taiga stave church: dark-oak stave walls, stepped gables, dragon-head peaks.
fn place_stave_church(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let along_x = r.width() >= r.depth();
    let (cx, cz) = r.center();
    let inner = if along_x {
        (x - cx).abs() <= r.width() / 4
    } else {
        (z - cz).abs() <= r.depth() / 4
    };
    let eaves = floor + 7 + if inner { 4 } else { 0 };
    let (roof_y, _) = gable_roof_y_pitch(r, x, z, eaves, 1, 2);
    let edge = r.on_edge(x, z);
    let stave = r.is_corner(x, z)
        || (along_x && (x - r.min_x) % 2 == 0)
        || (!along_x && (z - r.min_z) % 2 == 0);
    if edge {
        for y in floor..roof_y {
            sink.put(
                x,
                y,
                z,
                if stave {
                    Block::DARK_OAK_LOG.default_state
                } else {
                    palette.planks
                },
            );
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), true);
        } else if !r.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 3, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 4, z, Block::GLASS_PANE.default_state);
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        clear_above(sink, x, floor + 1, roof_y - 1, z);
        interior::furnish_chapel(sink, r, x, floor, z, roof_y, palette, true);
    }
    place_roof_pitched(sink, r, x, eaves, z, palette, 1, 2);
    // Dragon heads on the outer gable peaks and the stepped inner gable.
    let on_ridge = if along_x {
        (z - cz).abs() == 0
    } else {
        (x - cx).abs() == 0
    };
    let along = if along_x { x - cx } else { z - cz };
    let span = if along_x { r.width() } else { r.depth() };
    let outer_end = if along_x {
        x == r.min_x || x == r.max_x
    } else {
        z == r.min_z || z == r.max_z
    };
    let inner_end = along.abs() == span / 4;
    if on_ridge && (outer_end || inner_end) {
        sink.put(x, roof_y + 1, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, roof_y + 2, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, roof_y + 3, z, Block::DRAGON_HEAD.default_state);
    }
    clear_hill_above_roof(sink, x, roof_y, ground, z, false);
}

fn place_desert_church(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let eaves = floor + 8;
    let edge = r.on_edge(x, z);
    if edge {
        for y in floor..=eaves {
            sink.put(x, y, z, palette.bricks);
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), true);
        } else if !r.is_corner(x, z) && (x + z) % 4 == 0 {
            sink.put(x, floor + 3, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 4, z, Block::GLASS_PANE.default_state);
        }
        sink.put(x, eaves + 1, z, palette.brick_wall);
    } else {
        sink.put(x, floor, z, palette.path);
        clear_above(sink, x, floor + 1, eaves - 1, z);
        interior::furnish_chapel(sink, r, x, floor, z, eaves, palette, true);
    }
    place_flat_roof(sink, r, x, eaves, z, palette, BuildingKind::Church);
    clear_hill_above_roof(sink, x, eaves, ground, z, false);
}

fn place_windmill(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let (cx, cz) = lot.rect.center();
    let dx = x - cx;
    let dz = z - cz;
    let cheb = dx.abs().max(dz.abs());
    if cheb > 3 {
        return;
    }
    let taper = floor + 8;
    let cap = floor + 18;
    let mill_wall = |y: i32| {
        if y <= floor + 3 {
            palette.cobble
        } else {
            palette.planks
        }
    };
    if cheb == 3 {
        for y in floor..taper {
            sink.put(x, y, z, mill_wall(y));
        }
        if dx == 0 && dz == 3 {
            interior::place_door(sink, x, floor + 1, z, Dir::South, false);
        } else if (x + z) % 3 == 0 {
            sink.put(x, floor + 5, z, Block::GLASS_PANE.default_state);
        }
        place_mill_blades(sink, x, z, cx, cz, floor);
        return;
    }
    if cheb == 2 {
        clear_above(sink, x, floor + 1, taper - 1, z);
        sink.put(x, floor, z, palette.planks);
        for y in taper..cap {
            sink.put(x, y, z, mill_wall(y));
        }
        if (x + z) % 3 == 0 {
            sink.put(x, taper + 2, z, Block::GLASS_PANE.default_state);
        }
        interior::furnish_house(
            sink,
            BuildingKind::Windmill,
            lot.rect,
            x,
            floor,
            z,
            taper,
            1,
            palette,
        );
        sink.put(x, cap - 1, z, palette.planks);
        sink.put(x, cap, z, palette.roof_slab);
        place_mill_blades(sink, x, z, cx, cz, floor);
        return;
    }
    clear_above(sink, x, floor + 1, cap - 2, z);
    sink.put(x, floor, z, palette.planks);
    sink.put(x, floor + 6, z, palette.planks);
    sink.put(x, floor + 12, z, palette.planks);
    interior::furnish_house(
        sink,
        BuildingKind::Windmill,
        lot.rect,
        x,
        floor,
        z,
        cap - 1,
        2,
        palette,
    );
    if dx == 0 && dz == 0 {
        sink.put(x, cap - 1, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, cap, z, palette.roof_slab);
        sink.put(x, cap + 1, z, stair_state(palette.roof, Dir::South));
    } else {
        sink.put(x, cap - 1, z, palette.planks);
        sink.put(x, cap, z, palette.roof_slab);
    }
    place_mill_blades(sink, x, z, cx, cz, floor);
}

fn place_outpost(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let top = floor + lot.kind.wall_h();
    let base = ground.min(floor);
    if r.on_edge(x, z) {
        for y in base..=top {
            sink.put(
                x,
                y,
                z,
                if y <= floor + 2 || r.is_corner(x, z) {
                    Block::DARK_OAK_LOG.default_state
                } else if y == floor + 4 || y == floor + 8 {
                    Block::COBBLESTONE.default_state
                } else {
                    Block::DARK_OAK_PLANKS.default_state
                },
            );
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), false);
        } else if !r.is_corner(x, z) && (x + z) % 2 == 0 {
            sink.put(x, floor + 5, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 9, z, Block::GLASS_PANE.default_state);
        }
        sink.put(x, top + 1, z, Block::DARK_OAK_PLANKS.default_state);
        if r.is_corner(x, z) {
            sink.put(x, top + 2, z, Block::BLACK_BANNER.default_state);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, Block::COBBLESTONE.default_state);
        sink.put(x, floor + 4, z, Block::DARK_OAK_PLANKS.default_state);
        sink.put(x, floor + 8, z, Block::DARK_OAK_PLANKS.default_state);
        sink.put(x, top, z, Block::DARK_OAK_PLANKS.default_state);
        sink.put(x, top + 1, z, Block::DARK_OAK_PLANKS.default_state);
        if x == r.center().0 && z == r.center().1 {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 5, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, top - 1, z, Block::LANTERN.default_state);
        } else if (x + z) % 4 == 0 {
            sink.put(x, floor + 1, z, Block::CHEST.default_state);
        }
    }
    let _ = palette;
}

fn is_cathedral_mass(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::CathedralNave
            | CellKind::CathedralTransept
            | CellKind::CathedralChoir
            | CellKind::CathedralTower
    )
}

fn gothic_shell_edge(plan: &TownPlan, x: i32, z: i32) -> bool {
    Dir::ALL
        .iter()
        .any(|dir| !is_cathedral_mass(plan.occ.kind(x + dir.dx(), z + dir.dz())))
}

fn gothic_stone(x: i32, y: i32, z: i32, floor: i32) -> &'static BlockState {
    if y <= floor + 1 {
        if (x + z) % 2 == 0 {
            Block::ANDESITE.default_state
        } else {
            Block::STONE_BRICKS.default_state
        }
    } else if (x + z + y) % 6 == 0 {
        Block::STONE_BRICKS.default_state
    } else if (x + z) % 5 == 0 {
        Block::SMOOTH_STONE.default_state
    } else {
        Block::CALCITE.default_state
    }
}

fn gothic_roof_y(rect: Rect, x: i32, z: i32, eaves_y: i32) -> (i32, Dir) {
    let along_x = rect.width() >= rect.depth();
    let ridge = if along_x {
        rect.min_z.midpoint(rect.max_z)
    } else {
        rect.min_x.midpoint(rect.max_x)
    };
    let dist = if along_x {
        (z - ridge).abs()
    } else {
        (x - ridge).abs()
    };
    let span = if along_x { rect.depth() } else { rect.width() };
    let half = span / 2;
    let roof_y = eaves_y + 1 + (half - dist).max(0) * 2;
    let roof_dir = if along_x {
        if z < ridge { Dir::North } else { Dir::South }
    } else if x < ridge {
        Dir::West
    } else {
        Dir::East
    };
    (roof_y, roof_dir)
}

fn place_church_mass(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    kind: CellKind,
) {
    match plan.palette {
        PaletteKind::Sandstone => {
            place_basilica_mass(sink, plan, x, floor, ground, z, palette, kind);
        }
        PaletteKind::Spruce => {
            place_romanesque_mass(sink, plan, x, floor, ground, z, palette, kind);
        }
        PaletteKind::Oak => place_gothic_mass(sink, plan, x, floor, ground, z, palette, kind),
    }
}

fn basilica_stone(x: i32, y: i32, z: i32, floor: i32, palette: &Palette) -> &'static BlockState {
    if y <= floor + 1 {
        palette.cobble
    } else if (x + z + y) % 5 == 0 {
        palette.bricks
    } else {
        palette.planks
    }
}

fn place_basilica_mass(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    kind: CellKind,
) {
    let rect = blob_of(plan, x, z, kind);
    let floor = ridge_floor(plan, rect);
    foundation(sink, x, floor, ground, z, palette);
    widen_piling(sink, plan, x, floor, ground, z, palette);
    if kind == CellKind::CathedralTower {
        place_basilica_minaret(sink, rect, x, floor, ground, z, palette);
        return;
    }
    let (_cx, cz) = rect.center();
    let edge = gothic_shell_edge(plan, x, z);
    let west_front = kind == CellKind::CathedralNave && x == rect.min_x;
    if kind == CellKind::CathedralChoir {
        place_gold_dome(sink, rect, x, floor, ground, z, palette, edge);
        return;
    }
    let eaves = if west_front { floor + 14 } else { floor + 10 };
    let (roof_y, roof_dir) = gothic_roof_y(rect, x, z, eaves);
    if edge {
        let base = ground.min(floor);
        for y in base..roof_y {
            sink.put(x, y, z, basilica_stone(x, y, z, floor, palette));
        }
        if west_front && z == cz {
            interior::place_door(sink, x, floor + 1, z, Dir::West, true);
        }
        if !west_front && !rect.is_corner(x, z) && (x + z) % 3 == 0 {
            for y in floor + 3..=floor + 7 {
                sink.put(
                    x,
                    y,
                    z,
                    if y % 2 == 0 {
                        Block::ORANGE_STAINED_GLASS_PANE.default_state
                    } else {
                        Block::YELLOW_STAINED_GLASS_PANE.default_state
                    },
                );
            }
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 2, z);
        sink.put(x, floor, z, palette.bricks);
        sink.put(x, roof_y - 1, z, palette.planks);
        interior::furnish_cathedral(sink, rect, kind, x, floor, z, roof_y, palette);
    }
    if dist_to_ridge(rect, x, z) == 0 {
        sink.put(x, roof_y, z, palette.planks);
    } else {
        sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
    }
    if west_front {
        sink.put(x, roof_y + 1, z, palette.brick_wall);
    }
    clear_hill_above_roof(sink, x, roof_y, ground, z, false);
}

fn place_gold_dome(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    _edge: bool,
) {
    let (cx, cz) = rect.center();
    let dx = x - cx;
    let dz = z - cz;
    let dist2 = dx * dx + dz * dz;
    // 15-wide hemisphere (radius 7) over the crossing.
    let r = (rect.width().min(rect.depth()) / 2).clamp(6, 7);
    let r2 = r * r;
    let drum = floor + 10;
    let base = ground.min(floor);
    if dist2 > r2 {
        for y in base..=drum {
            sink.put(x, y, z, basilica_stone(x, y, z, floor, palette));
        }
        sink.put(x, drum + 1, z, Block::ORANGE_TERRACOTTA.default_state);
        clear_hill_above_roof(sink, x, drum + 1, ground, z, false);
        return;
    }
    let wall = dist2 >= (r - 1) * (r - 1);
    if wall {
        for y in base..=drum {
            sink.put(x, y, z, basilica_stone(x, y, z, floor, palette));
        }
        if (x + z) % 2 == 0 {
            sink.put(
                x,
                floor + 5,
                z,
                Block::ORANGE_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 6,
                z,
                Block::YELLOW_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 7,
                z,
                Block::ORANGE_STAINED_GLASS_PANE.default_state,
            );
        }
    } else {
        clear_above(sink, x, floor + 1, drum - 1, z);
        sink.put(x, floor, z, palette.bricks);
        interior::furnish_cathedral(
            sink,
            rect,
            CellKind::CathedralChoir,
            x,
            floor,
            z,
            drum,
            palette,
        );
    }
    // Hemisphere shell: each column's height is sqrt(R^2 - d^2).
    let h = (r2 - dist2).max(0).isqrt();
    let dome_y = drum + h;
    let gold = h >= r - 2 || dist2 <= 2;
    let cap = if gold {
        Block::GOLD_BLOCK.default_state
    } else if h <= 1 {
        Block::ORANGE_TERRACOTTA.default_state
    } else {
        Block::YELLOW_TERRACOTTA.default_state
    };
    sink.put(x, dome_y, z, cap);
    if h > 0 {
        sink.put(x, dome_y - 1, z, cap);
    }
    if dx == 0 && dz == 0 {
        sink.put(x, dome_y + 1, z, Block::GOLD_BLOCK.default_state);
        sink.put(x, dome_y + 2, z, palette.brick_wall);
        sink.put(x, dome_y + 3, z, Block::LANTERN.default_state);
    }
    clear_hill_above_roof(sink, x, dome_y + 1, ground, z, false);
}

fn place_basilica_minaret(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let (cx, cz) = rect.center();
    let cheb = (x - cx).abs().max((z - cz).abs());
    let manh = (x - cx).abs() + (z - cz).abs();
    let gallery = floor + 28;
    let lantern = gallery + 3;
    let cap = lantern + 2;
    let in_shaft = cheb <= 2 && manh <= 3;
    let edge = cheb == 2 || cheb == 3 || (cheb == 1 && manh == 3);
    let balcony = cheb == 4 && manh <= 7;
    let base = ground.min(floor);
    if balcony {
        let post = manh == 4;
        if post {
            for y in base..=gallery {
                sink.put(x, y, z, basilica_stone(x, y, z, floor, palette));
            }
        } else {
            for y in base..=floor {
                sink.put(x, y, z, palette.bricks);
            }
        }
        sink.put(x, gallery, z, Block::CUT_SANDSTONE.default_state);
        sink.put(x, gallery + 1, z, Block::SANDSTONE_SLAB.default_state);
        sink.put(x, gallery + 2, z, palette.brick_wall);
        clear_hill_above_roof(sink, x, gallery + 2, ground, z, false);
        return;
    }
    if !in_shaft && !edge {
        for y in base..=floor {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, floor + 1, z, palette.planks);
        return;
    }
    if edge {
        for y in base..=gallery {
            sink.put(x, y, z, basilica_stone(x, y, z, floor, palette));
        }
        sink.put(x, gallery + 1, z, palette.brick_wall);
        if (x + z) % 2 == 0 {
            sink.put(
                x,
                floor + 10,
                z,
                Block::ORANGE_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 18,
                z,
                Block::YELLOW_STAINED_GLASS_PANE.default_state,
            );
        }
    } else {
        clear_above(sink, x, floor + 1, lantern - 1, z);
        sink.put(x, floor, z, palette.bricks);
        for storey in [8, 16, 24] {
            sink.put(x, floor + storey, z, palette.planks);
        }
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, floor + 17, z, Block::LADDER.default_state);
            sink.put(x, floor + 25, z, Block::LADDER.default_state);
        }
    }
    for y in gallery + 1..=cap {
        if cheb <= 1 {
            sink.put(
                x,
                y,
                z,
                if y >= lantern {
                    Block::GOLD_BLOCK.default_state
                } else {
                    palette.bricks
                },
            );
        }
    }
    if cheb == 0 {
        sink.put(x, cap + 1, z, Block::GOLD_BLOCK.default_state);
        sink.put(x, cap + 2, z, Block::LANTERN.default_state);
    }
    clear_hill_above_roof(sink, x, cap + 1, ground, z, false);
}

fn romanesque_stone(x: i32, y: i32, z: i32, floor: i32, palette: &Palette) -> &'static BlockState {
    if y <= floor + 1 {
        palette.cobble
    } else if (x + z) % 3 == 0 {
        palette.log
    } else if (x + z + y) % 5 == 0 {
        palette.mossy_bricks
    } else {
        palette.bricks
    }
}

fn place_romanesque_mass(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    kind: CellKind,
) {
    let rect = blob_of(plan, x, z, kind);
    let floor = ridge_floor(plan, rect);
    foundation(sink, x, floor, ground, z, palette);
    widen_piling(sink, plan, x, floor, ground, z, palette);
    if kind == CellKind::CathedralTower {
        place_romanesque_tower(sink, rect, x, floor, ground, z, palette);
        return;
    }
    let (_cx, cz) = rect.center();
    let edge = gothic_shell_edge(plan, x, z);
    let west_front = kind == CellKind::CathedralNave && x == rect.min_x;
    let eaves = if west_front { floor + 14 } else { floor + 11 };
    let (roof_y, roof_dir) = romanesque_roof_y(rect, x, z, eaves);
    if edge {
        let base = ground.min(floor);
        for y in base..roof_y {
            sink.put(x, y, z, romanesque_stone(x, y, z, floor, palette));
        }
        if west_front && z == cz {
            interior::place_door(sink, x, floor + 1, z, Dir::West, true);
        }
        if !west_front && !rect.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 4, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 5, z, Block::GLASS_PANE.default_state);
            sink.put(
                x,
                floor + 6,
                z,
                stair_state(&Block::STONE_BRICK_STAIRS, Dir::South),
            );
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 2, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, roof_y - 1, z, palette.planks);
        interior::furnish_cathedral(sink, rect, kind, x, floor, z, roof_y, palette);
    }
    if dist_to_ridge(rect, x, z) == 0 {
        sink.put(x, roof_y, z, palette.planks);
        sink.put(x, roof_y + 1, z, palette.slab);
    } else {
        sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
    }
    clear_hill_above_roof(sink, x, roof_y + 1, ground, z, false);
}

fn romanesque_roof_y(rect: Rect, x: i32, z: i32, eaves_y: i32) -> (i32, Dir) {
    let along_x = rect.width() >= rect.depth();
    let ridge = if along_x {
        rect.min_z.midpoint(rect.max_z)
    } else {
        rect.min_x.midpoint(rect.max_x)
    };
    let dist = if along_x {
        (z - ridge).abs()
    } else {
        (x - ridge).abs()
    };
    let span = if along_x { rect.depth() } else { rect.width() };
    let half = span / 2;
    let roof_y = eaves_y + 1 + (half - dist).max(0) * 3;
    let roof_dir = if along_x {
        if z < ridge { Dir::North } else { Dir::South }
    } else if x < ridge {
        Dir::West
    } else {
        Dir::East
    };
    (roof_y, roof_dir)
}

fn place_romanesque_tower(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let (cx, cz) = rect.center();
    let cheb = (x - cx).abs().max((z - cz).abs());
    let manh = (x - cx).abs() + (z - cz).abs();
    let in_round = cheb <= 2 && manh <= 3;
    let edge = cheb == 2 || (cheb == 1 && manh == 3);
    let top = floor + 18;
    let base = ground.min(floor);
    if !in_round {
        let base = ground.min(floor);
        for y in base..=floor {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, floor + 1, z, palette.planks);
        return;
    }
    if edge {
        for y in base..=top {
            sink.put(x, y, z, romanesque_stone(x, y, z, floor, palette));
        }
        if (x + z) % 2 == 0 {
            sink.put(x, floor + 8, z, Block::GLASS_PANE.default_state);
            sink.put(x, floor + 9, z, Block::GLASS_PANE.default_state);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, floor + 8, z, palette.planks);
        sink.put(x, top, z, palette.bricks);
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, top - 1, z, Block::LANTERN.default_state);
        }
    }
    let cone = top + 1 + (2 - cheb).max(0) * 3;
    for y in top + 1..=cone {
        sink.put(
            x,
            y,
            z,
            if y == cone {
                stair_state(palette.roof, Dir::South)
            } else {
                palette.planks
            },
        );
    }
    clear_hill_above_roof(sink, x, cone, ground, z, false);
}

fn place_gothic_mass(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    _floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    kind: CellKind,
) {
    let rect = blob_of(plan, x, z, kind);
    let floor = ridge_floor(plan, rect);
    foundation(sink, x, floor, ground, z, palette);
    widen_piling(sink, plan, x, floor, ground, z, palette);
    if kind == CellKind::CathedralTower {
        place_gothic_tower(sink, plan, rect, x, floor, ground, z, palette);
        return;
    }
    let west_front = kind == CellKind::CathedralNave && x == rect.min_x;
    let rose_col = west_front && (z - rect.center().1).abs() <= 7;
    let rose_top = floor + 20;
    // Gable sits above the 13-wide wheel so deepslate bands cannot cross it.
    let eaves = if west_front { floor + 22 } else { floor + 16 };
    let (roof_y, roof_dir) = gothic_roof_y(rect, x, z, eaves);
    let edge = gothic_shell_edge(plan, x, z);
    let (cx, cz) = rect.center();
    if edge {
        let base = ground.min(floor);
        for y in base..roof_y {
            let dy = y - (floor + 13);
            let dz = z - cz;
            if rose_col && dy * dy + dz * dz <= 49 {
                continue;
            }
            sink.put(x, y, z, gothic_stone(x, y, z, floor));
        }
        if west_front && z == cz {
            interior::place_door(sink, x, floor + 1, z, Dir::West, true);
        }
        if !west_front && !rect.is_corner(x, z) && (x + z) % 3 == 0 {
            for y in floor + 4..=floor + 10 {
                let pane = if y % 2 == 0 {
                    Block::PURPLE_STAINED_GLASS_PANE.default_state
                } else {
                    Block::YELLOW_STAINED_GLASS_PANE.default_state
                };
                sink.put(x, y, z, pane);
            }
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 2, z);
        sink.put(x, floor, z, Block::SMOOTH_STONE.default_state);
        sink.put(x, roof_y - 1, z, Block::CALCITE.default_state);
        if (x - cx).abs() == 2 && (x - rect.min_x) % 5 == 2 && kind == CellKind::CathedralNave {
            for y in floor + 1..=floor + 12 {
                sink.put(x, y, z, Block::CALCITE.default_state);
            }
        }
        interior::furnish_cathedral(sink, rect, kind, x, floor, z, roof_y, palette);
    }
    if west_front {
        if roof_y > rose_top {
            sink.put(x, roof_y, z, Block::CALCITE.default_state);
        }
    } else if dist_to_ridge(rect, x, z) == 0 {
        sink.put(x, roof_y, z, Block::DEEPSLATE_TILES.default_state);
    } else {
        sink.put(
            x,
            roof_y,
            z,
            stair_state(&Block::DEEPSLATE_TILE_STAIRS, roof_dir),
        );
    }
    if rose_col {
        place_rose_window(sink, x, floor, z, cz);
    }
    clear_hill_above_roof(sink, x, roof_y, ground, z, false);
}

fn dist_to_ridge(rect: Rect, x: i32, z: i32) -> i32 {
    let along_x = rect.width() >= rect.depth();
    let ridge = if along_x {
        rect.min_z.midpoint(rect.max_z)
    } else {
        rect.min_x.midpoint(rect.max_x)
    };
    if along_x {
        (z - ridge).abs()
    } else {
        (x - ridge).abs()
    }
}

fn place_gothic_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    rect: Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let extra = ((plan.seed % 3) as i32) * 4;
    let top = floor + 32 + extra;
    let (cx, cz) = rect.center();
    let cheb = (x - cx).abs().max((z - cz).abs());
    let edge = gothic_shell_edge(plan, x, z) || rect.on_edge(x, z);
    let base = ground.min(floor);
    if edge {
        for y in base..=top {
            sink.put(x, y, z, gothic_stone(x, y, z, floor));
        }
        if !rect.is_corner(x, z) && (x + z) % 2 == 0 {
            sink.put(
                x,
                floor + 8,
                z,
                Block::PURPLE_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 9,
                z,
                Block::PURPLE_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 16,
                z,
                Block::PURPLE_STAINED_GLASS_PANE.default_state,
            );
            sink.put(
                x,
                floor + 24,
                z,
                Block::PURPLE_STAINED_GLASS_PANE.default_state,
            );
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, Block::SMOOTH_STONE.default_state);
        sink.put(x, floor + 8, z, palette.planks);
        sink.put(x, floor + 16, z, palette.planks);
        sink.put(x, floor + 24, z, palette.planks);
        sink.put(x, top, z, Block::STONE_BRICKS.default_state);
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, floor + 17, z, Block::LADDER.default_state);
            sink.put(x, floor + 25, z, Block::LADDER.default_state);
            sink.put(x, top - 1, z, Block::LANTERN.default_state);
        }
    }
    let spire_top = top + 1 + (3 - cheb).max(0) * 5;
    for y in top + 1..=spire_top {
        if y == spire_top {
            sink.put(
                x,
                y,
                z,
                stair_state(&Block::DEEPSLATE_TILE_STAIRS, Dir::South),
            );
        } else {
            sink.put(x, y, z, Block::STONE_BRICKS.default_state);
        }
    }
    clear_hill_above_roof(sink, x, spire_top, ground, z, false);
}

fn place_flying_buttress(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let mut mass_floor = floor;
    for dir in Dir::ALL {
        let nx = x + dir.dx();
        let nz = z + dir.dz();
        let kind = plan.occ.kind(nx, nz);
        if is_cathedral_mass(kind) {
            mass_floor = ridge_floor(plan, blob_of(plan, nx, nz, kind));
            break;
        }
    }
    foundation(sink, x, mass_floor, ground, z, palette);
    let dist = dist_to_cathedral_mass(plan, x, z);
    let against_mass = dist <= 2;
    let base = ground.min(mass_floor);
    if against_mass {
        sink.put(x, mass_floor, z, Block::ANDESITE.default_state);
        for y in [mass_floor + 8, mass_floor + 12, mass_floor + 16] {
            sink.put(x, y, z, stair_state(&Block::STONE_BRICK_STAIRS, Dir::South));
        }
        sink.put(x, mass_floor + 10, z, Block::STONE_BRICK_SLAB.default_state);
        clear_above(sink, x, mass_floor + 1, mass_floor + 7, z);
    } else {
        for y in base..=mass_floor + 18 {
            sink.put(
                x,
                y,
                z,
                if y <= mass_floor + 2 {
                    Block::ANDESITE.default_state
                } else if (x + z + y) % 3 == 0 {
                    Block::STONE_BRICKS.default_state
                } else {
                    Block::CALCITE.default_state
                },
            );
        }
        sink.put(x, mass_floor + 19, z, palette.brick_wall);
        sink.put(
            x,
            mass_floor + 20,
            z,
            stair_state(&Block::DEEPSLATE_TILE_STAIRS, Dir::South),
        );
    }
}

fn dist_to_cathedral_mass(plan: &TownPlan, x: i32, z: i32) -> i32 {
    let mut best = 8;
    for dz in -4..=4 {
        for dx in -4..=4 {
            if is_cathedral_mass(plan.occ.kind(x + dx, z + dz)) {
                best = best.min(dx.abs().max(dz.abs()));
            }
        }
    }
    best
}

fn place_cathedral(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(sink, lot.rect, x, floor, ground, z, palette, 16, true);
    let (roof_y, _) = gable_roof_y(lot.rect, x, z, floor + 16, 1);
    interior::furnish_chapel(sink, lot.rect, x, floor, z, roof_y, palette, true);
}

fn place_bell_tower(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let top = floor + lot.kind.wall_h();
    if r.on_edge(x, z) {
        for y in floor..=top {
            sink.put(x, y, z, palette.bricks);
        }
        if interior::gable_door_at(r, x, z) {
            interior::place_door(sink, x, floor + 1, z, interior::outward(r, x, z), true);
        } else if !r.is_corner(x, z) && y_open_belfry(x, z) {
            sink.put(x, top - 3, z, Block::IRON_BARS.default_state);
            sink.put(x, top - 2, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 6, z, Block::GLASS_PANE.default_state);
        }
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.bricks);
            sink.put(x, top + 2, z, palette.brick_wall);
        }
        if r.is_corner(x, z) {
            sink.put(x, top + 3, z, Block::LANTERN.default_state);
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, palette.planks);
        sink.put(x, floor + 6, z, palette.planks);
        sink.put(x, floor + 12, z, palette.planks);
        sink.put(x, top, z, palette.path);
        if x == r.center().0 && z == r.center().1 {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 7, z, Block::LADDER.default_state);
            sink.put(x, floor + 13, z, Block::LADDER.default_state);
            sink.put(x, top + 1, z, Block::LANTERN.default_state);
        }
    }
}

fn y_open_belfry(x: i32, z: i32) -> bool {
    (x + z) % 2 == 0
}

fn near_extractive(plan: &TownPlan, x: i32, z: i32) -> bool {
    if plan.strips_trees(x, z) {
        return true;
    }
    for dz in -2..=2 {
        for dx in -2..=2 {
            if matches!(
                plan.occ.kind(x + dx, z + dz),
                CellKind::MineHead
                    | CellKind::MineRail
                    | CellKind::MineSpoil
                    | CellKind::MineAdit
                    | CellKind::Ruin
                    | CellKind::TourneyList
                    | CellKind::TourneyStand
                    | CellKind::Burnt
                    | CellKind::Dock
                    | CellKind::Ship
                    | CellKind::Boat
                    | CellKind::Quay
                    | CellKind::Crane
                    | CellKind::Breakwater
                    | CellKind::SiegeTrench
                    | CellKind::CityDitch
                    | CellKind::WaterWheel
                    | CellKind::MillRace
                    | CellKind::Sluice
                    | CellKind::Retaining
                    | CellKind::Vine
                    | CellKind::Lighthouse
                    | CellKind::Beacon
                    | CellKind::Ford
                    | CellKind::Weir
                    | CellKind::CutStone
                    | CellKind::HerbGarden
                    | CellKind::Maypole
                    | CellKind::MarketCross
                    | CellKind::Pillory
                    | CellKind::Ropewalk
                    | CellKind::Slipway
                    | CellKind::Sawpit
            ) {
                return true;
            }
        }
    }
    false
}

fn shipyard_bench_y(plan: &TownPlan, x: i32, z: i32) -> i32 {
    let sea = plan.heights.sea_level;
    let bx = x.div_euclid(4) * 4;
    let bz = z.div_euclid(4) * 4;
    let mut sum = 0;
    let mut n = 0;
    for zz in bz..bz + 4 {
        for xx in bx..bx + 4 {
            sum += plan.heights.height(xx, zz).max(sea);
            n += 1;
        }
    }
    let h = if n == 0 {
        plan.heights.height(x, z).max(sea)
    } else {
        sum / n
    };
    sea + ((h - sea).max(0) / 3) * 3
}

fn place_shipyard_yard(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let y = shipyard_bench_y(plan, x, z).max(sea);
    let lo = ground.min(y).max(sink.floor_min() + 2);
    for yy in lo..=y {
        sink.put(
            x,
            yy,
            z,
            if yy == y {
                palette.grass
            } else {
                Block::DIRT.default_state
            },
        );
    }
    clear_above(sink, x, y + 1, y + 12, z);
    for dir in Dir::ALL {
        let ny = shipyard_bench_y(plan, x + dir.dx(), z + dir.dz());
        if ny < y {
            sink.put(x, y, z, palette.bricks);
            if ny <= y - 3 {
                sink.put(x, y + 1, z, palette.brick_wall);
            }
            break;
        }
    }
}

fn place_green(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    if plan.has_scene(x, z, crate::scenes::SceneKind::Shipyard) {
        place_shipyard_yard(sink, plan, x, ground, z, palette);
        return;
    }
    sink.put(x, ground, z, palette.grass);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 4, z);
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::LumberCamp)
    {
        place_lumber_yard(sink, x, ground, z, palette);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::CharcoalCamp)
    {
        place_charcoal_yard(sink, x, ground, z, palette);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::RoadsideInn)
    {
        place_inn_yard(sink, plan, x, ground, z, palette);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::BanditHideout)
    {
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::RuinedAbbey)
    {
        place_abbey_floor(sink, x, ground, z, palette);
        return;
    }
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::CastleRuin)
    {
        place_castle_ruin_yard(sink, x, ground, z, palette);
        return;
    }
    if near_extractive(plan, x, z) || in_mill_disc(plan, x, z) {
        return;
    }
    let mix = (x.wrapping_mul(17) + z.wrapping_mul(31) + plan.seed as i32).unsigned_abs();
    if palette.kind == PaletteKind::Sandstone {
        place_palm_column(sink, plan, x, ground, z);
        return;
    }
    match mix % 37 {
        0 => {
            sink.put(x, ground + 1, z, palette.log);
            sink.put(x, ground + 2, z, palette.log);
            sink.put(x, ground + 3, z, palette.leaves);
            sink.put(x, ground + 4, z, palette.leaves);
        }
        1 | 2 => sink.put(x, ground + 1, z, Block::SHORT_GRASS.default_state),
        3 => sink.put(x, ground + 1, z, Block::POPPY.default_state),
        4 => sink.put(x, ground + 1, z, Block::DANDELION.default_state),
        5 if plan.castle.inner.contains(x, z) => {
            sink.put(x, ground + 1, z, palette.cobble);
            sink.put(x, ground, z, MARKER_FLOOR.default_state);
        }
        _ => {}
    }
    if palette.kind == PaletteKind::Spruce && mix % 5 == 0 {
        sink.put(x, ground + 1, z, Block::SNOW.default_state);
    }
}

fn place_lumber_yard(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    let mix = (x.wrapping_mul(13) + z.wrapping_mul(7)).unsigned_abs();
    match mix % 11 {
        0 | 1 => {
            sink.put(x, ground + 1, z, palette.log);
            sink.put(x, ground + 2, z, palette.log);
            if mix % 2 == 0 {
                sink.put(x, ground + 3, z, palette.log);
            }
        }
        2 => sink.put(x, ground + 1, z, Block::CRAFTING_TABLE.default_state),
        3 => sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state),
        _ => {}
    }
}

fn place_charcoal_yard(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, Block::COARSE_DIRT.default_state);
    let mix = (x.wrapping_mul(11) + z.wrapping_mul(19)).unsigned_abs();
    match mix % 13 {
        0 | 1 => {
            sink.put(x, ground + 1, z, Block::COAL_BLOCK.default_state);
            sink.put(x, ground + 2, z, Block::COAL_BLOCK.default_state);
        }
        2 => {
            sink.put(x, ground + 1, z, palette.log);
            sink.put(x, ground + 2, z, palette.log);
            sink.put(x, ground + 3, z, palette.log);
        }
        3 => sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state),
        4 => sink.put(x, ground + 1, z, Block::FURNACE.default_state),
        5 => sink.put(x, ground + 1, z, Block::BARREL.default_state),
        _ => {}
    }
}

fn place_inn_yard(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let mix = (x.wrapping_mul(3) + z).unsigned_abs() % 5;
    sink.put(
        x,
        ground,
        z,
        if mix <= 1 {
            palette.cobble
        } else {
            palette.gravel
        },
    );
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 4, z);
    let beside_barn = plan.lots.iter().any(|lot| {
        lot.kind == BuildingKind::Barn
            && Dir::ALL
                .iter()
                .any(|dir| lot.rect.contains(x + dir.dx(), z + dir.dz()))
    });
    if beside_barn {
        let along = (x + z).rem_euclid(3);
        if along == 0 {
            sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
            sink.put(x, ground + 2, z, Block::HAY_BLOCK.default_state);
            if (x + z) % 6 == 0 {
                sink.put(x, ground + 3, z, Block::HAY_BLOCK.default_state);
            }
        } else if along == 1 {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.fence);
        }
    }
}

fn palm_hash(x: i32, z: i32, seed: u64) -> u32 {
    (x.wrapping_mul(17) + z.wrapping_mul(31) + seed as i32).unsigned_abs()
}

fn is_palm_origin(x: i32, z: i32, seed: u64) -> bool {
    palm_hash(x, z, seed) % 29 == 0
}

fn place_palm_column(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32) {
    // One column, grounded on this cell. A lean into a neighbour whose
    // ground differs by more than one block was a 3D-diagonal frond.
    if !is_palm_origin(x, z, plan.seed) {
        return;
    }
    for y in 1..=4 {
        sink.put(x, ground + y, z, Block::JUNGLE_LOG.default_state);
    }
    sink.put(x, ground + 5, z, Block::JUNGLE_LEAVES.default_state);
    sink.put(x, ground + 6, z, Block::JUNGLE_LEAVES.default_state);
}

fn place_eave(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    if let Some(lot) = plan
        .lots
        .iter()
        .find(|lot| lot.rect.inflate(1).contains(x, z))
    {
        // Overhang stair only. A fence fill here rendered as a solid brown
        // box and hid the timber-and-plaster walls.
        let eaves = floor.max(ground) + lot.kind.wall_h();
        if palette.kind == PaletteKind::Sandstone {
            let arcade = (x + z) % 2 == 0;
            if arcade {
                for y in ground + 1..eaves {
                    sink.put(x, y, z, palette.bricks);
                }
            }
            let face = Dir::ALL
                .iter()
                .copied()
                .find(|dir| lot.rect.contains(x - dir.dx(), z - dir.dz()))
                .unwrap_or(Dir::South);
            sink.put(x, eaves, z, stair_state(palette.roof, face));
        } else {
            if lot.kind.stories() >= 2
                && (lot.kind.timber_frame() || lot.kind == BuildingKind::TownHall)
            {
                place_jetty(sink, lot, x, floor.max(ground), z, palette);
            }
            place_roof_pitched(sink, lot.rect, x, eaves, z, palette, 1, roof_pitch(palette));
            if lot.kind == BuildingKind::Barn
                && plan
                    .scene_at(x, z)
                    .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::RoadsideInn)
            {
                let along = (x + z).rem_euclid(3);
                if along == 0 {
                    sink.put(x, floor.max(ground) + 1, z, Block::HAY_BLOCK.default_state);
                    sink.put(x, floor.max(ground) + 2, z, Block::HAY_BLOCK.default_state);
                } else if along == 1 {
                    sink.put(x, floor.max(ground) + 1, z, palette.fence);
                    sink.put(x, floor.max(ground) + 2, z, palette.fence);
                }
            }
        }
    }
}

fn place_jetty(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    z: i32,
    palette: &Palette,
) {
    let stories = lot.kind.stories().min(3);
    for s in 1..stories {
        let y = floor + s * 4;
        sink.put(x, y, z, palette.log);
        sink.put(x, y + 1, z, palette.planks);
        let stud = match lot.facing {
            Dir::North | Dir::South => (x - lot.rect.min_x) % 3 == 0,
            Dir::East | Dir::West => (z - lot.rect.min_z) % 3 == 0,
        };
        sink.put(
            x,
            y + 2,
            z,
            if stud {
                palette.log
            } else if window_here(lot, x, z, y + 2 - floor) {
                Block::GLASS_PANE.default_state
            } else {
                palette.infill(((x + y) as u8) & 7)
            },
        );
        sink.put(x, y + 3, z, palette.planks);
    }
}

fn place_harbor(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32, sea: i32) {
    let gorge = plan
        .scenes
        .iter()
        .any(|scene| scene.kind == crate::scenes::SceneKind::Gorge && scene.rect.contains(x, z));
    if gorge {
        let water_y = sea.min(ground).max(sink.floor_min() + 4);
        sink.put(x, water_y.saturating_sub(1), z, Block::DIRT.default_state);
        sink.put(x, water_y, z, Block::WATER.default_state);
        for y in water_y + 1..=ground.max(sea) + 4 {
            sink.put(x, y, z, Block::AIR.default_state);
        }
        return;
    }
    let water_top = sea.max(ground);
    sink.put(x, water_top - 1, z, Block::DIRT.default_state);
    sink.put(x, water_top, z, Block::WATER.default_state);
    if ground < sea {
        for y in ground + 1..=sea {
            sink.put(x, y, z, Block::WATER.default_state);
        }
    }
}

fn place_dock(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, sea: i32, palette: &Palette) {
    let y = sea.max(ground);
    sink.put(x, y, z, palette.planks);
    if (x + z) % 4 == 0 {
        sink.put(x, y + 1, z, palette.fence);
        if (x + z) % 8 == 0 {
            sink.put(x, y + 2, z, Block::LANTERN.default_state);
        }
    } else if (x + z) % 7 == 0 {
        sink.put(x, y + 1, z, Block::CHEST.default_state);
    }
}

fn dredge_hull(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, sea: i32) {
    if ground >= sea {
        return;
    }
    let keel = sea - 3;
    sink.put(x, keel - 1, z, Block::DIRT.default_state);
    for y in keel..=sea {
        sink.put(x, y, z, Block::WATER.default_state);
    }
}

fn ship_on_slipway(plan: &TownPlan, x: i32, z: i32) -> bool {
    let rect = blob_of(plan, x, z, CellKind::Ship);
    for zz in rect.min_z.saturating_sub(1)..=rect.max_z + 1 {
        for xx in rect.min_x.saturating_sub(1)..=rect.max_x + 1 {
            if plan.occ.kind(xx, zz) == CellKind::Slipway {
                return true;
            }
        }
    }
    false
}

fn place_slip_hull(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    rect: &Rect,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let slip = slip_rect_near(plan, x, z).unwrap_or(*rect);
    let out = slip_water_dir(plan, slip);
    let (along, length, across, beam) = match out {
        Dir::South => (z - rect.min_z, rect.depth(), x - rect.min_x, rect.width()),
        Dir::North => (rect.max_z - z, rect.depth(), x - rect.min_x, rect.width()),
        Dir::East => (x - rect.min_x, rect.width(), z - rect.min_z, rect.depth()),
        Dir::West => (rect.max_x - x, rect.width(), z - rect.min_z, rect.depth()),
    };
    let core_beam = if beam >= 7 { beam - 1 } else { beam };
    let scaffold = beam >= 7 && across == 0;
    let hull_across = if beam >= 7 { across - 1 } else { across };
    let mid = (core_beam - 1) / 2;
    let from_keel = if scaffold {
        core_beam
    } else {
        (hull_across - mid).abs()
    };
    let hull_half = hull_half_beam(along, length, mid.clamp(2, 3));
    let deck = slip_deck_y(plan, &slip, x, z, ground, sea);
    let lo = ground.min(deck).max(sink.floor_min() + 2);
    for yy in lo..=deck {
        sink.put(
            x,
            yy,
            z,
            if yy == deck {
                palette.planks
            } else {
                palette.bricks
            },
        );
    }
    clear_above(sink, x, deck + 1, deck + 14, z);
    if scaffold || from_keel > hull_half {
        sink.put(x, deck, z, palette.planks);
        sink.put(x, deck + 1, z, palette.log);
        if along % 2 == 0 {
            sink.put(x, deck + 2, z, palette.log);
            sink.put(x, deck + 3, z, palette.fence);
            sink.put(x, deck + 4, z, palette.slab);
            sink.put(x, deck + 5, z, palette.slab);
        } else {
            sink.put(x, deck + 2, z, palette.fence);
            sink.put(x, deck + 3, z, palette.fence);
        }
        return;
    }
    let rib = along % 2 == 0;
    let planked = along > length / 2;
    let keel = Block::DARK_OAK_LOG.default_state;
    sink.put(
        x,
        deck,
        z,
        if from_keel == 0 { keel } else { palette.planks },
    );
    if from_keel == 0 {
        sink.put(x, deck + 1, z, keel);
        sink.put(x, deck + 2, z, keel);
        if rib {
            sink.put(x, deck + 3, z, keel);
        }
    }
    if rib {
        let rib_h = 2 + (hull_half - from_keel).clamp(0, 4);
        for h in 1..=rib_h {
            sink.put(x, deck + h, z, keel);
        }
    } else if from_keel == hull_half {
        sink.put(x, deck + 1, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, deck + 2, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, deck + 3, z, Block::DARK_OAK_FENCE.default_state);
    }
    if planked && from_keel > 0 && from_keel <= hull_half {
        let h = if from_keel == hull_half { 3 } else { 4 };
        for yy in 1..=h {
            sink.put(x, deck + yy, z, Block::SPRUCE_PLANKS.default_state);
        }
        if from_keel == hull_half {
            sink.put(x, deck + h + 1, z, Block::DARK_OAK_FENCE.default_state);
        }
    }
}

fn hull_half_beam(along: i32, length: i32, mid: i32) -> i32 {
    let span = (length - 1).max(1);
    let bow_taper = 5.min(span / 2).max(2);
    let stern_taper = 3.min(span / 3).max(1);
    if along < bow_taper {
        return (mid * along / bow_taper).clamp(0, mid);
    }
    if along > span - stern_taper {
        let from_stern = span - along;
        return (mid * (from_stern + 1) / stern_taper).clamp(0, mid);
    }
    mid
}

fn sheer_rise(along: i32, length: i32) -> i32 {
    if along <= 4 {
        4 - along
    } else if along >= length - 4 {
        along - (length - 5)
    } else {
        0
    }
}

fn place_ship(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Ship);
    let sea = plan.heights.sea_level;
    let slip = ship_on_slipway(plan, x, z);
    if slip {
        place_slip_hull(sink, plan, &rect, x, ground, z, sea, palette);
        return;
    }
    dredge_hull(sink, x, ground, z, sea);
    let y = sea.max(ground);
    let along_x = rect.width() >= rect.depth();
    let length = if along_x { rect.width() } else { rect.depth() };
    let beam = if along_x { rect.depth() } else { rect.width() };
    let along = if along_x {
        x - rect.min_x
    } else {
        z - rect.min_z
    };
    let across = if along_x {
        z - rect.min_z
    } else {
        x - rect.min_x
    };
    let mid = (beam - 1) / 2;
    let from_keel = (across - mid).abs();
    let half = hull_half_beam(along, length, mid);
    if from_keel > half {
        if slip {
            place_slipway(sink, plan, x, ground, z, palette);
        } else {
            sink.put(x, y, z, Block::WATER.default_state);
        }
        return;
    }
    let bow = along <= 4;
    let stern = along >= length - 4;
    let rise = sheer_rise(along, length);
    let draft = if bow || stern { 1 } else { 2 };
    let dark = (along + from_keel) % 2 == 0;
    let hull_plank = if dark {
        Block::DARK_OAK_PLANKS.default_state
    } else {
        Block::SPRUCE_PLANKS.default_state
    };
    for d in 1..=draft {
        sink.put(
            x,
            y - d,
            z,
            if from_keel == 0 {
                Block::DARK_OAK_LOG.default_state
            } else {
                hull_plank
            },
        );
    }
    let gunwale = y + 1 + rise;
    sink.put(x, y, z, hull_plank);
    sink.put(
        x,
        y + 1,
        z,
        if dark {
            Block::SPRUCE_PLANKS.default_state
        } else {
            Block::DARK_OAK_PLANKS.default_state
        },
    );
    let on_rail = from_keel == half;
    if on_rail {
        sink.put(x, gunwale, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, gunwale + 1, z, Block::DARK_OAK_FENCE.default_state);
        if along % 2 == 0 {
            sink.put(x, gunwale + 2, z, Block::IRON_CHAIN.default_state);
        }
    }
    if bow && from_keel <= half.min(2) {
        sink.put(x, gunwale + 1, z, hull_plank);
        sink.put(x, gunwale + 2, z, hull_plank);
        if from_keel == 0 {
            sink.put(x, gunwale + 3, z, Block::SPRUCE_SLAB.default_state);
        } else {
            sink.put(x, gunwale + 3, z, Block::DARK_OAK_FENCE.default_state);
        }
    }
    if bow && from_keel == 0 && along <= 1 {
        sink.put(x, gunwale + 3, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, gunwale + 4, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, gunwale + 5, z, Block::LANTERN.default_state);
    }
    if bow && along == 3 && from_keel <= 1 {
        sink.put(x, y + 5, z, Block::WHITE_WOOL.default_state);
        sink.put(x, y + 6, z, Block::WHITE_WOOL.default_state);
    }
    if stern {
        sink.put(x, y + 2, z, Block::DARK_OAK_PLANKS.default_state);
        sink.put(x, y + 3, z, Block::DARK_OAK_PLANKS.default_state);
        if on_rail {
            sink.put(x, y + 4, z, Block::DARK_OAK_FENCE.default_state);
        } else if (x + z) % 3 == 0 {
            sink.put(x, y + 4, z, Block::CHEST.default_state);
        } else {
            sink.put(x, y + 4, z, Block::SPRUCE_SLAB.default_state);
        }
        if along == length - 1 && from_keel == 0 {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
            sink.put(x, y - 1, z, Block::DARK_OAK_FENCE.default_state);
            sink.put(x, y - 2, z, Block::DARK_OAK_FENCE.default_state);
        }
    }
    let mast = along == length / 2 && from_keel == 0;
    let yard = along == length / 2;
    let sail = along == length / 2 + 1;
    let billow = along == length / 2 + 2;
    if mast {
        for h in 2..=16 {
            sink.put(x, y + h, z, Block::SPRUCE_LOG.default_state);
        }
        sink.put(x, y + 13, z, Block::SPRUCE_PLANKS.default_state);
        sink.put(x, y + 14, z, Block::OAK_FENCE.default_state);
    }
    if yard {
        sink.put(x, y + 12, z, Block::SPRUCE_LOG.default_state);
        sink.put(x, y + 13, z, Block::OAK_FENCE.default_state);
        if from_keel == 1 {
            sink.put(x, y + 13, z, Block::SPRUCE_SLAB.default_state);
            sink.put(x, y + 14, z, Block::OAK_FENCE.default_state);
        }
    }
    if sail && from_keel <= 2 {
        for h in 5..=11 {
            sink.put(x, y + h, z, Block::WHITE_WOOL.default_state);
        }
        sink.put(x, y + 9, z, Block::WHITE_BANNER.default_state);
    }
    if billow && (1..=2).contains(&from_keel) {
        sink.put(x, y + 8, z, Block::WHITE_WOOL.default_state);
        sink.put(x, y + 9, z, Block::WHITE_WOOL.default_state);
        sink.put(x, y + 10, z, Block::WHITE_WOOL.default_state);
    }
    if yard && on_rail {
        for h in gunwale + 2..y + 12 {
            sink.put(x, h, z, Block::OAK_FENCE.default_state);
        }
        sink.put(x, y + 11, z, Block::IRON_CHAIN.default_state);
        sink.put(x, y + 13, z, Block::IRON_CHAIN.default_state);
    } else if on_rail && !bow && !stern {
        sink.put(x, gunwale + 2, z, Block::OAK_FENCE.default_state);
        if along % 4 == 1 {
            sink.put(x, gunwale + 3, z, Block::IRON_CHAIN.default_state);
        }
    } else if from_keel == 0 && !mast && !stern && along % 2 == 0 {
        sink.put(x, y + 2, z, Block::OAK_FENCE.default_state);
    } else if !stern && !bow && !on_rail && (x + z) % 5 == 0 {
        sink.put(x, y + 2, z, Block::BARREL.default_state);
    }
    if mast {
        sink.put(x, y + 16, z, Block::WHITE_BANNER.default_state);
    }
    let _ = palette;
}

fn place_fishing_boat(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Boat);
    let (cx, cz) = rect.center();
    let sea = plan.heights.sea_level;
    dredge_hull(sink, x, ground, z, sea);
    let y = sea.max(ground);
    let along_x = rect.width() >= rect.depth();
    let length = if along_x { rect.width() } else { rect.depth() };
    let beam = if along_x { rect.depth() } else { rect.width() };
    let along = if along_x {
        x - rect.min_x
    } else {
        z - rect.min_z
    };
    let across = if along_x {
        z - rect.min_z
    } else {
        x - rect.min_x
    };
    let mid = (beam - 1) / 2;
    let from_keel = (across - mid).abs();
    let half = hull_half_beam(along, length, mid.max(1));
    if from_keel > half {
        sink.put(x, y, z, Block::WATER.default_state);
        return;
    }
    sink.put(x, y - 1, z, Block::SPRUCE_PLANKS.default_state);
    sink.put(x, y, z, Block::SPRUCE_PLANKS.default_state);
    let rail = from_keel == half;
    if rail {
        sink.put(x, y + 1, z, Block::OAK_LOG.default_state);
    } else {
        sink.put(x, y + 1, z, Block::SPRUCE_SLAB.default_state);
    }
    if x == cx && z == cz {
        for h in 2..=5 {
            sink.put(x, y + h, z, Block::OAK_FENCE.default_state);
        }
        sink.put(x, y + 6, z, Block::LANTERN.default_state);
    }
    if along == length / 2 + 1 && from_keel <= 1 {
        for h in 3..=5 {
            sink.put(x, y + h, z, Block::WHITE_WOOL.default_state);
        }
    } else if !rail && (x + z) % 3 == 0 {
        sink.put(x, y + 1, z, Block::CHEST.default_state);
    }
    let _ = palette;
}

fn place_quay(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, sea: i32, palette: &Palette) {
    let y = ground.max(sea);
    let bed = ground.min(sea).max(sink.floor_min() + 2);
    for yy in bed..y {
        sink.put(
            x,
            yy,
            z,
            if yy <= sea {
                palette.cobble
            } else {
                palette.andesite
            },
        );
    }
    sink.put(x, y - 1, z, palette.andesite);
    let timber = (x + z) % 2 == 0;
    sink.put(
        x,
        y,
        z,
        if timber {
            palette.planks
        } else {
            Block::STONE_BRICKS.default_state
        },
    );
    sink.put(
        x,
        y + 1,
        z,
        if timber {
            palette.planks
        } else {
            palette.cobble
        },
    );
    if ground < sea {
        for yy in ground + 1..=sea {
            sink.put(x, yy, z, palette.cobble);
        }
    }
    let mix = (x + z).rem_euclid(6);
    if mix == 0 {
        sink.put(x, y + 2, z, palette.brick_wall);
        sink.put(x, y + 3, z, Block::LANTERN.default_state);
    } else if mix == 1 {
        sink.put(x, y + 2, z, Block::BARREL.default_state);
    } else if mix == 2 {
        sink.put(x, y + 2, z, Block::CHEST.default_state);
    } else if mix == 3 {
        sink.put(x, y + 2, z, Block::HAY_BLOCK.default_state);
    } else {
        sink.put(x, y + 2, z, palette.brick_wall);
    }
}

fn crane_pad(plan: &TownPlan, x: i32, z: i32) -> i32 {
    let mut pad = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if plan.occ.kind(x + dx, z + dz) == CellKind::Crane {
                pad += 1;
            }
        }
    }
    pad
}

fn quarry_crane_boom_dir(plan: &TownPlan, x: i32, z: i32) -> Dir {
    let mut best = Dir::South;
    let mut best_d = i32::MAX;
    for dir in Dir::ALL {
        for dist in 1..=8 {
            let nx = x + dir.dx() * dist;
            let nz = z + dir.dz() * dist;
            if plan.occ.kind(nx, nz) == CellKind::CampCourt && dist < best_d {
                best_d = dist;
                best = dir;
                break;
            }
        }
    }
    best
}

fn crane_mast_cell(plan: &TownPlan, x: i32, z: i32) -> (i32, i32) {
    let rect = blob_of(plan, x, z, CellKind::Crane);
    let mut best = (x, z);
    let mut best_pad = -1i32;
    for zz in rect.min_z..=rect.max_z {
        for xx in rect.min_x..=rect.max_x {
            if plan.occ.kind(xx, zz) != CellKind::Crane {
                continue;
            }
            let pad = crane_pad(plan, xx, zz);
            if pad > best_pad {
                best_pad = pad;
                best = (xx, zz);
            }
        }
    }
    best
}

fn boom_step_from_mast(x: i32, z: i32, mx: i32, mz: i32, boom: Dir) -> Option<i32> {
    (0..=8).find(|&step| mx + boom.dx() * step == x && mz + boom.dz() * step == z)
}

fn place_quarry_crane(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let (mx, mz) = crane_mast_cell(plan, x, z);
    let boom = quarry_crane_boom_dir(plan, mx, mz);
    let mast_y = plan.heights.height(mx, mz);
    let y = ground;
    sink.put(x, y, z, Block::STONE_BRICKS.default_state);
    sink.put(x, y + 1, z, palette.cobble);
    clear_above(sink, x, y + 2, mast_y + 14, z);
    let log = Block::OAK_LOG.default_state;
    let step = boom_step_from_mast(x, z, mx, mz, boom);
    if x == mx && z == mz {
        for yy in y + 2..=mast_y.max(y) + 10 {
            sink.put(x, yy, z, log);
        }
        sink.put(x, mast_y.max(y) + 11, z, Block::OAK_FENCE.default_state);
        sink.put(x, mast_y.max(y) + 12, z, Block::LANTERN.default_state);
        return;
    }
    if let Some(dist) = step.filter(|&d| d >= 1) {
        let jib = mast_y.max(y) + 9;
        sink.put(x, jib, z, log);
        sink.put(x, jib + 1, z, log);
        if dist >= 3 {
            for dy in 1..=5 {
                let cy = mast_y + 9 - dy;
                if cy > y + 2 {
                    sink.put(x, cy, z, Block::IRON_CHAIN.default_state);
                }
            }
            sink.put(x, y + 2, z, Block::IRON_BARS.default_state);
        }
    }
}

fn shipyard_crane_boom_dir(plan: &TownPlan, x: i32, z: i32) -> Dir {
    let (mx, mz) = crane_mast_cell(plan, x, z);
    let rect = blob_of(plan, mx, mz, CellKind::Crane);
    let arms = [
        (Dir::East, rect.max_x - mx),
        (Dir::West, mx - rect.min_x),
        (Dir::South, rect.max_z - mz),
        (Dir::North, mz - rect.min_z),
    ];
    if let Some((dir, len)) = arms.iter().copied().max_by_key(|(_, len)| *len)
        && len > 0
    {
        return dir;
    }
    if let Some(slip) = slip_rect_near(plan, mx, mz) {
        return slip_water_dir(plan, slip);
    }
    Dir::South
}

fn place_shipyard_crane(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let (mx, mz) = crane_mast_cell(plan, x, z);
    let boom = shipyard_crane_boom_dir(plan, mx, mz);
    let mast_y = shipyard_bench_y(plan, mx, mz)
        .max(plan.heights.height(mx, mz))
        .max(sea);
    let y = shipyard_bench_y(plan, x, z).max(ground).max(sea);
    let bed = plan.heights.height(x, z).min(ground).min(y);
    let lo = bed.max(sink.floor_min() + 2);
    for yy in lo..=y {
        sink.put(x, yy, z, Block::STONE_BRICKS.default_state);
    }
    sink.put(x, y + 1, z, palette.cobble);
    clear_above(sink, x, y + 2, mast_y + 16, z);
    let log = Block::OAK_LOG.default_state;
    if x == mx && z == mz {
        for yy in y + 2..=mast_y.max(y) + 10 {
            sink.put(x, yy, z, log);
        }
        let top = mast_y.max(y);
        sink.put(x, top + 11, z, Block::OAK_FENCE.default_state);
        sink.put(x, top + 12, z, Block::LANTERN.default_state);
        return;
    }
    if let Some(dist) = boom_step_from_mast(x, z, mx, mz, boom).filter(|&d| d >= 1) {
        let jib = mast_y + 9;
        sink.put(x, jib, z, log);
        sink.put(x, jib + 1, z, log);
        if dist >= 3 {
            for dy in 1..=5 {
                let cy = mast_y + 9 - dy;
                if cy > y + 2 {
                    sink.put(x, cy, z, Block::IRON_CHAIN.default_state);
                }
            }
            sink.put(x, y + 2, z, Block::IRON_BARS.default_state);
        }
    }
}

fn place_crane(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    if plan
        .scene_at(x, z)
        .is_some_and(|scene| scene.kind == crate::scenes::SceneKind::Quarry)
    {
        place_quarry_crane(sink, plan, x, ground, z, palette);
        return;
    }
    if plan.has_scene(x, z, crate::scenes::SceneKind::Shipyard) {
        place_shipyard_crane(sink, plan, x, ground, z, sea, palette);
        return;
    }
    let y = ground.max(sea);
    let pad = crane_pad(plan, x, z);
    sink.put(x, y, z, Block::STONE_BRICKS.default_state);
    sink.put(x, y + 1, z, palette.cobble);
    if plan.heights.is_dry(x, z) && pad >= 8 {
        for h in 2..=12 {
            sink.put(x, y + h, z, Block::OAK_LOG.default_state);
        }
        sink.put(x, y + 13, z, Block::OAK_FENCE.default_state);
        sink.put(x, y + 12, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, y + 11, z, Block::WHITE_WOOL.default_state);
        sink.put(x, y + 14, z, Block::LANTERN.default_state);
    } else if plan.heights.is_dry(x, z)
        && Dir::ALL.iter().any(|dir| {
            let nx = x + dir.dx();
            let nz = z + dir.dz();
            plan.occ.kind(nx, nz) == CellKind::Crane && crane_pad(plan, nx, nz) >= 8
        })
    {
        for h in 2..=11 {
            sink.put(x, y + h, z, Block::OAK_LOG.default_state);
        }
        sink.put(x, y + 12, z, Block::OAK_FENCE.default_state);
    }
}

fn place_breakwater(
    sink: &mut impl BlockSink,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let top = sea.max(ground);
    for y in (top - 2).max(sink.floor_min() + 2)..=top + 1 {
        let mix = (x.wrapping_mul(5) + z + y) as u8 % 3;
        sink.put(
            x,
            y,
            z,
            if mix == 0 {
                Block::STONE_BRICKS.default_state
            } else if mix == 1 {
                palette.cobble
            } else {
                palette.andesite
            },
        );
    }
    if (x + z) % 4 == 0 {
        sink.put(x, top + 2, z, palette.brick_wall);
    }
}

fn place_bandit_ravine(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let across = matches!(plan.occ.kind(x + 1, z), CellKind::SiegeTrench)
        && matches!(plan.occ.kind(x - 1, z), CellKind::SiegeTrench);
    let along = matches!(plan.occ.kind(x, z + 1), CellKind::SiegeTrench)
        && matches!(plan.occ.kind(x, z - 1), CellKind::SiegeTrench);
    let keel = across || along;
    let depth = if keel { 5 } else { 3 };
    let bed = ground - depth;
    sink.put(x, bed, z, palette.andesite);
    if keel && (x + z) % 4 == 0 {
        sink.put(x, bed + 1, z, Block::WATER.default_state);
    } else {
        sink.put(x, bed + 1, z, Block::COARSE_DIRT.default_state);
    }
    for y in bed + 2..=ground {
        sink.put(x, y, z, Block::AIR.default_state);
    }
    if !keel {
        sink.put(x, ground - 1, z, palette.cobble);
    }
    let crossing = Dir::ALL.iter().any(|dir| {
        matches!(
            plan.occ.kind(x + dir.dx(), z + dir.dz()),
            CellKind::CampCourt | CellKind::PalisadeGate
        )
    });
    if crossing {
        sink.put(x, ground, z, Block::SPRUCE_LOG.default_state);
        sink.put(x, ground + 1, z, Block::SPRUCE_FENCE.default_state);
        return;
    }
    clear_above(sink, x, ground + 1, ground + 6, z);
}

fn place_siege_trench(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground - 2, z, palette.andesite);
    sink.put(x, ground - 1, z, Block::COARSE_DIRT.default_state);
    sink.put(x, ground, z, Block::AIR.default_state);
    if (x + z) % 3 == 0 {
        sink.put(x, ground - 1, z, Block::COBBLESTONE.default_state);
    }
    clear_above(sink, x, ground + 1, ground + 6, z);
}

fn place_city_ditch(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let sea = plan.heights.sea_level;
    let wet = plan.heights.height(x, z) <= sea + 2 || !plan.heights.is_dry(x, z);
    if wet {
        let water_y = sea.min(ground);
        for y in water_y - 3..=water_y {
            sink.put(x, y, z, Block::WATER.default_state);
        }
        sink.put(x, water_y - 4, z, palette.andesite);
        let inner = Dir::ALL.iter().any(|dir| {
            matches!(
                plan.occ.kind(x + dir.dx(), z + dir.dz()),
                CellKind::TownWall | CellKind::TownTower | CellKind::TownGate
            )
        });
        if inner {
            sink.put(x, water_y, z, palette.city_masonry(1));
            sink.put(x, water_y + 1, z, palette.city_masonry(2));
            if (x + z) & 1 == 0 {
                sink.put(x, water_y + 2, z, palette.brick_wall);
            }
        }
        clear_above(sink, x, water_y + 1, water_y + 8, z);
        return;
    }
    sink.put(x, ground - 3, z, palette.andesite);
    sink.put(x, ground - 2, z, Block::COARSE_DIRT.default_state);
    sink.put(x, ground - 1, z, Block::COARSE_DIRT.default_state);
    sink.put(x, ground, z, Block::AIR.default_state);
    if (x + z) % 4 == 0 {
        sink.put(x, ground - 1, z, palette.cobble);
    } else if (x + z) % 4 == 2 {
        sink.put(x, ground - 1, z, palette.mossy);
    }
    clear_above(sink, x, ground + 1, ground + 8, z);
}

fn in_frontier(plan: &TownPlan, x: i32, z: i32) -> bool {
    plan.scenes
        .iter()
        .any(|scene| scene.kind == crate::scenes::SceneKind::Frontier && scene.rect.contains(x, z))
}

fn in_caravanserai(plan: &TownPlan, x: i32, z: i32) -> bool {
    plan.scenes.iter().any(|scene| {
        scene.kind == crate::scenes::SceneKind::Caravanserai && scene.rect.contains(x, z)
    })
}

fn place_minaret(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let rect = blob_of(plan, x, z, CellKind::TownTower);
    let (cx, cz) = rect.center();
    let dist = (x - cx).abs().max((z - cz).abs());
    let top = floor + 16;
    if dist == 0 {
        for y in floor..=top + 4 {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, top + 5, z, palette.brick_wall);
        sink.put(x, top + 6, z, Block::LANTERN.default_state);
        return;
    }
    for y in floor..=top {
        sink.put(x, y, z, palette.bricks);
    }
    if (x + z) % 2 == 0 {
        sink.put(x, floor + 8, z, Block::GLASS_PANE.default_state);
        sink.put(x, floor + 12, z, Block::GLASS_PANE.default_state);
    }
    let roof_y = top + 1 + (2 - dist).max(0) * 2;
    for y in top + 1..roof_y {
        sink.put(x, y, z, roof_cap_block(palette));
    }
    let dir = if (x - cx).abs() >= (z - cz).abs() {
        if x < cx { Dir::West } else { Dir::East }
    } else if z < cz {
        Dir::North
    } else {
        Dir::South
    };
    sink.put(x, roof_y, z, stair_state(palette.roof, dir));
}

fn place_scarred_curtain(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let mix = (x.wrapping_mul(17) + z.wrapping_mul(11) + plan.seed as i32).unsigned_abs();
    let height = if mix % 5 == 0 { 3 } else { 6 };
    let top = floor + height;
    for y in floor..=top {
        let v = ((x.wrapping_mul(3) + z + y) as u8) & 15;
        sink.put(x, y, z, palette.foundation(v % 5 == 0));
    }
    if mix % 4 == 0 {
        for y in floor + 2..=floor + 4 {
            sink.put(x, y, z, Block::AIR.default_state);
        }
        sink.put(x, floor + 1, z, Block::COBBLESTONE.default_state);
        sink.put(x, ground + 1, z, Block::COBBLESTONE.default_state);
    }
    if mix % 3 == 0 {
        sink.put(x, ground, z, Block::COAL_BLOCK.default_state);
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
    }
    if mix % 2 == 0 {
        sink.put(x, top + 1, z, palette.brick_wall);
    }
}

fn place_collapsed_tower(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let mix = (x.wrapping_mul(23) + z.wrapping_mul(5) + plan.seed as i32).unsigned_abs();
    let top = floor + 5 + (mix % 4) as i32;
    let edge = Dir::ALL
        .iter()
        .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::TownTower);
    if edge {
        for y in floor..=top {
            if mix % 3 == 0 && y >= top - 2 {
                sink.put(x, y, z, Block::COBBLESTONE.default_state);
            } else {
                sink.put(x, y, z, palette.foundation(false));
            }
        }
        if mix % 2 == 0 {
            sink.put(x, ground + 1, z, Block::COBBLESTONE.default_state);
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, top, z, Block::COBBLESTONE.default_state);
    }
}

fn apply_column_damage(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    z: i32,
    kind: CellKind,
    floor: i32,
    ground: i32,
    palette: &Palette,
) {
    let state = crate::damage::damage_state_for_plan(plan);
    if !state.is_damaged() {
        return;
    }
    let ruined = state == crate::damage::DamageState::Ruined;
    let mix = (x.wrapping_mul(29) + z.wrapping_mul(13) + plan.seed as i32).unsigned_abs();
    let hole = if ruined { mix % 3 == 0 } else { mix % 5 == 0 };
    match kind {
        CellKind::TownWall
        | CellKind::CastleWall
        | CellKind::CastleInnerWall
        | CellKind::TownTower
        | CellKind::CastleTower => {
            if hole {
                for y in floor + 2..=floor + 4 {
                    sink.put(x, y, z, Block::AIR.default_state);
                }
                sink.put(x, ground + 1, z, Block::COBBLESTONE.default_state);
            }
            if mix % 4 == 0 {
                sink.put(x, ground, z, Block::COAL_BLOCK.default_state);
            }
        }
        CellKind::Building if hole => {
            sink.put(x, ground, z, Block::COAL_BLOCK.default_state);
            if ruined {
                sink.put(x, floor + 3, z, Block::AIR.default_state);
                sink.put(x, floor + 4, z, Block::AIR.default_state);
            }
        }
        _ => {}
    }
    let _ = palette;
}

fn place_tourney_list(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, Block::SAND.default_state);
    clear_above(sink, x, ground + 1, ground + 6, z);
    let rect = blob_of(plan, x, z, CellKind::TourneyList);
    let along_x = rect.width() >= rect.depth();
    let (cx, cz) = rect.center();
    let rail = if along_x { z == cz } else { x == cx };
    let along = if along_x {
        x - rect.min_x
    } else {
        z - rect.min_z
    };
    let last = if along_x {
        rect.width() - 1
    } else {
        rect.depth() - 1
    };
    let end = along == 0 || along == last;
    if end {
        if rail {
            sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
            sink.put(x, ground + 2, z, Block::OAK_FENCE_GATE.default_state);
            sink.put(x, ground + 3, z, Block::OAK_FENCE.default_state);
        } else {
            for y in ground + 1..=ground + 4 {
                sink.put(x, y, z, palette.log);
            }
            sink.put(x, ground + 5, z, Block::RED_BANNER.default_state);
            if (x + z) % 2 == 0 {
                sink.put(x, ground + 1, z, Block::GRINDSTONE.default_state);
            } else {
                sink.put(x, ground + 1, z, Block::BARREL.default_state);
            }
            sink.put(x, ground + 2, z, Block::OAK_FENCE.default_state);
        }
        return;
    }
    if rail {
        sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, palette.slab);
        if along % 8 == 0 {
            sink.put(x, ground + 2, z, Block::OAK_FENCE.default_state);
            sink.put(x, ground + 3, z, Block::RED_BANNER.default_state);
        }
    }
}

fn place_tourney_pavilion(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.path);
    let rect = blob_of(plan, x, z, CellKind::Tent);
    let along_x = rect.width() >= rect.depth();
    let (cx, cz) = rect.center();
    let ridge_dist = if along_x {
        (z - cz).abs()
    } else {
        (x - cx).abs()
    };
    let h = (5 - ridge_dist).max(3);
    let stripe = if along_x { x } else { z };
    let dye = (rect.min_x + rect.min_z).rem_euclid(4);
    let (a, b, pennant) = match dye {
        0 => (
            Block::RED_WOOL.default_state,
            Block::WHITE_WOOL.default_state,
            Block::RED_BANNER.default_state,
        ),
        1 => (
            Block::RED_WOOL.default_state,
            Block::YELLOW_WOOL.default_state,
            Block::YELLOW_BANNER.default_state,
        ),
        2 => (
            Block::BLUE_WOOL.default_state,
            Block::WHITE_WOOL.default_state,
            Block::BLUE_BANNER.default_state,
        ),
        _ => (
            Block::YELLOW_WOOL.default_state,
            Block::BLACK_WOOL.default_state,
            Block::BLACK_BANNER.default_state,
        ),
    };
    let wool = if stripe % 2 == 0 { a } else { b };
    let edge = rect.on_edge(x, z);
    if rect.is_corner(x, z) {
        for y in ground + 1..=ground + h {
            sink.put(x, y, z, Block::DARK_OAK_FENCE.default_state);
        }
        sink.put(x, ground + h, z, wool);
        sink.put(x, ground + h + 1, z, pennant);
        return;
    }
    if edge {
        for y in ground + 1..=ground + h {
            sink.put(x, y, z, wool);
        }
        return;
    }
    // Fill the canopy so adjacent columns still 6-connect when the pad steps.
    for y in ground + 2..=ground + h {
        sink.put(x, y, z, wool);
    }
    if ridge_dist == 0 && x == cx && z == cz {
        sink.put(x, ground + 1, z, Block::CHEST.default_state);
        sink.put(x, ground + h + 1, z, pennant);
    }
}

fn place_royal_box(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    rect: Rect,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let owner = crate::siege::TownOwner::kingdom(&crate::faction::KingdomId::for_cell(
        plan.origin.cell_x,
        plan.origin.cell_z,
    ));
    let cloth = crate::siege::owner_wool(&owner).default_state;
    let flag = crate::siege::owner_banner(&owner).default_state;
    let floor = ground + 2;
    let eaves = floor + 3;
    let pitch = 1;
    let (roof_y, _) = gable_roof_y_pitch(rect, x, z, eaves, 1, pitch);
    foundation(sink, x, floor, ground, z, palette);
    sink.put(x, floor, z, palette.planks);
    let edge = rect.on_edge(x, z);
    let (cx, cz) = rect.center();
    if rect.is_corner(x, z) {
        for y in ground + 1..=roof_y {
            sink.put(x, y, z, palette.log);
        }
        sink.put(x, roof_y + 1, z, flag);
    } else if edge {
        for y in floor + 1..=roof_y {
            sink.put(
                x,
                y,
                z,
                if (y - floor) % 2 == 0 {
                    palette.planks
                } else {
                    palette.fence
                },
            );
        }
        if (x + z) % 3 == 0 {
            sink.put(x, eaves + 1, z, flag);
        }
        // Stairs down to the lists on the long inner edge.
        let list_side = Dir::ALL.iter().any(|dir| {
            plan.occ.kind(x + dir.dx(), z + dir.dz()) == CellKind::TourneyList
                || plan.occ.kind(x + dir.dx() * 2, z + dir.dz() * 2) == CellKind::TourneyList
        });
        if list_side {
            sink.put(x, ground + 1, z, stair_state(palette.stairs, Dir::South));
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 1, z);
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::OAK_STAIRS.default_state);
            sink.put(x, roof_y - 1, z, Block::LANTERN.default_state);
        } else if (x + z) % 4 == 0 {
            sink.put(x, floor + 1, z, cloth);
        }
    }
    place_roof_pitched(sink, rect, x, eaves, z, palette, 1, pitch);
    if (x == cx && (z == rect.min_z || z == rect.max_z))
        || (z == cz && (x == rect.min_x || x == rect.max_x))
    {
        sink.put(x, roof_y + 1, z, flag);
    }
}

fn place_tourney_stand(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::TourneyStand);
    if rect.width() <= 8 && rect.depth() <= 6 {
        place_royal_box(sink, plan, rect, x, ground, z, palette);
        return;
    }
    let (cx, cz) = rect.center();
    let face = if rect.width() >= rect.depth() {
        if matches!(plan.occ.kind(cx, rect.min_z - 2), CellKind::TourneyList)
            || matches!(plan.occ.kind(cx, rect.min_z - 1), CellKind::TourneyList)
        {
            Dir::North
        } else {
            Dir::South
        }
    } else if matches!(plan.occ.kind(rect.min_x - 2, cz), CellKind::TourneyList)
        || matches!(plan.occ.kind(rect.min_x - 1, cz), CellKind::TourneyList)
    {
        Dir::West
    } else {
        Dir::East
    };
    let from_front = match face {
        Dir::North => z - rect.min_z,
        Dir::South => rect.max_z - z,
        Dir::West => x - rect.min_x,
        Dir::East => rect.max_x - x,
    };
    let tier = from_front.clamp(0, 2);
    sink.put(x, ground, z, palette.planks);
    let y = ground + 1 + tier;
    clear_above(sink, x, y + 1, ground + 8, z);
    sink.put(x, y, z, stair_state(palette.stairs, face));
    if tier == 1 {
        sink.put(x, y, z, palette.planks);
    }
    if tier == 2 {
        sink.put(x, y + 1, z, palette.fence);
        if (x + z) % 4 == 0 {
            sink.put(x, y + 2, z, Block::RED_BANNER.default_state);
        }
    }
}

fn place_water_wheel(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    const WHEEL_R: i32 = 5;
    let sea = plan.heights.sea_level;
    let water_y = plan.heights.height(x, z).min(sea).max(ground.min(sea));
    let mill = plan
        .lots
        .iter()
        .filter(|lot| lot.kind == BuildingKind::Watermill)
        .min_by_key(|lot| {
            let (cx, cz) = lot.rect.center();
            (x - cx).abs() + (z - cz).abs()
        });
    let (along, out) = mill.map_or_else(
        || {
            let blob = blob_of(plan, x, z, CellKind::WaterWheel);
            let (cx, cz) = blob.center();
            if blob.width() >= blob.depth() {
                (x - cx, (z - blob.min_z).min(blob.max_z - z) + 1)
            } else {
                (z - cz, (x - blob.min_x).min(blob.max_x - x) + 1)
            }
        },
        |mill| {
            let (cx, cz) = mill.rect.center();
            let along = if matches!(mill.facing, Dir::East | Dir::West) {
                z - cz
            } else {
                x - cx
            };
            let out = match mill.facing {
                Dir::West => x - mill.rect.max_x,
                Dir::East => mill.rect.min_x - x,
                Dir::North => z - mill.rect.max_z,
                Dir::South => mill.rect.min_z - z,
            };
            (along, out)
        },
    );
    // Carve a well so bank dirt cannot hide the disk.
    let hub = sea;
    clear_above(sink, x, hub - WHEEL_R, hub + WHEEL_R + 2, z);
    sink.put(x, water_y.saturating_sub(1), z, palette.cobble);
    if water_y <= sea {
        sink.put(x, water_y, z, Block::WATER.default_state);
        for y in (water_y + 1)..=(hub + WHEEL_R + 2) {
            sink.put(x, y, z, Block::AIR.default_state);
        }
    }
    if out <= 2 {
        // Gap and visible axle standing out from the mill wall.
        if along == 0 {
            sink.put(x, hub, z, Block::STRIPPED_OAK_LOG.default_state);
        }
        return;
    }
    // Standing disk, at least 9 across, half below the waterline.
    for dy in -WHEEL_R..=WHEEL_R {
        let d2 = along * along + dy * dy;
        if d2 > WHEEL_R * WHEEL_R {
            continue;
        }
        let y = hub + dy;
        let on_axis = along == 0 || dy == 0;
        let on_diag = along.abs() == dy.abs();
        let spoke = on_axis || on_diag;
        let rim = d2 >= (WHEEL_R - 1) * (WHEEL_R - 1);
        let paddle = rim && spoke;
        let hub_cell = along == 0 && dy == 0;
        // Fill the disk (pale birch) so an iso at 45° still reads as a wheel,
        // not a handful of rim planks beside the mill.
        let block = if hub_cell {
            Block::STRIPPED_OAK_LOG.default_state
        } else if paddle {
            Block::DARK_OAK_PLANKS.default_state
        } else if rim {
            Block::STRIPPED_DARK_OAK_LOG.default_state
        } else {
            Block::BIRCH_PLANKS.default_state
        };
        sink.put(x, y, z, block);
    }
}

fn place_shrine(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.cobble);
    clear_above(sink, x, ground + 1, ground + 6, z);
    let centre = Dir::ALL
        .iter()
        .all(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) == CellKind::Shrine);
    sink.put(x, ground + 1, z, palette.bricks);
    if centre {
        sink.put(x, ground + 2, z, palette.brick_wall);
        sink.put(x, ground + 3, z, palette.brick_wall);
        sink.put(x, ground + 4, z, palette.brick_wall);
        sink.put(x, ground + 5, z, Block::RED_BANNER.default_state);
    } else {
        let face = Dir::ALL
            .iter()
            .copied()
            .find(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) != CellKind::Shrine)
            .unwrap_or(Dir::South);
        sink.put(x, ground + 2, z, stair_state(palette.brick_stairs, face));
        if (x + z) % 3 == 0 {
            sink.put(x, ground + 3, z, Block::LANTERN.default_state);
        }
    }
}

fn place_field_wall(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 4, z);
    if plan.has_scene(x, z, crate::scenes::SceneKind::HuntingLodge) {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, palette.fence);
        if (x + z) % 5 == 0 {
            sink.put(x, ground + 3, z, palette.log);
        }
        return;
    }
    sink.put(x, ground + 1, z, palette.cobble);
    if (x + z) % 2 == 0 {
        sink.put(x, ground + 2, z, palette.cobble_wall);
    } else {
        sink.put(x, ground + 2, z, palette.andesite);
    }
}

fn mill_channel(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::MillRace | CellKind::Sluice | CellKind::WaterWheel | CellKind::Harbor
    )
}

fn place_mill_race(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let water_y = sea.min(ground + 1);
    sink.put(x, water_y - 1, z, palette.cobble);
    let bank = Dir::ALL
        .iter()
        .any(|dir| !mill_channel(plan.occ.kind(x + dir.dx(), z + dir.dz())));
    if bank {
        sink.put(x, water_y, z, palette.cobble);
        sink.put(x, water_y + 1, z, palette.cobble_wall);
        clear_above(sink, x, water_y + 2, water_y + 8, z);
    } else {
        sink.put(x, water_y, z, Block::WATER.default_state);
        clear_above(sink, x, water_y + 1, water_y + 8, z);
    }
}

fn place_sluice(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    place_mill_race(sink, plan, x, ground, z, sea, palette);
    let water_y = sea.min(ground + 1);
    sink.put(x, water_y + 1, z, Block::OAK_TRAPDOOR.default_state);
    sink.put(x, water_y + 2, z, palette.fence);
}

fn place_retaining(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let planned = plan.heights.height(x, z).max(ground);
    sink.put(x, ground, z, palette.cobble);
    for y in ground + 1..=planned.min(ground + 4) {
        sink.put(x, y, z, palette.cobble);
    }
    sink.put(x, ground + 1, z, palette.cobble);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 5, ground + 10, z);
    if (x + z) % 5 == 0 {
        sink.put(x, ground + 5, z, Block::LANTERN.default_state);
    }
}

fn place_vine(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    terrace_edge(sink, plan, x, ground, z, palette);
    let mix = (x + z).rem_euclid(4);
    match mix {
        0 => {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.fence);
            sink.put(x, ground + 3, z, palette.leaves);
            sink.put(x, ground + 4, z, Block::PURPLE_WOOL.default_state);
        }
        1 => {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.leaves);
            sink.put(x, ground + 3, z, palette.leaves);
        }
        2 => {
            sink.put(x, ground + 1, z, Block::SWEET_BERRY_BUSH.default_state);
            sink.put(x, ground + 2, z, palette.leaves);
        }
        _ => {
            sink.put(x, ground + 1, z, palette.fence);
            sink.put(x, ground + 2, z, palette.leaves);
            sink.put(x, ground + 3, z, Block::PURPLE_WOOL.default_state);
        }
    }
    clear_above(sink, x, ground + 5, ground + 8, z);
}

fn place_lighthouse(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Lighthouse);
    let (cx, cz) = rect.center();
    let dx = x - cx;
    let dz = z - cz;
    let cheb = dx.abs().max(dz.abs());
    let manh = dx.abs() + dz.abs();
    let base = (sea - 2).max(sink.floor_min() + 2);
    let gallery = sea.max(ground) + 13;
    let lantern = gallery + 1;
    let cap = lantern + 3;
    for y in base..=cap {
        let taper = if y < gallery - 1 { 2 } else { 1 };
        let in_ring = cheb <= taper && manh <= taper + 1;
        let interior = cheb == 0;
        if y >= lantern {
            if interior {
                sink.put(
                    x,
                    y,
                    z,
                    if y == lantern + 1 {
                        Block::GLOWSTONE.default_state
                    } else {
                        Block::SEA_LANTERN.default_state
                    },
                );
            } else if cheb == 1 {
                sink.put(x, y, z, Block::GLASS.default_state);
            } else if y == lantern && cheb == 2 {
                sink.put(x, y, z, palette.fence);
            }
            continue;
        }
        if y == gallery && cheb == 2 {
            sink.put(x, y, z, palette.fence);
            continue;
        }
        if !in_ring {
            continue;
        }
        if interior {
            if y > sea {
                sink.put(x, y, z, Block::LADDER.default_state);
            }
            continue;
        }
        let band = y.rem_euclid(5);
        sink.put(
            x,
            y,
            z,
            if band == 0 {
                Block::STONE_BRICKS.default_state
            } else if band == 1 {
                Block::SANDSTONE.default_state
            } else if cheb == taper {
                palette.cobble
            } else {
                palette.andesite
            },
        );
    }
    if cheb <= 1 {
        sink.put(x, cap + 1, z, palette.brick_wall);
        sink.put(x, cap + 2, z, Block::LANTERN.default_state);
    }
    if dx == 0 && dz == 0 {
        sink.put(x, cap + 3, z, Block::LANTERN.default_state);
    }
}

fn mill_water_face(lot: &Lot, x: i32, z: i32) -> bool {
    match lot.facing {
        Dir::West => x == lot.rect.max_x,
        Dir::East => x == lot.rect.min_x,
        Dir::North => z == lot.rect.max_z,
        Dir::South => z == lot.rect.min_z,
    }
}

fn place_watermill(
    sink: &mut impl BlockSink,
    _plan: &TownPlan,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let eaves = floor + lot.kind.wall_h();
    let desert = palette.kind == PaletteKind::Sandstone;
    let timber = lot.kind.timber_frame() && !desert;
    let pitch = roof_pitch(palette);
    let (roof_y, _) = gable_roof_y_pitch(r, x, z, eaves, 1, pitch);
    let plaster = house_plaster(lot, palette);
    let edge = r.on_edge(x, z);
    let front = is_front(lot, x, z);
    let door = front && x == door_x(lot) && z == door_z(lot);
    let (cx, cz) = r.center();
    let axle = mill_water_face(lot, x, z)
        && !r.is_corner(x, z)
        && if matches!(lot.facing, Dir::East | Dir::West) {
            z == cz
        } else {
            x == cx
        };
    if edge {
        sink.put(x, floor, z, palette.cobble);
        sink.put(
            x,
            floor + 1,
            z,
            if desert {
                palette.bricks
            } else {
                palette.foundation((x + z) % 5 == 0)
            },
        );
        for y in floor + 2..=roof_y {
            let stud = match lot.facing {
                Dir::North | Dir::South => (x - r.min_x) % 3 == 0,
                Dir::East | Dir::West => (z - r.min_z) % 3 == 0,
            };
            let b = if desert {
                palette.bricks
            } else if r.is_corner(x, z) || (timber && stud) {
                palette.log
            } else if window_here(lot, x, z, y - floor) && y < eaves {
                Block::GLASS_PANE.default_state
            } else if timber {
                plaster
            } else {
                palette.planks
            };
            sink.put(x, y, z, b);
        }
        if r.is_corner(x, z) {
            for y in floor..=roof_y {
                sink.put(x, y, z, if desert { palette.bricks } else { palette.log });
            }
        }
        if axle {
            sink.put(x, floor + 1, z, palette.stripped);
        }
        if door && !axle {
            interior::place_door(sink, x, floor + 1, z, lot.facing, false);
            sink.put(
                x,
                floor + 3,
                z,
                if desert { palette.bricks } else { palette.log },
            );
        }
    } else {
        clear_above(sink, x, floor + 1, roof_y - 1, z);
        sink.put(x, floor, z, palette.planks);
        interior::furnish_house(sink, lot.kind, r, x, floor, z, roof_y, 1, palette);
    }
    place_roof_pitched(sink, r, x, eaves, z, palette, 1, pitch);
    clear_hill_above_roof(sink, x, roof_y.max(eaves + 1), ground, z, false);
}

fn place_winepress(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_house(sink, lot, x, floor, ground, z, palette);
    let r = lot.rect;
    if r.on_edge(x, z) {
        for y in floor - 3..floor {
            sink.put(x, y, z, palette.cobble);
        }
        return;
    }
    let (cx, cz) = r.center();
    sink.put(x, floor - 3, z, palette.cobble);
    sink.put(x, floor - 2, z, Block::AIR.default_state);
    sink.put(x, floor - 1, z, Block::AIR.default_state);
    if x == cx && z == cz {
        sink.put(x, floor, z, Block::OAK_TRAPDOOR.default_state);
        sink.put(x, floor - 2, z, Block::BARREL.default_state);
    } else if (x + z) % 3 == 0 {
        sink.put(x, floor - 2, z, Block::BARREL.default_state);
    }
}

fn place_bridge(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let blob = blob_of(plan, x, z, CellKind::Bridge);
    let along_x = blob.width() >= blob.depth();
    let edge = if along_x {
        !matches!(plan.occ.kind(x, z - 1), CellKind::Bridge)
            || !matches!(plan.occ.kind(x, z + 1), CellKind::Bridge)
    } else {
        !matches!(plan.occ.kind(x - 1, z), CellKind::Bridge)
            || !matches!(plan.occ.kind(x + 1, z), CellKind::Bridge)
    };
    let mut deck = sea.max(ground) + 3;
    for dir in Dir::ALL {
        for step in 2..=6 {
            let nx = x + dir.dx() * step;
            let nz = z + dir.dz() * step;
            if plan.heights.is_dry(nx, nz)
                && matches!(
                    plan.occ.kind(nx, nz),
                    CellKind::Road
                        | CellKind::TownGate
                        | CellKind::TownTower
                        | CellKind::Building
                        | CellKind::Green
                )
            {
                deck = deck.max(plan.heights.height(nx, nz) + 1);
            }
        }
    }
    let along = if along_x { x } else { z };
    let pier = along.rem_euclid(7) == 0;
    let foot = (sea - 4).min(ground).max(sink.floor_min() + 2);
    if pier {
        for y in foot..=deck {
            sink.put(
                x,
                y,
                z,
                if y <= sea {
                    palette.cobble
                } else {
                    palette.bricks
                },
            );
        }
    } else {
        let phase = along.rem_euclid(7);
        let mid = phase.min(7 - phase);
        let arch_y = (deck - 3 + mid).min(deck - 1);
        sink.put(x, foot.max(sea - 1), z, palette.cobble);
        if arch_y > sea {
            sink.put(
                x,
                arch_y,
                z,
                stair_state(
                    palette.cobble_stairs,
                    if along_x { Dir::East } else { Dir::South },
                ),
            );
        }
    }
    sink.put(x, deck, z, palette.bricks);
    clear_above(sink, x, deck + 1, deck + 6, z);
    if edge {
        sink.put(x, deck + 1, z, palette.brick_wall);
        if along % 4 == 0 {
            sink.put(x, deck + 2, z, Block::LANTERN.default_state);
        }
    }
}

fn place_town_hall(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_house(sink, lot, x, floor, ground, z, palette);
    let r = lot.rect;
    let tower = x >= r.max_x - 2 && z <= r.min_z + 2;
    if !tower {
        return;
    }
    let top = floor + 22;
    for y in floor + lot.kind.wall_h()..=top {
        sink.put(
            x,
            y,
            z,
            if r.is_corner(x, z) {
                palette.log
            } else if y == floor + 18 && (x + z) % 2 == 0 {
                Block::YELLOW_TERRACOTTA.default_state
            } else if y == floor + 18 {
                Block::BLACK_WOOL.default_state
            } else {
                palette.bricks
            },
        );
    }
    if r.is_corner(x, z) {
        sink.put(x, top + 1, z, palette.brick_wall);
        sink.put(x, top + 2, z, Block::LANTERN.default_state);
    } else if x == r.max_x - 1 && z == r.min_z + 1 {
        sink.put(x, top + 1, z, Block::GOLD_BLOCK.default_state);
    }
}

fn place_stall(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let r = lot.rect;
    sink.put(x, ground, z, palette.planks);
    let dye = match lot.id % 4 {
        0 => Block::RED_WOOL.default_state,
        1 => Block::YELLOW_WOOL.default_state,
        2 => Block::BLUE_WOOL.default_state,
        _ => Block::GREEN_WOOL.default_state,
    };
    if r.is_corner(x, z) {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, palette.fence);
    } else if r.on_edge(x, z) {
        sink.put(x, ground + 1, z, palette.fence);
    } else {
        let mix = (x.wrapping_mul(5) + z.wrapping_mul(11) + lot.id as i32).unsigned_abs();
        sink.put(
            x,
            ground + 1,
            z,
            match mix % 5 {
                0 => Block::BARREL.default_state,
                1 => Block::HAY_BLOCK.default_state,
                2 => Block::COMPOSTER.default_state,
                3 => Block::CHEST.default_state,
                _ => palette.planks,
            },
        );
    }
    sink.put(x, ground + 3, z, dye);
}

fn is_front(lot: &Lot, x: i32, z: i32) -> bool {
    match lot.facing {
        Dir::North => z == lot.rect.min_z,
        Dir::South => z == lot.rect.max_z,
        Dir::West => x == lot.rect.min_x,
        Dir::East => x == lot.rect.max_x,
    }
}

/// Offline volume used by the renderer.
pub struct VolumeSink {
    pub min_x: i32,
    pub min_y: i32,
    pub min_z: i32,
    pub sx: i32,
    pub sy: i32,
    pub sz: i32,
    pub voxels: Vec<Option<pumpkin_data::BlockStateId>>,
}

impl VolumeSink {
    /// Empty volume covering `plan` plus a vertical band around the surface.
    #[must_use]
    pub fn for_plan(plan: &TownPlan, y_pad: i32) -> Self {
        let (lo, hi) = plan.heights.range(
            plan.bounds.min_x,
            plan.bounds.min_z,
            plan.bounds.max_x,
            plan.bounds.max_z,
        );
        Self::for_bounds(
            plan.bounds.min_x,
            plan.bounds.min_z,
            plan.bounds.max_x,
            plan.bounds.max_z,
            lo - 12,
            hi + 48 + y_pad,
        )
    }

    /// Empty volume covering an arbitrary box.
    #[must_use]
    pub fn for_bounds(
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
        min_y: i32,
        max_y: i32,
    ) -> Self {
        let sx = max_x - min_x + 1;
        let sz = max_z - min_z + 1;
        let sy = max_y - min_y + 1;
        let n = (sx * sy * sz).max(0) as usize;
        Self {
            min_x,
            min_y,
            min_z,
            sx,
            sy,
            sz,
            voxels: vec![None; n],
        }
    }

    fn idx(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let lx = x - self.min_x;
        let ly = y - self.min_y;
        let lz = z - self.min_z;
        if lx < 0 || ly < 0 || lz < 0 || lx >= self.sx || ly >= self.sy || lz >= self.sz {
            return None;
        }
        Some(((ly * self.sz + lz) * self.sx + lx) as usize)
    }

    /// Block at `(x,y,z)` if present.
    #[must_use]
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<&'static BlockState> {
        self.idx(x, y, z)
            .and_then(|i| self.voxels[i])
            .map(BlockState::from_id)
    }

    /// Highest non-air block in the column.
    #[must_use]
    pub fn top(&self, x: i32, z: i32) -> Option<(i32, &'static BlockState)> {
        for y in (self.min_y..self.min_y + self.sy).rev() {
            if let Some(state) = self.get(x, y, z)
                && !state.is_air()
            {
                return Some((y, state));
            }
        }
        None
    }
}

impl BlockSink for VolumeSink {
    fn put(&mut self, x: i32, y: i32, z: i32, state: &BlockState) {
        if let Some(i) = self.idx(x, y, z) {
            self.voxels[i] = Some(state.id);
        }
    }

    fn floor_min(&self) -> i32 {
        self.min_y
    }
}

/// Fills terrain from the plan height grid, then stamps the town.
#[must_use]
pub fn fill_volume(plan: &TownPlan) -> VolumeSink {
    let mut volume = VolumeSink::for_plan(plan, 16);
    let palette = Palette::new(plan.palette);
    let sea = plan.heights.sea_level;
    for z in plan.bounds.min_z..=plan.bounds.max_z {
        for x in plan.bounds.min_x..=plan.bounds.max_x {
            let surface = plan.heights.height(x, z);
            let ground = surface - 1;
            let kind = plan.occ.kind(x, z);
            let mut bottom = ground - 3;
            for dir in Dir::ALL {
                let nh = plan.heights.height(x + dir.dx(), z + dir.dz());
                if nh < surface - 2 {
                    bottom = bottom.min(nh - 1);
                }
            }
            bottom = bottom.max(ground - 28).max(volume.min_y);
            for y in bottom..=ground {
                let dy = ground - y;
                let block = if dy == 0 && (occupy_raises(kind) || ground > sea) {
                    palette.grass
                } else if dy >= 3 {
                    palette.andesite
                } else {
                    Block::DIRT.default_state
                };
                volume.put(x, y, z, block);
            }
            if !occupy_raises(kind) && ground < sea {
                for y in ground + 1..=sea {
                    volume.put(x, y, z, Block::WATER.default_state);
                }
            }
            stamp_column(&mut volume, plan, x, z, surface, None);
        }
    }
    volume
}

/// One intended town block. Repair bots can diff this against the live world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlannedBlock {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Vanilla [`BlockStateId`] as `u16`.
    pub state: u16,
}

impl PlannedBlock {
    /// Resolves the vanilla block state.
    #[must_use]
    pub fn block_state(self) -> &'static BlockState {
        BlockStateId::new_or_air(self.state).to_state()
    }
}

struct CollectingSink {
    min_y: i32,
    blocks: BTreeMap<(i32, i32, i32), u16>,
}

impl BlockSink for CollectingSink {
    fn put(&mut self, x: i32, y: i32, z: i32, state: &BlockState) {
        self.blocks.insert((x, y, z), state.id.as_u16());
    }

    fn floor_min(&self) -> i32 {
        self.min_y
    }
}

impl CollectingSink {
    fn new(min_y: i32) -> Self {
        Self {
            min_y,
            blocks: BTreeMap::new(),
        }
    }

    fn into_blocks(self) -> Vec<PlannedBlock> {
        self.blocks
            .into_iter()
            .map(|((x, y, z), state)| PlannedBlock { x, y, z, state })
            .collect()
    }
}

fn planned_surface(plan: &TownPlan, x: i32, z: i32) -> i32 {
    plan.heights.height(x, z)
}

/// Intended town blocks for one column, from the plan height field (no chunk gen).
#[must_use]
pub fn planned_column(plan: &TownPlan, x: i32, z: i32) -> Vec<PlannedBlock> {
    if column_kind(plan, x, z) == CellKind::Empty {
        return Vec::new();
    }
    let mut sink = CollectingSink::new(plan.heights.sea_level - 16);
    stamp_column(&mut sink, plan, x, z, planned_surface(plan, x, z), None);
    sink.into_blocks()
}

/// Intended block at `(x, y, z)`, or `None` if the plan does not write that cell.
#[must_use]
pub fn planned_block(plan: &TownPlan, x: i32, y: i32, z: i32) -> Option<PlannedBlock> {
    planned_column(plan, x, z)
        .into_iter()
        .find(|block| block.y == y)
}

/// Intended town blocks inside `bounds` (inclusive). No chunk generation.
#[must_use]
pub fn planned_blocks_in(plan: &TownPlan, bounds: crate::geom::Rect) -> Vec<PlannedBlock> {
    let mut sink = CollectingSink::new(plan.heights.sea_level - 16);
    let min_x = bounds.min_x.max(plan.bounds.min_x);
    let max_x = bounds.max_x.min(plan.bounds.max_x);
    let min_z = bounds.min_z.max(plan.bounds.min_z);
    let max_z = bounds.max_z.min(plan.bounds.max_z);
    if min_x > max_x || min_z > max_z {
        return Vec::new();
    }
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            if plan.occ.kind(x, z) == CellKind::Empty {
                continue;
            }
            stamp_column(&mut sink, plan, x, z, planned_surface(plan, x, z), None);
        }
    }
    sink.into_blocks()
}

/// Intended blocks of civilian lot `lot_id`.
#[must_use]
pub fn planned_building_blocks(plan: &TownPlan, lot_id: u16) -> Vec<PlannedBlock> {
    let Some(lot) = plan.lots.iter().find(|lot| lot.id == lot_id) else {
        return Vec::new();
    };
    planned_blocks_in(plan, lot.rect)
}

/// Intended block at world `(x, y, z)` for `seed`. Builds the cell plan from
/// noise heights if needed; never generates chunks.
#[must_use]
pub fn planned_block_at(seed: u64, x: i32, y: i32, z: i32) -> Option<PlannedBlock> {
    crate::plan::plan_at(seed, x, z)?.planned_block(x, y, z)
}

/// Intended town column at world `(x, z)` for `seed`.
#[must_use]
pub fn planned_column_at(seed: u64, x: i32, z: i32) -> Vec<PlannedBlock> {
    crate::plan::plan_at(seed, x, z)
        .map(|plan| plan.planned_column(x, z))
        .unwrap_or_default()
}

/// Intended blocks inside `bounds` for the town cell that contains the rect.
#[must_use]
pub fn planned_blocks_in_at(seed: u64, bounds: crate::geom::Rect) -> Vec<PlannedBlock> {
    let (cx, cz) = bounds.center();
    crate::plan::plan_at(seed, cx, cz)
        .map(|plan| plan.planned_blocks_in(bounds))
        .unwrap_or_default()
}

/// Intended blocks of civilian lot `lot_id` in `(seed, cell)`.
#[must_use]
pub fn planned_building_blocks_for(
    seed: u64,
    cell_x: i32,
    cell_z: i32,
    lot_id: u16,
) -> Vec<PlannedBlock> {
    crate::plan::plan_from_cache(seed, cell_x, cell_z)
        .map(|plan| plan.planned_building_blocks(lot_id))
        .unwrap_or_default()
}

impl TownPlan {
    /// Intended block at `(x, y, z)`, deterministically from this plan. No chunk gen.
    #[must_use]
    pub fn planned_block(&self, x: i32, y: i32, z: i32) -> Option<PlannedBlock> {
        planned_block(self, x, y, z)
    }

    /// Intended block at a world position.
    #[must_use]
    pub fn planned_at(&self, pos: BlockPos) -> Option<PlannedBlock> {
        self.planned_block(pos.0.x, pos.0.y, pos.0.z)
    }

    /// Intended town blocks in one column.
    #[must_use]
    pub fn planned_column(&self, x: i32, z: i32) -> Vec<PlannedBlock> {
        planned_column(self, x, z)
    }

    /// Intended town blocks in an inclusive XZ rectangle.
    #[must_use]
    pub fn planned_blocks_in(&self, bounds: crate::geom::Rect) -> Vec<PlannedBlock> {
        planned_blocks_in(self, bounds)
    }

    /// Intended blocks of a civilian lot. Iterate to repair one building.
    #[must_use]
    pub fn planned_building_blocks(&self, lot_id: u16) -> Vec<PlannedBlock> {
        planned_building_blocks(self, lot_id)
    }

    /// Intended blocks of the keep (castle core).
    #[must_use]
    pub fn planned_keep_blocks(&self) -> Vec<PlannedBlock> {
        planned_blocks_in(self, self.castle.keep)
    }
}

#[cfg(test)]
mod roof_tests {
    use super::*;
    use crate::castle::TowerShape;

    #[test]
    fn spire_faces_are_equal_height() {
        let spec = TowerSpec {
            cx: 0,
            cz: 0,
            radius: 10,
            height: 24,
            shape: TowerShape::Square,
            roof: RoofKind::Spire,
            inner: true,
        };
        let top = 80;
        let (cardinal, _) = mass_roof_y(spec, 10, 0, top);
        let (corner, _) = mass_roof_y(spec, 10, 10, top);
        let (mid_card, _) = mass_roof_y(spec, 5, 0, top);
        let (mid_diag, _) = mass_roof_y(spec, 5, 5, top);
        let (apex, _) = mass_roof_y(spec, 0, 0, top);
        assert_eq!(cardinal, corner, "eaves must match on every face");
        assert_eq!(mid_card, mid_diag, "equal rings must sit at equal height");
        assert!(apex > cardinal, "apex should rise above the eaves");
    }

    fn block_tag(state: &BlockState) -> &'static str {
        let name = Block::from_state_id(state.id).name;
        if name.contains("dark_oak_stairs") {
            "dStair"
        } else if name.contains("dark_oak") {
            "dOak"
        } else if name.contains("oak_stairs") {
            "oStair"
        } else if name.contains("lantern") {
            "lantern"
        } else if name.contains("brick") {
            "brick"
        } else if name.contains("cobble") {
            "cobble"
        } else if name.contains("andesite") || name.contains("tuff") {
            "stone"
        } else if name.contains("plank") {
            "plank"
        } else if name.contains("banner") {
            "banner"
        } else if name.contains("wall") {
            "wall"
        } else {
            name
        }
    }

    fn roof_structure_y(blocks: &[PlannedBlock], x: i32, z: i32) -> Option<(i32, &'static str)> {
        let mut best: Option<(i32, &'static str)> = None;
        for block in blocks {
            if block.x != x || block.z != z {
                continue;
            }
            let state = block.block_state();
            if state.is_air() {
                continue;
            }
            let tag = block_tag(state);
            if matches!(tag, "lantern" | "banner" | "wall") {
                continue;
            }
            if best.is_none_or(|(y, _)| block.y > y) {
                best = Some((block.y, tag));
            }
        }
        best
    }

    #[test]
    #[allow(clippy::print_stdout, clippy::unwrap_used)]
    fn harbour_keep_roof_has_no_interior_valley() {
        use pumpkin_data::dimension::Dimension;
        use pumpkin_util::world_seed::Seed;
        use pumpkin_world::generation::get_world_gen;
        use std::sync::Arc;

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
        let primary = plan
            .castle
            .keep_masses
            .iter()
            .copied()
            .find(|mass| mass.spec.inner && matches!(mass.spec.roof, RoofKind::Spire))
            .or_else(|| plan.castle.keep_masses.first().copied())
            .expect("harbour keep mass");
        let spec = primary.spec;
        let (cx, cz) = (spec.cx, spec.cz);
        let dump = spec.bounds().inflate(6);
        let hall = plan.castle.hall;
        let slice = crate::geom::Rect::new(
            dump.min_x.min(hall.min_x),
            dump.min_z.min(hall.min_z),
            dump.max_x.max(hall.max_x),
            dump.max_z.max(hall.max_z),
        );
        let blocks = planned_blocks_in(&plan, slice);

        println!(
            "keep {:?} hall {hall:?} spire cx={cx} cz={cz} r={} h={}",
            plan.castle.keep, spec.radius, spec.height
        );
        for mass in &plan.castle.keep_masses {
            println!(
                "mass cx={} cz={} r={} h={} shape={:?} roof={:?}",
                mass.spec.cx,
                mass.spec.cz,
                mass.spec.radius,
                mass.spec.height,
                mass.spec.shape,
                mass.spec.roof
            );
        }

        println!("--- x-y slice at spire cz={cz} ---");
        for x in slice.min_x..=slice.max_x {
            let kind = plan.occ.kind(x, cz);
            let mass = plan.castle.keep_mass_at(x, cz);
            let Some((y, tag)) = roof_structure_y(&blocks, x, cz) else {
                continue;
            };
            println!(
                "x={x:>6} y={y:<3} {tag:<8} kind={kind:?} roof={:?}",
                mass.map(|m| m.spec.roof)
            );
        }
        println!("--- z-y slice at spire cx={cx} ---");
        for z in slice.min_z..=slice.max_z {
            let kind = plan.occ.kind(cx, z);
            let mass = plan.castle.keep_mass_at(cx, z);
            let Some((y, tag)) = roof_structure_y(&blocks, cx, z) else {
                continue;
            };
            println!(
                "z={z:>6} y={y:<3} {tag:<8} kind={kind:?} roof={:?}",
                mass.map(|m| m.spec.roof)
            );
        }

        let mut valleys = Vec::new();
        let bounds = spec.bounds();
        for z in bounds.min_z..=bounds.max_z {
            for x in bounds.min_x..=bounds.max_x {
                let Some(mass) = plan.castle.keep_mass_at(x, z) else {
                    continue;
                };
                if mass.spec.cx != cx || mass.spec.cz != cz {
                    continue;
                }
                let Some((y, _)) = roof_structure_y(&blocks, x, z) else {
                    continue;
                };
                let dx = x - cx;
                let dz = z - cz;
                let d = dx.abs().max(dz.abs());
                if d == 0 {
                    continue;
                }
                let px = x - dx.signum();
                let pz = z - dz.signum();
                let Some((py, _)) = roof_structure_y(&blocks, px, pz) else {
                    continue;
                };
                if y > py {
                    valleys.push(format!(
                        "({x},{z}) y={y} cheb={d} inward ({px},{pz}) y={py}"
                    ));
                }
            }
        }
        assert!(
            valleys.is_empty(),
            "keep roof valley (far column higher than inward neighbor):\n{}",
            valleys.join("\n")
        );
    }
}
