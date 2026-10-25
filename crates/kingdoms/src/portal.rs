//! Minimal [`WorldPortalExt`] used by tests and offline chunk pregen.

use pumpkin_data::{Block, BlockState, BlockStateId, Mirror, Rotation};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::generation::proto_chunk::GenerationCache;
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

/// Accepts every placement. Enough for feature generation and decoration.
pub struct PregenPortal;

impl WorldPortalExt for PregenPortal {
    fn can_place_at(
        &self,
        _block: &Block,
        _state: &BlockState,
        _block_accessor: &dyn BlockAccessor,
        _block_pos: &BlockPos,
    ) -> bool {
        true
    }

    fn mirror(&self, block: &Block, state_id: BlockStateId, mirror: Mirror) -> &'static BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &Block,
        state_id: BlockStateId,
        rotation: Rotation,
    ) -> &'static BlockState {
        block.rotate(state_id, rotation)
    }

    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn GenerationCache,
        _biome: &'static pumpkin_data::chunk::Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }
}
