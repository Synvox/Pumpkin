//! Small geometry helpers for the town planner.

/// Cardinal direction, clockwise from north.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Dir {
    North,
    East,
    South,
    West,
}

impl Dir {
    /// Unit step on X.
    #[must_use]
    pub const fn dx(self) -> i32 {
        match self {
            Self::East => 1,
            Self::West => -1,
            Self::North | Self::South => 0,
        }
    }

    /// Unit step on Z.
    #[must_use]
    pub const fn dz(self) -> i32 {
        match self {
            Self::South => 1,
            Self::North => -1,
            Self::East | Self::West => 0,
        }
    }

    /// Opposite direction.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::North => Self::South,
            Self::South => Self::North,
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    /// All four directions, north first.
    pub const ALL: [Self; 4] = [Self::North, Self::East, Self::South, Self::West];
}

/// Inclusive axis-aligned rectangle on XZ.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rect {
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
}

impl Rect {
    /// Creates a rect. Swaps inverted corners.
    #[must_use]
    pub const fn new(min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Self {
        Self {
            min_x: if min_x <= max_x { min_x } else { max_x },
            min_z: if min_z <= max_z { min_z } else { max_z },
            max_x: if min_x <= max_x { max_x } else { min_x },
            max_z: if min_z <= max_z { max_z } else { min_z },
        }
    }

    /// Width in blocks (inclusive).
    #[must_use]
    pub const fn width(self) -> i32 {
        self.max_x - self.min_x + 1
    }

    /// Depth in blocks (inclusive).
    #[must_use]
    pub const fn depth(self) -> i32 {
        self.max_z - self.min_z + 1
    }

    /// Whether `(x, z)` is inside.
    #[must_use]
    pub const fn contains(self, x: i32, z: i32) -> bool {
        x >= self.min_x && x <= self.max_x && z >= self.min_z && z <= self.max_z
    }

    /// Whether the two rects overlap.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_z <= other.max_z
            && self.max_z >= other.min_z
    }

    /// Center block.
    #[must_use]
    pub const fn center(self) -> (i32, i32) {
        (
            self.min_x + (self.max_x - self.min_x) / 2,
            self.min_z + (self.max_z - self.min_z) / 2,
        )
    }

    /// Grows by `pad` on every side.
    #[must_use]
    pub const fn inflate(self, pad: i32) -> Self {
        Self::new(
            self.min_x - pad,
            self.min_z - pad,
            self.max_x + pad,
            self.max_z + pad,
        )
    }

    /// Inset by `pad`. Degenerate if too small.
    #[must_use]
    pub const fn deflate(self, pad: i32) -> Self {
        Self::new(
            self.min_x + pad,
            self.min_z + pad,
            self.max_x - pad,
            self.max_z - pad,
        )
    }

    /// Whether `(x, z)` is on the perimeter.
    #[must_use]
    pub const fn on_edge(self, x: i32, z: i32) -> bool {
        self.contains(x, z)
            && (x == self.min_x || x == self.max_x || z == self.min_z || z == self.max_z)
    }

    /// Whether `(x, z)` is a corner.
    #[must_use]
    pub const fn is_corner(self, x: i32, z: i32) -> bool {
        (x == self.min_x || x == self.max_x) && (z == self.min_z || z == self.max_z)
    }
}

/// Integer Bresenham line, inclusive of both ends.
pub fn bresenham(x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<(i32, i32)> {
    let mut points = Vec::new();
    let mut x = x0;
    let mut z = z0;
    let dx = (x1 - x0).abs();
    let dz = -(z1 - z0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx + dz;
    loop {
        points.push((x, z));
        if x == x1 && z == z1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dz {
            err += dz;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            z += sz;
        }
    }
    points
}

/// Chebyshev distance.
#[must_use]
pub const fn chebyshev(ax: i32, az: i32, bx: i32, bz: i32) -> i32 {
    let dx = (ax - bx).abs();
    let dz = (az - bz).abs();
    if dx > dz { dx } else { dz }
}

/// Squared Euclidean distance.
#[must_use]
pub const fn dist2(ax: i32, az: i32, bx: i32, bz: i32) -> i64 {
    let dx = (ax - bx) as i64;
    let dz = (az - bz) as i64;
    dx * dx + dz * dz
}

/// Whether `(x, z)` is inside the closed polygon `verts` (ray cast).
#[must_use]
pub fn point_in_poly(x: i32, z: i32, verts: &[(i32, i32)]) -> bool {
    if verts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = verts.len() - 1;
    for i in 0..verts.len() {
        let (xi, zi) = verts[i];
        let (xj, zj) = verts[j];
        if (zi > z) != (zj > z) {
            let den = zj - zi;
            if den != 0 {
                let xinters = xi + (z - zi) * (xj - xi) / den;
                if x < xinters {
                    inside = !inside;
                }
            }
        }
        j = i;
    }
    inside
}

/// Squared distance from `(px, pz)` to the segment `a→b`.
#[must_use]
pub fn dist2_to_segment(px: i32, pz: i32, ax: i32, az: i32, bx: i32, bz: i32) -> i64 {
    let vx = i64::from(bx - ax);
    let vz = i64::from(bz - az);
    let wx = i64::from(px - ax);
    let wz = i64::from(pz - az);
    let c2 = vx * vx + vz * vz;
    if c2 == 0 {
        return wx * wx + wz * wz;
    }
    let c1 = vx * wx + vz * wz;
    if c1 <= 0 {
        return wx * wx + wz * wz;
    }
    if c1 >= c2 {
        let dx = i64::from(px - bx);
        let dz = i64::from(pz - bz);
        return dx * dx + dz * dz;
    }
    // (w - t v)^2 with t = c1/c2, multiplied through by c2^2 then divided.
    let nx = wx * c2 - c1 * vx;
    let nz = wz * c2 - c1 * vz;
    (nx * nx + nz * nz) / (c2 * c2)
}

/// Whether `(x, z)` is within `radius` of any edge of `verts` (closed).
#[must_use]
pub fn near_poly_edge(x: i32, z: i32, verts: &[(i32, i32)], radius: i32) -> bool {
    if verts.len() < 2 {
        return false;
    }
    let lim = i64::from(radius) * i64::from(radius) + 1;
    for pair in verts.windows(2) {
        if dist2_to_segment(x, z, pair[0].0, pair[0].1, pair[1].0, pair[1].1) <= lim {
            return true;
        }
    }
    let last = verts[verts.len() - 1];
    let first = verts[0];
    dist2_to_segment(x, z, last.0, last.1, first.0, first.1) <= lim
}

/// Axis-aligned bounds of `verts`.
#[must_use]
pub fn poly_bounds(verts: &[(i32, i32)]) -> Option<Rect> {
    let (first, rest) = verts.split_first()?;
    let mut min_x = first.0;
    let mut min_z = first.1;
    let mut max_x = first.0;
    let mut max_z = first.1;
    for &(x, z) in rest {
        min_x = min_x.min(x);
        min_z = min_z.min(z);
        max_x = max_x.max(x);
        max_z = max_z.max(z);
    }
    Some(Rect::new(min_x, min_z, max_x, max_z))
}

/// Integer length of a 2D vector, floored.
#[must_use]
pub fn isqrt(dx: i32, dz: i32) -> i32 {
    let s = dist2(0, 0, dx, dz);
    (s as f64).sqrt() as i32
}
