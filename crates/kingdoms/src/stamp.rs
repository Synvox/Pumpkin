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
use crate::cells::CellKind;
use crate::geom::Dir;
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
            if !plan.bounds.contains(x, z) {
                continue;
            }
            let surface = ctx.surface_height_exclusive(x, z);
            let biome = ctx.get_biome(x, surface, z);
            stamp_column(ctx, plan, x, z, surface, Some(biome));
        }
    }
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
    let kind = plan.occ.kind(x, z);
    let mut ground = surface - 1;
    if ground <= sink.floor_min() + 2 {
        return;
    }
    if occupy_raises(kind) && ground < plan.heights.sea_level + 1 {
        raise_out_of_water(sink, x, ground, z, plan.heights.sea_level, &palette);
        ground = plan.heights.sea_level + 1;
    }
    let planned = plan.heights.height(x, z).max(ground);
    let floor = terrace_floor(ground, planned);
    if occupy_raises(kind) {
        clear_above(sink, x, ground + 1, ground + 28, z);
    }
    match kind {
        CellKind::Empty => {}
        CellKind::Road | CellKind::BarnYard => place_road(sink, plan, x, ground, z, &palette, None),
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
        CellKind::Farm => place_farm(sink, plan, x, ground, z, false),
        CellKind::FarmEdge => place_farm(sink, plan, x, ground, z, true),
        CellKind::Well => place_well(sink, plan, x, ground, z, &palette),
        CellKind::TownWall => place_curtain(sink, plan, x, floor, ground, z, &palette, 6, false),
        CellKind::TownTower => place_tower(sink, plan, x, floor, ground, z, &palette, 5, 14),
        CellKind::TownGate => place_gate(sink, plan, x, floor, ground, z, &palette, 8),
        CellKind::Building => {
            if let Some(lot) = plan.lots.iter().find(|l| l.id == plan.occ.lot(x, z)) {
                place_building(sink, plan, lot, x, ground, z, &palette);
            }
        }
        CellKind::CastleWall | CellKind::CastleInnerWall => {
            place_curtain(sink, plan, x, floor, ground, z, &palette, 8, true);
        }
        CellKind::CastleTower => place_tower(sink, plan, x, floor, ground, z, &palette, 7, 18),
        CellKind::CastleGate => place_gatehouse(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleBarbican => place_barbican(sink, x, floor, ground, z, &palette),
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
            place_gabled(
                sink,
                plan.castle.barracks,
                x,
                ridge_floor(plan, plan.castle.barracks),
                ground,
                z,
                &palette,
                6,
                true,
            );
        }
        CellKind::CastleKitchen => {
            place_gabled(
                sink,
                plan.castle.kitchen,
                x,
                ridge_floor(plan, plan.castle.kitchen),
                ground,
                z,
                &palette,
                5,
                false,
            );
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
fn ridge_floor(plan: &TownPlan, rect: crate::geom::Rect) -> i32 {
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
    if (x + z) % 11 == 0 {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, Block::WHITE_WOOL.default_state);
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
    // Arrow slits on the outer face.
    if castle && (x + z) % 4 == 0 {
        sink.put(x, floor + 4, z, Block::IRON_BARS.default_state);
    }
    let outer = wall_is_outer(plan, x, z);
    if outer {
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.masonry(1));
            sink.put(x, top + 2, z, palette.brick_wall);
        }
    } else {
        sink.put(x, top + 1, z, palette.path);
        if (x + z) % 5 == 0 {
            sink.put(x, top + 2, z, Block::LANTERN.default_state);
        }
    }
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
    let edge = r.on_edge(x, z);
    foundation(sink, x, floor, ground, z, palette);
    let (cx, cz) = r.center();
    let passage = match plan.castle.gate_dir {
        Dir::North | Dir::South => (x - cx).abs() <= 2,
        Dir::East | Dir::West => (z - cz).abs() <= 2,
    };
    if passage && !edge {
        sink.put(x, floor, z, palette.path);
        clear_above(sink, x, floor + 1, floor + 4, z);
        sink.put(x, floor + 1, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 2, z, Block::IRON_BARS.default_state);
        sink.put(x, floor + 3, z, Block::IRON_BARS.default_state);
        for y in floor + 5..=floor + 13 {
            sink.put(x, y, z, palette.masonry(2));
        }
        sink.put(x, floor + 14, z, palette.brick_wall);
        return;
    }
    for y in floor..=floor + 13 {
        sink.put(x, y, z, palette.masonry(((x + y) as u8) & 15));
    }
    if ((x + z) & 1) == 0 {
        sink.put(x, floor + 14, z, palette.masonry(1));
        sink.put(x, floor + 15, z, palette.brick_wall);
    }
    if r.is_corner(x, z) {
        sink.put(x, floor + 16, z, Block::RED_BANNER.default_state);
    }
}

