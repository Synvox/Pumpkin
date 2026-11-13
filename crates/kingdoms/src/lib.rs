//! Medieval kingdoms world-gen plugin.
//!
//! Feature code lives here so upstream Pumpkin files stay limited to the
//! small generation hooks in `pumpkin-world` / `pumpkin-core`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::manual_is_multiple_of,
    clippy::manual_midpoint,
    clippy::many_single_char_names,
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::trivially_copy_pass_by_ref,
    clippy::tuple_array_conversions,
    clippy::type_complexity
)]

use std::sync::Arc;

use pumpkin_core::plugin::{
    EventHandler, EventPriority, Payload, PluginFuture, TypedEventHandler, world::WorldInitEvent,
};
use pumpkin_core::server::Server;
use pumpkin_core::world::World;
use pumpkin_data::dimension::Dimension;
use tracing::info;

pub mod abstract_combat;
pub mod bounty;
pub mod buildings;
pub mod castle;
pub mod cells;
pub mod chat;
pub mod chunk_fill;
pub mod command;
pub mod damage;
pub mod director;
pub mod engines;
pub mod events;
pub mod faction;
pub mod flow;
pub mod geom;
pub mod goals;
pub mod height;
pub mod http;
pub mod info;
pub mod interior;
pub mod lod;
pub mod marker;
pub mod morale;
pub mod palette;
pub mod plan;
pub mod populate;
pub mod portal;
pub mod raid;
pub mod ram;
pub mod render;
pub mod repair;
pub mod rng;
pub mod roles;
pub mod scene;
pub mod scenes;
pub mod schedule;
pub mod siege;
pub mod sim;
pub mod stamp;
pub mod town;
pub mod watertight;
pub mod wfc;

pub use chat::ShoutKind;
pub use damage::{DamageState, damage_state, note_capture, set_damage_state, step_repair};
pub use director::{
    BattleState, RaidId, battle_state, join_kingdom, query_battle_state, set_target_town,
    start_raid, stop_raid,
};
pub use faction::{Faction, KingdomId, is_allied, is_hostile, kingdom_color_rgb};
pub use info::{
    SpawnPoint, SpawnRole, TownInfo, town_id, town_info, town_info_at, town_info_from_id,
};
pub use plan::{plan_at, plan_from_cache};
pub use raid::{RaidSide, RaidSpec};
pub use roles::VillagerRole;
pub use scene::{TownInfoScene, TownScene};
pub use scenes::{Scene, SceneKind};
pub use siege::TownOwner;
pub use stamp::{
    PlannedBlock, planned_block, planned_block_at, planned_blocks_in, planned_blocks_in_at,
    planned_building_blocks, planned_building_blocks_for, planned_column, planned_column_at,
};
pub use watertight::{BuildingCheck, TownCheck, check_floating_fragments, check_town};

use height::register_world_gen;
use marker::register_hooks;

/// Installs the plugin on a running server: command, hooks, and future worlds.
pub fn install(server: &Server) {
    pumpkin_core::server::KINGDOMS_TICK_WORLDS_INLINE
        .store(true, std::sync::atomic::Ordering::Relaxed);
    server.command_dispatcher.rcu(|dispatcher| {
        let mut next = (**dispatcher).clone();
        command::register(&mut next, &server.permission_manager);
        Arc::new(next)
    });

    for world in server.worlds.load().iter() {
        attach_world_arc(world);
    }

    register_world_init_handler(server);
    events::register(server);
    crate::http::spawn_if_enabled();
    info!("Kingdoms plugin installed (towns, villagers, raids)");
}

/// Registers the decorator, village filter, and height sampler on an overworld.
pub fn attach_world(world: &World) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    let world_gen = world.level.world_gen();
    register_world_gen(Arc::clone(&world_gen));
    register_hooks(world_gen.extensions());
}

/// Same as [`attach_world`], and keeps an `Arc` so populate/raid can spawn entities.
pub fn attach_world_arc(world: &Arc<World>) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    attach_world(world);
    director::lock().attach_world(world);
    let seed = world.level.seed.0;
    let root = world.level.level_folder.root_folder.clone();
    let info = world.level_info.load();
    let extra = director::owned_cells();
    plan::bind_and_warmup(seed, root, info.spawn_x, info.spawn_z, &extra);
}

