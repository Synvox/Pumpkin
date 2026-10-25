//! Copies real Pumpkin-generated chunks (features + kingdoms decorator) into a volume.

use pumpkin_data::BlockState;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk_system::generation::generate_single_chunk_with_radius;
use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum};
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::proto_chunk::ProtoChunk;
use rayon::prelude::*;

use crate::plan::TownPlan;
use crate::portal::PregenPortal;
use crate::stamp::{BlockSink, VolumeSink};

/// Generates every chunk covering `plan` through Features (which runs the
/// kingdoms decorator) and copies the result into a render volume.
#[must_use]
pub fn fill_volume_from_chunks(plan: &TownPlan, world_gen: &WorldGenerator) -> VolumeSink {
    let pad = 16;
    let min_x = plan.bounds.min_x - pad;
    let max_x = plan.bounds.max_x + pad;
    let min_z = plan.bounds.min_z - pad;
    let max_z = plan.bounds.max_z + pad;
    let min_cx = min_x.div_euclid(16);
    let max_cx = max_x.div_euclid(16);
    let min_cz = min_z.div_euclid(16);
    let max_cz = max_z.div_euclid(16);
    let (lo, hi) = plan.heights.range(
        plan.bounds.min_x,
        plan.bounds.min_z,
        plan.bounds.max_x,
        plan.bounds.max_z,
    );
    let min_y = lo - 24;
    let max_y = hi + 56;
    let mut volume = VolumeSink::for_bounds(min_x, min_z, max_x, max_z, min_y, max_y);

    let coords: Vec<(i32, i32)> = (min_cz..=max_cz)
        .flat_map(|cz| (min_cx..=max_cx).map(move |cx| (cx, cz)))
        .collect();

    let columns: Vec<((i32, i32), ProtoChunk)> = coords
        .par_iter()
        .filter_map(|&(cx, cz)| {
            let generated = generate_single_chunk_with_radius(
                world_gen,
                &PregenPortal,
                cx,
                cz,
                StagedChunkEnum::Features,
                1,
            );
            match generated {
                Chunk::Proto(proto) => Some(((cx, cz), *proto)),
                Chunk::Level(_) => None,
            }
        })
        .collect();

    for ((cx, cz), chunk) in columns {
        copy_chunk(&mut volume, &chunk, cx, cz);
    }
    volume
}

fn copy_chunk(volume: &mut VolumeSink, chunk: &ProtoChunk, cx: i32, cz: i32) {
    let min_x = cx * 16;
    let min_z = cz * 16;
    let min_y = volume.min_y.max(i32::from(chunk.bottom_y()));
    let max_y = (volume.min_y + volume.sy - 1)
        .min(i32::from(chunk.bottom_y()) + i32::from(chunk.height()) - 1);
    for z in min_z..min_z + 16 {
        for x in min_x..min_x + 16 {
            for y in min_y..=max_y {
                let id = chunk.get_block_state(&Vector3::new(x, y, z));
                let state = BlockState::from_id(id);
                if !state.is_air() {
                    volume.put(x, y, z, state);
                }
            }
        }
    }
}
