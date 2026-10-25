//! Top-down hillshade and isometric voxel renders of a planned town.

use image::{ImageBuffer, Rgb, RgbImage};
use pumpkin_data::map_color::MapColor;
use pumpkin_data::{Block, BlockState};

use crate::plan::TownPlan;
use crate::stamp::{VolumeSink, fill_volume};

/// Planner-only renders (no chunk generation).
pub fn write_planner_renders(
    plan: &TownPlan,
    out_dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<()> {
    write_renders(plan, &fill_volume(plan), out_dir, stem)
}

/// Renders a town volume into PNG files under `out_dir`.
pub fn write_renders(
    plan: &TownPlan,
    volume: &VolumeSink,
    out_dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let top = render_topdown(plan, volume);
    let iso = render_iso(
        volume,
        plan.bounds.min_x,
        plan.bounds.min_z,
        plan.bounds.max_x,
        plan.bounds.max_z,
        4,
        1,
    );
    let castle = plan.castle.bounds;
    let castle_iso = render_iso(
        volume,
        castle.min_x - 8,
        castle.min_z - 8,
        castle.max_x + 8,
        castle.max_z + 8,
        6,
        1,
    );
    let (mx, mz) = plan.market.center();
    let street = render_iso(volume, mx - 32, mz - 32, mx + 32, mz + 32, 7, 1);
    let keep = plan.castle.keep.center();
    let bailey = render_iso(
        volume,
        keep.0 - 28,
        keep.1 - 28,
        keep.0 + 28,
        keep.1 + 28,
        7,
        1,
    );
    save(&out_dir.join(format!("{stem}-topdown.png")), &top)?;
    save(&out_dir.join(format!("{stem}-iso.png")), &iso)?;
    save(&out_dir.join(format!("{stem}-castle.png")), &castle_iso)?;
    save(&out_dir.join(format!("{stem}-street.png")), &street)?;
    save(&out_dir.join(format!("{stem}-bailey.png")), &bailey)?;
    Ok(())
}

fn save(path: &std::path::Path, image: &RgbImage) -> std::io::Result<()> {
    image
        .save(path)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

fn block_rgb(state: &BlockState) -> [u8; 3] {
    let block = Block::from_state_id(state.id);
    let id = block.id;
    // Hand-tuned colors for the blocks towns actually use. Mojang textures
    // are not redistributed here.
    if id == Block::GRASS_BLOCK.id {
        return [86, 158, 61];
    }
    if id == Block::DIRT.id || id == Block::DIRT_PATH.id {
        return [134, 96, 67];
    }
    if id == Block::COBBLESTONE.id || id == Block::MOSSY_COBBLESTONE.id {
        return [127, 127, 127];
    }
    if id == Block::STONE_BRICKS.id || id == Block::CRACKED_STONE_BRICKS.id {
        return [122, 122, 122];
    }
    if id == Block::MOSSY_STONE_BRICKS.id {
        return [115, 121, 105];
    }
    if id == Block::ANDESITE.id {
        return [136, 136, 136];
    }
    if id == Block::OAK_LOG.id || id == Block::STRIPPED_OAK_LOG.id {
        return [109, 85, 50];
    }
    if id == Block::SPRUCE_LOG.id {
        return [58, 37, 16];
    }
    if id == Block::OAK_PLANKS.id {
        return [162, 130, 78];
    }
    if id == Block::WHITE_TERRACOTTA.id
        || id == Block::CALCITE.id
        || id == Block::BIRCH_PLANKS.id
        || id == Block::LIGHT_GRAY_TERRACOTTA.id
    {
        return [220, 214, 200];
    }
    if id == Block::DARK_OAK_STAIRS.id || id == Block::DARK_OAK_SLAB.id {
        return [67, 43, 20];
    }
    if id == Block::SPRUCE_STAIRS.id
        || id == Block::SPRUCE_PLANKS.id
        || id == Block::SPRUCE_SLAB.id
        || id == Block::STRIPPED_SPRUCE_LOG.id
    {
        return [114, 84, 48];
    }
    if id == Block::STONE_BRICK_STAIRS.id || id == Block::STONE_BRICK_SLAB.id {
        return [122, 122, 122];
    }
    if id == Block::IRON_BARS.id {
        return [90, 90, 98];
    }
    if id == Block::RED_BANNER.id {
        return [160, 32, 32];
    }
    if id == Block::OAK_LEAVES.id || id == Block::SPRUCE_LEAVES.id {
        return [48, 116, 40];
    }
    if id == Block::WATER.id {
        return [45, 82, 162];
    }
    if id == Block::SAND.id || id == Block::SANDSTONE.id || id == Block::CUT_SANDSTONE.id {
        return [219, 207, 163];
    }
    if id == Block::FARMLAND.id {
        return [95, 58, 31];
    }
    if id == Block::WHEAT.id || id == Block::CARROTS.id || id == Block::POTATOES.id {
        return [180, 170, 55];
    }
    if id == Block::HAY_BLOCK.id {
        return [182, 154, 52];
    }
    if let Some(color) = MapColor::from_id(block.map_color) {
        let (r, g, b) = color.rgb;
        if r != 0 || g != 0 || b != 0 {
            return [r, g, b];
        }
    }
    [96, 96, 96]
}

fn shade(rgb: [u8; 3], light: f32) -> Rgb<u8> {
    let f = light.clamp(0.25, 1.35);
    Rgb([
        (f32::from(rgb[0]) * f).min(255.0) as u8,
        (f32::from(rgb[1]) * f).min(255.0) as u8,
        (f32::from(rgb[2]) * f).min(255.0) as u8,
    ])
}

/// Top-down map: block color plus a simple hillshade.
#[must_use]
pub fn render_topdown(plan: &TownPlan, volume: &VolumeSink) -> RgbImage {
    let w = plan.bounds.width() as u32;
    let h = plan.bounds.depth() as u32;
    let scale = 3u32;
    let mut raw: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
    for z in plan.bounds.min_z..=plan.bounds.max_z {
        for x in plan.bounds.min_x..=plan.bounds.max_x {
            let px = (x - plan.bounds.min_x) as u32;
            let pz = (z - plan.bounds.min_z) as u32;
            let Some((y, state)) = volume.top(x, z) else {
                raw.put_pixel(px, pz, Rgb([20, 20, 30]));
                continue;
            };
            let n = volume.top(x, z - 1).map_or(y, |(ny, _)| ny);
            let wgt = volume.top(x - 1, z).map_or(y, |(ny, _)| ny);
            let slope = (n - y) as f32 * 0.12 + (wgt - y) as f32 * 0.08;
            let height_term = ((y - plan.heights.sea_level) as f32) * 0.008;
            raw.put_pixel(px, pz, shade(block_rgb(state), 0.92 + slope + height_term));
        }
    }
    let mut scaled = ImageBuffer::new(w * scale, h * scale);
    for y in 0..h {
        for x in 0..w {
            let p = *raw.get_pixel(x, y);
            for sy in 0..scale {
                for sx in 0..scale {
                    scaled.put_pixel(x * scale + sx, y * scale + sy, p);
                }
            }
        }
    }
    scaled
}

/// 3/4 isometric voxel view with face lighting.
#[must_use]
pub fn render_iso(
    volume: &VolumeSink,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    tile: i32,
    pitch: i32,
) -> RgbImage {
    let tile = tile.max(2);
    let tw = tile;
    let th = (tile / (1 + pitch.max(1))).max(1);
    let min_y = volume.min_y;
    let max_y = volume.min_y + volume.sy - 1;
    let span_x = max_x - min_x + 1;
    let span_z = max_z - min_z + 1;
    let span_y = max_y - min_y + 1;
    let width = ((span_x + span_z) * tw + 8) as u32;
    let height = ((span_x + span_z) * th + span_y * tile + 8) as u32;
    let origin_x = span_z * tw + 4;
    let origin_y = span_y * tile + 4;
    let mut image = ImageBuffer::from_pixel(width.max(1), height.max(1), Rgb([168u8, 198, 230]));
    // Back-to-front: high Z, then X, then Y.
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let top_y = volume.top(x, z).map_or(min_y, |(y, _)| y);
            let draw_min = column_draw_min(volume, x, z, top_y, min_y);
            for y in draw_min..=max_y {
                let Some(state) = volume.get(x, y, z) else {
                    continue;
                };
                if state.is_air() {
                    continue;
                }
                let rgb = block_rgb(state);
                let lx = x - min_x;
                let lz = z - min_z;
                let ly = y - min_y;
                let sx = origin_x + (lx - lz) * tw;
                let sy = origin_y + (lx + lz) * th - ly * tile;
                // Ambient occlusion from neighbors above/north/west.
                let occl = occlusion(volume, x, y, z);
                draw_cube(&mut image, sx, sy, tw, th, tile, rgb, occl);
            }
        }
    }
    image
}

