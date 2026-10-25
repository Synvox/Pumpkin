//! Per-column stamping. Each chunk writes only its own columns.

use std::collections::BTreeMap;

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::BlockStateId;
use pumpkin_data::biome::Biome;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::generation::hooks::ChunkDecorationContext;

use crate::buildings::{BuildingKind, Lot};
use crate::cells::CellKind;
use crate::geom::Dir;
use crate::palette::{Palette, PaletteKind, stair_state};
use crate::plan::TownPlan;
use crate::wfc::{WallPart, wall_part};

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
    let ground = surface - 1;
    if ground <= sink.floor_min() + 2 {
        return;
    }
    let planned = plan.heights.height(x, z);
    let floor = terrace_floor(ground, planned);
    match kind {
        CellKind::Empty => {}
        CellKind::Road | CellKind::BarnYard => place_road(sink, x, ground, z, &palette, None),
        CellKind::RoadStairN => place_road(sink, x, ground, z, &palette, Some(Dir::North)),
        CellKind::RoadStairS => place_road(sink, x, ground, z, &palette, Some(Dir::South)),
        CellKind::RoadStairE => place_road(sink, x, ground, z, &palette, Some(Dir::East)),
        CellKind::RoadStairW => place_road(sink, x, ground, z, &palette, Some(Dir::West)),
        CellKind::Market => place_market(sink, x, ground, z, &palette),
        CellKind::Farm => place_farm(sink, plan, x, ground, z, false),
        CellKind::FarmEdge => place_farm(sink, plan, x, ground, z, true),
        CellKind::Well => place_well(sink, plan, x, ground, z, &palette),
        CellKind::TownWall => place_curtain(sink, x, floor, ground, z, &palette, 6, false),
        CellKind::TownTower => place_tower(sink, x, floor, ground, z, &palette, 5, 12),
        CellKind::TownGate => place_gate(sink, x, floor, ground, z, &palette, 8),
        CellKind::Building => {
            if let Some(lot) = plan.lots.iter().find(|l| l.id == plan.occ.lot(x, z)) {
                place_building(sink, lot, x, floor, ground, z, &palette);
            }
        }
        CellKind::CastleWall | CellKind::CastleInnerWall => {
            place_curtain(sink, x, floor, ground, z, &palette, 8, true);
        }
        CellKind::CastleTower => place_tower(sink, x, floor, ground, z, &palette, 7, 18),
        CellKind::CastleGate => place_gatehouse(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleBarbican => place_barbican(sink, x, floor, ground, z, &palette),
        CellKind::CastleKeep => place_keep(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleHall => place_hall(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleChapel => place_chapel(sink, plan, x, floor, ground, z, &palette),
        CellKind::CastleStables => {
            place_outbuilding(sink, plan.castle.stables, x, floor, ground, z, &palette, 5);
        }
        CellKind::CastleBarracks => {
            place_outbuilding(sink, plan.castle.barracks, x, floor, ground, z, &palette, 6);
        }
        CellKind::CastleKitchen => {
            place_outbuilding(sink, plan.castle.kitchen, x, floor, ground, z, &palette, 5);
        }
        CellKind::CastleCourt => place_court(sink, x, ground, z, &palette),
        CellKind::CastleMoat => place_moat(sink, x, ground, z),
    }
}

fn terrace_floor(ground: i32, planned: i32) -> i32 {
    let raised = planned.max(ground - MAX_CUT);
    if ground - raised > MAX_FOUNDATION {
        ground - 2
    } else {
        raised
    }
}

fn foundation(
    sink: &mut impl BlockSink,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let start = ground.min(floor) - 1;
    let end = floor - 1;
    if end < start {
        return;
    }
    for y in start..=end {
        let mossy = (x + z + y) % 8 == 0;
        sink.put(x, y, z, palette.foundation(mossy));
    }
    if ground > floor + MAX_CUT {
        // Stilts on the downhill? handled by start/end. Uphill: don't carve a pit.
        sink.put(x, ground, z, palette.cobble);
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
    clear_above(sink, x, ground + 1, ground + 2, z);
    if let Some(dir) = stair {
        sink.put(x, ground + 1, z, stair_state(palette.cobble_stairs, dir));
    }
    if mix.is_multiple_of(17) {
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
        sink.put(x, ground + 1, z, Block::OAK_FENCE.default_state);
        return;
    }
    if (x + z * 3) % 13 == 0 {
        sink.put(x, ground, z, Block::WATER.default_state);
        return;
    }
    sink.put(x, ground, z, Block::FARMLAND.default_state);
    let age = ((plan.seed as i32) ^ x ^ (z * 17)).unsigned_abs() % 8;
    // Default wheat; age variation is visual enough via hay stacks.
    sink.put(x, ground + 1, z, Block::WHEAT.default_state);
    if age == 0 {
        sink.put(x, ground + 1, z, Block::HAY_BLOCK.default_state);
    }
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
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    height: i32,
    castle: bool,
) {
    foundation(sink, x, floor, ground, z, palette);
    let top = floor + height;
    for y in floor..=top {
        let v = ((x + z + y) as u8) & 7;
        sink.put(
            x,
            y,
            z,
            if castle {
                palette.masonry(v)
            } else {
                palette.foundation(v == 0)
            },
        );
    }
    // Crenellations follow the local top, so the wall steps with the ridge.
    if ((x + z) & 1) == 0 {
        sink.put(x, top + 1, z, palette.brick_wall);
    } else {
        sink.put(x, top + 1, z, palette.slab);
    }
}

fn place_tower(
    sink: &mut impl BlockSink,
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
        sink.put(x, y, z, palette.masonry(((x + y) as u8) & 7));
    }
    // Pyramid cap
    let lx = (x & 7) - 3;
    let lz = (z & 7) - 3;
    let ring = lx.abs().max(lz.abs());
    if ring <= half / 2 {
        sink.put(x, top + 1, z, palette.bricks);
        if ring <= 1 {
            sink.put(x, top + 2, z, palette.bricks);
        }
        if ring == 0 {
            sink.put(x, top + 3, z, Block::LANTERN.default_state);
        }
    }
    if (x + z) % 4 == 0 {
        sink.put(x, floor + 4, z, Block::IRON_BARS.default_state);
    }
}

fn place_gate(
    sink: &mut impl BlockSink,
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
        for y in floor + 4..=floor + height {
            sink.put(x, y, z, palette.masonry(1));
        }
    } else {
        place_curtain(sink, x, floor, ground, z, palette, height, true);
    }
}

fn place_gatehouse(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let r = plan.castle.gatehouse;
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
        for y in floor + 5..=floor + 12 {
            sink.put(x, y, z, palette.masonry(2));
        }
        return;
    }
    for y in floor..=floor + 12 {
        sink.put(x, y, z, palette.masonry(((x + y) as u8) & 7));
    }
    sink.put(x, floor + 13, z, palette.brick_wall);
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
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    let r = plan.castle.keep;
    foundation(sink, x, floor, ground, z, palette);
    let edge = r.on_edge(x, z);
    let inner_edge = r.deflate(1).on_edge(x, z);
    let floors = 5;
    let storey = 5;
    let top = floor + floors * storey;
    if edge || inner_edge {
        for y in floor..=top {
            sink.put(
                x,
                y,
                z,
                palette.masonry(((x.wrapping_mul(3) + y) as u8) & 7),
            );
        }
        if ((x + z) & 1) == 0 {
            sink.put(x, top + 1, z, palette.brick_wall);
        }
        // Arrow slits
        if !r.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 7, z, Block::IRON_BARS.default_state);
            sink.put(x, floor + 12, z, Block::IRON_BARS.default_state);
        }
    } else {
        sink.put(x, floor, z, palette.planks);
        for s in 1..floors {
            sink.put(x, floor + s * storey, z, palette.planks);
        }
        clear_above(sink, x, floor + 1, floor + storey - 1, z);
        // Roof
        let dx = (x - r.min_x).min(r.max_x - x);
        let dz = (z - r.min_z).min(r.max_z - z);
        let dist = dx.min(dz);
        let roof_y = top - dist.clamp(0, 6);
        sink.put(x, roof_y, z, palette.bricks);
        if dist <= 1 {
            sink.put(x, roof_y + 1, z, palette.slab);
        }
    }
}

