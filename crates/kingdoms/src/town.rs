//! Deterministic town-cell planner.
//!
//! One town per `CELL_SIZE`×`CELL_SIZE` block region, placed as a pure function
//! of `(seed, cell_x, cell_z)`.

/// Side length of a town planning cell, in blocks.
pub const CELL_SIZE: i32 = 2048;

/// Keep the whole castle+town inside its cell.
pub const TOWN_MARGIN: i32 = 400;

/// Blocks around a town where vanilla villages are vetoed.
pub const VILLAGE_EXCLUSION_RADIUS: i32 = 360;

/// Conservative radius used to decide which chunks a town may touch.
pub const TOWN_INFLUENCE: i32 = 360;

/// Mixer seed so town positions do not line up with vanilla structure salts.
const TOWN_SALT: u64 = 0x4B49_4E47_444F_4D53; // "KINGDOMS"

/// Planned town location in a cell (search origin, not the well).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TownPos {
    /// World X of the planning origin.
    pub x: i32,
    /// World Z of the planning origin.
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
    let inner = (CELL_SIZE - TOWN_MARGIN * 2) as u64;
    let ox = TOWN_MARGIN + (hash % inner) as i32;
    let oz = TOWN_MARGIN + ((hash >> 32) % inner) as i32;
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

/// Towns whose footprint may touch chunk `(chunk_x, chunk_z)`.
#[must_use]
pub fn towns_touching_chunk(seed: u64, chunk_x: i32, chunk_z: i32) -> Vec<TownPos> {
    let min_x = chunk_x * 16;
    let min_z = chunk_z * 16;
    let max_x = min_x + 15;
    let max_z = min_z + 15;
    let mut towns = Vec::new();
    let pad = TOWN_INFLUENCE;
    let min_cell_x = (min_x - pad).div_euclid(CELL_SIZE);
    let max_cell_x = (max_x + pad).div_euclid(CELL_SIZE);
    let min_cell_z = (min_z - pad).div_euclid(CELL_SIZE);
    let max_cell_z = (max_z + pad).div_euclid(CELL_SIZE);
    let radius2 = i64::from(TOWN_INFLUENCE + 16) * i64::from(TOWN_INFLUENCE + 16);
    for cell_x in min_cell_x..=max_cell_x {
        for cell_z in min_cell_z..=max_cell_z {
            let town = town_in_cell(seed, cell_x, cell_z);
            let cx = min_x.midpoint(max_x);
            let cz = min_z.midpoint(max_z);
            if dist2(cx, cz, town.x, town.z) <= radius2 {
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
                let local_x = town.x - cell_x * CELL_SIZE;
                let local_z = town.z - cell_z * CELL_SIZE;
                assert!((TOWN_MARGIN..CELL_SIZE - TOWN_MARGIN).contains(&local_x));
                assert!((TOWN_MARGIN..CELL_SIZE - TOWN_MARGIN).contains(&local_z));
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
