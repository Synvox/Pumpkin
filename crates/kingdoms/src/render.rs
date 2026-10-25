//! Top-down hillshade and isometric voxel renders of a planned town.

use image::{ImageBuffer, Rgb, RgbImage};
use pumpkin_data::map_color::MapColor;
use pumpkin_data::{Block, BlockState};

use crate::buildings::BuildingKind;
use crate::castle::RoofKind;
use crate::interior::keep_storey_label;
use crate::plan::TownPlan;
use crate::stamp::{VolumeSink, fill_volume, ridge_floor};

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
    let (gx, gz) = plan.castle.gatehouse.center();
    let gate = render_iso(volume, gx - 22, gz - 22, gx + 22, gz + 22, 8, 1);
    let skyline = render_iso(
        volume,
        castle.min_x - 4,
        castle.min_z - 4,
        castle.max_x + 4,
        castle.max_z + 4,
        5,
        1,
    );
    save(&out_dir.join(format!("{stem}-topdown.png")), &top)?;
    save(&out_dir.join(format!("{stem}-iso.png")), &iso)?;
    save(&out_dir.join(format!("{stem}-castle.png")), &castle_iso)?;
    save(&out_dir.join(format!("{stem}-street.png")), &street)?;
    save(&out_dir.join(format!("{stem}-bailey.png")), &bailey)?;
    save(&out_dir.join(format!("{stem}-castle-gate.png")), &gate)?;
    save(
        &out_dir.join(format!("{stem}-castle-skyline.png")),
        &skyline,
    )?;
    write_cutaways(plan, volume, out_dir, stem)?;
    Ok(())
}