fn place_hall(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(
        sink,
        plan.castle.hall,
        x,
        floor,
        ground,
        z,
        palette,
        7,
        true,
    );
}

fn place_chapel(
    sink: &mut impl BlockSink,
    plan: &TownPlan,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(
        sink,
        plan.castle.chapel,
        x,
        floor,
        ground,
        z,
        palette,
        8,
        true,
    );
    let r = plan.castle.chapel;
    let (cx, cz) = r.center();
    if x == cx && z == cz {
        for y in floor + 8..=floor + 16 {
            sink.put(x, y, z, palette.bricks);
        }
        sink.put(x, floor + 17, z, Block::LANTERN.default_state);
    }
}

fn place_outbuilding(
    sink: &mut impl BlockSink,
    rect: crate::geom::Rect,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
    wall_h: i32,
) {
    place_gabled(sink, rect, x, floor, ground, z, palette, wall_h, false);
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
    let along_x = rect.width() >= rect.depth();
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
    let roof_y = floor + wall_h + 1 - dist.min(6);
    let roof_dir = if along_x {
        if z < ridge { Dir::North } else { Dir::South }
    } else if x < ridge {
        Dir::West
    } else {
        Dir::East
    };
    sink.put(x, roof_y, z, stair_state(palette.roof, roof_dir));
}

