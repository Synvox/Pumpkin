//! Per-block occupancy of a planned town.

use crate::geom::{Rect, near_poly_edge, point_in_poly, poly_bounds};

/// What the planner assigned to one XZ column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CellKind {
    Empty = 0,
    Road,
    RoadStairN,
    RoadStairS,
    RoadStairE,
    RoadStairW,
    Market,
    Farm,
    FarmEdge,
    TownWall,
    TownTower,
    TownGate,
    Well,
    Building,
    BarnYard,
    CastleWall,
    CastleTower,
    CastleGate,
    CastleBarbican,
    CastleKeep,
    CastleHall,
    CastleChapel,
    CastleStables,
    CastleBarracks,
    CastleKitchen,
    CastleCourt,
    CastleMoat,
    CastleInnerWall,
    /// Grass courtyard or garden (not paved).
    Green,
    /// Roof overhang beside a building.
    Eave,
    /// Hedge or leaf screen.
    Hedge,
    /// Open water kept as a harbor basin.
    Harbor,
    /// Plank walk on the water's edge.
    Dock,
    /// Road crossing water.
    Bridge,
    /// Fruit trees on grass.
    Orchard,
    /// Fenced grazing green.
    Pasture,
    /// Headstones and paths.
    Grave,
    /// Paved gate or market plaza.
    Plaza,
    /// Camp palisade of logs.
    Palisade,
    /// Wool tent.
    Tent,
    /// Broken masonry, no roof.
    Ruin,
    /// Outpost walk / hill tower pad.
    WatchPost,
    /// Basin, statue, or fountain ring.
    Fountain,
    /// Open dirt court inside a palisade.
    CampCourt,
    /// Larger command tent.
    WarTent,
    /// Iron-bar cage.
    Cage,
    /// Palisade opening with posts.
    PalisadeGate,
    /// Multi-column trebuchet.
    SiegeTreb,
    /// Covered battering ram.
    SiegeRam,
    /// Stacked siege ladders.
    SiegeLadder,
    /// Windmill sail column (outside the tower lot).
    WindSail,
    /// Cloister arcade.
    Arcade,
    /// Graveyard lychgate.
    Lychgate,
    /// Mountain-fortress curtain.
    FortressWall,
    /// Mountain-fortress keep.
    FortressKeep,
    /// Mountain-fortress tower.
    FortressTower,
    /// Mountain-fortress court.
    FortressCourt,
    /// Cathedral nave.
    CathedralNave,
    /// Cathedral transept.
    CathedralTransept,
    /// Cathedral choir and apse.
    CathedralChoir,
    /// West tower with spire.
    CathedralTower,
    /// Flying buttress / pinnacle (exterior).
    CathedralButtress,
    /// Palisade wall-walk between stakes.
    PalisadeWalk,
    /// Mine headframe over a shaft.
    MineHead,
    /// Rail and cart stand.
    MineRail,
    /// Gravel and cobble spoil heap.
    MineSpoil,
    /// Cog hull on harbor water.
    Ship,
    /// Jousting lists (sand lane).
    TourneyList,
    /// Tournament stand or pavilion pad.
    TourneyStand,
    /// Stone quay along the harbour.
    Quay,
    /// Harbour crane pad.
    Crane,
    /// Small fishing-boat hull.
    Boat,
    /// Timber adit portal into the hillside.
    MineAdit,
    /// Charred house shell in a siege-scarred frontier town.
    Burnt,
    /// Stone breakwater in harbour water.
    Breakwater,
    /// Siege trench / ditch outside a damaged curtain.
    SiegeTrench,
    /// Vertical mill wheel in river water.
    WaterWheel,
    /// Mill race / leat feeding a watermill.
    MillRace,
    /// Sluice gate on a mill race.
    Sluice,
    /// Cobble retaining wall beside a switchback.
    Retaining,
    /// Terraced grape vines on a hillside abbey.
    Vine,
    /// Harbour lighthouse on the breakwater end.
    Lighthouse,
    /// Wayside shrine beside a road.
    Shrine,
    /// Dry-stone field wall along a farm or road.
    FieldWall,
    /// Keep-courtyard flag plinth (pedestal, mast, pressure plates).
    FlagPlinth,
    /// Dry fosse outside the lower-city curtain.
    CityDitch,
    /// Menhir in a hinterland stone circle.
    StandingStone,
    /// Crossroads gallows post.
    Gallows,
    /// Bandit loot pile of chests and barrels.
    LootPile,
}

impl CellKind {
    /// Whether this cell is part of the castle complex.
    #[must_use]
    pub const fn is_castle(self) -> bool {
        matches!(
            self,
            Self::CastleWall
                | Self::CastleTower
                | Self::CastleGate
                | Self::CastleBarbican
                | Self::CastleKeep
                | Self::CastleHall
                | Self::CastleChapel
                | Self::CastleStables
                | Self::CastleBarracks
                | Self::CastleKitchen
                | Self::CastleCourt
                | Self::CastleMoat
                | Self::CastleInnerWall
                | Self::FlagPlinth
        )
    }

    /// Stone volumes that can bound an interior room.
    #[must_use]
    pub const fn is_shell(self) -> bool {
        matches!(
            self,
            Self::CastleWall
                | Self::CastleInnerWall
                | Self::CastleTower
                | Self::CastleGate
                | Self::CastleBarbican
                | Self::CastleKeep
                | Self::CastleHall
                | Self::CastleChapel
                | Self::TownWall
                | Self::TownTower
                | Self::CathedralNave
                | Self::CathedralTransept
                | Self::CathedralChoir
                | Self::CathedralTower
        )
    }
}

