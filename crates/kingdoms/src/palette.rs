//! Biome-driven block palettes. Buildings stay medieval; only materials swap.

use pumpkin_data::biome::Biome;
use pumpkin_data::{Block, BlockState};

/// Which wood-and-stone set a building uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PaletteKind {
    Oak,
    Spruce,
    Sandstone,
}

impl PaletteKind {
    /// Palette for a vanilla biome id.
    #[must_use]
    pub fn from_biome(biome: &Biome) -> Self {
        match biome.registry_id {
            "taiga"
            | "snowy_taiga"
            | "old_growth_pine_taiga"
            | "old_growth_spruce_taiga"
            | "grove"
            | "snowy_plains"
            | "snowy_slopes" => Self::Spruce,
            "desert" | "badlands" | "eroded_badlands" | "wooded_badlands" => Self::Sandstone,
            _ => Self::Oak,
        }
    }
}

/// Concrete blocks for one palette.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub kind: PaletteKind,
    pub log: &'static BlockState,
    pub stripped: &'static BlockState,
    pub planks: &'static BlockState,
    pub stairs: &'static Block,
    pub slab: &'static BlockState,
    pub fence: &'static BlockState,
    pub wall_fill: &'static BlockState,
    pub plaster: &'static BlockState,
    pub cobble: &'static BlockState,
    pub mossy: &'static BlockState,
    pub bricks: &'static BlockState,
    pub mossy_bricks: &'static BlockState,
    pub cracked_bricks: &'static BlockState,
    pub brick_stairs: &'static Block,
    pub brick_wall: &'static BlockState,
    pub cobble_stairs: &'static Block,
    pub cobble_wall: &'static BlockState,
    pub roof: &'static Block,
    pub roof_slab: &'static BlockState,
    pub path: &'static BlockState,
    pub gravel: &'static BlockState,
    pub sand: &'static BlockState,
}

impl Palette {
    /// Builds the material set for `kind`.
    #[must_use]
    pub fn new(kind: PaletteKind) -> Self {
        match kind {
            PaletteKind::Oak => Self {
                kind,
                log: Block::OAK_LOG.default_state,
                stripped: Block::STRIPPED_OAK_LOG.default_state,
                planks: Block::OAK_PLANKS.default_state,
                stairs: &Block::OAK_STAIRS,
                slab: Block::OAK_SLAB.default_state,
                fence: Block::OAK_FENCE.default_state,
                wall_fill: Block::OAK_PLANKS.default_state,
                plaster: Block::WHITE_TERRACOTTA.default_state,
                cobble: Block::COBBLESTONE.default_state,
                mossy: Block::MOSSY_COBBLESTONE.default_state,
                bricks: Block::STONE_BRICKS.default_state,
                mossy_bricks: Block::MOSSY_STONE_BRICKS.default_state,
                cracked_bricks: Block::CRACKED_STONE_BRICKS.default_state,
                brick_stairs: &Block::STONE_BRICK_STAIRS,
                brick_wall: Block::STONE_BRICK_WALL.default_state,
                cobble_stairs: &Block::COBBLESTONE_STAIRS,
                cobble_wall: Block::COBBLESTONE_WALL.default_state,
                roof: &Block::DARK_OAK_STAIRS,
                roof_slab: Block::DARK_OAK_SLAB.default_state,
                path: Block::DIRT_PATH.default_state,
                gravel: Block::GRAVEL.default_state,
                sand: Block::SAND.default_state,
            },
            PaletteKind::Spruce => Self {
                kind,
                log: Block::SPRUCE_LOG.default_state,
                stripped: Block::STRIPPED_SPRUCE_LOG.default_state,
                planks: Block::SPRUCE_PLANKS.default_state,
                stairs: &Block::SPRUCE_STAIRS,
                slab: Block::SPRUCE_SLAB.default_state,
                fence: Block::SPRUCE_FENCE.default_state,
                wall_fill: Block::SPRUCE_PLANKS.default_state,
                plaster: Block::LIGHT_GRAY_TERRACOTTA.default_state,
                cobble: Block::COBBLESTONE.default_state,
                mossy: Block::MOSSY_COBBLESTONE.default_state,
                bricks: Block::STONE_BRICKS.default_state,
                mossy_bricks: Block::MOSSY_STONE_BRICKS.default_state,
                cracked_bricks: Block::CRACKED_STONE_BRICKS.default_state,
                brick_stairs: &Block::STONE_BRICK_STAIRS,
                brick_wall: Block::STONE_BRICK_WALL.default_state,
                cobble_stairs: &Block::COBBLESTONE_STAIRS,
                cobble_wall: Block::COBBLESTONE_WALL.default_state,
                roof: &Block::SPRUCE_STAIRS,
                roof_slab: Block::SPRUCE_SLAB.default_state,
                path: Block::DIRT_PATH.default_state,
                gravel: Block::GRAVEL.default_state,
                sand: Block::SNOW_BLOCK.default_state,
            },
            PaletteKind::Sandstone => Self {
                kind,
                log: Block::SPRUCE_LOG.default_state,
                stripped: Block::STRIPPED_SPRUCE_LOG.default_state,
                planks: Block::SMOOTH_SANDSTONE.default_state,
                stairs: &Block::SANDSTONE_STAIRS,
                slab: Block::SANDSTONE_SLAB.default_state,
                fence: Block::SPRUCE_FENCE.default_state,
                wall_fill: Block::CUT_SANDSTONE.default_state,
                plaster: Block::SANDSTONE.default_state,
                cobble: Block::SANDSTONE.default_state,
                mossy: Block::SMOOTH_SANDSTONE.default_state,
                bricks: Block::CUT_SANDSTONE.default_state,
                mossy_bricks: Block::SANDSTONE.default_state,
                cracked_bricks: Block::SMOOTH_SANDSTONE.default_state,
                brick_stairs: &Block::SANDSTONE_STAIRS,
                brick_wall: Block::SANDSTONE_WALL.default_state,
                cobble_stairs: &Block::SANDSTONE_STAIRS,
                cobble_wall: Block::SANDSTONE_WALL.default_state,
                roof: &Block::SPRUCE_STAIRS,
                roof_slab: Block::SPRUCE_SLAB.default_state,
                path: Block::SANDSTONE.default_state,
                gravel: Block::SAND.default_state,
                sand: Block::SAND.default_state,
            },
        }
    }

    /// Foundation / retaining block, with light aging.
    #[must_use]
    pub fn foundation(self, mossy: bool) -> &'static BlockState {
        if mossy { self.mossy } else { self.cobble }
    }

    /// Castle masonry, with light aging.
    #[must_use]
    pub fn masonry(self, variant: u8) -> &'static BlockState {
        match variant % 7 {
            0..=2 => self.bricks,
            3 | 4 => self.cobble,
            5 => self.mossy_bricks,
            _ => self.cracked_bricks,
        }
    }
}

/// Stair block facing `dir` (the descending direction of the stair).
#[must_use]
pub fn stair_state(block: &Block, dir: crate::geom::Dir) -> &'static BlockState {
    use pumpkin_data::block_properties::{Half, HorizontalFacing, StairsProperties, StairsShape};
    let facing = match dir {
        crate::geom::Dir::North => HorizontalFacing::North,
        crate::geom::Dir::South => HorizontalFacing::South,
        crate::geom::Dir::East => HorizontalFacing::East,
        crate::geom::Dir::West => HorizontalFacing::West,
    };
    let props = StairsProperties {
        facing,
        half: Half::Bottom,
        shape: StairsShape::Straight,
        waterlogged: false,
    };
    BlockState::from_id(props.to_state_id(block))
}
