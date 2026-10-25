//! Per-column furniture. Each chunk writes only its own columns.
//!
//! World gen writes block states only. Chests, furnaces, beds and lecterns
//! will not get block entities until a later placement path exists.

use pumpkin_data::Block;
use pumpkin_data::BlockState;
use pumpkin_data::block_properties::{
    DoorHinge, DoubleBlockHalf, HorizontalFacing, OakDoorLikeProperties,
};

use crate::buildings::BuildingKind;
use crate::castle::{RoofKind, TowerSpec};
use crate::geom::{Dir, Rect};
use crate::palette::{Palette, stair_state};
use crate::stamp::BlockSink;

/// Closed spruce or oak door, lower then upper.
pub fn place_door(sink: &mut impl BlockSink, x: i32, y: i32, z: i32, facing: Dir, spruce: bool) {
    let facing = match facing {
        Dir::North => HorizontalFacing::North,
        Dir::South => HorizontalFacing::South,
        Dir::East => HorizontalFacing::East,
        Dir::West => HorizontalFacing::West,
    };
    let lower = OakDoorLikeProperties {
        facing,
        half: DoubleBlockHalf::Lower,
        hinge: DoorHinge::Left,
        open: false,
        powered: false,
    };
    let upper = OakDoorLikeProperties {
        half: DoubleBlockHalf::Upper,
        ..lower
    };
    let block = if spruce {
        &Block::SPRUCE_DOOR
    } else {
        &Block::OAK_DOOR
    };
    sink.put(x, y, z, BlockState::from_id(lower.to_state_id(block)));
    sink.put(x, y + 1, z, BlockState::from_id(upper.to_state_id(block)));
}

/// Outward facing from a rect edge.
#[must_use]
pub fn outward(rect: Rect, x: i32, z: i32) -> Dir {
    if z == rect.min_z {
        Dir::North
    } else if z == rect.max_z {
        Dir::South
    } else if x == rect.min_x {
        Dir::West
    } else {
        Dir::East
    }
}

/// Door column on the long side, at the midpoint.
#[must_use]
pub fn gable_door_at(rect: Rect, x: i32, z: i32) -> bool {
    if rect.is_corner(x, z) || !rect.on_edge(x, z) {
        return false;
    }
    let (cx, cz) = rect.center();
    if rect.width() >= rect.depth() {
        z == rect.max_z && x == cx
    } else {
        x == rect.max_x && z == cz
    }
}

/// Lantern on a fence post so it hangs without a neighbour write.
fn hang_lantern(sink: &mut impl BlockSink, x: i32, y: i32, z: i32) {
    sink.put(x, y, z, Block::OAK_FENCE.default_state);
    sink.put(x, y - 1, z, Block::LANTERN.default_state);
}

fn light_here(x: i32, z: i32, period: i32) -> bool {
    period > 0 && (x + z * 3).rem_euclid(period) == 0
}

/// Great hall: dais, tables, benches, runner, hearth, banners, chandeliers.
pub fn furnish_hall(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    palette: &Palette,
) {
    if rect.on_edge(x, z) {
        if !gable_door_at(rect, x, z) {
            decorate_wall(sink, rect, x, floor, z, true);
        }
        return;
    }
    let (cx, _) = rect.center();
    let lx = x - rect.min_x;
    let lz = z - rect.min_z;
    let depth = rect.depth();
    let aisle = x == cx;
    let dais = lz <= 2;

    if aisle {
        sink.put(x, floor + 1, z, Block::RED_CARPET.default_state);
    }
    if dais && lz == 1 && aisle {
        sink.put(x, floor + 1, z, stair_state(palette.stairs, Dir::South));
        sink.put(x, floor + 2, z, Block::RED_BANNER.default_state);
    } else if dais && (x - cx).abs() <= 2 && lz == 2 {
        sink.put(x, floor + 1, z, palette.slab);
    }
    if !dais && !aisle && (x - cx).abs() == 2 && lz % 3 != 0 {
        sink.put(x, floor + 1, z, palette.slab);
    }
    if !dais && (x - cx).abs() == 3 && lz % 3 != 0 {
        let dir = if x < cx { Dir::East } else { Dir::West };
        sink.put(x, floor + 1, z, stair_state(palette.stairs, dir));
    }
    if lz == depth - 3 && (x - cx).abs() <= 1 {
        sink.put(x, floor, z, palette.cobble);
        sink.put(x, floor + 1, z, Block::CAMPFIRE.default_state);
        if aisle {
            for y in floor + 2..roof_y {
                sink.put(x, y, z, palette.cobble);
            }
        }
    }
    if light_here(x, z, 5) && roof_y >= floor + 4 {
        hang_lantern(sink, x, roof_y - 2, z);
    }
    if lx == 2 && lz == depth / 2 {
        sink.put(x, floor + 1, z, Block::BARREL.default_state);
    }
}