fn place_court(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32, palette: &Palette) {
    let mix = (x * 5 + z * 2).unsigned_abs() % 4;
    let block = match mix {
        0 => palette.gravel,
        1 => palette.cobble,
        _ => palette.path,
    };
    sink.put(x, ground, z, block);
    clear_above(sink, x, ground + 1, ground + 2, z);
}

fn place_moat(sink: &mut impl BlockSink, x: i32, ground: i32, z: i32) {
    sink.put(x, ground, z, Block::DIRT.default_state);
    sink.put(x, ground + 1, z, Block::WATER.default_state);
    clear_above(sink, x, ground + 2, ground + 3, z);
}

fn place_building(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    match lot.kind {
        BuildingKind::Windmill => place_windmill(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Church => place_town_church(sink, lot, x, floor, ground, z, palette),
        BuildingKind::Stall => place_stall(sink, x, ground, z, palette),
        BuildingKind::Blacksmith => {
            place_house(sink, lot, x, floor, ground, z, palette, true);
            if x == lot.rect.center().0 && z == lot.rect.center().1 {
                sink.put(x, floor + 1, z, Block::ANVIL.default_state);
                sink.put(x + 1, floor + 1, z, Block::CAMPFIRE.default_state);
            }
        }
        _ => place_house(
            sink,
            lot,
            x,
            floor,
            ground,
            z,
            palette,
            lot.kind == BuildingKind::Timber,
        ),
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
    timber: bool,
) {
    foundation(sink, x, floor, ground, z, palette);
    let r = lot.rect;
    let wall_h = lot.kind.wall_h();
    let edge = r.on_edge(x, z);
    if edge {
        let part = facade_part(lot, x, z, 1);
        let block = match part {
            WallPart::Beam | WallPart::Post => palette.log,
            WallPart::Window | WallPart::Shutter => Block::GLASS_PANE.default_state,
            WallPart::Door if is_front(lot, x, z) => Block::AIR.default_state,
            _ if timber => palette.plaster,
            _ => palette.planks,
        };
        sink.put(x, floor, z, palette.log);
        for y in floor + 1..=floor + wall_h {
            let p = facade_part(lot, x, z, (y - floor) as usize);
            let b = match p {
                WallPart::Beam | WallPart::Post => palette.log,
                WallPart::Window if y == floor + 2 => Block::GLASS_PANE.default_state,
                WallPart::Door if y <= floor + 2 && is_front(lot, x, z) => Block::AIR.default_state,
                _ if timber => palette.plaster,
                _ => palette.planks,
            };
            sink.put(x, y, z, b);
        }
        if r.is_corner(x, z) {
            for y in floor..=floor + wall_h {
                sink.put(x, y, z, palette.log);
            }
        }
        let _ = block;
    } else {
        sink.put(x, floor, z, palette.planks);
        clear_above(sink, x, floor + 1, floor + wall_h - 1, z);
    }
    // Gable roof
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
    let roof_y = floor + wall_h + 1 - dist.min(5);
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

fn place_town_church(
    sink: &mut impl BlockSink,
    lot: &Lot,
    x: i32,
    floor: i32,
    ground: i32,
    z: i32,
    palette: &Palette,
) {
    place_gabled(sink, lot.rect, x, floor, ground, z, palette, 7, true);
    let (cx, _cz) = lot.rect.center();
    if (x - cx).abs() <= 1 && (z - lot.rect.min_z).abs() <= 1 {
        for y in floor + 7..=floor + 14 {
            sink.put(x, y, z, palette.bricks);
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
        for y in floor..=floor + 10 {
            sink.put(x, y, z, palette.planks);
        }
        sink.put(x, floor + 11, z, palette.roof_slab);
    }
    // Blades
    if dx.abs() == dz.abs() && dx.abs() >= 1 && dx.abs() <= 3 {
        sink.put(x, floor + 8 + dx, z, Block::WHITE_WOOL.default_state);
        sink.put(x, floor + 8 - dx, z, Block::BROWN_WOOL.default_state);
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

fn facade_part(lot: &Lot, x: i32, z: i32, row: usize) -> WallPart {
    let (along, width) = match lot.facing {
        Dir::North | Dir::South => (x - lot.rect.min_x, lot.rect.width()),
        Dir::East | Dir::West => (z - lot.rect.min_z, lot.rect.depth()),
    };
    if !is_front(lot, x, z) {
        return if (x + z + row as i32) % 4 == 0 {
            WallPart::Window
        } else {
            WallPart::Plaster
        };
    }
    let w = width.max(1) as usize;
    let i = row.min(2) * w + (along.clamp(0, width - 1) as usize);
    lot.facade
        .get(i)
        .copied()
        .map_or(WallPart::Plaster, wall_part)
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
        let min_y = plan.heights.sea_level - 8;
        let max_y = plan.heights.sea_level + 80 + y_pad;
        let sx = plan.bounds.width();
        let sz = plan.bounds.depth();
        let sy = max_y - min_y + 1;
        let n = (sx * sy * sz) as usize;
        Self {
            min_x: plan.bounds.min_x,
            min_y,
            min_z: plan.bounds.min_z,
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
    for z in plan.bounds.min_z..=plan.bounds.max_z {
        for x in plan.bounds.min_x..=plan.bounds.max_x {
            let surface = plan.heights.height(x, z);
            let ground = surface - 1;
            for dy in 0..6 {
                let y = ground - dy;
                let block = if y < plan.heights.sea_level - 2 {
                    Block::STONE.default_state
                } else if dy == 0 && ground >= plan.heights.sea_level {
                    Block::GRASS_BLOCK.default_state
                } else {
                    Block::DIRT.default_state
                };
                volume.put(x, y, z, block);
            }
            if ground < plan.heights.sea_level {
                for y in ground + 1..=plan.heights.sea_level {
                    volume.put(x, y, z, Block::WATER.default_state);
                }
            }
            let _ = palette;
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