fn place_barbican(
    sink: &mut impl BlockSink,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    foundation(sink, x, floor, ground, z, palette);
    for y in floor..=floor + 7 {
        sink.put(x, y, z, palette.masonry(3));
    }
    if ((x + z) & 1) == 0 {
        sink.put(x, floor + 8, z, palette.brick_wall);
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
    let r = plan.castle.keep;
    let floor = ridge_floor(plan, r);
    foundation(sink, x, floor, ground, z, palette);
    let edge = r.on_edge(x, z);
    let inner_edge = r.deflate(1).on_edge(x, z);
    let floors = 4;
    let storey = 5;
    let top = floor + floors * storey;
    if edge || inner_edge {
        for y in floor..=top {
            sink.put(
                x,
                y,
                z,
                palette.masonry(((x.wrapping_mul(3) + y) as u8) & 15),
            );
        }
        if edge && ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.masonry(2));
            sink.put(x, top + 2, z, palette.brick_wall);
        }
        if !r.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 7, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 12, z, Block::IRON_BARS.default_state);
        }
        if r.is_corner(x, z) {
            sink.put(x, top + 3, z, Block::RED_BANNER.default_state);
        }
        return;
    }
    clear_above(sink, x, floor + 1, top + 8, z);
    sink.put(x, floor, z, palette.planks);
    for s in 1..floors {
        sink.put(x, floor + s * storey, z, palette.planks);
    }
    if (x + z) % 15 == 0 {
        sink.put(x, floor + 1, z, Block::LANTERN.default_state);
    }
    let dx = (x - r.min_x).min(r.max_x - x);
    let dz = (z - r.min_z).min(r.max_z - z);
    let rise = dx.min(dz).min(8);
    let roof_y = top + rise;
    let (cx, cz) = r.center();
    let roof_dir = if dz <= dx {
        if z < cz { Dir::North } else { Dir::South }
    } else if x < cx {
        Dir::West
    } else {
        Dir::East
    };
    sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
    if dx.min(dz) >= 6 {
        sink.put(x, roof_y + 1, z, palette.roof_slab);
    }
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
    let floor = ridge_floor(plan, plan.castle.hall);
    place_gabled(
        sink,
        plan.castle.hall,
        x,
        floor,
        ground,
        z,
        palette,
        8,
        true,
    );
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
    place_gabled(sink, r, x, floor, ground, z, palette, 8, true);
    let (cx, cz) = r.center();
    if x == cx && z == cz {
        for y in floor + 8..=floor + 16 {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, floor + 17, z, Block::LANTERN.default_state);
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
    let edge = rect.on_edge(x, z);
    let open_front = z == rect.max_z && !rect.is_corner(x, z);
    if edge && !open_front {
        for y in floor..=floor + 4 {
            sink.put(x, y, z, palette.planks);
        }
        if (x + z) % 3 == 0 {
            sink.put(x, floor + 2, z, Block::OAK_FENCE.default_state);
        }
    } else if open_front {
        sink.put(x, floor, z, palette.path);
        sink.put(x, floor + 1, z, Block::OAK_FENCE.default_state);
    } else {
        sink.put(x, floor, z, palette.path);
        if (x + z) % 7 == 0 {
            sink.put(x, floor + 1, z, Block::HAY_BLOCK.default_state);
        }
    }
    place_roof(sink, rect, x, floor + 5, z, palette, 1);
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
    let edge = rect.on_edge(x, z);
    let wall = if stone {
        palette.bricks
    } else {
        palette.planks
    };
    if edge {
        for y in floor..=floor + wall_h {
            sink.put(x, y, z, wall);
        }
        if !rect.is_corner(x, z) && (x + z) % 4 == 0 {
            sink.put(x, floor + 2, z, Block::GLASS_PANE.default_state);
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        clear_above(sink, x, floor + 1, floor + wall_h - 1, z);
    }
    place_roof(sink, rect, x, floor + wall_h, z, palette, 1);
}

fn place_roof(
    sink: &mut impl BlockSink,
    rect: crate::geom::Rect,
    x: i32,
    eaves_y: i32,
    z: i32,
    palette: &Palette,
    overhang: i32,
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
    let span = if along_x { rect.depth() } else { rect.width() };
    let half = span / 2 + overhang;
    let roof_y = eaves_y + 1 + (half - dist).clamp(0, 10);
    let roof_dir = if along_x {
        if z < ridge { Dir::North } else { Dir::South }
    } else if x < ridge {
        Dir::West
    } else {
        Dir::East
    };
    sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
    if dist == 0 {
        sink.put(x, roof_y + 1, z, palette.roof_slab);
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
    let floor = terrace_floor(ground, mean);
    match lot.kind {
        BuildingKind::Windmill => place_windmill(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Church => place_town_church(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Stall => place_stall(sink, x, ground, z, palette),
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
    let timber = lot.kind.timber_frame();
    let edge = r.on_edge(x, z);
    let front = is_front(lot, x, z);
    let door = front && x == door_x(lot) && z == door_z(lot);
    if edge {
        // Stone / cobble foundation storey.
        sink.put(x, floor, z, palette.cobble);
        sink.put(x, floor + 1, z, palette.foundation((x + z) % 5 == 0));
        for y in floor + 2..=floor + wall_h {
            let storey_seam = (y - floor) % 4 == 0;
            let b = if r.is_corner(x, z) || storey_seam {
                palette.log
            } else if door && y <= floor + 3 {
                Block::AIR.default_state
            } else if window_here(lot, x, z, y - floor) {
                Block::GLASS_PANE.default_state
            } else if timber {
                palette.infill(((x + y) as u8) & 7)
            } else if lot.kind == BuildingKind::Blacksmith || lot.kind == BuildingKind::Barn {
                palette.cobble
            } else {
                palette.planks
            };
            sink.put(x, y, z, b);
        }
        if door {
            sink.put(x, floor + 1, z, Block::AIR.default_state);
            sink.put(x, floor + 2, z, Block::AIR.default_state);
            sink.put(x, floor + 3, z, palette.log);
        }
        if front && !door && (x + z) % 4 == 1 {
            sink.put(x, floor + 2, z, Block::OAK_TRAPDOOR.default_state);
        }
        if r.is_corner(x, z) {
            for y in floor..=floor + wall_h {
                sink.put(x, y, z, palette.log);
            }
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        for s in 1..lot.kind.stories() {
            sink.put(x, floor + s * 4, z, palette.planks);
        }
        clear_above(sink, x, floor + 1, floor + wall_h - 1, z);
        if lot.kind == BuildingKind::Tavern && (x + z) % 9 == 0 {
            sink.put(x, floor + 1, z, Block::BARREL.default_state);
        }
    }
    place_roof(sink, r, x, floor + wall_h, z, palette, 1);
    // Chimney on a back corner.
    if r.is_corner(x, z) && !front {
        for y in floor + wall_h..=floor + wall_h + 3 {
            sink.put(x, y, z, palette.cobble);
        }
        sink.put(x, floor + wall_h + 4, z, Block::CAMPFIRE.default_state);
    }
    // Dormer bump on the ridge.
    if !edge && (x + z) % 8 == 0 {
        let extra = floor + wall_h + 3;
        sink.put(x, extra, z, stair_state(palette.roof, lot.facing));
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

fn place_town_church(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(sink, lot.rect, x, floor, ground, z, palette, 8, true);
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
    let r2 = dx * dx + dz * dz;
    if r2 <= 4 {
        for y in floor..=floor + 12 {
            sink.put(x, y, z, palette.planks);
        }
        sink.put(x, floor + 13, z, palette.roof_slab);
    }
    if dx.abs() == dz.abs() && dx.abs() >= 1 && dx.abs() <= 3 {
        sink.put(x, floor + 10 + dx, z, Block::WHITE_WOOL.default_state);
        sink.put(x, floor + 10 - dx, z, Block::BROWN_WOOL.default_state);
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
    sink.put(x, ground, z, palette.grass);
    terrace_edge(sink, plan, x, ground, z, palette);
    clear_above(sink, x, ground + 1, ground + 4, z);
    let mix = (x.wrapping_mul(17) + z.wrapping_mul(31) + plan.seed as i32).unsigned_abs();
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
        place_roof(
            sink,
            lot.rect,
            x,
            floor.max(ground) + lot.kind.wall_h(),
            z,
            palette,
            1,
        );
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

fn place_stall(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    sink.put(x, ground, z, palette.planks);
    if (x + z) % 2 == 0 {
        sink.put(x, ground + 1, z, palette.fence);
        sink.put(x, ground + 2, z, Block::RED_WOOL.default_state);
    }
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
    if !plan.bounds.contains(x, z) || plan.occ.kind(x, z) == CellKind::Empty {
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
