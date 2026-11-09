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
    if !plan.strips_trees(x, z) {
        return;
    }
    if keeps_planned_timber(column_kind(plan, x, z)) {
        return;
    }
    let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
    let top = surface.max(planned) + 8;
    let air = Block::AIR.default_state;
    for y in planned.saturating_sub(4)..=top {
        let id = Block::from_state_id(ctx.get_block_state(x, y, z)).id;
        if is_vanilla_tree_part(id) {
            ctx.set_block_state(x, y, z, air);
        }
    }
}

fn keeps_planned_timber(kind: CellKind) -> bool {
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
    )
}

fn is_vanilla_tree_part(id: pumpkin_data::BlockId) -> bool {
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
    let palette = Palette::new(biome.map_or(plan.palette, PaletteKind::from_biome));
    let kind = column_kind(plan, x, z);
    let mut ground = surface - 1;
    if ground <= sink.floor_min() + 2 {
        return;
    }
    // Features trees make `surface` the canopy. Snap tree-strip columns
    // (scene yards, walled town, market crop, building keepout) back to
    // the planner height so trunks and hanging leaves are cleared.
    // fill_volume has `ground + 1 == planned`; only then skip the
    // tree-height strip, or halls get a cave the watertight flood follows.
    if plan.strips_trees(x, z) {
        let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
        let sea = plan.heights.sea_level;
        let top = ground.max(planned);
        if plan.heights.is_dry(x, z) {
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
            ground + 42
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
        CellKind::Harbor | CellKind::Dock | CellKind::Ship | CellKind::Boat | CellKind::Breakwater
    ) {
        let planned = plan.heights.height(x, z);
        let sea = plan.heights.sea_level;
        if planned <= sea + 1 {
            if ground > sea + 2 {
                clear_above(sink, x, sea.min(planned), ground + 8, z);
                ground = planned.min(sea).max(sink.floor_min() + 3);
            }
            // Lily pads sit on the water block; clear from sea so they cannot
            // survive as green squares on the basin.
            clear_above(sink, x, sea, sea + 24, z);
        }
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
            )
        {
            clear_above(sink, x, sea.saturating_sub(4), sea.max(ground) + 24, z);
        } else if !occupy_raises(kind) {
            clear_above(sink, x, ground + 1, ground + 40, z);
        }
    }
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
        CellKind::Plaza => place_plaza(sink, x, ground, z, &palette),
        CellKind::Fountain => place_fountain(sink, plan, x, ground, z, &palette),
        CellKind::Farm => place_farm(sink, plan, x, ground, z, false),
        CellKind::FarmEdge => place_farm(sink, plan, x, ground, z, true),
        CellKind::Orchard => place_orchard(sink, plan, x, ground, z, &palette),
        CellKind::Pasture => place_pasture(sink, x, ground, z, &palette),
        CellKind::Grave => place_grave(sink, x, ground, z, &palette),
        CellKind::Palisade => place_palisade(sink, x, ground, z, &palette),
        CellKind::PalisadeWalk => place_palisade_walk(sink, x, ground, z, &palette),
        CellKind::Tent => place_tent(sink, plan, x, ground, z, &palette),
        CellKind::CampCourt => place_camp_court(sink, x, ground, z, &palette),
        CellKind::Ruin => place_ruin(sink, plan, x, ground, z, &palette),
        CellKind::WatchPost => place_watch_post(sink, x, floor, ground, z, &palette),
        CellKind::Well => place_well(sink, plan, x, ground, z, &palette),
        CellKind::TownWall => {
            if in_frontier(plan, x, z) {
                place_scarred_curtain(sink, plan, x, floor, ground, z, &palette);
            } else {
                place_curtain(sink, plan, x, floor, ground, z, &palette, 6, false);
            }
        }
        CellKind::TownTower => {
            if in_frontier(plan, x, z) {
                place_collapsed_tower(sink, plan, x, floor, ground, z, &palette);
            } else {
                place_tower(sink, plan, x, floor, ground, z, &palette, 5, 14);
            }
        }
        CellKind::TownGate => place_gate(sink, plan, x, floor, ground, z, &palette, 8),
        CellKind::Building => {
            if let Some(lot) = plan.lots.iter().find(|l| l.id == plan.occ.lot(x, z)) {
                place_building(sink, plan, lot, x, ground, z, &palette);
            }
        }
        CellKind::CastleWall => {
            place_curtain(
                sink,
                plan,
                x,
                floor,
                ground,
                z,
                &palette,
                plan.castle.outer_ring.height,
                true,
            );
        }
        CellKind::CastleInnerWall => {
            place_curtain(
                sink,
                plan,
                x,
                floor,
                ground,
                z,
                &palette,
                plan.castle.inner_ring.height,
                true,
            );
        }
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
        CellKind::Harbor => place_harbor(sink, x, ground, z, plan.heights.sea_level),
        CellKind::Dock => place_dock(sink, x, ground, z, plan.heights.sea_level, &palette),
        CellKind::Bridge => place_bridge(sink, x, ground, z, plan.heights.sea_level, &palette),
        CellKind::WarTent => place_ridge_tent(sink, plan, x, ground, z, &palette, true),
        CellKind::Cage => place_cage(sink, plan, x, ground, z, &palette),
        CellKind::PalisadeGate => place_palisade_gate(sink, plan, x, ground, z, &palette),
        CellKind::SiegeTreb => place_trebuchet(sink, plan, x, ground, z, &palette),
        CellKind::SiegeRam => place_ram(sink, plan, x, ground, z, &palette),
        CellKind::SiegeLadder => place_siege_ladder(sink, x, ground, z, &palette),
        CellKind::WindSail => place_wind_sail(sink, plan, x, ground, z, &palette),
        CellKind::Arcade => place_arcade(sink, plan, x, ground, z, &palette),
        CellKind::Lychgate => place_lychgate(sink, plan, x, ground, z, &palette),
        CellKind::FortressWall => {
            place_curtain(sink, plan, x, floor, ground, z, &palette, 8, true);
        }
        CellKind::FortressKeep => {
            let rect = blob_of(plan, x, z, CellKind::FortressKeep);
            let keep_floor = ridge_floor(plan, rect);
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
            place_gothic_mass(sink, plan, x, floor, ground, z, &palette, kind);
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
        CellKind::TourneyList => place_tourney_list(sink, x, ground, z, &palette),
        CellKind::TourneyStand => place_tourney_stand(sink, plan, x, ground, z, &palette),
        CellKind::Burnt => place_burnt(sink, plan, x, ground, z, &palette),
        CellKind::Breakwater => {
            place_breakwater(sink, x, ground, z, plan.heights.sea_level, &palette);
        }
        CellKind::SiegeTrench => place_siege_trench(sink, x, ground, z, &palette),
    }
    apply_column_damage(sink, plan, x, z, kind, floor, ground, &palette);
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
            | CellKind::SiegeTrench
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
    let block = match mix % 5 {
        0 | 1 => palette.path,
        2 => palette.gravel,
        _ => palette.cobble,
    };
    sink.put(x, ground, z, block);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 3, z);
    if let Some(dir) = stair {
        sink.put(x, ground + 1, z, stair_state(palette.cobble_stairs, dir));
    }
    if mix.is_multiple_of(19) {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, Block::LANTERN.default_state);
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

