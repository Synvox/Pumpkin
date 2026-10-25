//! Cobblestone-and-oak well stamped on a local foundation pad.

use pumpkin_data::{Block, BlockState, structures::StructureKeys};
use pumpkin_world::generation::hooks::{
    ChunkDecorationContext, ChunkDecorator, StructureStartFilter, StructureStartFilterContext,
};
use std::sync::Arc;

use crate::town::{is_inside_village_exclusion, towns_touching_chunk};

/// Half-extent of the well footprint (5×5).
pub const WELL_RADIUS: i32 = 2;
const FOUNDATION_DEPTH: i32 = 1;

/// Distinctive well-floor block used by tests and `/locatetown` checks.
pub const MARKER_FLOOR: &Block = &Block::MOSSY_COBBLESTONE;

/// Vanilla village starts the plugin vetoes inside a town plot.
const fn is_village(structure: StructureKeys) -> bool {
    matches!(
        structure,
        StructureKeys::VillagePlains
            | StructureKeys::VillageDesert
            | StructureKeys::VillageSavanna
            | StructureKeys::VillageSnowy
            | StructureKeys::VillageTaiga
    )
}

/// Decorator that stamps the milestone-0 well.
pub struct KingdomsDecorator;

impl ChunkDecorator for KingdomsDecorator {
    fn decorate(&self, ctx: &mut ChunkDecorationContext<'_>) {
        let towns = towns_touching_chunk(ctx.seed(), ctx.chunk_x(), ctx.chunk_z());
        for town in towns {
            stamp_well_in_chunk(ctx, town.x, town.z);
        }
    }
}

/// Structure filter that skips vanilla villages near a town marker.
pub struct KingdomsStructureFilter;

impl StructureStartFilter for KingdomsStructureFilter {
    fn allow_structure_start(&self, ctx: &StructureStartFilterContext) -> bool {
        if !is_village(ctx.structure) {
            return true;
        }
        let center_x = ctx.chunk_x * 16 + 8;
        let center_z = ctx.chunk_z * 16 + 8;
        !is_inside_village_exclusion(ctx.seed, center_x, center_z)
    }
}

/// Registers both world-gen hooks on `extensions`.
pub fn register_hooks(extensions: &pumpkin_world::generation::hooks::WorldGenExtensions) {
    extensions.register_decorator(Arc::new(KingdomsDecorator));
    extensions.register_structure_filter(Arc::new(KingdomsStructureFilter));
}

/// Stamps the well columns that belong to the context's center chunk.
pub fn stamp_well_in_chunk(ctx: &mut ChunkDecorationContext<'_>, town_x: i32, town_z: i32) {
    let chunk_x = ctx.chunk_x();
    let chunk_z = ctx.chunk_z();
    let cobble = Block::COBBLESTONE.default_state;
    let mossy = MARKER_FLOOR.default_state;
    let oak = Block::OAK_LOG.default_state;
    let water = Block::WATER.default_state;

    for dx in -WELL_RADIUS..=WELL_RADIUS {
        for dz in -WELL_RADIUS..=WELL_RADIUS {
            let x = town_x + dx;
            let z = town_z + dz;
            if x >> 4 != chunk_x || z >> 4 != chunk_z {
                continue;
            }

            let surface = ctx.surface_height_exclusive(x, z);
            let ground = surface - 1;
            if ground <= ctx.min_y() {
                continue;
            }

            for y in (ground - FOUNDATION_DEPTH)..=ground {
                ctx.set_block_state(x, y, z, cobble);
            }

            let is_corner = dx.abs() == WELL_RADIUS && dz.abs() == WELL_RADIUS;
            let is_center = dx == 0 && dz == 0;
            let is_inner = dx.abs() <= 1 && dz.abs() <= 1;

            if is_center {
                ctx.set_block_state(x, ground, z, mossy);
                ctx.set_block_state(x, ground + 1, z, water);
            } else if is_inner {
                ctx.set_block_state(x, ground + 1, z, cobble);
                ctx.set_block_state(x, ground + 2, z, cobble);
            }

            if is_corner {
                ctx.set_block_state(x, ground + 1, z, oak);
                ctx.set_block_state(x, ground + 2, z, oak);
                ctx.set_block_state(x, ground + 3, z, oak);
            }
        }
    }
}

/// Whether `state` is the well's mossy floor.
#[must_use]
pub fn is_marker_floor(state: &BlockState) -> bool {
    state.id == MARKER_FLOOR.default_state.id
}