/// Chapel / town church: pews, aisle, altar, candles, hanging light.
pub fn furnish_chapel(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    palette: &Palette,
    stained: bool,
) {
    if rect.on_edge(x, z) {
        if gable_door_at(rect, x, z) {
            return;
        }
        if stained && !rect.is_corner(x, z) && (x + z) % 3 == 0 {
            sink.put(x, floor + 3, z, Block::RED_STAINED_GLASS_PANE.default_state);
            sink.put(
                x,
                floor + 4,
                z,
                Block::YELLOW_STAINED_GLASS_PANE.default_state,
            );
        } else {
            decorate_wall(sink, rect, x, floor, z, true);
        }
        return;
    }
    let (cx, _) = rect.center();
    let lz = z - rect.min_z;
    let aisle = x == cx;
    if aisle {
        sink.put(x, floor + 1, z, Block::RED_CARPET.default_state);
    }
    if lz == 1 && (x - cx).abs() <= 1 {
        sink.put(x, floor + 1, z, palette.slab);
        if aisle {
            sink.put(x, floor + 2, z, Block::CANDLE.default_state);
            sink.put(x, floor + 3, z, Block::LANTERN.default_state);
        }
    }
    if lz >= 3 && lz % 2 == 0 && (x - cx).abs() >= 2 && (x - cx).abs() <= 4 {
        let dir = if x < cx { Dir::East } else { Dir::West };
        sink.put(x, floor + 1, z, stair_state(palette.stairs, dir));
    }
    if light_here(x, z, 6) && roof_y >= floor + 4 {
        hang_lantern(sink, x, roof_y - 2, z);
    }
}

/// Kitchen: hearths, cauldron, barrels, crafting, pantry.
pub fn furnish_kitchen(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    palette: &Palette,
) {
    if rect.on_edge(x, z) {
        if !gable_door_at(rect, x, z) {
            decorate_wall(sink, rect, x, floor, z, false);
        }
        return;
    }
    let lx = x - rect.min_x;
    let lz = z - rect.min_z;
    if lz == 1 && lx % 2 == 1 {
        sink.put(
            x,
            floor + 1,
            z,
            if lx % 4 == 1 {
                Block::FURNACE.default_state
            } else {
                Block::SMOKER.default_state
            },
        );
    } else if lx == 1 && lz == 2 {
        sink.put(x, floor + 1, z, Block::CAULDRON.default_state);
    } else if lx == 1 && lz == 3 {
        sink.put(x, floor + 1, z, Block::CRAFTING_TABLE.default_state);
    } else if (lx + lz) % 4 == 0 {
        sink.put(x, floor + 1, z, Block::BARREL.default_state);
    } else if (lx + lz) % 5 == 1 {
        sink.put(x, floor + 1, z, Block::HAY_BLOCK.default_state);
    }
    if light_here(x, z, 4) && roof_y >= floor + 4 {
        hang_lantern(sink, x, (roof_y - 2).max(floor + 3), z);
    }
    let _ = palette;
}

