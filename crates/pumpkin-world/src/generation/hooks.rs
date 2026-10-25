//! Optional world-generation extension points.
//!
//! These hooks are empty by default. When nothing is registered, generation
//! pays one atomic load per hook site and otherwise behaves like vanilla.
//!
//! Hosts and native plugins can register:
//! - [`ChunkDecorator`]s that run after vanilla feature/structure placement
//! - [`StructureStartFilter`]s that can veto a vanilla structure start
//!
//! Surface height at an arbitrary column, without generating a chunk, lives on
//! [`crate::generation::generator::VanillaGenerator::sample_surface_height`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use pumpkin_data::biome::Biome;
use pumpkin_data::structures::StructureKeys;
use pumpkin_data::{BlockState, BlockStateId};
use pumpkin_util::math::vector3::Vector3;

use super::proto_chunk::GenerationCache;

/// A decorator that runs once per chunk after vanilla features and structures.
///
/// The supplied [`ChunkDecorationContext`] can read and write the chunk being
/// generated plus its already-loaded 1-chunk neighborhood (the same write
/// radius vanilla features use). Implementations must be deterministic for a
/// given seed and chunk position.
pub trait ChunkDecorator: Send + Sync {
    /// Place extra blocks for the center chunk of `ctx`.
    fn decorate(&self, ctx: &mut ChunkDecorationContext<'_>);
}

/// A filter that can prevent a vanilla structure start from being stored.
///
/// Return `false` to skip the start. When several filters are registered, a
/// start is placed only if every filter allows it.
pub trait StructureStartFilter: Send + Sync {
    /// Whether `ctx.structure` may start in `ctx`'s chunk.
    fn allow_structure_start(&self, ctx: &StructureStartFilterContext) -> bool;
}

/// Read-only inputs for [`StructureStartFilter`].
#[derive(Clone, Copy, Debug)]
pub struct StructureStartFilterContext {
    /// World seed used by the noise generator.
    pub seed: u64,
    /// Chunk X of the candidate start.
    pub chunk_x: i32,
    /// Chunk Z of the candidate start.
    pub chunk_z: i32,
    /// Structure type being considered.
    pub structure: StructureKeys,
}

impl StructureStartFilterContext {
    /// Creates a filter context for `structure` at `(chunk_x, chunk_z)`.
    #[must_use]
    pub const fn new(seed: u64, chunk_x: i32, chunk_z: i32, structure: StructureKeys) -> Self {
        Self {
            seed,
            chunk_x,
            chunk_z,
            structure,
        }
    }
}

/// Read-write view of the chunk (and its 1-chunk neighborhood) passed to a decorator.
pub struct ChunkDecorationContext<'a> {
    cache: &'a mut dyn GenerationCache,
    seed: u64,
}

impl<'a> ChunkDecorationContext<'a> {
    /// Wraps an existing generation cache.
    pub fn new(cache: &'a mut dyn GenerationCache, seed: u64) -> Self {
        Self { cache, seed }
    }

    /// World seed of the generator that produced this chunk.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Chunk X being decorated (the cache center).
    #[must_use]
    pub fn chunk_x(&self) -> i32 {
        self.cache.get_center_chunk().x
    }

    /// Chunk Z being decorated (the cache center).
    #[must_use]
    pub fn chunk_z(&self) -> i32 {
        self.cache.get_center_chunk().z
    }

    /// Lowest block Y in this dimension.
    #[must_use]
    pub fn min_y(&self) -> i32 {
        i32::from(self.cache.get_center_chunk().bottom_y())
    }