fn place_plaza(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
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
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, Block::LANTERN.default_state);
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

fn place_pasture(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.grass);
    clear_above(sink, x, ground + 1, ground + 3, z);
    if (x + z) % 13 == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    } else if (x + z) % 9 == 0 {
        sink.put(x, ground + 1, z, palette.fence);
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

fn place_palisade(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.cobble);
    for y in ground + 1..=ground + 5 {
        sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
    }
    sink.put(x, ground + 6, z, Block::DARK_OAK_FENCE.default_state);
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
    place_ridge_tent(sink, plan, x, ground, z, palette, false);
}

fn place_camp_court(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.path);
    clear_above(sink, x, ground + 1, ground + 6, z);
    let mix = (x.wrapping_mul(13) + z.wrapping_mul(7)).unsigned_abs();
    if mix % 28 == 0 {
        sink.put(x, ground + 1, z, Block::CAMPFIRE.default_state);
    } else if mix % 22 == 0 {
        sink.put(x, ground + 1, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 2, z, Block::DARK_OAK_FENCE.default_state);
        sink.put(x, ground + 3, z, Block::BLACK_BANNER.default_state);
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

fn place_trebuchet(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.cobble);
    let rect = blob_of(plan, x, z, CellKind::SiegeTreb);
    let along_x = rect.width() >= rect.depth();
    let (cx, cz) = rect.center();
    let corner = rect.is_corner(x, z);
    if corner {
        for y in ground + 1..=ground + 8 {
            sink.put(x, y, z, Block::DARK_OAK_LOG.default_state);
        }
    } else if rect.on_edge(x, z) {
        sink.put(x, ground + 2, z, Block::DARK_OAK_LOG.default_state);
        sink.put(x, ground + 5, z, Block::DARK_OAK_FENCE.default_state);
    }
    let on_arm = if along_x { z == cz } else { x == cx };
    if on_arm {
        let t = if along_x {
            x - rect.min_x
        } else {
            z - rect.min_z
        };
        let span = if along_x { rect.width() } else { rect.depth() };
        let arm_y = ground + 8;
        sink.put(x, arm_y, z, Block::DARK_OAK_LOG.default_state);
        if t <= 1 {
            sink.put(x, arm_y - 1, z, palette.cobble);
            sink.put(x, arm_y - 2, z, Block::ANVIL.default_state);
        } else if t >= span - 2 {
            sink.put(x, arm_y + 1, z, Block::DARK_OAK_FENCE.default_state);
            sink.put(x, arm_y + 2, z, Block::DARK_OAK_FENCE.default_state);
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
    let court = Dir::ALL
        .iter()
        .any(|dir| plan.occ.kind(x + dir.dx(), z + dir.dz()) == CellKind::Green);
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
    let (wx, wz) = plan.well;
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
            sink.put(x, top + 1, z, palette.masonry(1));
            sink.put(x, top + 2, z, palette.brick_wall);
        } else if castle {
            // Same-column corbel: an outward stair standing in for a machicolation.
            if let Some(face) = outer_facing(plan, x, z) {
                sink.put(x, top, z, stair_state(palette.brick_stairs, face));
            }
            sink.put(x, top + 1, z, palette.fence);
            sink.put(x, top + 2, z, palette.slab);
        }
        if castle && r_banner(x, z) {
            sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
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
                | CellKind::Plaza
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
                | CellKind::Plaza
        )
    })
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
        if !matches!(spec.roof, RoofKind::Crenellated) && roof_y > top + 1 {
            for y in top + 1..roof_y {
                sink.put(x, y, z, palette.roof_slab);
            }
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
        RoofKind::Spire => (top + 1 + (spec.radius - dist).max(0) * 2, out),
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
        RoofKind::Cone | RoofKind::Hip | RoofKind::Gable { .. } | RoofKind::Spire => {
            // Interior rafter only. Planks on the rim turned keep roofs into
            // brown wedges and hid the masonry eaves.
            if !on_shell(spec, x, z) {
                sink.put(x, roof_y - 1, z, palette.planks);
            }
            sink.put(x, roof_y, z, stair_state(palette.roof, dir));
            let dx = x - spec.cx;
            let dz = z - spec.cz;
            if isqrt(dx, dz) <= 1 {
                sink.put(x, roof_y + 1, z, palette.roof_slab);
            }
            if matches!(spec.roof, RoofKind::Spire) && dx == 0 && dz == 0 {
                for y in roof_y + 1..=roof_y + 6 {
                    sink.put(x, y, z, palette.bricks);
                }
                sink.put(x, roof_y + 7, z, Block::LANTERN.default_state);
            }
        }
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
    sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
    if dist == 0 {
        sink.put(x, roof_y + 1, z, palette.roof_slab);
    }
    if palette.kind == PaletteKind::Spruce {
        sink.put(x, roof_y + 1, z, Block::SNOW.default_state);
        if dist == 0 {
            sink.put(x, roof_y + 2, z, Block::SNOW.default_state);
        }
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
    let radius = 6;
    for y in cy - radius..=cy + radius {
        let dy = y - cy;
        let dz = z - cz;
        let d2 = dy * dy + dz * dz;
        if d2 > radius * radius {
            continue;
        }
        // 12 petals: 4 cardinals plus 8 at ~30°/60° (|2a|==|b|).
        let spoke = dy == 0 || dz == 0 || dy.abs() * 2 == dz.abs() || dz.abs() * 2 == dy.abs();
        let ring = d2 >= radius * radius - 8 || (14..=18).contains(&d2) || d2 <= 2;
        let petal = !spoke && !ring && (dy.abs() == 3 || dz.abs() == 3);
        sink.put(
            x,
            y,
            z,
            if ring || spoke || petal {
                Block::CALCITE.default_state
            } else if dz > 0 {
                Block::PURPLE_STAINED_GLASS.default_state
            } else {
                Block::YELLOW_STAINED_GLASS.default_state
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
    let floor = mean.max(plan.heights.sea_level + 2);
    match lot.kind {
        BuildingKind::Windmill => place_windmill(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Church => place_town_church(sink, lot, x, floor, ground, z, palette),
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
    let edge = r.on_edge(x, z);
    let front = is_front(lot, x, z);
    let door = front && x == door_x(lot) && z == door_z(lot);
    let wall_top = if desert { eaves } else { roof_y };
    if edge {
        sink.put(x, floor, z, palette.cobble);
        sink.put(x, floor + 1, z, palette.foundation((x + z) % 5 == 0));
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
                palette.infill(((x + y) as u8) & 7)
            } else if lot.kind == BuildingKind::Blacksmith
                || lot.kind == BuildingKind::Barn
                || lot.kind == BuildingKind::Monastery
                || lot.kind == BuildingKind::Cathedral
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
    if kind == CellKind::CathedralTower {
        place_gothic_tower(sink, plan, rect, x, floor, ground, z, palette);
        return;
    }
    let west_front = kind == CellKind::CathedralNave && x == rect.min_x;
    let rose_col = west_front && (z - rect.center().1).abs() <= 6;
    let rose_top = floor + 19;
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
            if rose_col && dy * dy + dz * dz <= 36 {
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
    let top = floor + 26;
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
        }
    } else {
        clear_above(sink, x, floor + 1, top - 1, z);
        sink.put(x, floor, z, Block::SMOOTH_STONE.default_state);
        sink.put(x, floor + 8, z, palette.planks);
        sink.put(x, floor + 16, z, palette.planks);
        sink.put(x, top, z, Block::STONE_BRICKS.default_state);
        if x == cx && z == cz {
            sink.put(x, floor + 1, z, Block::LADDER.default_state);
            sink.put(x, floor + 9, z, Block::LADDER.default_state);
            sink.put(x, floor + 17, z, Block::LADDER.default_state);
            sink.put(x, top - 1, z, Block::LANTERN.default_state);
        }
    }
    let spire_top = top + 1 + (2 - cheb).max(0) * 3;
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
    let against_mass = Dir::ALL
        .iter()
        .any(|dir| is_cathedral_mass(plan.occ.kind(x + dir.dx(), z + dir.dz())));
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
            ) {
                return true;
            }
        }
    }
    false
}

fn place_green(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    sink.put(x, ground, z, palette.grass);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 4, z);
    if near_extractive(plan, x, z) || in_mill_disc(plan, x, z) || plan.strips_trees(x, z) {
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

fn palm_hash(x: i32, z: i32, seed: u64) -> u32 {
    (x.wrapping_mul(17) + z.wrapping_mul(31) + seed as i32).unsigned_abs()
}

fn palm_lean(x: i32, z: i32, seed: u64) -> Dir {
    Dir::ALL[(palm_hash(x, z, seed) as usize) % 4]
}

fn is_palm_origin(x: i32, z: i32, seed: u64) -> bool {
    palm_hash(x, z, seed) % 29 == 0
}

fn place_palm_column(sink: &mut impl BlockSink, plan: &TownPlan, x: i32, ground: i32, z: i32) {
    if is_palm_origin(x, z, plan.seed) {
        for y in 1..=3 {
            sink.put(x, ground + y, z, Block::JUNGLE_LOG.default_state);
        }
    }
    for dir in Dir::ALL {
        let ox = x - dir.dx();
        let oz = z - dir.dz();
        if !is_palm_origin(ox, oz, plan.seed) || palm_lean(ox, oz, plan.seed) != dir {
            continue;
        }
        sink.put(x, ground + 4, z, Block::JUNGLE_LOG.default_state);
        sink.put(x, ground + 5, z, Block::JUNGLE_LEAVES.default_state);
        sink.put(x, ground + 6, z, Block::JUNGLE_LEAVES.default_state);
    }
    for dir in Dir::ALL {
        let cx = x - dir.dx();
        let cz = z - dir.dz();
        for lean in Dir::ALL {
            let ox = cx - lean.dx();
            let oz = cz - lean.dz();
            if is_palm_origin(ox, oz, plan.seed) && palm_lean(ox, oz, plan.seed) == lean {
                sink.put(x, ground + 5, z, Block::JUNGLE_LEAVES.default_state);
                if (x + z) % 2 == 0 {
                    sink.put(x, ground + 6, z, Block::JUNGLE_LEAVES.default_state);
                }
            }
        }
    }
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

fn place_harbor(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, sea: i32) {
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

fn place_ship(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let rect = blob_of(plan, x, z, CellKind::Ship);
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
    // Pointed bow: drop the outer cells at the forward end.
    if along <= 1 && from_keel >= (mid - along).max(0) && from_keel > 0 {
        sink.put(x, y, z, Block::WATER.default_state);
        return;
    }
    sink.put(x, y - 1, z, Block::OAK_LOG.default_state);
    sink.put(x, y, z, Block::OAK_PLANKS.default_state);
    let stern = along >= length - 4;
    if rect.on_edge(x, z) {
        sink.put(x, y + 1, z, Block::OAK_LOG.default_state);
        if stern {
            sink.put(x, y + 2, z, Block::OAK_PLANKS.default_state);
            sink.put(x, y + 3, z, Block::OAK_PLANKS.default_state);
            sink.put(x, y + 4, z, Block::OAK_FENCE.default_state);
        } else if rect.is_corner(x, z) {
            sink.put(x, y + 2, z, Block::OAK_FENCE.default_state);
        }
    } else {
        sink.put(x, y + 1, z, Block::OAK_SLAB.default_state);
        if stern {
            sink.put(x, y + 3, z, Block::OAK_PLANKS.default_state);
            if (x + z) % 3 == 0 {
                sink.put(x, y + 4, z, Block::CHEST.default_state);
            }
        }
        if x == cx && z == cz {
            for h in 2..=12 {
                sink.put(x, y + h, z, Block::OAK_FENCE.default_state);
            }
            // Furled sail: a rolled wool bundle on the mast, not a flying sheet.
            sink.put(x, y + 7, z, Block::WHITE_WOOL.default_state);
            sink.put(x, y + 8, z, Block::WHITE_WOOL.default_state);
            sink.put(x, y + 9, z, Block::WHITE_WOOL.default_state);
            sink.put(x, y + 13, z, Block::WHITE_BANNER.default_state);
        } else if (along_x && z == cz && (x - cx).abs() == 1)
            || (!along_x && x == cx && (z - cz).abs() == 1)
        {
            sink.put(x, y + 7, z, Block::WHITE_WOOL.default_state);
            sink.put(x, y + 8, z, Block::OAK_SLAB.default_state);
        } else if !stern && (x + z) % 5 == 0 {
            sink.put(x, y + 2, z, Block::BARREL.default_state);
        }
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
    sink.put(x, y, z, Block::OAK_PLANKS.default_state);
    if rect.on_edge(x, z) {
        sink.put(x, y + 1, z, Block::OAK_LOG.default_state);
    } else if x == cx && z == cz {
        sink.put(x, y + 1, z, Block::OAK_FENCE.default_state);
        sink.put(x, y + 2, z, Block::WHITE_WOOL.default_state);
        sink.put(x, y + 3, z, Block::LANTERN.default_state);
    } else {
        sink.put(x, y + 1, z, Block::OAK_SLAB.default_state);
        if (x + z) % 3 == 0 {
            sink.put(x, y + 1, z, Block::CHEST.default_state);
        }
    }
    let _ = palette;
}

fn place_quay(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, sea: i32, palette: &Palette) {
    let y = ground.max(sea);
    sink.put(x, y - 1, z, palette.andesite);
    sink.put(x, y, z, Block::STONE_BRICKS.default_state);
    sink.put(x, y + 1, z, palette.cobble);
    if ground < sea {
        for yy in ground + 1..=sea {
            sink.put(x, yy, z, palette.cobble);
        }
    }
    if (x + z) % 4 == 0 {
        sink.put(x, y + 2, z, palette.brick_wall);
        sink.put(x, y + 3, z, Block::LANTERN.default_state);
    } else if (x + z) % 3 == 0 {
        sink.put(x, y + 2, z, Block::CHEST.default_state);
    } else {
        sink.put(x, y + 2, z, palette.brick_wall);
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
    let y = ground.max(sea);
    let mut pad = 0;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if plan.occ.kind(x + dx, z + dz) == CellKind::Crane {
                pad += 1;
            }
        }
    }
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
    } else {
        sink.put(x, y + 11, z, Block::OAK_LOG.default_state);
        sink.put(x, y + 12, z, Block::OAK_FENCE.default_state);
        if (x + z) % 2 == 0 {
            sink.put(x, y + 10, z, Block::OAK_FENCE.default_state);
        }
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

fn place_siege_trench(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground - 2, z, palette.andesite);
    sink.put(x, ground - 1, z, Block::COARSE_DIRT.default_state);
    sink.put(x, ground, z, Block::AIR.default_state);
    if (x + z) % 3 == 0 {
        sink.put(x, ground - 1, z, Block::COBBLESTONE.default_state);
    }
    clear_above(sink, x, ground + 1, ground + 6, z);
}

fn in_frontier(plan: &TownPlan, x: i32, z: i32) -> bool {
    plan.scenes
        .iter()
        .any(|scene| scene.kind == crate::scenes::SceneKind::Frontier && scene.rect.contains(x, z))
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

fn place_tourney_list(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, Block::SAND.default_state);
    clear_above(sink, x, ground + 1, ground + 4, z);
    let _ = palette;
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
    foundation(sink, x, ground, ground, z, palette);
    let pavilion = rect.width() <= 6 && rect.depth() <= 6;
    if pavilion {
        if rect.is_corner(x, z) {
            for y in ground + 1..=ground + 4 {
                sink.put(x, y, z, Block::OAK_FENCE.default_state);
            }
            sink.put(x, ground + 5, z, Block::RED_WOOL.default_state);
            sink.put(x, ground + 6, z, Block::RED_BANNER.default_state);
        } else if rect.on_edge(x, z) {
            sink.put(x, ground + 5, z, Block::RED_WOOL.default_state);
            sink.put(x, ground + 1, z, Block::OAK_STAIRS.default_state);
        } else {
            sink.put(x, ground + 1, z, Block::OAK_PLANKS.default_state);
            sink.put(x, ground + 5, z, Block::RED_WOOL.default_state);
            if x == rect.center().0 && z == rect.center().1 {
                sink.put(x, ground + 2, z, Block::CHEST.default_state);
                sink.put(x, ground + 6, z, Block::LANTERN.default_state);
            }
        }
        return;
    }
    let row = if rect.width() >= rect.depth() {
        (z - rect.min_z).min(rect.max_z - z)
    } else {
        (x - rect.min_x).min(rect.max_x - x)
    };
    sink.put(
        x,
        ground + 1 + row.min(3),
        z,
        Block::OAK_STAIRS.default_state,
    );
    if rect.on_edge(x, z) && (x + z) % 3 == 0 {
        sink.put(x, ground + 5, z, Block::OAK_FENCE.default_state);
        sink.put(x, ground + 6, z, Block::RED_BANNER.default_state);
    }
}

fn place_bridge(
    sink: &mut impl BlockSink,
    x: i32,
    ground: i32,
    z: i32,
    sea: i32,
    palette: &Palette,
) {
    let y = sea.max(ground) + 1;
    sink.put(x, y, z, palette.planks);
    if (x + z) % 3 == 0 {
        sink.put(x, y + 1, z, palette.fence);
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
            for dy in 0..4 {
                let y = ground - dy;
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