/// Barracks: bunks, weapon benches, chests.
pub fn furnish_barracks(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    palette: &Palette,
) {
    if rect.on_edge(x, z) {
        if !gable_door_at(rect, x, z) {
            decorate_wall(sink, rect, x, floor, z, false);
        }
        return;
    }
    let lx = x - rect.min_x;
    let lz = z - rect.min_z;
    if lx % 3 == 1 && lz % 2 == 1 {
        sink.put(x, floor + 1, z, Block::WHITE_BED.default_state);
        if roof_y >= floor + 5 {
            sink.put(x, floor + 3, z, Block::WHITE_BED.default_state);
        }
    } else if lx == 1 && lz == 2 {
        sink.put(x, floor + 1, z, Block::GRINDSTONE.default_state);
    } else if lx == 1 && lz == 3 {
        sink.put(x, floor + 1, z, Block::FLETCHING_TABLE.default_state);
    } else if lx == 1 && lz == 4 {
        sink.put(x, floor + 1, z, Block::SMITHING_TABLE.default_state);
    } else if (lx + lz) % 6 == 0 {
        sink.put(x, floor + 1, z, Block::CHEST.default_state);
    }
    if light_here(x, z, 4) {
        sink.put(x, floor + 2, z, Block::LANTERN.default_state);
    }
    let _ = palette;
}

/// Stables: stalls, hay, troughs. Front is fenced, not open air.
pub fn furnish_stables(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    palette: &Palette,
) {
    if rect.on_edge(x, z) {
        return;
    }
    let lx = x - rect.min_x;
    if lx % 4 == 0 {
        sink.put(x, floor + 1, z, Block::OAK_FENCE.default_state);
    } else if lx % 4 == 2 && (z - rect.min_z) == 1 {
        sink.put(x, floor + 1, z, Block::CAULDRON.default_state);
    } else if (x + z) % 5 == 0 {
        sink.put(x, floor + 1, z, Block::HAY_BLOCK.default_state);
    }
    if light_here(x, z, 5) && roof_y >= floor + 4 {
        hang_lantern(sink, x, roof_y - 2, z);
    }
    let _ = palette;
}

/// Town house / tavern / smith / barn interiors.
pub fn furnish_house(
    sink: &mut impl BlockSink,
    kind: BuildingKind,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    stories: i32,
    palette: &Palette,
) {
    if rect.on_edge(x, z) {
        decorate_wall(sink, rect, x, floor, z, false);
        return;
    }
    match kind {
        BuildingKind::Church => furnish_chapel(sink, rect, x, floor, z, roof_y, palette, true),
        BuildingKind::Blacksmith => furnish_smith(sink, rect, x, floor, z, roof_y),
        BuildingKind::Barn => furnish_barn(sink, rect, x, floor, z, roof_y),
        BuildingKind::Tavern => furnish_tavern(sink, rect, x, floor, z, roof_y, stories, palette),
        BuildingKind::Stall => {}
        BuildingKind::Windmill => furnish_windmill_room(sink, rect, x, floor, z, roof_y),
        _ => furnish_dwelling(sink, rect, x, floor, z, roof_y, stories, palette),
    }
}

