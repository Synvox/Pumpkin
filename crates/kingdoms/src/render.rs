//! Top-down hillshade and isometric voxel renders of a planned town.

use image::{ImageBuffer, Rgb, RgbImage};
use pumpkin_data::map_color::MapColor;
use pumpkin_data::{Block, BlockState};

use crate::buildings::{BuildingKind, Lot};
use crate::castle::RoofKind;
use crate::cells::CellKind;
use crate::geom::{Dir, Rect};
use crate::interior::keep_storey_label;
use crate::palette::PaletteKind;
use crate::plan::TownPlan;
use crate::scenes::SceneKind;
use crate::siege::FLAG_MAST_FENCES;
use crate::stamp::{BlockSink, VolumeSink, fill_volume, ridge_floor};

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
        3,
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
    for (rot, name) in [(0u8, "se"), (1u8, "sw"), (2u8, "nw"), (3u8, "ne")] {
        save(
            &out_dir.join(format!("{stem}-castle-skyline-{name}.png")),
            &render_iso_view(
                volume,
                keep.0 - 36,
                keep.1 - 36,
                keep.0 + 36,
                keep.1 + 36,
                8,
                1,
                |_, _, _| true,
                Some("Castle skyline · spires, towers, banners"),
                false,
                rot,
            ),
        )?;
    }
    write_cutaways(plan, volume, out_dir, stem)?;
    write_scene_renders(plan, volume, out_dir, stem)?;
    write_flag_renders(plan, volume, out_dir, stem)?;
    Ok(())
}

