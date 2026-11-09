//! Civilian lots and building kinds. Facades use WFC; volumes are parametric.

use crate::geom::{Dir, Rect};
use crate::rng::TownRng;
use crate::wfc::collapse_facade;

/// Kind of civilian building.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BuildingKind {
    Cottage,
    Timber,
    Townhouse,
    Longhouse,
    Church,
    Tavern,
    Blacksmith,
    Barn,
    Windmill,
    Stall,
    Monastery,
    Watchtower,
    Outpost,
    Cathedral,
    BellTower,
    TownHall,
    /// Riverside mill house with a water wheel (not a windmill).
    Watermill,
}

impl BuildingKind {
    /// Typical footprint.
    #[must_use]
    pub const fn size(self) -> (i32, i32) {
        match self {
            Self::Cottage => (8, 7),
            Self::Timber => (10, 8),
            Self::Townhouse => (9, 8),
            Self::Longhouse => (18, 8),
            Self::Church => (16, 11),
            Self::Tavern => (13, 10),
            Self::Blacksmith => (11, 9),
            Self::Barn => (14, 10),
            Self::Windmill => (7, 7),
            Self::Stall => (5, 5),
            Self::Monastery => (18, 12),
            Self::Watchtower => (7, 7),
            Self::Outpost => (5, 5),
            Self::Cathedral => (32, 22),
            Self::BellTower => (5, 5),
            Self::TownHall => (14, 12),
            Self::Watermill => (7, 6),
        }
    }

    /// Storeys above the foundation (1 = cottage).
    #[must_use]
    pub const fn stories(self) -> i32 {
        match self {
            Self::Cottage
            | Self::Stall
            | Self::Barn
            | Self::Windmill
            | Self::Watermill
            | Self::Monastery => 1,
            Self::Longhouse | Self::Blacksmith | Self::Cathedral => 1,
            Self::Tavern | Self::Church => 2,
            Self::Timber
            | Self::Townhouse
            | Self::Watchtower
            | Self::Outpost
            | Self::BellTower
            | Self::TownHall => 3,
        }
    }

    /// Wall height above the local floor (not including roof).
    #[must_use]
    pub const fn wall_h(self) -> i32 {
        match self {
            Self::Stall => 3,
            Self::Cottage => 5,
            Self::Longhouse | Self::Blacksmith | Self::Barn => 6,
            Self::Timber => 11,
            Self::Tavern => 8,
            Self::Townhouse => 13,
            Self::Church => 8,
            Self::Windmill => 18,
            Self::Monastery => 9,
            Self::Watchtower => 14,
            Self::Outpost => 12,
            Self::Cathedral => 16,
            Self::BellTower => 18,
            Self::TownHall => 16,
            Self::Watermill => 8,
        }
    }

    /// Whether walls use a timber frame with plaster infill.
    #[must_use]
    pub const fn timber_frame(self) -> bool {
        matches!(
            self,
            Self::Cottage | Self::Timber | Self::Townhouse | Self::Tavern | Self::Longhouse
        )
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
