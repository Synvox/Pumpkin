//! Seeded wave-function collapse for timber-frame facades.
//!
//! Tiles are 1×1 wall cells. Constraints are edge keys. Tie-break is lowest
//! tile id so a failed entropy pick is deterministic.

use crate::rng::TownRng;

/// A facade tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub id: u8,
    /// Left / right / down / up edge keys. Matching keys may touch.
    pub edges: [u8; 4],
    /// Ground-floor only.
    pub ground_only: bool,
    /// Not on the ground floor.
    pub upper_only: bool,
}

/// Tile set used for timber-frame houses (~36 tiles).
#[must_use]
pub fn timber_tiles() -> Vec<Tile> {
    // Edge keys: 0 plaster, 1 beam, 2 window, 3 door, 4 post
    const P: u8 = 0;
    const B: u8 = 1;
    const W: u8 = 2;
    const D: u8 = 3;
    const O: u8 = 4;
    let mut tiles = Vec::new();
    let mut add = |id: u8, l: u8, r: u8, d: u8, u: u8, ground: bool, upper: bool| {
        tiles.push(Tile {
            id,
            edges: [l, r, d, u],
            ground_only: ground,
            upper_only: upper,
        });
    };
    // plaster field
    add(0, P, P, P, P, false, false);
    add(1, P, P, P, P, false, false);
    add(2, P, P, B, P, false, false);
    add(3, P, P, P, B, false, false);
    // vertical beams
    add(4, P, B, B, B, false, false);
    add(5, B, P, B, B, false, false);
    add(6, B, B, B, B, false, false);
    add(7, P, B, P, P, false, false);
    add(8, B, P, P, P, false, false);
    // horizontal beams
    add(9, B, B, P, P, false, false);
    add(10, B, B, B, P, false, false);
    add(11, B, B, P, B, false, false);
    // cross
    add(12, B, B, B, B, false, false);
    add(13, P, B, B, P, false, false);
    add(14, B, P, B, P, false, false);
    // windows
    add(15, P, P, P, P, false, false);
    add(16, P, P, B, P, false, true);
    add(17, P, B, P, P, false, true);
    add(18, B, P, P, P, false, true);
    add(19, P, P, P, B, false, true);
    add(20, W, P, P, P, false, true);
    add(21, P, W, P, P, false, true);
    // doors (ground)
    add(22, P, P, D, P, true, false);
    add(23, P, D, D, P, true, false);
    add(24, D, P, D, P, true, false);
    add(25, P, P, P, D, true, false);
    // posts / corners
    add(26, O, P, B, B, false, false);
    add(27, P, O, B, B, false, false);
    add(28, O, O, B, B, false, false);
    add(29, B, B, O, P, false, false);
    // wattle / brick infill
    add(30, P, P, P, P, false, false);
    add(31, P, P, B, B, false, false);
    add(32, B, P, P, B, false, false);
    add(33, P, B, P, B, false, false);
    // shutters
    add(34, P, P, P, P, false, true);
    add(35, P, P, B, P, false, true);
    tiles
}

const LEFT: usize = 0;
const RIGHT: usize = 1;
const DOWN: usize = 2;
const UP: usize = 3;

fn compatible(a: &Tile, b: &Tile, dir: usize) -> bool {
    match dir {
        LEFT => a.edges[LEFT] == b.edges[RIGHT],
        RIGHT => a.edges[RIGHT] == b.edges[LEFT],
        DOWN => a.edges[DOWN] == b.edges[UP],
        UP => a.edges[UP] == b.edges[DOWN],
        _ => false,
    }
}

fn allowed_at(tile: &Tile, y: usize) -> bool {
    if tile.ground_only && y != 0 {
        return false;
    }
    if tile.upper_only && y == 0 {
        return false;
    }
    true
}

/// Collapses a `width × height` facade. `door_x` pins a door on the ground row.
#[must_use]
pub fn collapse_facade(
    rng: &mut TownRng,
    width: usize,
    height: usize,
    door_x: Option<usize>,
) -> Vec<u8> {
    let tiles = timber_tiles();
    for _ in 0..4 {
        if let Some(grid) = try_collapse(rng, &tiles, width, height, door_x) {
            return grid;
        }
    }
    fallback_facade(width, height, door_x)
}