    /// Column height in blocks.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.cache.get_center_chunk().height()
    }

    /// Sea level used by the generator.
    #[must_use]
    pub fn sea_level(&self) -> i32 {
        self.cache.get_sea_level()
    }

    /// Vanilla feature write radius in chunks (Chebyshev). Decorators should
    /// stay inside this neighborhood.
    #[must_use]
    pub const fn write_radius_chunks(&self) -> i32 {
        1
    }

    /// Block state at world `(x, y, z)`, or air if the column is outside the cache.
    #[must_use]
    pub fn get_block_state(&self, x: i32, y: i32, z: i32) -> BlockStateId {
        GenerationCache::get_block_state(self.cache, &Vector3::new(x, y, z))
    }

    /// Writes `block_state` at world `(x, y, z)` if that column is in the cache.
    pub fn set_block_state(&mut self, x: i32, y: i32, z: i32, block_state: &BlockState) {
        GenerationCache::set_block_state(self.cache, &Vector3::new(x, y, z), block_state);
    }

    /// Biome used for terrain generation at world `(x, y, z)`.
    #[must_use]
    pub fn get_biome(&self, x: i32, y: i32, z: i32) -> &'static Biome {
        self.cache.get_biome_for_terrain_gen(x, y, z)
    }

    /// First free block Y above the motion-blocking surface at `(x, z)`.
    #[must_use]
    pub fn surface_height_exclusive(&self, x: i32, z: i32) -> i32 {
        self.cache.top_motion_blocking_block_height_exclusive(x, z)
    }

    /// Mutable access to the underlying cache for callers that need more than
    /// the helpers above.
    pub fn cache(&mut self) -> &mut dyn GenerationCache {
        self.cache
    }
}

struct WorldGenExtensionsInner {
    decorator_count: AtomicUsize,
    filter_count: AtomicUsize,
    decorators: RwLock<Vec<Arc<dyn ChunkDecorator>>>,
    structure_filters: RwLock<Vec<Arc<dyn StructureStartFilter>>>,
}

/// Per-world registry of optional generation hooks.
///
/// Cheap to clone (`Arc`). Empty registries are the default and must stay
/// effectively free on the generation hot path.
#[derive(Clone)]
pub struct WorldGenExtensions {
    inner: Arc<WorldGenExtensionsInner>,
}