fn write_cutaways(
    plan: &TownPlan,
    volume: &VolumeSink,
    out_dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<()> {
    let hall = plan.castle.hall;
    let hall_floor = ridge_floor(plan, hall);
    save(
        &out_dir.join(format!("{stem}-hall-cutaway.png")),
        &render_room(
            volume,
            hall,
            hall_floor,
            hall_floor + 5,
            "Great hall · tables, dais, hearth",
        ),
    )?;

    let chapel = plan.castle.chapel;
    let chapel_floor = ridge_floor(plan, chapel);
    save(
        &out_dir.join(format!("{stem}-chapel-cutaway.png")),
        &render_room(
            volume,
            chapel,
            chapel_floor,
            chapel_floor + 5,
            "Chapel · pews, altar, stained glass",
        ),
    )?;

    let kitchen = plan.castle.kitchen;
    let kitchen_floor = ridge_floor(plan, kitchen);
    save(
        &out_dir.join(format!("{stem}-kitchen-cutaway.png")),
        &render_room(
            volume,
            kitchen,
            kitchen_floor,
            kitchen_floor + 4,
            "Kitchen · furnaces, cauldron, stores",
        ),
    )?;

    let barracks = plan.castle.barracks;
    let barracks_floor = ridge_floor(plan, barracks);
    save(
        &out_dir.join(format!("{stem}-barracks-cutaway.png")),
        &render_room(
            volume,
            barracks,
            barracks_floor,
            barracks_floor + 5,
            "Barracks · bunks and weapon racks",
        ),
    )?;

    let house_lot = plan.lots.iter().find(|lot| {
        matches!(
            lot.kind,
            BuildingKind::Cottage
                | BuildingKind::Timber
                | BuildingKind::Townhouse
                | BuildingKind::Tavern
        )
    });
    if let Some(lot) = house_lot {
        let floor = lot_floor(plan, lot);
        save(
            &out_dir.join(format!("{stem}-house-cutaway.png")),
            &render_room(
                volume,
                lot.rect,
                floor,
                floor + 3,
                "Town house · bed, table, hearth",
            ),
        )?;
        save(
            &out_dir.join(format!("{stem}-house.png")),
            &render_exterior(
                volume,
                lot.rect,
                2,
                "Town house · timber frame, closed gable",
            ),
        )?;
    }

    let keep_mass = plan
        .castle
        .keep_masses
        .iter()
        .max_by_key(|mass| mass.spec.height)
        .copied();
    if let Some(mass) = keep_mass {
        let floor = ridge_floor(plan, mass.spec.bounds());
        let stories = (mass.spec.height / 4).max(1);
        let mut mid = None;
        for storey in 0..stories {
            let y0 = floor + storey * 4;
            let label = keep_storey_label(mass.spec.height, mass.spec.inner, storey);
            let title = format!("Keep · storey {storey} · {label}");
            let image = render_room(volume, mass.spec.bounds(), y0, y0 + 3, &title);
            save(
                &out_dir.join(format!("{stem}-keep-storey-{storey}.png")),
                &image,
            )?;
            if storey == stories / 2 {
                mid = Some(image);
            }
        }
        if let Some(image) = mid {
            save(&out_dir.join(format!("{stem}-keep-cutaway.png")), &image)?;
        }
        let gable_bounds = plan
            .castle
            .keep_masses
            .iter()
            .find(|m| matches!(m.spec.roof, RoofKind::Gable { .. }))
            .map_or(mass.spec.bounds(), |m| m.spec.bounds());
        save(
            &out_dir.join(format!("{stem}-keep.png")),
            &render_exterior(volume, gable_bounds.inflate(3), 3, "Keep · closed gables"),
        )?;
    } else {
        let keep = plan.castle.keep;
        let floor = ridge_floor(plan, keep);
        save(
            &out_dir.join(format!("{stem}-keep-cutaway.png")),
            &render_room(volume, keep, floor, floor + 5, "Keep · enclosed floors"),
        )?;
        save(
            &out_dir.join(format!("{stem}-keep.png")),
            &render_exterior(volume, keep.inflate(2), 3, "Keep · closed gables"),
        )?;
    }
    Ok(())
}

fn lot_floor(plan: &TownPlan, lot: &crate::buildings::Lot) -> i32 {
    plan.heights
        .mean(
            lot.rect.min_x,
            lot.rect.min_z,
            lot.rect.max_x,
            lot.rect.max_z,
        )
        .max(plan.heights.sea_level + 2)
}

fn render_room(
    volume: &VolumeSink,
    rect: crate::geom::Rect,
    floor: i32,
    lid: i32,
    title: &str,
) -> RgbImage {
    let cx = rect.center().0;
    render_iso_view(
        volume,
        rect.min_x - 2,
        rect.min_z - 2,
        rect.max_x + 2,
        rect.max_z + 2,
        0,
        1,
        |x, y, _z| x <= cx + 1 && y >= floor - 1 && y <= lid,
        Some(title),
        true,
    )
}

fn render_exterior(
    volume: &VolumeSink,
    rect: crate::geom::Rect,
    pad: i32,
    title: &str,
) -> RgbImage {
    render_iso_view(
        volume,
        rect.min_x - pad,
        rect.min_z - pad,
        rect.max_x + pad,
        rect.max_z + pad,
        0,
        1,
        |_, _, _| true,
        Some(title),
        false,
    )
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
    if id == Block::TUFF.id {
        return [108, 109, 102];
    }
    if id == Block::DEEPSLATE_BRICKS.id || id == Block::DEEPSLATE.id {
        return [70, 70, 74];
    }
    if id == Block::VINE.id {
        return [46, 110, 40];
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
    if id == Block::LANTERN.id || id == Block::CANDLE.id {
        return [240, 190, 70];
    }
    if id == Block::BOOKSHELF.id {
        return [140, 90, 45];
    }
    if id == Block::RED_CARPET.id || id == Block::RED_WOOL.id || id == Block::RED_BED.id {
        return [160, 36, 36];
    }
    if id == Block::WHITE_BED.id {
        return [230, 230, 225];
    }
    if id == Block::OAK_DOOR.id || id == Block::SPRUCE_DOOR.id {
        return [110, 78, 40];
    }
    if id == Block::RED_STAINED_GLASS_PANE.id {
        return [170, 40, 40];
    }
    if id == Block::YELLOW_STAINED_GLASS_PANE.id {
        return [210, 190, 50];
    }
    if id == Block::FURNACE.id || id == Block::SMOKER.id || id == Block::BLAST_FURNACE.id {
        return [80, 80, 80];
    }
    if id == Block::BARREL.id || id == Block::CHEST.id {
        return [130, 88, 40];
    }
    if id == Block::RED_BANNER.id {
        return [160, 32, 32];
    }
    if id == Block::OAK_FENCE.id || id == Block::SPRUCE_FENCE.id {
        return [120, 90, 50];
    }
    if id == Block::RED_WOOL.id || id == Block::RED_TERRACOTTA.id {
        return [160, 36, 36];
    }
    if id == Block::WHITE_WOOL.id {
        return [230, 230, 225];
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
    render_iso_view(
        volume,
        min_x,
        min_z,
        max_x,
        max_z,
        tile,
        pitch,
        |_, _, _| true,
        None,
        false,
    )
}

/// Isometric view that keeps only voxels matching `keep`.
#[must_use]
pub fn render_iso_cut(
    volume: &VolumeSink,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    tile: i32,
    pitch: i32,
    keep: impl Fn(i32, i32, i32) -> bool,
) -> RgbImage {
    render_iso_view(
        volume, min_x, min_z, max_x, max_z, tile, pitch, keep, None, false,
    )
}

fn render_iso_view(
    volume: &VolumeSink,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    tile: i32,
    pitch: i32,
    keep: impl Fn(i32, i32, i32) -> bool,
    title: Option<&str>,
    fill_light: bool,
) -> RgbImage {
    let Some((bx0, by0, bz0, bx1, by1, bz1)) =
        occupied_bounds(volume, min_x, max_x, min_z, max_z, &keep)
    else {
        return ImageBuffer::from_pixel(8, 8, Rgb([168u8, 198, 230]));
    };
    let min_x = bx0 - 1;
    let max_x = bx1 + 1;
    let min_z = bz0 - 1;
    let max_z = bz1 + 1;
    let min_y = by0 - 1;
    let max_y = by1 + 1;
    let span_x = max_x - min_x + 1;
    let span_z = max_z - min_z + 1;
    let span_y = max_y - min_y + 1;
    let tile = if tile <= 0 {
        let span = (span_x + span_z).max(1);
        ((1600 - 8) / span).clamp(12, 28)
    } else {
        tile.max(2)
    };
    let tw = tile;
    let th = (tile / (1 + pitch.max(1))).max(1);
    let label_h = if title.is_some() { 28 } else { 0 };
    let width = ((span_x + span_z) * tw + 8) as u32;
    let height = ((span_x + span_z) * th + span_y * tile + 8 + label_h) as u32;
    let origin_x = span_z * tw + 4;
    let origin_y = span_y * tile + 4 + label_h;
    let mut image = ImageBuffer::from_pixel(width.max(1), height.max(1), Rgb([168u8, 198, 230]));
    let light = if fill_light { 1.18 } else { 1.0 };
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let top_y = volume.top(x, z).map_or(min_y, |(y, _)| y);
            let draw_min = column_draw_min(volume, x, z, top_y, min_y);
            for y in draw_min.max(min_y)..=max_y {
                if !keep(x, y, z) {
                    continue;
                }
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
                let occl = if fill_light {
                    0.92
                } else {
                    occlusion(volume, x, y, z)
                };
                draw_cube(&mut image, sx, sy, tw, th, tile, rgb, occl * light);
            }
        }
    }
    if let Some(title) = title {
        blit_label(&mut image, title);
    }
    image
}

fn occupied_bounds(
    volume: &VolumeSink,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    keep: &impl Fn(i32, i32, i32) -> bool,
) -> Option<(i32, i32, i32, i32, i32, i32)> {
    let mut found = false;
    let mut x0 = i32::MAX;
    let mut x1 = i32::MIN;
    let mut y0 = i32::MAX;
    let mut y1 = i32::MIN;
    let mut z0 = i32::MAX;
    let mut z1 = i32::MIN;
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            if let Some((top, _)) = volume.top(x, z) {
                let draw_min = column_draw_min(volume, x, z, top, volume.min_y);
                for y in draw_min..=top {
                    if !keep(x, y, z) {
                        continue;
                    }
                    let Some(state) = volume.get(x, y, z) else {
                        continue;
                    };
                    if state.is_air() {
                        continue;
                    }
                    found = true;
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                    z0 = z0.min(z);
                    z1 = z1.max(z);
                }
            }
        }
    }
    found.then_some((x0, y0, z0, x1, y1, z1))
}

