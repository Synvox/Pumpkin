//! Civilian lots and building kinds. Facades use WFC; volumes are parametric.

use crate::geom::{Dir, Rect};
use crate::rng::TownRng;
use crate::wfc::collapse_facade;

/// Kind of civilian building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BuildingKind {
    Cottage,
    Timber,
    Longhouse,
    Church,
    Tavern,
    Blacksmith,
    Barn,
    Windmill,
    Stall,
}

impl BuildingKind {
    /// Typical footprint.
    #[must_use]
    pub const fn size(self) -> (i32, i32) {
        match self {
            Self::Cottage => (9, 9),
            Self::Timber => (11, 9),
            Self::Longhouse => (16, 8),
            Self::Church => (18, 11),
            Self::Tavern => (12, 10),
            Self::Blacksmith => (11, 9),
            Self::Barn => (14, 10),
            Self::Windmill => (7, 7),
            Self::Stall => (5, 5),
        }
    }

    /// Wall height above the local floor (not including roof).
    #[must_use]
    pub const fn wall_h(self) -> i32 {
        match self {
            Self::Cottage | Self::Stall => 4,
            Self::Timber | Self::Blacksmith | Self::Barn => 5,
            Self::Longhouse | Self::Tavern => 6,
            Self::Church => 7,
            Self::Windmill => 10,
        }
    }
}

/// A placed building lot.
#[derive(Clone, Debug)]
pub struct Lot {
    pub id: u16,
    pub kind: BuildingKind,
    pub rect: Rect,
    pub facing: Dir,
    pub seed: u64,
    /// Collapsed front facade, row-major, height = 3.
    pub facade: Vec<u8>,
}

impl Lot {
    /// Builds a lot and collapses its front facade.
    #[must_use]
    pub fn new(id: u16, kind: BuildingKind, rect: Rect, facing: Dir, seed: u64) -> Self {
        let mut rng = TownRng::new(seed);
        let width = match facing {
            Dir::North | Dir::South => rect.width(),
            Dir::East | Dir::West => rect.depth(),
        } as usize;
        let door = width / 2;
        let facade = collapse_facade(&mut rng, width.max(3), 3, Some(door));
        Self {
            id,
            kind,
            rect,
            facing,
            seed,
            facade,
        }
    }
}