impl Default for WorldGenExtensions {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenExtensions {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(WorldGenExtensionsInner {
                decorator_count: AtomicUsize::new(0),
                filter_count: AtomicUsize::new(0),
                decorators: RwLock::new(Vec::new()),
                structure_filters: RwLock::new(Vec::new()),
            }),
        }
    }

    /// Shared empty registry for generators that do not own extensions.
    #[must_use]
    pub fn empty() -> &'static Self {
        static EMPTY: std::sync::LazyLock<WorldGenExtensions> =
            std::sync::LazyLock::new(WorldGenExtensions::new);
        &EMPTY
    }

    /// Whether any decorator is registered.
    #[must_use]
    pub fn has_decorators(&self) -> bool {
        self.inner.decorator_count.load(Ordering::Relaxed) != 0
    }

    /// Whether any structure-start filter is registered.
    #[must_use]
    pub fn has_structure_filters(&self) -> bool {
        self.inner.filter_count.load(Ordering::Relaxed) != 0
    }

    /// Adds a decorator. Order is registration order.
    pub fn register_decorator(&self, decorator: Arc<dyn ChunkDecorator>) {
        let mut decorators = self
            .inner
            .decorators
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        decorators.push(decorator);
        self.inner
            .decorator_count
            .store(decorators.len(), Ordering::Relaxed);
    }

    /// Adds a structure-start filter. Order is registration order.
    pub fn register_structure_filter(&self, filter: Arc<dyn StructureStartFilter>) {
        let mut filters = self
            .inner
            .structure_filters
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        filters.push(filter);
        self.inner
            .filter_count
            .store(filters.len(), Ordering::Relaxed);
    }

    /// Runs every decorator against `cache`. No-op when none are registered.
    pub fn decorate(&self, cache: &mut dyn GenerationCache, seed: u64) {
        if !self.has_decorators() {
            return;
        }
        let decorators = {
            let guard = self
                .inner
                .decorators
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };
        let mut ctx = ChunkDecorationContext::new(cache, seed);
        for decorator in decorators {
            decorator.decorate(&mut ctx);
        }
    }

    /// Returns `false` if any filter vetoes this start. `true` when unused.
    #[must_use]
    pub fn allow_structure_start(&self, ctx: &StructureStartFilterContext) -> bool {
        if !self.has_structure_filters() {
            return true;
        }
        let filters = {
            let guard = self
                .inner
                .structure_filters
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.clone()
        };
        filters
            .iter()
            .all(|filter| filter.allow_structure_start(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::generator::{VanillaGenerator, WorldGenerator};
    use crate::generation::{get_world_gen, proto_chunk::ProtoChunk};
    use pumpkin_data::Block;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::world_seed::Seed;

    struct RecordingDecorator {
        block: &'static BlockState,
    }

    impl ChunkDecorator for RecordingDecorator {
        fn decorate(&self, ctx: &mut ChunkDecorationContext<'_>) {
            let x = ctx.chunk_x() * 16;
            let z = ctx.chunk_z() * 16;
            let y = ctx.min_y() + 8;
            ctx.set_block_state(x, y, z, self.block);
        }
    }

    struct VetoStructure {
        structure: StructureKeys,
    }

    impl StructureStartFilter for VetoStructure {
        fn allow_structure_start(&self, ctx: &StructureStartFilterContext) -> bool {
            ctx.structure != self.structure
        }
    }

    #[test]
    fn empty_extensions_are_inert() {
        let extensions = WorldGenExtensions::new();
        assert!(!extensions.has_decorators());
        assert!(!extensions.has_structure_filters());
        let ctx = StructureStartFilterContext::new(1, 0, 0, StructureKeys::VillagePlains);
        assert!(extensions.allow_structure_start(&ctx));
    }

    #[test]
    fn decorator_writes_center_chunk_block() {
        let world_gen = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let mut chunk = ProtoChunk::new(2, -3, &world_gen);
        let extensions = WorldGenExtensions::new();
        extensions.register_decorator(Arc::new(RecordingDecorator {
            block: Block::COBBLESTONE.default_state,
        }));

        extensions.decorate(&mut chunk, 1);

        let id = GenerationCache::get_block_state(
            &chunk,
            &Vector3::new(32, chunk.bottom_y() as i32 + 8, -48),
        );
        assert_eq!(id, Block::COBBLESTONE.default_state.id);
    }

    #[test]
    fn structure_filter_vetoes_matching_key() {
        let extensions = WorldGenExtensions::new();
        extensions.register_structure_filter(Arc::new(VetoStructure {
            structure: StructureKeys::VillagePlains,
        }));
        let village = StructureStartFilterContext::new(0, 1, 2, StructureKeys::VillagePlains);
        let mineshaft = StructureStartFilterContext::new(0, 1, 2, StructureKeys::Mineshaft);
        assert!(!extensions.allow_structure_start(&village));
        assert!(extensions.allow_structure_start(&mineshaft));
    }

    #[test]
    fn sample_surface_height_is_inside_generation_shape() {
        let generator = VanillaGenerator::new_with_settings(Seed(42), Dimension::OVERWORLD, None);
        let height = generator.sample_surface_height(0, 0);
        let min_y = i32::from(generator.settings.shape.min_y);
        let max_y = i32::from(generator.settings.shape.max_y());
        assert!(
            (min_y..=max_y).contains(&height),
            "surface height {height} outside {min_y}..={max_y}"
        );
    }

    #[test]
    fn world_generator_sample_is_none_for_flat() {
        let world_gen = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            true,
            vec![],
            "minecraft:plains".to_string(),
        );
        assert!(world_gen.sample_surface_height(0, 0).is_none());
        assert!(!world_gen.extensions().has_decorators());
    }

    #[test]
    fn unused_extensions_leave_structure_starts_unchanged() {
        let world_gen = get_world_gen(
            Seed(1),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let WorldGenerator::Noise(generator) = &*world_gen else {
            panic!("expected noise generator");
        };
        assert!(!generator.extensions.has_structure_filters());

        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        chunk.step_to_biomes(generator);
        chunk.set_structure_starts(generator);
        // The empty-filter path must complete; presence of starts is seed-specific.
        let _ = chunk.structure_starts.len();
    }

    #[test]
    fn structure_filter_can_clear_a_known_start() {
        let seed = Seed(1_782_124_772_053_846_960);
        let world_gen = get_world_gen(seed, Dimension::OVERWORLD, false, Vec::new(), String::new());
        let WorldGenerator::Noise(generator) = &*world_gen else {
            panic!("expected noise generator");
        };

        let mut allowed = ProtoChunk::new(-553, 173, &world_gen);
        allowed.step_to_biomes(generator);
        allowed.set_structure_starts(generator);
        assert!(
            allowed.has_structure(StructureKeys::Monument),
            "fixture chunk should own a monument start"
        );

        generator
            .extensions
            .register_structure_filter(Arc::new(VetoStructure {
                structure: StructureKeys::Monument,
            }));
        let mut vetoed = ProtoChunk::new(-553, 173, &world_gen);
        vetoed.step_to_biomes(generator);
        vetoed.set_structure_starts(generator);
        assert!(!vetoed.has_structure(StructureKeys::Monument));
    }
}
