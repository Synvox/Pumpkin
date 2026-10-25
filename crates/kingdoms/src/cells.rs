//! Per-block occupancy of a planned town.

use crate::geom::Rect;

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