fn furnish_dwelling(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    stories: i32,
    palette: &Palette,
) {
    let (cx, cz) = rect.center();
    let back = (x - cx).abs() + (z - cz).abs();
    for s in 0..stories {
        let y0 = floor + s * 4;
        if y0 + 1 >= roof_y {
            break;
        }
        if x == cx && z == cz {
            sink.put(x, y0 + 1, z, palette.slab);
            if s == 0 {
                sink.put(x, y0 + 1, z, Block::CRAFTING_TABLE.default_state);
            }
        } else if s == 0 && x == cx - 1 && z == cz {
            sink.put(x, y0 + 1, z, Block::FURNACE.default_state);
        } else if s == 0 && x == cx + 1 && z == cz {
            sink.put(x, y0 + 1, z, Block::CHEST.default_state);
        } else if back >= 2 && (x + z + s) % 5 == 0 {
            sink.put(x, y0 + 1, z, Block::WHITE_BED.default_state);
        } else if (x + z) % 7 == 2 && s == 0 {
            sink.put(x, y0 + 1, z, Block::BARREL.default_state);
        }
        if (x + z + s) % 4 == 0 {
            sink.put(x, y0 + 2, z, Block::LANTERN.default_state);
        }
        if s == 0 && x == cx && z == cz + 1 {
            sink.put(x, y0 + 1, z, Block::RED_CARPET.default_state);
        }
    }
    if roof_y >= floor + 5 && light_here(x, z, 6) {
        hang_lantern(sink, x, roof_y - 2, z);
    }
}

fn furnish_tavern(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
    stories: i32,
    palette: &Palette,
) {
    let (cx, cz) = rect.center();
    let lx = x - rect.min_x;
    let lz = z - rect.min_z;
    if (x - cx).abs() <= 1 && lz % 3 == 1 {
        sink.put(x, floor + 1, z, palette.slab);
    } else if (x - cx).abs() == 2 && lz % 3 == 1 {
        sink.put(
            x,
            floor + 1,
            z,
            stair_state(palette.stairs, if x < cx { Dir::East } else { Dir::West }),
        );
    } else if lx == 1 && lz == 1 {
        sink.put(x, floor + 1, z, Block::BARREL.default_state);
    } else if lx == 2 && lz == 1 {
        sink.put(x, floor + 1, z, Block::CAULDRON.default_state);
    } else if x == cx && z == cz {
        sink.put(x, floor + 1, z, Block::CRAFTING_TABLE.default_state);
    }
    if stories >= 2 && z == cz && (x - cx).abs() >= 2 {
        sink.put(x, floor + 5, z, Block::WHITE_BED.default_state);
    }
    if light_here(x, z, 4) {
        sink.put(x, floor + 3, z, Block::LANTERN.default_state);
    }
    if roof_y >= floor + 6 && light_here(x + 1, z, 5) {
        hang_lantern(sink, x, roof_y - 2, z);
    }
}

fn furnish_smith(sink: &mut impl BlockSink, rect: Rect, x: i32, floor: i32, z: i32, roof_y: i32) {
    let (cx, cz) = rect.center();
    if x == cx && z == cz {
        sink.put(x, floor + 1, z, Block::ANVIL.default_state);
    } else if x == cx - 1 && z == cz {
        sink.put(x, floor + 1, z, Block::FURNACE.default_state);
    } else if x == cx + 1 && z == cz {
        sink.put(x, floor + 1, z, Block::BLAST_FURNACE.default_state);
    } else if x == cx && z == cz - 1 {
        sink.put(x, floor + 1, z, Block::SMITHING_TABLE.default_state);
    } else if (x + z) % 5 == 0 {
        sink.put(x, floor + 1, z, Block::CHEST.default_state);
    }
    if light_here(x, z, 4) && roof_y >= floor + 4 {
        hang_lantern(sink, x, roof_y - 2, z);
    }
}

fn furnish_barn(sink: &mut impl BlockSink, rect: Rect, x: i32, floor: i32, z: i32, roof_y: i32) {
    let lx = x - rect.min_x;
    if lx % 4 == 0 {
        sink.put(x, floor + 1, z, Block::OAK_FENCE.default_state);
    } else if (x + z) % 4 == 0 {
        sink.put(x, floor + 1, z, Block::HAY_BLOCK.default_state);
    } else if (x + z) % 7 == 1 {
        sink.put(x, floor + 1, z, Block::CHEST.default_state);
    }
    if light_here(x, z, 5) && roof_y >= floor + 4 {
        hang_lantern(sink, x, roof_y - 2, z);
    }
}