fn column_draw_min(volume: &VolumeSink, x: i32, z: i32, top_y: i32, min_y: i32) -> i32 {
    let mut vis = top_y;
    let mut ground_run = 0;
    let stop = (top_y - 48).max(min_y);
    let mut y = top_y;
    while y >= stop {
        if let Some(state) = volume.get(x, y, z)
            && !state.is_air()
        {
            vis = y;
            if is_natural_ground(state) {
                ground_run += 1;
                if ground_run >= 3 {
                    break;
                }
            } else {
                ground_run = 0;
            }
        }
        y -= 1;
    }
    vis
}

fn is_natural_ground(state: &BlockState) -> bool {
    let id = Block::from_state_id(state.id).id;
    id == Block::GRASS_BLOCK.id
        || id == Block::DIRT.id
        || id == Block::DIRT_PATH.id
        || id == Block::FARMLAND.id
        || id == Block::STONE.id
        || id == Block::GRAVEL.id
        || id == Block::SAND.id
        || id == Block::RED_SAND.id
        || id == Block::WATER.id
        || id == Block::SNOW.id
        || id == Block::SNOW_BLOCK.id
        || id == Block::POWDER_SNOW.id
        || id == Block::DEEPSLATE.id
        || id == Block::TUFF.id
        || id == Block::CLAY.id
        || id == Block::MUD.id
        || id == Block::GRANITE.id
        || id == Block::DIORITE.id
}