/// Dense occupancy grid covering the town AABB.
#[derive(Clone, Debug)]
pub struct Occupancy {
    pub bounds: Rect,
    kind: Vec<CellKind>,
    lot: Vec<u16>,
}

impl Occupancy {
    /// Empty grid over `bounds`.
    #[must_use]
    pub fn new(bounds: Rect) -> Self {
        let n = (bounds.width() * bounds.depth()) as usize;
        Self {
            bounds,
            kind: vec![CellKind::Empty; n],
            lot: vec![0; n],
        }
    }

    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        if !self.bounds.contains(x, z) {
            return None;
        }
        let lx = x - self.bounds.min_x;
        let lz = z - self.bounds.min_z;
        Some((lz * self.bounds.width() + lx) as usize)
    }

    /// Kind at `(x, z)`, or `Empty` outside.
    #[must_use]
    pub fn kind(&self, x: i32, z: i32) -> CellKind {
        self.idx(x, z).map_or(CellKind::Empty, |i| self.kind[i])
    }

    /// Lot id at `(x, z)` (`0` = none).
    #[must_use]
    pub fn lot(&self, x: i32, z: i32) -> u16 {
        self.idx(x, z).map_or(0, |i| self.lot[i])
    }

    /// Writes `kind` if the cell is empty or `overwrite`.
    pub fn set(&mut self, x: i32, z: i32, kind: CellKind, overwrite: bool) {
        if let Some(i) = self.idx(x, z)
            && (overwrite || self.kind[i] == CellKind::Empty)
        {
            self.kind[i] = kind;
        }
    }

    /// Writes a building cell with `lot_id`.
    pub fn set_building(&mut self, x: i32, z: i32, lot_id: u16, overwrite: bool) {
        if let Some(i) = self.idx(x, z)
            && (overwrite || self.kind[i] == CellKind::Empty)
        {
            self.kind[i] = CellKind::Building;
            self.lot[i] = lot_id;
        }
    }

    /// Fills a rectangle.
    pub fn fill(&mut self, rect: Rect, kind: CellKind, overwrite: bool) {
        for z in rect.min_z..=rect.max_z {
            for x in rect.min_x..=rect.max_x {
                self.set(x, z, kind, overwrite);
            }
        }
    }

    /// Fills a building rectangle.
    pub fn fill_building(&mut self, rect: Rect, lot_id: u16, overwrite: bool) {
        for z in rect.min_z..=rect.max_z {
            for x in rect.min_x..=rect.max_x {
                self.set_building(x, z, lot_id, overwrite);
            }
        }
    }

    /// Fills the interior of a closed polygon.
    pub fn fill_poly(&mut self, verts: &[(i32, i32)], kind: CellKind, overwrite: bool) {
        let Some(bounds) = poly_bounds(verts) else {
            return;
        };
        for z in bounds.min_z.max(self.bounds.min_z)..=bounds.max_z.min(self.bounds.max_z) {
            for x in bounds.min_x.max(self.bounds.min_x)..=bounds.max_x.min(self.bounds.max_x) {
                if point_in_poly(x, z, verts) {
                    self.set(x, z, kind, overwrite);
                }
            }
        }
    }

    /// Paints columns within `radius` of a closed polygon's edges.
    pub fn stroke_poly(
        &mut self,
        verts: &[(i32, i32)],
        radius: i32,
        kind: CellKind,
        overwrite: bool,
    ) {
        let Some(bounds) = poly_bounds(verts) else {
            return;
        };
        let pad = radius + 1;
        for z in (bounds.min_z - pad).max(self.bounds.min_z)
            ..=(bounds.max_z + pad).min(self.bounds.max_z)
        {
            for x in (bounds.min_x - pad).max(self.bounds.min_x)
                ..=(bounds.max_x + pad).min(self.bounds.max_x)
            {
                if near_poly_edge(x, z, verts, radius) {
                    self.set(x, z, kind, overwrite);
                }
            }
        }
    }

    /// Paints a thick polyline.
    pub fn stroke(&mut self, points: &[(i32, i32)], radius: i32, kind: CellKind, overwrite: bool) {
        for &(x, z) in points {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    if dx * dx + dz * dz <= radius * radius + 1 {
                        self.set(x + dx, z + dz, kind, overwrite);
                    }
                }
            }
        }
    }

    /// Paints a Chebyshev-thick polyline (a `radius` of 1 is three blocks wide).
    pub fn stroke_square(
        &mut self,
        points: &[(i32, i32)],
        radius: i32,
        kind: CellKind,
        overwrite: bool,
    ) {
        for &(x, z) in points {
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    self.set(x + dx, z + dz, kind, overwrite);
                }
            }
        }
    }

    /// Whether `rect` is free of non-empty cells (and in bounds).
    #[must_use]
    pub fn is_free(&self, rect: Rect) -> bool {
        if !self.bounds.contains(rect.min_x, rect.min_z)
            || !self.bounds.contains(rect.max_x, rect.max_z)
        {
            return false;
        }
        for z in rect.min_z..=rect.max_z {
            for x in rect.min_x..=rect.max_x {
                if self.kind(x, z) != CellKind::Empty {
                    return false;
                }
            }
        }
        true
    }
}
