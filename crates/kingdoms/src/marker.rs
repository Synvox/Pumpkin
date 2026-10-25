//! World-gen hooks: stamp the planned town and veto nearby villages.

use pumpkin_data::structures::StructureKeys;
use pumpkin_world::generation::hooks::{
    ChunkDecorationContext, ChunkDecorator, StructureStartFilter, StructureStartFilterContext,
};
use std::sync::Arc;

use crate::plan::plan_from_cache;
use crate::stamp::stamp_plan_in_chunk;
use crate::town::{is_inside_village_exclusion, towns_touching_chunk};

/// Distinctive well-floor block used by tests.
pub use crate::stamp::MARKER_FLOOR;

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

/// Decorator that stamps the planned medieval town.
pub struct KingdomsDecorator;

impl ChunkDecorator for KingdomsDecorator {
    fn decorate(&self, ctx: &mut ChunkDecorationContext<'_>) {
        let towns = towns_touching_chunk(ctx.seed(), ctx.chunk_x(), ctx.chunk_z());
        for town in towns {
            if let Some(plan) = plan_from_cache(ctx.seed(), town.cell_x, town.cell_z) {
                stamp_plan_in_chunk(ctx, &plan);
            }
        }
    }
}

/// Structure filter that skips vanilla villages near a town.
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