fn write_flag_renders(
    plan: &TownPlan,
    volume: &VolumeSink,
    out_dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<()> {
    let (fx, fz) = plan.castle.flag;
    let castle = &plan.castle;
    let plinth_col = |x: i32, z: i32| {
        (x - fx).abs() <= 2
            && (z - fz).abs() <= 2
            && castle.keep_mass_at(x, z).is_none()
            && !castle.hall.contains(x, z)
            && !castle.chapel.contains(x, z)
            && !castle.kitchen.contains(x, z)
    };
    let mast_col = |x: i32, z: i32| plinth_col(x, z) && (x - fx).abs() <= 1 && (z - fz).abs() <= 1;
    let mut owned = clone_volume(volume);
    paint_flag_cloth(&mut owned, fx, fz, None);
    let crop = render_iso_view(
        &owned,
        fx - 4,
        fz - 4,
        fx + 4,
        fz + 4,
        28,
        1,
        |x, _, z| mast_col(x, z),
        Some("Flag plinth"),
        false,
        0,
    );
    save(&out_dir.join(format!("{stem}-flag-se.png")), &crop)?;
    let (kx, kz) = castle.keep.center();
    let mast_top = owned.top(fx, fz).map_or(80, |(y, _)| y);
    let court_floor = mast_top - FLAG_MAST_FENCES - 1;
    let mut rot = 0u8;
    let mut best = i32::MIN;
    for candidate in 0u8..4 {
        let (lx, lz) = iso_local(
            candidate,
            fx,
            fz,
            kx.min(fx),
            kz.min(fz),
            kx.max(fx),
            kz.max(fz),
        );
        let score = lx + lz;
        if score > best {
            best = score;
            rot = candidate;
        }
    }
    let court = render_iso_view(
        &owned,
        fx.min(kx) - 6,
        fz.min(kz) - 6,
        fx.max(kx) + 6,
        fz.max(kz) + 6,
        20,
        1,
        |x, y, z| {
            if castle.keep_mass_at(x, z).is_some() || castle.hall.contains(x, z) {
                y <= mast_top
            } else if plinth_col(x, z) {
                true
            } else {
                matches!(
                    plan.occ.kind(x, z),
                    CellKind::CastleCourt | CellKind::Plaza | CellKind::FlagPlinth
                ) && y <= court_floor + 1
            }
        },
        Some("Flag, keep behind"),
        false,
        rot,
    );
    save(&out_dir.join(format!("{stem}-flag-court-se.png")), &court)?;
    let mut flipped = clone_volume(volume);
    paint_flag_cloth(&mut flipped, fx, fz, Some(Block::BLACK_WOOL.default_state));
    let after = render_iso_view(
        &flipped,
        fx - 4,
        fz - 4,
        fx + 4,
        fz + 4,
        28,
        1,
        |x, _, z| mast_col(x, z),
        Some("Flag after capture"),
        false,
        0,
    );
    save(&out_dir.join(format!("{stem}-flag-se-flipped.png")), &after)?;
    Ok(())
}

fn clone_volume(volume: &VolumeSink) -> VolumeSink {
    VolumeSink {
        min_x: volume.min_x,
        min_y: volume.min_y,
        min_z: volume.min_z,
        sx: volume.sx,
        sy: volume.sy,
        sz: volume.sz,
        voxels: volume.voxels.clone(),
    }
}

fn paint_flag_cloth(volume: &mut VolumeSink, fx: i32, fz: i32, cloth: Option<&BlockState>) {
    let mut painted_banner = false;
    for y in volume.min_y..volume.min_y + volume.sy {
        let Some(state) = volume.get(fx, y, fz) else {
            continue;
        };
        let id = Block::from_state_id(state.id).id;
        let mapped = if id == Block::GREEN_BANNER.id {
            Block::GREEN_WOOL.default_state
        } else if id == Block::BLUE_BANNER.id {
            Block::BLUE_WOOL.default_state
        } else if id == Block::BLACK_BANNER.id {
            Block::BLACK_WOOL.default_state
        } else {
            continue;
        };
        volume.put(fx, y, fz, cloth.unwrap_or(mapped));
        painted_banner = true;
        break;
    }
    if !painted_banner && let Some((top, _)) = volume.top(fx, fz) {
        volume.put(
            fx,
            top + 1,
            fz,
            cloth.unwrap_or(Block::GREEN_WOOL.default_state),
        );
    }
    let Some(recolor) = cloth else {
        return;
    };
    for dz in -2..=2 {
        for dx in -2..=2 {
            if dx == 0 && dz == 0 {
                continue;
            }
            for y in volume.min_y..volume.min_y + volume.sy {
                let Some(state) = volume.get(fx + dx, y, fz + dz) else {
                    continue;
                };
                let id = Block::from_state_id(state.id).id;
                if id == Block::GREEN_WOOL.id
                    || id == Block::BLUE_WOOL.id
                    || id == Block::BLACK_WOOL.id
                    || id == Block::GREEN_BANNER.id
                    || id == Block::BLUE_BANNER.id
                    || id == Block::BLACK_BANNER.id
                {
                    volume.put(fx + dx, y, fz + dz, recolor);
                }
            }
        }
    }
}

fn write_scene_renders(
    plan: &TownPlan,
    volume: &VolumeSink,
    out_dir: &std::path::Path,
    stem: &str,
) -> std::io::Result<()> {
    let overview = overview_bounds(plan);
    let span = (overview.width() + overview.depth()).max(1);
    let tile = (1600 / span).clamp(2, 6);
    save(
        &out_dir.join(format!("{stem}-aerial.png")),
        &render_iso_view(
            volume,
            overview.min_x,
            overview.min_z,
            overview.max_x,
            overview.max_z,
            tile,
            1,
            |_, _, _| true,
            Some("Castle town · farms, roads, hinterland"),
            false,
            0,
        ),
    )?;
    let mega = megacity_bounds(plan);
    let mega_span = (mega.width() + mega.depth()).max(1);
    let mega_tile = (1800 / mega_span).clamp(2, 5);
    for (rot, name) in [(0u8, "se"), (1u8, "sw"), (2u8, "nw")] {
        save(
            &out_dir.join(format!("{stem}-showcase-{name}.png")),
            &render_iso_view(
                volume,
                mega.min_x,
                mega.min_z,
                mega.max_x,
                mega.max_z,
                mega_tile,
                1,
                |_, _, _| true,
                Some("Megacity · castle, cathedral, market, harbour, mills"),
                false,
                rot,
            ),
        )?;
    }

    let mut seen = Vec::new();
    for scene in &plan.scenes {
        if seen.contains(&scene.kind) {
            continue;
        }
        seen.push(scene.kind);
        let pad = match scene.kind {
            SceneKind::Harbour | SceneKind::Riverside => 4,
            SceneKind::PillagerCamp
            | SceneKind::Farmstead
            | SceneKind::Hamlet
            | SceneKind::Cathedral
            | SceneKind::MountainFortress
            | SceneKind::Monastery
            | SceneKind::Mine
            | SceneKind::Ruin
            | SceneKind::Tourney
            | SceneKind::Frontier
            | SceneKind::Manor
            | SceneKind::Vineyard
            | SceneKind::Gorge
            | SceneKind::StaveChurch
            | SceneKind::Caravanserai
            | SceneKind::MarketQuarter
            | SceneKind::CraftsmenRow
            | SceneKind::NobleQuarter
            | SceneKind::DocksQuarter
            | SceneKind::LumberCamp
            | SceneKind::CathedralClose
            | SceneKind::Extramural
            | SceneKind::RuinedAbbey
            | SceneKind::CastleRuin
            | SceneKind::BanditHideout
            | SceneKind::HermitTower
            | SceneKind::StoneCircle
            | SceneKind::Gallows
            | SceneKind::TollBridge
            | SceneKind::CharcoalCamp
            | SceneKind::RoadsideInn
            | SceneKind::WalledMarket
            | SceneKind::MillHamlet
            | SceneKind::Beacon
            | SceneKind::Quarry
            | SceneKind::SiegeCamp => 6,
            _ => 3,
        };
        let mut rect = scene.rect;
        if scene.kind == SceneKind::Harbour {
            rect = harbour_scene_rect(plan, rect);
        } else if scene.kind == SceneKind::Cathedral {
            rect = cathedral_focus_rect(plan, rect);
        } else if scene.kind == SceneKind::StaveChurch {
            if let Some(lot) = plan.lots.iter().find(|lot| {
                lot.kind == BuildingKind::StaveChurch && scene.rect.intersects(lot.rect)
            }) {
                rect = lot.rect.inflate(6);
            }
        } else if scene.kind == SceneKind::Vineyard {
            // Keep the outer terraces in frame.
        } else if scene.kind == SceneKind::MillHamlet {
            rect = mill_hamlet_focus_rect(plan, rect);
        } else if scene.kind == SceneKind::Tourney {
            rect = tourney_focus_rect(plan, rect);
        } else if scene.kind == SceneKind::WalledMarket {
            rect = scene.rect.inflate(2);
        } else if rect.width() > 56 || rect.depth() > 56 {
            let (cx, cz) = rect.center();
            rect = Rect::new(cx - 28, cz - 28, cx + 27, cz + 27);
        }
        let name = scene.kind.label().to_ascii_lowercase().replace(' ', "-");
        save(
            &out_dir.join(format!("{stem}-scene-{name}.png")),
            &render_exterior(volume, rect, pad, scene.kind.label()),
        )?;
    }
    if let Some(light) = largest_blob(plan, CellKind::Lighthouse) {
        save(
            &out_dir.join(format!("{stem}-scene-lighthouse.png")),
            &render_exterior(volume, light.inflate(8), 3, "Lighthouse · breakwater tip"),
        )?;
    }
    if let Some(mill) = plan
        .lots
        .iter()
        .find(|lot| lot.kind == BuildingKind::Windmill)
    {
        save(
            &out_dir.join(format!("{stem}-scene-windmill.png")),
            &render_exterior(
                volume,
                mill.rect.inflate(16),
                2,
                "Windmill · open-air sails",
            ),
        )?;
    }
    if let Some(mill) = plan
        .lots
        .iter()
        .find(|lot| lot.kind == BuildingKind::Watermill)
    {
        let mut rect = mill.rect.inflate(4);
        if let Some(wheel) = wheel_for_mill(plan, mill.rect) {
            rect = union_rect(rect, wheel.inflate(1));
            save(
                &out_dir.join(format!("{stem}-scene-water-wheel.png")),
                &render_wheel_elevation(volume, wheel, "Water wheel"),
            )?;
        }
        let river_rot = plan
            .lots
            .iter()
            .filter(|lot| lot.kind == BuildingKind::Watermill)
            .map(|lot| mill_river_rot(plan, lot.rect, lot.facing))
            .find(|&rot| rot == 0)
            .unwrap_or(2);
        save(
            &out_dir.join(format!("{stem}-scene-watermill.png")),
            &render_exterior_rot(
                volume,
                rect,
                1,
                "Watermill · wheel from the river",
                river_rot,
            ),
        )?;
        for (rot, name) in [(0u8, "se"), (1u8, "sw"), (2u8, "nw"), (3u8, "ne")] {
            save(
                &out_dir.join(format!("{stem}-scene-watermill-{name}.png")),
                &render_exterior_rot(volume, rect, 1, &format!("Watermill · {name}"), rot),
            )?;
        }
    }
    save(
        &out_dir.join(format!("{stem}-scene-market.png")),
        &render_exterior(volume, plan.market.inflate(16), 3, "Market square"),
    )?;
    if plan.capital {
        save(
            &out_dir.join(format!("{stem}-scene-capital.png")),
            &render_exterior(
                volume,
                plan.castle.bounds.inflate(6),
                4,
                "Royal capital · inner and outer bailey",
            ),
        )?;
    }
    if plan.walled_city {
        if let Some(walk) = wall_walk_rect(plan) {
            save(
                &out_dir.join(format!("{stem}-scene-wall-walk.png")),
                &render_exterior(volume, walk, 2, "City wall-walk · crenellated curtain"),
            )?;
            save(
                &out_dir.join(format!("{stem}-scene-curtain-ring.png")),
                &render_exterior(
                    volume,
                    walk.inflate(10),
                    2,
                    "City curtain · pomerium, wall and fosse",
                ),
            )?;
        }
        if let Some(cat) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == SceneKind::Cathedral)
        {
            save(
                &out_dir.join(format!("{stem}-scene-cathedral-city.png")),
                &render_exterior(
                    volume,
                    cathedral_focus_rect(plan, cat.rect),
                    3,
                    match plan.palette {
                        PaletteKind::Sandstone => {
                            "Cathedral · sandstone basilica, minaret and gold dome"
                        }
                        PaletteKind::Spruce => "Cathedral · Romanesque spruce and stone",
                        PaletteKind::Oak => "Cathedral · gothic towers and spires",
                    },
                ),
            )?;
        }
        if let Some(sub) = plan
            .scenes
            .iter()
            .find(|scene| scene.kind == SceneKind::Extramural)
        {
            save(
                &out_dir.join(format!("{stem}-scene-extramural.png")),
                &render_exterior(
                    volume,
                    sub.rect.inflate(4),
                    2,
                    "Extramural suburb · cottages, tannery, almshouse",
                ),
            )?;
        }
    }
    if let Some(hull) = largest_blob(plan, CellKind::Ship) {
        save(
            &out_dir.join(format!("{stem}-scene-harbour-ship.png")),
            &render_exterior(volume, hull.inflate(6), 2, "Harbour · cog at the quay"),
        )?;
        save(
            &out_dir.join(format!("{stem}-scene-harbour-ship-close.png")),
            &render_exterior_rot(
                volume,
                hull.inflate(4),
                2,
                "Cog · pointed bow, crow's nest, billowed sail, yard",
                ship_beam_rot(hull),
            ),
        )?;
    }
    for kind in [
        SceneKind::RuinedAbbey,
        SceneKind::CastleRuin,
        SceneKind::BanditHideout,
        SceneKind::HermitTower,
        SceneKind::StoneCircle,
        SceneKind::TollBridge,
        SceneKind::CharcoalCamp,
        SceneKind::RoadsideInn,
        SceneKind::WalledMarket,
        SceneKind::MillHamlet,
        SceneKind::Beacon,
        SceneKind::Tourney,
        SceneKind::Monastery,
        SceneKind::Vineyard,
        SceneKind::Quarry,
        SceneKind::SiegeCamp,
        SceneKind::Riverside,
    ] {
        let Some(scene) = plan.scenes.iter().find(|scene| scene.kind == kind) else {
            continue;
        };
        let rect = scene.rect.inflate(2);
        let name = kind.label().to_ascii_lowercase().replace(' ', "-");
        let title = match kind {
            SceneKind::RuinedAbbey => "Ruined abbey · Tintern nave, tracery, arcade, graves",
            SceneKind::CastleRuin => "Castle ruin · broken curtain and keep on a hill",
            SceneKind::BanditHideout => "Bandit hideout · spread camp, spit, wagon, lookouts",
            SceneKind::HermitTower => "Hermit's tower · balcony, chimney, garden",
            SceneKind::StoneCircle => "Stone circle · menhirs and fallen lintel",
            SceneKind::TollBridge => "Toll bridge · gatehouse over the river",
            SceneKind::CharcoalCamp => "Charcoal camp · hut, mounds and hearths",
            SceneKind::RoadsideInn => "Roadside inn · cobbled yard, wagon, well, jettied tavern",
            SceneKind::WalledMarket => "Walled market · guildhall, stalls, palisade, well",
            SceneKind::MillHamlet => "Mill hamlet · wheel, race, granary",
            SceneKind::Beacon => "Hilltop beacon · brazier and watch-fire",
            SceneKind::Tourney => "Jousting lists · sand lane, stands, pavilions beside the castle",
            SceneKind::Monastery => {
                "Priory · cloister garth, chapter house, scriptorium, herb garden"
            }
            SceneKind::Vineyard => "Vineyard · terraced hillside rows and press house",
            SceneKind::Quarry => "Quarry · cranes, cut stone, mason's lodge",
            SceneKind::SiegeCamp => "Siege camp · trebuchets, tents, palisaded earthworks",
            SceneKind::Riverside => "Riverside village · stone bridge, ford and fishing weirs",
            _ => kind.label(),
        };
        save(
            &out_dir.join(format!("{stem}-scene-{name}-close.png")),
            &render_exterior_rot(volume, rect, 2, title, 0),
        )?;
        save(
            &out_dir.join(format!("{stem}-scene-{name}-close-sw.png")),
            &render_exterior_rot(volume, rect, 2, title, 1),
        )?;
    }
    if let Some(market) = fish_market_lot(plan) {
        save(
            &out_dir.join(format!("{stem}-scene-fish-market.png")),
            &render_exterior(
                volume,
                market.rect.inflate(10),
                3,
                "Harbour · fish market on the quay",
            ),
        )?;
    }
    Ok(())
}

