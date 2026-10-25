//! Medieval kingdoms world-gen plugin (milestone 0).
//!
//! Feature code lives here so upstream Pumpkin files stay limited to the
//! small generation hooks in `pumpkin-world` / `pumpkin-core`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::sync::Arc;

use pumpkin_core::plugin::{
    EventHandler, EventPriority, Payload, PluginFuture, TypedEventHandler, world::WorldInitEvent,
};
use pumpkin_core::server::Server;
use pumpkin_core::world::World;
use pumpkin_data::dimension::Dimension;
use tracing::info;

pub mod command;
pub mod marker;
pub mod town;

use marker::register_hooks;

/// Installs the plugin on a running server: command, hooks, and future worlds.
pub fn install(server: &Server) {
    server.command_dispatcher.rcu(|dispatcher| {
        let mut next = (**dispatcher).clone();
        command::register(&mut next, &server.permission_manager);
        Arc::new(next)
    });

    for world in server.worlds.load().iter() {
        attach_world(world);
    }

    register_world_init_handler(server);
    info!("Kingdoms plugin installed (town cells + /locatetown)");
}

/// Registers the decorator and village filter on an overworld.
pub fn attach_world(world: &World) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    register_hooks(world.level.world_gen().extensions());
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
        attach_world(&event.world);
        Box::pin(async {})
    }

    fn handle_blocking<'a>(
        &'a self,
        _server: &'a Arc<Server>,
        event: &'a mut WorldInitEvent,
    ) -> PluginFuture<'a, ()> {
        attach_world(&event.world);
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::Block;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::math::vector3::Vector3;
    use pumpkin_util::world_seed::Seed;
    use pumpkin_world::chunk_system::generation::generate_single_chunk_with_radius;
    use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum, generate_single_chunk};
    use pumpkin_world::generation::get_world_gen;
    use pumpkin_world::generation::hooks::ChunkDecorationContext;
    use pumpkin_world::generation::proto_chunk::ProtoChunk;
    use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

    use crate::marker::{MARKER_FLOOR, stamp_well_in_chunk};
    use crate::town::town_in_cell;

    struct TestPortal;

    impl WorldPortalExt for TestPortal {
        fn can_place_at(
            &self,
            _block: &pumpkin_data::Block,
            _state: &pumpkin_data::BlockState,
            _block_accessor: &dyn BlockAccessor,
            _block_pos: &pumpkin_util::math::position::BlockPos,
        ) -> bool {
            true
        }

        fn mirror(
            &self,
            block: &pumpkin_data::Block,
            state_id: pumpkin_data::BlockStateId,
            mirror: pumpkin_data::Mirror,
        ) -> &'static pumpkin_data::BlockState {
            block.mirror(state_id, mirror)
        }

        fn rotate(
            &self,
            block: &pumpkin_data::Block,
            state_id: pumpkin_data::BlockStateId,
            rotation: pumpkin_data::Rotation,
        ) -> &'static pumpkin_data::BlockState {
            block.rotate(state_id, rotation)
        }

        fn spawn_mobs_for_chunk_generation(
            &self,
            _cache: &mut dyn pumpkin_world::generation::proto_chunk::GenerationCache,
            _biome: &'static pumpkin_data::chunk::Biome,
            _chunk_x: i32,
            _chunk_z: i32,
        ) {
        }
    }

    fn stamp_on_chunk(chunk: &mut ProtoChunk, seed: u64, town_x: i32, town_z: i32) {
        let mut ctx = ChunkDecorationContext::new(chunk, seed);
        stamp_well_in_chunk(&mut ctx, town_x, town_z);
    }

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
        let world_gen = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        let town = town_in_cell(seed.0, 0, 0);
        // Force a border-spanning well by stamping at a chunk seam, then also
        // stamp the planned town through the public decorator path.
        let border_x = 16;
        let border_z = 0;
        let origin = (border_x >> 4, border_z >> 4);
        let west = (origin.0 - 1, origin.1);

        let orders = [[origin, west], [west, origin]];
        let mut floors = Vec::new();

        for order in orders {
            let mut chunks = std::collections::HashMap::new();
            for &(cx, cz) in &order {
                let generated = generate_single_chunk_with_radius(
                    &world_gen,
                    &TestPortal,
                    cx,
                    cz,
                    StagedChunkEnum::Carvers,
                    1,
                );
                chunks.insert((cx, cz), extract_proto(generated));
            }
            for &(cx, cz) in &order {
                let mut center = chunks.remove(&(cx, cz)).unwrap();
                stamp_on_chunk(&mut center, seed.0, border_x, border_z);
                chunks.insert((cx, cz), center);
            }
            let mut found = Vec::new();
            for dx in -2..=2 {
                for dz in -2..=2 {
                    let x = border_x + dx;
                    let z = border_z + dz;
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
            floors[0]
                .iter()
                .any(|(x, _, z, _)| *x == border_x && *z == border_z),
            "mossy well floor missing at the seam"
        );
        let _ = town;
    }

    #[test]
    fn decorator_hook_places_town_well() {
        let seed = Seed(11);
        let world_gen = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        register_hooks(world_gen.extensions());
        let town = town_in_cell(seed.0, 0, 0);
        let cx = town.x >> 4;
        let cz = town.z >> 4;
        let generated =
            generate_single_chunk(&world_gen, &TestPortal, cx, cz, StagedChunkEnum::Features);
        let chunk = extract_proto(generated);
        assert!(
            marker_at(&chunk, town.x, town.z).is_some(),
            "decorator did not place the well floor at {} {}",
            town.x,
            town.z
        );
        let oak = Block::OAK_LOG.default_state.id;
        let corner_x = town.x - 2;
        let corner_z = town.z - 2;
        if corner_x >> 4 == cx && corner_z >> 4 == cz {
            let min_y = i32::from(chunk.bottom_y());
            let height = i32::from(chunk.height());
            let has_oak = (min_y..min_y + height)
                .any(|y| chunk.get_block_state(&Vector3::new(corner_x, y, corner_z)) == oak);
            assert!(has_oak, "oak post missing at well corner");
        }
    }
}