fn fallback_facade(width: usize, height: usize, door_x: Option<usize>) -> Vec<u8> {
    let mut grid = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if y == 0 && door_x == Some(x) {
                grid[i] = 22;
            } else if x == 0 || x + 1 == width {
                grid[i] = 6;
            } else if y == height.saturating_sub(1) {
                grid[i] = 11;
            } else if (x + y) % 3 == 0 && y > 0 {
                grid[i] = 16;
            } else {
                grid[i] = 0;
            }
        }
    }
    grid
}

fn try_collapse(
    rng: &mut TownRng,
    tiles: &[Tile],
    width: usize,
    height: usize,
    door_x: Option<usize>,
) -> Option<Vec<u8>> {
    let n = width * height;
    let mut wave: Vec<u64> = vec![(1u64 << tiles.len()) - 1; n];
    for (i, tile) in tiles.iter().enumerate() {
        for y in 0..height {
            for x in 0..width {
                if !allowed_at(tile, y) {
                    wave[y * width + x] &= !(1u64 << i);
                }
            }
        }
    }
    if let Some(dx) = door_x
        && dx < width
    {
        let mut mask = 0u64;
        for (i, tile) in tiles.iter().enumerate() {
            if tile.id == 22 || tile.id == 23 || tile.id == 24 {
                mask |= 1u64 << i;
            }
        }
        wave[dx] = mask;
    }

    let mut collapsed = vec![None; n];
    for _ in 0..n {
        let Some(cell) = pick_cell(&wave, &collapsed, rng) else {
            break;
        };
        let options = bits(&wave[cell]);
        if options.is_empty() {
            return None;
        }
        let choice = *rng.choose(&options)?;
        collapsed[cell] = Some(choice);
        wave[cell] = 1u64 << choice;
        if !propagate(&mut wave, tiles, width, height, cell) {
            return None;
        }
    }
    if collapsed.iter().any(Option::is_none) {
        return None;
    }
    Some(
        collapsed
            .iter()
            .map(|id| tiles[id.unwrap_or(0)].id)
            .collect(),
    )
}

fn pick_cell(wave: &[u64], collapsed: &[Option<usize>], rng: &mut TownRng) -> Option<usize> {
    let mut best_entropy = u32::MAX;
    let mut candidates = Vec::new();
    for (i, mask) in wave.iter().enumerate() {
        if collapsed[i].is_some() {
            continue;
        }
        let e = mask.count_ones();
        if e == 0 {
            continue;
        }
        if e < best_entropy {
            best_entropy = e;
            candidates.clear();
            candidates.push(i);
        } else if e == best_entropy {
            candidates.push(i);
        }
    }
    if candidates.is_empty() {
        return None;
    }
    // Fixed tie-break: lowest index first, then a seeded shuffle of equals.
    candidates.sort_unstable();
    if candidates.len() == 1 {
        return candidates.first().copied();
    }
    rng.choose(&candidates).copied()
}

fn bits(mask: &u64) -> Vec<usize> {
    let mut out = Vec::new();
    for i in 0..64 {
        if mask & (1u64 << i) != 0 {
            out.push(i);
        }
    }
    out
}

fn propagate(wave: &mut [u64], tiles: &[Tile], width: usize, height: usize, start: usize) -> bool {
    let mut stack = vec![start];
    let dirs = [
        (LEFT, -1i32, 0i32),
        (RIGHT, 1, 0),
        (DOWN, 0, -1),
        (UP, 0, 1),
    ];
    while let Some(cell) = stack.pop() {
        let x = cell % width;
        let y = cell / width;
        for &(dir, dx, dy) in &dirs {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                continue;
            }
            let nidx = ny as usize * width + nx as usize;
            let before = wave[nidx];
            let mut next = 0u64;
            for ni in bits(&before) {
                let nt = &tiles[ni];
                let ok = bits(&wave[cell])
                    .iter()
                    .any(|&ci| compatible(&tiles[ci], nt, dir));
                if ok {
                    next |= 1u64 << ni;
                }
            }
            if next == 0 {
                return false;
            }
            if next != before {
                wave[nidx] = next;
                stack.push(nidx);
            }
        }
    }
    true
}

/// Interprets a WFC tile as a wall block class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallPart {
    Plaster,
    Beam,
    Window,
    Door,
    Post,
    Shutter,
}

/// Maps a collapsed tile id to a wall part.
#[must_use]
pub fn wall_part(id: u8) -> WallPart {
    match id {
        4..=14 | 29 | 32 | 33 => WallPart::Beam,
        15..=21 => WallPart::Window,
        22..=25 => WallPart::Door,
        26..=28 => WallPart::Post,
        34 | 35 => WallPart::Shutter,
        _ => WallPart::Plaster,
    }
}