fn register_world_init_handler(server: &Server) {
    let handler = Arc::new(TypedEventHandler {
        handler: Arc::new(AttachOnWorldInit),
        priority: EventPriority::Normal,
        blocking: true,
        source: Some("kingdoms".to_string()),
        _phantom: std::marker::PhantomData,
    });
    server.plugin_manager.handlers.rcu(|handlers| {
        let mut next = (**handlers).clone();
        next.entry(WorldInitEvent::get_name_static())
            .or_default()
            .push(handler.clone());
        Arc::new(next)
    });
}

struct AttachOnWorldInit;

impl EventHandler<WorldInitEvent> for AttachOnWorldInit {
    fn handle<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a WorldInitEvent,
    ) -> PluginFuture<'a, ()> {
        attach_world_arc(&event.world);
        Box::pin(async {})
    }

    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut WorldInitEvent,
    ) -> PluginFuture<'a, ()> {
        attach_world_arc(&event.world);
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::register_world_gen;
    use crate::plan::{build_plan, plan_for};
    use crate::portal::PregenPortal;
    use crate::stamp::{
        MARKER_FLOOR, is_vanilla_tree_part, keeps_planned_timber, stamp_plan_in_chunk,
    };
    use crate::town::town_in_cell;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_util::world_seed::Seed;
    use pumpkin_world::chunk_system::generation::generate_single_chunk_with_radius;
    use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum};
    use pumpkin_world::generation::get_world_gen;
    use pumpkin_world::generation::hooks::ChunkDecorationContext;
    use pumpkin_world::generation::proto_chunk::ProtoChunk;

    fn extract_proto(chunk: Chunk) -> ProtoChunk {
        match chunk {
            Chunk::Proto(proto) => *proto,
            Chunk::Level(_) => panic!("expected proto chunk"),
        }
    }

    fn marker_at(chunk: &ProtoChunk, x: i32, z: i32) -> Option<(i32, pumpkin_data::BlockStateId)> {
        let min_y = i32::from(chunk.bottom_y());
        let height = i32::from(chunk.height());
        for y in min_y..min_y + height {
            let id = chunk.get_block_state(&Vector3::new(x, y, z));
            if id == MARKER_FLOOR.default_state.id {
                return Some((y, id));
            }
        }
        None
    }

    #[test]
    fn well_is_identical_for_two_chunk_orders() {
        let seed = Seed(20_261_025);
        let boxed = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        let world_gen = Arc::from(boxed);
        register_world_gen(Arc::clone(&world_gen));
        register_hooks(world_gen.extensions());
        let origin = town_in_cell(seed.0, 0, 0);
        let plan = plan_for(seed.0, 0, 0, &world_gen);
        let (wx, wz) = plan.well;
        let origin_chunk = (wx >> 4, wz >> 4);
        let west = (origin_chunk.0 - 1, origin_chunk.1);
        let orders = [[origin_chunk, west], [west, origin_chunk]];
        let mut floors = Vec::new();

        for order in orders {
            let mut chunks = std::collections::HashMap::new();
            for &(cx, cz) in &order {
                let generated = generate_single_chunk_with_radius(
                    &world_gen,
                    &PregenPortal,
                    cx,
                    cz,
                    StagedChunkEnum::Carvers,
                    1,
                );
                chunks.insert((cx, cz), extract_proto(generated));
            }
            for &(cx, cz) in &order {
                let mut center = chunks.remove(&(cx, cz)).unwrap();
                let mut ctx = ChunkDecorationContext::new(&mut center, seed.0);
                stamp_plan_in_chunk(&mut ctx, &plan);
                chunks.insert((cx, cz), center);
            }
            let mut found = Vec::new();
            for dx in -2..=2 {
                for dz in -2..=2 {
                    let x = wx + dx;
                    let z = wz + dz;
                    let key = (x >> 4, z >> 4);
                    if let Some(chunk) = chunks.get(&key)
                        && let Some((y, id)) = marker_at(chunk, x, z)
                    {
                        found.push((x, y, z, id));
                    }
                }
            }
            floors.push(found);
        }

        assert_eq!(floors[0], floors[1]);
        assert!(
            floors[0].iter().any(|(x, _, z, _)| *x == wx && *z == wz),
            "mossy well floor missing at the seam"
        );
        let _ = origin;
    }

    #[test]
    fn generated_chunks_inside_ring_have_no_vanilla_trees() {
        use pumpkin_data::Block;
        let seed = Seed(20_261_025);
        let boxed = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        let world_gen = Arc::from(boxed);
        register_world_gen(Arc::clone(&world_gen));
        register_hooks(world_gen.extensions());
        let plan = plan_for(seed.0, 0, 0, &world_gen);
        let wall = plan.town.inflate(2);
        let mut sample = (wall.min_x, (wall.min_z + wall.max_z) / 2);
        'find: for z in wall.min_z - 3..=wall.max_z + 3 {
            for x in wall.min_x - 3..=wall.min_x + 6 {
                if matches!(
                    plan.occ.kind(x, z),
                    crate::cells::CellKind::Plaza
                        | crate::cells::CellKind::TownWall
                        | crate::cells::CellKind::CityDitch
                ) && plan.strips_trees(x, z)
                {
                    sample = (x, z);
                    break 'find;
                }
            }
        }
        let (sx, sz) = sample;
        let cx0 = (sx >> 4) - 1;
        let cz0 = (sz >> 4) - 1;
        let mut leaks = Vec::new();
        for cz in cz0..=cz0 + 2 {
            for cx in cx0..=cx0 + 2 {
                let generated = generate_single_chunk_with_radius(
                    &world_gen,
                    &PregenPortal,
                    cx,
                    cz,
                    StagedChunkEnum::Features,
                    1,
                );
                let proto = extract_proto(generated);
                let min_y = i32::from(proto.bottom_y());
                let height = i32::from(proto.height());
                for z in cz * 16..cz * 16 + 16 {
                    for x in cx * 16..cx * 16 + 16 {
                        if !plan.strips_trees(x, z) {
                            continue;
                        }
                        let kind = plan.occ.kind(x, z);
                        let planned = plan.heights.height(x, z).max(plan.heights.sea_level);
                        let garden = matches!(
                            kind,
                            crate::cells::CellKind::Green | crate::cells::CellKind::Orchard
                        );
                        if keeps_planned_timber(kind) && !garden {
                            continue;
                        }
                        for y in min_y..min_y + height {
                            if garden && y <= planned + 6 {
                                continue;
                            }
                            let id = proto.get_block_state(&Vector3::new(x, y, z));
                            let block = Block::from_state_id(id);
                            if is_vanilla_tree_part(block.id) {
                                leaks.push(format!("{x},{y},{z} {} kind={kind:?}", block.name));
                                if leaks.len() >= 8 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(
            leaks.is_empty(),
            "vanilla trees inside the ring: {}",
            leaks.join("; ")
        );
    }

    #[test]
    fn castle_meets_minimum_footprint() {
        let seed = Seed(11);
        let boxed = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        let world_gen = Arc::from(boxed);
        let plan = plan_for(seed.0, 0, 0, &world_gen);
        let (w, d) = plan.castle_footprint();
        assert!(
            w >= 128 && d >= 128,
            "castle {w}x{d} is smaller than 8x8 chunks"
        );
    }

    #[test]
    fn synthetic_hillside_plan_is_stable() {
        let origin = town_in_cell(99, 1, -1);
        let grid = crate::height::HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            72,
            22,
            10,
        );
        let a = build_plan(
            99,
            origin,
            grid.clone(),
            crate::palette::PaletteKind::Spruce,
        );
        let b = build_plan(99, origin, grid, crate::palette::PaletteKind::Spruce);
        assert_eq!(a.bounds, b.bounds);
        assert_eq!(a.castle.keep, b.castle.keep);
        assert_eq!(a.lots.len(), b.lots.len());
        for z in a.bounds.min_z..=a.bounds.max_z {
            for x in a.bounds.min_x..=a.bounds.max_x {
                assert_eq!(a.occ.kind(x, z), b.occ.kind(x, z));
            }
        }
    }
}