fn wheel_for_mill(plan: &TownPlan, mill: Rect) -> Option<Rect> {
    let pad = mill.inflate(8);
    let mut found: Option<Rect> = None;
    for z in pad.min_z..=pad.max_z {
        for x in pad.min_x..=pad.max_x {
            if plan.occ.kind(x, z) != CellKind::WaterWheel {
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

fn largest_blob(plan: &TownPlan, kind: CellKind) -> Option<Rect> {
    let mut best: Option<Rect> = None;
    let mut best_n = 0;
    for z in plan.occ.bounds.min_z..=plan.occ.bounds.max_z {
        for x in plan.occ.bounds.min_x..=plan.occ.bounds.max_x {
            if plan.occ.kind(x, z) != kind {
                continue;
            }
            if best.is_some_and(|rect| rect.contains(x, z)) {
                continue;
            }
            let blob = blob_kind(plan, x, z, kind);
            let n = blob.width() * blob.depth();
            if n > best_n {
                best_n = n;
                best = Some(blob);
            }
        }
    }
    best
}

fn blob_kind(plan: &TownPlan, x: i32, z: i32, kind: CellKind) -> Rect {
    let mut min_x = x;
    let mut max_x = x;
    let mut min_z = z;
    let mut max_z = z;
    for dz in -8..=8 {
        for dx in -8..=8 {
            if plan.occ.kind(x + dx, z + dz) == kind {
                min_x = min_x.min(x + dx);
                max_x = max_x.max(x + dx);
                min_z = min_z.min(z + dz);
                max_z = max_z.max(z + dz);
            }
        }
    }
    Rect::new(min_x, min_z, max_x, max_z)
}

fn wall_walk_rect(plan: &TownPlan) -> Option<Rect> {
    let wall = plan.town.inflate(2);
    let (cx, _) = wall.center();
    let candidates = [
        Rect::new(cx - 12, wall.max_z - 8, cx + 12, wall.max_z + 4),
        Rect::new(cx - 12, wall.min_z - 4, cx + 12, wall.min_z + 8),
        Rect::new(
            wall.max_x - 8,
            wall.center().1 - 12,
            wall.max_x + 4,
            wall.center().1 + 12,
        ),
        Rect::new(
            wall.min_x - 4,
            wall.center().1 - 12,
            wall.min_x + 8,
            wall.center().1 + 12,
        ),
    ];
    candidates.into_iter().find(|rect| {
        (rect.min_z..=rect.max_z).any(|z| {
            (rect.min_x..=rect.max_x).any(|x| {
                matches!(
                    plan.occ.kind(x, z),
                    CellKind::TownWall | CellKind::TownTower | CellKind::TownGate
                )
            })
        })
    })
}

/// Crop used for aerials and Features-chunk rerenders.
#[must_use]
pub fn overview_bounds(plan: &TownPlan) -> Rect {
    let mut bounds = plan.castle.bounds.inflate(8);
    bounds = union_rect(bounds, plan.market.inflate(24));
    bounds = union_rect(bounds, plan.town.inflate(8));
    for scene in &plan.scenes {
        if scene.kind == SceneKind::Harbour {
            bounds = union_rect(bounds, scene.rect.inflate(8));
            for kind in [
                CellKind::Ship,
                CellKind::Boat,
                CellKind::Breakwater,
                CellKind::Lighthouse,
                CellKind::Quay,
                CellKind::Crane,
                CellKind::Dock,
            ] {
                if let Some(blob) = largest_blob(plan, kind) {
                    bounds = union_rect(bounds, blob.inflate(10));
                }
            }
            continue;
        }
        bounds = union_rect(bounds, scene.rect.inflate(8));
    }
    for lot in &plan.lots {
        if lot.kind == BuildingKind::Windmill {
            bounds = union_rect(bounds, lot.rect.inflate(16));
        }
    }
    Rect::new(
        bounds.min_x.max(plan.bounds.min_x),
        bounds.min_z.max(plan.bounds.min_z),
        bounds.max_x.min(plan.bounds.max_x),
        bounds.max_z.min(plan.bounds.max_z),
    )
}

fn megacity_bounds(plan: &TownPlan) -> Rect {
    let mut bounds = overview_bounds(plan);
    for kind in [
        CellKind::Ship,
        CellKind::Boat,
        CellKind::Breakwater,
        CellKind::Lighthouse,
        CellKind::WaterWheel,
        CellKind::Quay,
        CellKind::MillRace,
    ] {
        if let Some(blob) = largest_blob(plan, kind) {
            bounds = union_rect(bounds, blob.inflate(10));
        }
    }
    for lot in &plan.lots {
        if matches!(lot.kind, BuildingKind::Watermill | BuildingKind::Windmill) {
            bounds = union_rect(bounds, lot.rect.inflate(12));
        }
    }
    Rect::new(
        bounds.min_x.max(plan.bounds.min_x),
        bounds.min_z.max(plan.bounds.min_z),
        bounds.max_x.min(plan.bounds.max_x),
        bounds.max_z.min(plan.bounds.max_z),
    )
}

fn mill_river_rot(plan: &TownPlan, mill: Rect, facing: Dir) -> u8 {
    let water = wheel_for_mill(plan, mill).map_or_else(
        || facing.opposite(),
        |wheel| {
            let (wx, wz) = wheel.center();
            let (mx, mz) = mill.center();
            if (wx - mx).abs() >= (wz - mz).abs() {
                if wx >= mx { Dir::East } else { Dir::West }
            } else if wz >= mz {
                Dir::South
            } else {
                Dir::North
            }
        },
    );
    match water {
        Dir::East | Dir::South => 0,
        Dir::West | Dir::North => 2,
    }
}

fn ship_beam_rot(hull: Rect) -> u8 {
    u8::from(hull.width() < hull.depth())
}

fn iso_local(
    rot: u8,
    x: i32,
    z: i32,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
) -> (i32, i32) {
    match rot % 4 {
        0 => (x - min_x, z - min_z),
        1 => (z - min_z, max_x - x),
        2 => (max_x - x, max_z - z),
        _ => (max_z - z, x - min_x),
    }
}

fn union_rect(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.min_x.min(b.min_x),
        a.min_z.min(b.min_z),
        a.max_x.max(b.max_x),
        a.max_z.max(b.max_z),
    )
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
            hall_floor + 8,
            "Great hall · tables, banners, chandeliers",
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
        let (cut_title, ext_title) = match plan.palette {
            PaletteKind::Sandstone => (
                "Desert house · furnished rooms, flat roof",
                "Desert house · sandstone, flat roof, dome",
            ),
            PaletteKind::Spruce => (
                "Town house · bed, table, hearth",
                "Town house · dark timber, steep snowy gable",
            ),
            PaletteKind::Oak => (
                "Town house · bed, table, hearth",
                "Town house · timber frame, closed gable",
            ),
        };
        save(
            &out_dir.join(format!("{stem}-house-cutaway.png")),
            &render_room(volume, lot.rect, floor, floor + 3, cut_title),
        )?;
        save(
            &out_dir.join(format!("{stem}-house.png")),
            &render_exterior(volume, lot.rect, 2, ext_title),
        )?;
    }

    let inn_lot = plan.lots.iter().find(|lot| {
        lot.kind == BuildingKind::Tavern
            && plan.scenes.iter().any(|scene| {
                scene.kind == SceneKind::RoadsideInn && scene.rect.intersects(lot.rect)
            })
    });
    if let Some(lot) = inn_lot {
        let floor = lot_floor(plan, lot);
        save(
            &out_dir.join(format!("{stem}-inn-cutaway.png")),
            &render_room(
                volume,
                lot.rect,
                floor,
                floor + 8,
                "Inn · bar, kegs, hearth, rooms",
            ),
        )?;
        save(
            &out_dir.join(format!("{stem}-inn.png")),
            &render_exterior(
                volume,
                lot.rect.inflate(4),
                2,
                "Inn · jetty, dormers, coaching yard",
            ),
        )?;
    }

    let guild_lot = plan.lots.iter().find(|lot| {
        lot.kind == BuildingKind::TownHall
            && plan.scenes.iter().any(|scene| {
                scene.kind == SceneKind::WalledMarket && scene.rect.intersects(lot.rect)
            })
    });
    if let Some(lot) = guild_lot {
        let floor = lot_floor(plan, lot);
        save(
            &out_dir.join(format!("{stem}-guildhall-cutaway.png")),
            &render_room(
                volume,
                lot.rect,
                floor,
                floor + 8,
                "Guildhall · furnished hall in the walled market",
            ),
        )?;
    }

    let mill_lot = plan.lots.iter().find(|lot| {
        lot.kind == BuildingKind::Watermill
            && plan
                .scenes
                .iter()
                .any(|scene| scene.kind == SceneKind::MillHamlet && scene.rect.intersects(lot.rect))
    });
    if let Some(lot) = mill_lot {
        let floor = lot_floor(plan, lot);
        let mut view = lot.rect;
        if let Some(wheel) = wheel_for_mill(plan, lot.rect) {
            view = union_rect(view, wheel);
        }
        for z in lot.rect.min_z - 10..=lot.rect.max_z + 10 {
            for x in lot.rect.min_x - 10..=lot.rect.max_x + 10 {
                if matches!(plan.occ.kind(x, z), CellKind::MillRace | CellKind::Sluice) {
                    view = union_rect(view, Rect::new(x, z, x, z));
                }
            }
        }
        let (cx, cz) = lot.rect.center();
        let sea = plan.heights.sea_level;
        save(
            &out_dir.join(format!("{stem}-mill-hamlet-cutaway.png")),
            &render_iso_view(
                volume,
                view.min_x,
                view.min_z,
                view.max_x,
                view.max_z,
                0,
                1,
                |x, y, z| {
                    let cut = if matches!(lot.facing, Dir::East | Dir::West) {
                        z <= cz + 1
                    } else {
                        x <= cx + 1
                    };
                    cut && y >= sea - 6 && y <= floor + 10
                },
                Some("Mill hamlet · mill house, wheel and race"),
                true,
                0,
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

    if let Some(scene) = plan
        .scenes
        .iter()
        .find(|scene| scene.kind == SceneKind::Cathedral)
    {
        let mut nave: Option<Rect> = None;
        for z in scene.rect.min_z..=scene.rect.max_z {
            for x in scene.rect.min_x..=scene.rect.max_x {
                if plan.occ.kind(x, z) != CellKind::CathedralNave {
                    continue;
                }
                nave = Some(nave.map_or_else(
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
        if let Some(nave) = nave {
            let floor = ridge_floor(plan, nave);
            save(
                &out_dir.join(format!("{stem}-cathedral-cutaway.png")),
                &render_room(
                    volume,
                    nave,
                    floor,
                    floor + 10,
                    "Cathedral · nave cutaway, pews, altar, chandeliers",
                ),
            )?;
            let cz = nave.center().1;
            save(
                &out_dir.join(format!("{stem}-scene-rose.png")),
                &render_exterior(
                    volume,
                    Rect::new(nave.min_x - 2, cz - 8, nave.min_x + 4, cz + 8),
                    3,
                    "Cathedral · rose window",
                ),
            )?;
            save(
                &out_dir.join(format!("{stem}-scene-rose-west.png")),
                &render_elevation_west(
                    volume,
                    nave.min_x,
                    nave.min_z - 6,
                    nave.max_z + 6,
                    floor,
                    floor + 36,
                    floor + 21,
                    "West front",
                ),
            )?;
        }
    }
    Ok(())
}

fn lot_floor(plan: &TownPlan, lot: &crate::buildings::Lot) -> i32 {
    if lot.kind == BuildingKind::Watermill {
        let (_, hi) = plan.heights.range(
            lot.rect.min_x,
            lot.rect.min_z,
            lot.rect.max_x,
            lot.rect.max_z,
        );
        return hi.max(plan.heights.sea_level + 2);
    }
    plan.heights
        .mean(
            lot.rect.min_x,
            lot.rect.min_z,
            lot.rect.max_x,
            lot.rect.max_z,
        )
        .max(plan.heights.sea_level + 2)
}

fn mill_hamlet_focus_rect(plan: &TownPlan, fallback: Rect) -> Rect {
    let Some(mill) = plan
        .lots
        .iter()
        .find(|lot| lot.kind == BuildingKind::Watermill && fallback.intersects(lot.rect))
    else {
        return fallback;
    };
    let mut rect = mill.rect.inflate(6);
    if let Some(wheel) = wheel_for_mill(plan, mill.rect) {
        rect = union_rect(rect, wheel.inflate(2));
    }
    for lot in &plan.lots {
        if matches!(lot.kind, BuildingKind::Barn | BuildingKind::Cottage)
            && fallback.intersects(lot.rect)
        {
            rect = union_rect(rect, lot.rect.inflate(2));
        }
    }
    rect
}

fn tourney_focus_rect(plan: &TownPlan, fallback: Rect) -> Rect {
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
    for z in fallback.min_z..=fallback.max_z {
        for x in fallback.min_x..=fallback.max_x {
            if matches!(
                plan.occ.kind(x, z),
                CellKind::TourneyList | CellKind::TourneyStand | CellKind::Tent
            ) {
                grow(&mut rect, x, z);
            }
        }
    }
    rect.unwrap_or(fallback).inflate(3)
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
        0,
    )
}

fn cathedral_focus_rect(plan: &TownPlan, fallback: Rect) -> Rect {
    let Some(nave) = largest_blob(plan, CellKind::CathedralNave) else {
        return fallback;
    };
    let mut rect = nave.inflate(8);
    for kind in [
        CellKind::CathedralTower,
        CellKind::CathedralChoir,
        CellKind::CathedralTransept,
        CellKind::CathedralButtress,
    ] {
        if let Some(blob) = largest_blob(plan, kind) {
            rect = union_rect(rect, blob.inflate(4));
        }
    }
    rect
}

fn stall_touches(plan: &TownPlan, lot: &Lot, kinds: &[CellKind]) -> bool {
    (lot.rect.min_z.saturating_sub(6)..=lot.rect.max_z + 6).any(|z| {
        (lot.rect.min_x.saturating_sub(6)..=lot.rect.max_x + 6)
            .any(|x| kinds.contains(&plan.occ.kind(x, z)))
    })
}

fn fish_market_lot(plan: &TownPlan) -> Option<&Lot> {
    let fleet = [
        CellKind::Ship,
        CellKind::Boat,
        CellKind::Dock,
        CellKind::Crane,
    ];
    let quay = [CellKind::Quay, CellKind::Harbor];
    plan.lots
        .iter()
        .find(|lot| lot.kind == BuildingKind::Stall && stall_touches(plan, lot, &fleet))
        .or_else(|| {
            plan.lots
                .iter()
                .find(|lot| lot.kind == BuildingKind::Stall && stall_touches(plan, lot, &quay))
        })
}

fn harbour_scene_rect(plan: &TownPlan, scene: Rect) -> Rect {
    let hull = largest_blob(plan, CellKind::Ship);
    let crane = largest_blob(plan, CellKind::Crane);
    let dock = largest_blob(plan, CellKind::Dock);
    let shore = hull.or(crane).or(dock).unwrap_or(scene);
    let mut rect = shore.inflate(14);
    if let Some(h) = hull {
        rect = union_rect(rect, h.inflate(8));
    }
    let (sx, sz) = shore.center();
    if rect.width() > 72 {
        rect = Rect::new(sx - 36, rect.min_z, sx + 35, rect.max_z);
    }
    if rect.depth() > 72 {
        rect = Rect::new(rect.min_x, sz - 36, rect.max_x, sz + 35);
    }
    rect
}

fn render_wheel_elevation(volume: &VolumeSink, wheel: Rect, title: &str) -> RgbImage {
    let along_x = wheel.width() >= wheel.depth();
    let (along0, along1) = if along_x {
        (wheel.min_x, wheel.max_x)
    } else {
        (wheel.min_z, wheel.max_z)
    };
    let (cross0, cross1) = if along_x {
        (wheel.min_z, wheel.max_z)
    } else {
        (wheel.min_x, wheel.max_x)
    };
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for along in along0..=along1 {
        for cross in cross0..=cross1 {
            let (x, z) = if along_x {
                (along, cross)
            } else {
                (cross, along)
            };
            if let Some((top, _)) = volume.top(x, z) {
                max_y = max_y.max(top);
                min_y = min_y.min(top - 12);
            }
        }
    }
    if min_y > max_y {
        min_y = 0;
        max_y = 16;
    }
    let scale = 14u32;
    let span = (along1 - along0 + 3).max(1) as u32;
    let span_y = (max_y - min_y + 1).max(1) as u32;
    let label_h = 28u32;
    let width = (span * scale + 8).max(220);
    let height = span_y * scale + 8 + label_h;
    let mut image = ImageBuffer::from_pixel(width, height, Rgb([168u8, 198, 230]));
    for along in along0..=along1 {
        for y in min_y..=max_y {
            let mut chosen = None;
            for cross in cross0..=cross1 {
                let (x, z) = if along_x {
                    (along, cross)
                } else {
                    (cross, along)
                };
                let Some(state) = volume.get(x, y, z).filter(|s| !s.is_air()) else {
                    continue;
                };
                let id = Block::from_state_id(state.id).id;
                let wooden = id == Block::STRIPPED_OAK_LOG.id
                    || id == Block::OAK_LOG.id
                    || id == Block::OAK_PLANKS.id
                    || id == Block::OAK_FENCE.id;
                if wooden {
                    chosen = Some(state);
                    break;
                }
                if chosen.is_none() {
                    chosen = Some(state);
                }
            }
            let Some(state) = chosen else {
                continue;
            };
            let rgb = block_rgb(state);
            let px = ((along - along0 + 1) as u32) * scale + 4;
            let py = ((max_y - y) as u32) * scale + 4 + label_h;
            for sy in 0..scale {
                for sx in 0..scale {
                    image.put_pixel(px + sx, py + sy, Rgb(rgb));
                }
            }
        }
    }
    blit_label(&mut image, title);
    image
}

fn render_elevation_west(
    volume: &VolumeSink,
    wall_x: i32,
    min_z: i32,
    max_z: i32,
    min_y: i32,
    max_y: i32,
    rose_top: i32,
    title: &str,
) -> RgbImage {
    let scale = 14u32;
    let span_z = (max_z - min_z + 1).max(1) as u32;
    let span_y = (max_y - min_y + 1).max(1) as u32;
    let label_h = 28u32;
    let width = span_z * scale + 8;
    let height = span_y * scale + 8 + label_h;
    let mut image = ImageBuffer::from_pixel(width, height, Rgb([168u8, 198, 230]));
    for z in min_z..=max_z {
        for y in min_y..=max_y {
            let Some(state) = facade_state(volume, wall_x, y, z) else {
                continue;
            };
            let id = Block::from_state_id(state.id).id;
            if y <= rose_top
                && (id == Block::DEEPSLATE_TILES.id
                    || id == Block::DEEPSLATE_TILE_STAIRS.id
                    || id == Block::DEEPSLATE_TILE_SLAB.id)
            {
                continue;
            }
            let rgb = block_rgb(state);
            let px = ((z - min_z) as u32) * scale + 4;
            let py = ((max_y - y) as u32) * scale + 4 + label_h;
            for sy in 0..scale {
                for sx in 0..scale {
                    image.put_pixel(px + sx, py + sy, Rgb(rgb));
                }
            }
        }
    }
    blit_label(&mut image, title);
    image
}

fn facade_state(volume: &VolumeSink, wall_x: i32, y: i32, z: i32) -> Option<&BlockState> {
    for dx in -1..=4 {
        if let Some(state) = volume.get(wall_x + dx, y, z).filter(|s| !s.is_air()) {
            return Some(state);
        }
    }
    None
}

fn render_exterior(
    volume: &VolumeSink,
    rect: crate::geom::Rect,
    pad: i32,
    title: &str,
) -> RgbImage {
    render_exterior_rot(volume, rect, pad, title, 0)
}

fn render_exterior_rot(
    volume: &VolumeSink,
    rect: crate::geom::Rect,
    pad: i32,
    title: &str,
    rot: u8,
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
        rot,
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
    if id == Block::DEEPSLATE_TILES.id
        || id == Block::DEEPSLATE_TILE_STAIRS.id
        || id == Block::DEEPSLATE_TILE_SLAB.id
        || id == Block::DEEPSLATE_TILE_WALL.id
    {
        return [48, 50, 54];
    }
    if id == Block::SMOOTH_STONE.id || id == Block::SMOOTH_STONE_SLAB.id {
        return [176, 176, 172];
    }
    if id == Block::PURPLE_STAINED_GLASS_PANE.id || id == Block::PURPLE_STAINED_GLASS.id {
        return [118, 46, 156];
    }
    if id == Block::BLUE_STAINED_GLASS_PANE.id || id == Block::BLUE_STAINED_GLASS.id {
        return [46, 78, 178];
    }
    if id == Block::RED_STAINED_GLASS.id {
        return [170, 40, 40];
    }
    if id == Block::YELLOW_STAINED_GLASS_PANE.id || id == Block::YELLOW_STAINED_GLASS.id {
        return [216, 178, 56];
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
    if id == Block::BIRCH_PLANKS.id {
        return [216, 204, 160];
    }
    if id == Block::WHITE_TERRACOTTA.id
        || id == Block::CALCITE.id
        || id == Block::LIGHT_GRAY_TERRACOTTA.id
    {
        return [220, 214, 200];
    }
    if id == Block::DARK_OAK_STAIRS.id
        || id == Block::DARK_OAK_SLAB.id
        || id == Block::DARK_OAK_LOG.id
        || id == Block::STRIPPED_DARK_OAK_LOG.id
        || id == Block::DARK_OAK_PLANKS.id
        || id == Block::DARK_OAK_FENCE.id
        || id == Block::DARK_OAK_FENCE_GATE.id
    {
        return [78, 52, 24];
    }
    if id == Block::DARK_OAK_LEAVES.id {
        return [36, 90, 28];
    }
    if id == Block::BROWN_WOOL.id {
        return [114, 71, 40];
    }
    if id == Block::PURPLE_WOOL.id {
        return [118, 46, 156];
    }
    if id == Block::IRON_CHAIN.id {
        return [70, 70, 78];
    }
    if id == Block::CAMPFIRE.id {
        return [220, 110, 32];
    }
    if id == Block::ANVIL.id {
        return [72, 72, 76];
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
    if id == Block::YELLOW_BANNER.id || id == Block::YELLOW_WOOL.id {
        return [230, 200, 40];
    }
    if id == Block::BLACK_BANNER.id || id == Block::BLACK_WOOL.id {
        return [28, 28, 32];
    }
    if id == Block::GRAY_WOOL.id {
        return [90, 90, 94];
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
    if id == Block::GLOWSTONE.id {
        return [248, 210, 90];
    }
    if id == Block::SEA_LANTERN.id {
        return [172, 220, 214];
    }
    if id == Block::GLASS.id || id == Block::GLASS_PANE.id {
        return [190, 220, 230];
    }
    if id == Block::SWEET_BERRY_BUSH.id {
        return [62, 118, 40];
    }
    if id == Block::OAK_LEAVES.id || id == Block::SPRUCE_LEAVES.id || id == Block::JUNGLE_LEAVES.id
    {
        return [48, 116, 40];
    }
    if id == Block::JUNGLE_LOG.id {
        return [86, 62, 28];
    }
    if id == Block::WHITE_BANNER.id {
        return [236, 236, 230];
    }
    if id == Block::SNOW.id || id == Block::SNOW_BLOCK.id {
        return [240, 244, 248];
    }
    if id == Block::RAIL.id {
        return [90, 90, 90];
    }
    if id == Block::GRINDSTONE.id {
        return [130, 130, 128];
    }
    if id == Block::WATER.id {
        return [45, 82, 162];
    }
    if id == Block::GOLD_BLOCK.id {
        return [252, 210, 42];
    }
    if id == Block::YELLOW_TERRACOTTA.id {
        return [230, 168, 42];
    }
    if id == Block::ORANGE_TERRACOTTA.id {
        return [220, 118, 36];
    }
    if id == Block::ORANGE_STAINED_GLASS_PANE.id {
        return [220, 130, 40];
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
        0,
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
        volume, min_x, min_z, max_x, max_z, tile, pitch, keep, None, false, 0,
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
    rot: u8,
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
    let (span_lx, span_lz) = if rot % 2 == 1 {
        (span_z, span_x)
    } else {
        (span_x, span_z)
    };
    let tile = if tile <= 0 {
        let span = (span_lx + span_lz).max(1);
        ((1600 - 8) / span).clamp(12, 28)
    } else {
        tile.max(2)
    };
    let tw = tile;
    let th = (tile / (1 + pitch.max(1))).max(1);
    let label_h = if title.is_some() { 28 } else { 0 };
    let width = ((span_lx + span_lz) * tw + 8) as u32;
    let height = ((span_lx + span_lz) * th + span_y * tile + 8 + label_h) as u32;
    let origin_x = span_lz * tw + 4;
    let origin_y = span_y * tile + 4 + label_h;
    let mut image = ImageBuffer::from_pixel(width.max(1), height.max(1), Rgb([168u8, 198, 230]));
    let light = if fill_light { 1.18 } else { 1.0 };
    let mut paint = |x: i32, z: i32| {
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
            let (lx, lz) = iso_local(rot, x, z, min_x, min_z, max_x, max_z);
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
    };
    match rot % 4 {
        0 => {
            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    paint(x, z);
                }
            }
        }
        1 => {
            for z in min_z..=max_z {
                for x in (min_x..=max_x).rev() {
                    paint(x, z);
                }
            }
        }
        2 => {
            for z in (min_z..=max_z).rev() {
                for x in (min_x..=max_x).rev() {
                    paint(x, z);
                }
            }
        }
        _ => {
            for z in (min_z..=max_z).rev() {
                for x in min_x..=max_x {
                    paint(x, z);
                }
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