fn occlusion(volume: &VolumeSink, x: i32, y: i32, z: i32) -> f32 {
    let mut n = 0;
    if volume.get(x, y + 1, z).is_some() {
        n += 1;
    }
    if volume.get(x - 1, y, z).is_some() {
        n += 1;
    }
    if volume.get(x, y, z - 1).is_some() {
        n += 1;
    }
    1.0 - 0.08 * n as f32
}

fn draw_cube(
    image: &mut RgbImage,
    sx: i32,
    sy: i32,
    tw: i32,
    th: i32,
    tile: i32,
    rgb: [u8; 3],
    occl: f32,
) {
    let w = image.width() as i32;
    let h = image.height() as i32;
    // Top face
    for dz in 0..th {
        for dx in -tw + dz..=tw - dz {
            put(image, w, h, sx + dx, sy + dz, shade(rgb, 1.15 * occl));
        }
        for dx in -tw + dz..=tw - dz {
            put(
                image,
                w,
                h,
                sx + dx,
                sy + th * 2 - dz,
                shade(rgb, 1.15 * occl),
            );
        }
    }
    // Left (west) face
    for y in 0..tile {
        for x in 0..tw {
            put(
                image,
                w,
                h,
                sx - tw + x,
                sy + th + y + x * th / tw.max(1),
                shade(rgb, 0.72 * occl),
            );
        }
    }
    // Right (south) face
    for y in 0..tile {
        for x in 0..tw {
            put(
                image,
                w,
                h,
                sx + x,
                sy + th + y + (tw - 1 - x) * th / tw.max(1),
                shade(rgb, 0.88 * occl),
            );
        }
    }
}

fn put(image: &mut RgbImage, w: i32, h: i32, x: i32, y: i32, color: Rgb<u8>) {
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    image.put_pixel(x as u32, y as u32, color);
}