fn furnish_windmill_room(
    sink: &mut impl BlockSink,
    rect: Rect,
    x: i32,
    floor: i32,
    z: i32,
    roof_y: i32,
) {
    let (cx, cz) = rect.center();
    if x == cx && z == cz {
        sink.put(x, floor + 1, z, Block::HAY_BLOCK.default_state);
        for y in floor + 2..roof_y {
            if (y - floor) % 4 != 0 {
                sink.put(x, y, z, Block::LADDER.default_state);
            }
        }
    } else if (x + z) % 3 == 0 {
        sink.put(x, floor + 1, z, Block::BARREL.default_state);
    }
    if light_here(x, z, 3) {
        sink.put(x, floor + 2, z, Block::LANTERN.default_state);
    }
}

/// Keep / tower interiors: storey floors are already placed by the stamper.
pub fn furnish_keep_mass(
    sink: &mut impl BlockSink,
    spec: TowerSpec,
    x: i32,
    floor: i32,
    z: i32,
    roof_cap: i32,
    palette: &Palette,
    cellar: bool,
) {
    if spec.on_rim(x, z) {
        return;
    }
    let dx = x - spec.cx;
    let dz = z - spec.cz;
    let stories = (spec.height / 4).max(1);
    let stair_col = dx == 1 && dz == 0;
    if cellar {
        let cy = floor - 5;
        if stair_col {
            sink.put(x, floor, z, Block::AIR.default_state);
            sink.put(x, floor - 1, z, Block::LADDER.default_state);
            sink.put(x, floor - 2, z, Block::LADDER.default_state);
            sink.put(x, floor - 3, z, Block::LADDER.default_state);
            sink.put(x, floor - 4, z, Block::LADDER.default_state);
        }
        if (dx + dz).abs() >= 2 && (x + z) % 3 == 0 {
            sink.put(x, cy + 1, z, Block::IRON_BARS.default_state);
            sink.put(x, cy + 2, z, Block::IRON_BARS.default_state);
        } else if (x + z) % 5 == 0 {
            sink.put(x, cy + 1, z, Block::CHEST.default_state);
        }
        sink.put(x, cy + 3, z, Block::LANTERN.default_state);
    }
    for s in 0..stories {
        let y0 = floor + s * 4;
        if y0 + 1 >= roof_cap {
            break;
        }
        if stair_col {
            for y in y0 + 1..y0 + 4 {
                if y < roof_cap {
                    sink.put(x, y, z, Block::LADDER.default_state);
                }
            }
            continue;
        }
        let room = keep_room(spec, s, stories);
        furnish_keep_room(sink, room, spec, x, y0, z, dx, dz, palette);
        if (x + z + s) % 3 == 0 {
            sink.put(x, y0 + 2, z, Block::LANTERN.default_state);
        }
    }
    if matches!(spec.roof, RoofKind::Crenellated) && dx.abs() + dz.abs() == 1 {
        sink.put(x, floor + spec.height + 1, z, Block::LANTERN.default_state);
    }
}

#[derive(Clone, Copy)]
enum KeepRoom {
    Storage,
    Guest,
    Solar,
    Bedchamber,
    Guard,
}

fn keep_room(spec: TowerSpec, storey: i32, stories: i32) -> KeepRoom {
    if spec.height >= 20 {
        match storey {
            0 => KeepRoom::Storage,
            1 => KeepRoom::Guest,
            2 => KeepRoom::Solar,
            s if s == stories - 1 => KeepRoom::Guard,
            _ => KeepRoom::Bedchamber,
        }
    } else if spec.inner {
        match storey {
            0 => KeepRoom::Storage,
            1 => KeepRoom::Solar,
            _ => KeepRoom::Bedchamber,
        }
    } else {
        match storey {
            0 => KeepRoom::Storage,
            _ => KeepRoom::Guard,
        }
    }
}

