//! Deterministic town-cell planner.
//!
//! One town per `CELL_SIZE`×`CELL_SIZE` block region, placed as a pure function
//! of `(seed, cell_x, cell_z)`.

/// Side length of a town planning cell, in blocks.
pub const CELL_SIZE: i32 = 2048;

/// Blocks around a town marker where vanilla villages are vetoed.
pub const VILLAGE_EXCLUSION_RADIUS: i32 = 48;

/// Mixer seed so town positions do not line up with vanilla structure salts.
const TOWN_SALT: u64 = 0x4B49_4E47_444F_4D53; // "KINGDOMS"

/// Planned town location in a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TownPos {
    /// World X of the well center.
    pub x: i32,
    /// World Z of the well center.
    pub z: i32,
    /// Planning-cell X (`floor(x / CELL_SIZE)`).
    pub cell_x: i32,
    /// Planning-cell Z (`floor(z / CELL_SIZE)`).
    pub cell_z: i32,
}

#[must_use]
const fn mix_town(seed: u64, cell_x: i32, cell_z: i32) -> u64 {
    let mut hash = seed ^ TOWN_SALT;
    hash ^= (cell_x as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    hash = hash.rotate_left(31);
    hash ^= (cell_z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    hash = hash.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hash ^ (hash >> 33)
}

/// Town in planning cell `(cell_x, cell_z)` for `seed`.
#[must_use]
pub const fn town_in_cell(seed: u64, cell_x: i32, cell_z: i32) -> TownPos {
    let hash = mix_town(seed, cell_x, cell_z);
    // Keep the 5×5 well inside the cell so planning stays a function of one cell.
    let inner = (CELL_SIZE - 8) as u64;
    let ox = 4 + (hash % inner) as i32;
    let oz = 4 + ((hash >> 32) % inner) as i32;
    TownPos {
        x: cell_x.wrapping_mul(CELL_SIZE) + ox,
        z: cell_z.wrapping_mul(CELL_SIZE) + oz,
        cell_x,
        cell_z,
    }
}

/// Town whose cell contains world `(block_x, block_z)`.
#[must_use]
pub const fn town_containing(seed: u64, block_x: i32, block_z: i32) -> TownPos {
    town_in_cell(
        seed,
        block_x.div_euclid(CELL_SIZE),
        block_z.div_euclid(CELL_SIZE),
    )
}

/// Nearest town to `(block_x, block_z)`, searching this cell and its neighbors.
#[must_use]
pub fn nearest_town(seed: u64, block_x: i32, block_z: i32) -> TownPos {
    let cell_x = block_x.div_euclid(CELL_SIZE);
    let cell_z = block_z.div_euclid(CELL_SIZE);
    let mut best = town_in_cell(seed, cell_x, cell_z);
    let mut best_dist = dist2(block_x, block_z, best.x, best.z);
    for dx in -1..=1 {
        for dz in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let candidate = town_in_cell(seed, cell_x + dx, cell_z + dz);
            let dist = dist2(block_x, block_z, candidate.x, candidate.z);
            if dist < best_dist {
                best = candidate;
                best_dist = dist;
            }
        }
    }
    best
}

#[must_use]
const fn dist2(ax: i32, az: i32, bx: i32, bz: i32) -> i64 {
    let dx = (ax - bx) as i64;
    let dz = (az - bz) as i64;
    dx * dx + dz * dz
}

/// Whether `(block_x, block_z)` is inside a town's village-exclusion disk.
#[must_use]
pub fn is_inside_village_exclusion(seed: u64, block_x: i32, block_z: i32) -> bool {
    let radius2 = i64::from(VILLAGE_EXCLUSION_RADIUS) * i64::from(VILLAGE_EXCLUSION_RADIUS);
    let cell_x = block_x.div_euclid(CELL_SIZE);
    let cell_z = block_z.div_euclid(CELL_SIZE);
    for dx in -1..=1 {
        for dz in -1..=1 {
            let town = town_in_cell(seed, cell_x + dx, cell_z + dz);
            if dist2(block_x, block_z, town.x, town.z) <= radius2 {
                return true;
            }
        }
    }
    false
}

/// Towns whose well may touch chunk `(chunk_x, chunk_z)`.
#[must_use]
pub fn towns_touching_chunk(seed: u64, chunk_x: i32, chunk_z: i32) -> Vec<TownPos> {
    let min_x = chunk_x * 16;
    let min_z = chunk_z * 16;
    let max_x = min_x + 15;
    let max_z = min_z + 15;
    let mut towns = Vec::new();
    let min_cell_x = (min_x - 2).div_euclid(CELL_SIZE);
    let max_cell_x = (max_x + 2).div_euclid(CELL_SIZE);
    let min_cell_z = (min_z - 2).div_euclid(CELL_SIZE);
    let max_cell_z = (max_z + 2).div_euclid(CELL_SIZE);
    for cell_x in min_cell_x..=max_cell_x {
        for cell_z in min_cell_z..=max_cell_z {
            let town = town_in_cell(seed, cell_x, cell_z);
            if town.x + 2 >= min_x
                && town.x - 2 <= max_x
                && town.z + 2 >= min_z
                && town.z - 2 <= max_z
            {
                towns.push(town);
            }
        }
    }
    towns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn town_is_deterministic() {
        let a = town_in_cell(123, 0, 0);
        let b = town_in_cell(123, 0, 0);
        assert_eq!(a, b);
        assert_ne!(a, town_in_cell(124, 0, 0));
        assert_ne!(a, town_in_cell(123, 1, 0));
    }

    #[test]
    fn town_stays_inside_its_cell() {
        for cell_x in -2..=2 {
            for cell_z in -2..=2 {
                let town = town_in_cell(99, cell_x, cell_z);
                assert_eq!(town.x.div_euclid(CELL_SIZE), cell_x);
                assert_eq!(town.z.div_euclid(CELL_SIZE), cell_z);
                assert_eq!(town.cell_x, cell_x);
                assert_eq!(town.cell_z, cell_z);
            }
        }
    }

    #[test]
    fn nearest_town_at_a_marker_is_that_town() {
        let seed = 7;
        let town = town_in_cell(seed, 3, -2);
        assert_eq!(nearest_town(seed, town.x, town.z), town);
        assert_eq!(nearest_town(seed, town.x + 1, town.z - 1), town);
    }
}