fn blit_label(image: &mut RgbImage, title: &str) {
    let w = image.width();
    let bar = 24u32;
    if w == 0 || image.height() < bar {
        return;
    }
    for y in 0..bar {
        for x in 0..w {
            image.put_pixel(x, y, Rgb([28, 32, 40]));
        }
    }
    blit_text(image, 8, 6, title, Rgb([240, 236, 220]));
}

fn blit_text(image: &mut RgbImage, x0: i32, y0: i32, text: &str, color: Rgb<u8>) {
    let mut x = x0;
    let w = image.width() as i32;
    let h = image.height() as i32;
    for ch in text.chars() {
        let bits = glyph(ch);
        for (row, mask) in bits.iter().enumerate() {
            for col in 0..5 {
                if mask & (1 << (4 - col)) != 0 {
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let px = x + col * 2 + dx;
                            let py = y0 + row as i32 * 2 + dy;
                            if px >= 0 && py >= 0 && px < w && py < h {
                                image.put_pixel(px as u32, py as u32, color);
                            }
                        }
                    }
                }
            }
        }
        x += 12;
        if x > w - 12 {
            break;
        }
    }
}

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100,
        ],
        ',' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00100, 0b00100, 0b01000,
        ],
        ':' => [
            0b00000, 0b00100, 0b00100, 0b00000, 0b00100, 0b00100, 0b00000,
        ],
        '+' => [
            0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000,
        ],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '·' | '*' => [
            0b00000, 0b00000, 0b00100, 0b01110, 0b00100, 0b00000, 0b00000,
        ],
        _ => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000,
        ],
    }
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