fn furnish_keep_room(
    sink: &mut impl BlockSink,
    room: KeepRoom,
    spec: TowerSpec,
    x: i32,
    y0: i32,
    z: i32,
    dx: i32,
    dz: i32,
    palette: &Palette,
) {
    match room {
        KeepRoom::Storage => {
            if (dx + dz) % 2 == 0 {
                sink.put(x, y0 + 1, z, Block::BARREL.default_state);
            } else if (x + z) % 4 == 0 {
                sink.put(x, y0 + 1, z, Block::CHEST.default_state);
            }
        }
        KeepRoom::Guest => {
            if dx == -1 && dz == 1 {
                sink.put(x, y0 + 1, z, Block::WHITE_BED.default_state);
            } else if dx == -2 && dz == 0 {
                sink.put(x, y0 + 1, z, Block::CHEST.default_state);
            } else if dx == 0 && dz == -1 {
                sink.put(x, y0 + 1, z, Block::RED_CARPET.default_state);
            }
        }
        KeepRoom::Solar => {
            if dx.abs() == spec.radius / 2 && dz == 0 {
                sink.put(x, y0 + 1, z, Block::BOOKSHELF.default_state);
            } else if dx == 0 && dz == 0 {
                sink.put(x, y0 + 1, z, Block::LECTERN.default_state);
            } else if dx == 1 && dz == 1 {
                sink.put(x, y0 + 1, z, Block::CARTOGRAPHY_TABLE.default_state);
            } else if dx == 0 && dz == 1 {
                sink.put(x, y0 + 1, z, palette.slab);
            }
        }
        KeepRoom::Bedchamber => {
            if dx == 0 && dz == -1 {
                sink.put(x, y0 + 1, z, Block::RED_BED.default_state);
            } else if dx == -1 && dz == -1 {
                sink.put(x, y0 + 1, z, Block::CHEST.default_state);
            } else if dx == 1 && dz == -1 {
                sink.put(x, y0 + 1, z, Block::BARREL.default_state);
            } else if dx == 0 && dz == 1 {
                sink.put(x, y0 + 1, z, palette.slab);
            } else if dx.abs() + dz.abs() == 2 {
                sink.put(x, y0 + 1, z, Block::RED_CARPET.default_state);
            }
        }
        KeepRoom::Guard => {
            if dx == 0 && dz == 0 {
                sink.put(x, y0 + 1, z, Block::FLETCHING_TABLE.default_state);
            } else if (dx + dz) % 2 == 0 {
                sink.put(x, y0 + 1, z, Block::CHEST.default_state);
            }
        }
    }
}

/// Gatehouse winch room over the passage.
pub fn furnish_winch(sink: &mut impl BlockSink, x: i32, floor: i32, z: i32, at_center: bool) {
    if at_center {
        sink.put(x, floor + 7, z, Block::OAK_FENCE.default_state);
        sink.put(x, floor + 9, z, Block::LANTERN.default_state);
    } else {
        sink.put(x, floor + 7, z, Block::BARREL.default_state);
    }
}

fn decorate_wall(sink: &mut impl BlockSink, rect: Rect, x: i32, floor: i32, z: i32, fancy: bool) {
    if rect.is_corner(x, z) {
        return;
    }
    if fancy && (x + z) % 5 == 1 {
        sink.put(x, floor + 3, z, Block::RED_WOOL.default_state);
        sink.put(x, floor + 4, z, Block::RED_BANNER.default_state);
    } else if (x + z) % 7 == 0 {
        sink.put(x, floor + 3, z, Block::GREEN_WOOL.default_state);
    }
}

/// Wall torch / lantern on a rim column (same column, no neighbour write).
pub fn rim_sconce(sink: &mut impl BlockSink, x: i32, y: i32, z: i32) {
    if (x + z) % 4 == 0 {
        sink.put(x, y, z, Block::LANTERN.default_state);
    }
}
