//! Shared flow fields for large fights.
//!
//! One BFS per destination (gate / keep) on a 4-block grid. Every combatant
//! then looks up a waypoint in O(1) instead of running its own path search.
//! Squad leaders are the only ones that spend [`crate::lod::PathBudget`] on a
//! live `navigator.set_progress`; followers steer along the same field.

use std::collections::VecDeque;

use crate::scene::{ScenePost, TownScene};

/// Coarse grid size in blocks.
pub const CELL: i32 = 4;
/// Direct steering range (blocks) when the destination is in sight.
pub const DIRECT_RANGE: f64 = 16.0;
/// Combatants that share a cached path.
pub const SQUAD_SIZE: u32 = 8;
/// How often a squad leader refreshes live navigation.
pub const LEADER_PATH_PERIOD: u32 = 12;
/// How often a follower refreshes live navigation.
pub const FOLLOWER_PATH_PERIOD: u32 = 24;
/// How often anyone refreshes when already close.
pub const CLOSE_PATH_PERIOD: u32 = 6;

const NORTH: u8 = 1;
const EAST: u8 = 2;
const SOUTH: u8 = 3;
const WEST: u8 = 4;

/// Whether this entity id is the cheap squad-leader slot (`(id-1) % 8 == 0`).
#[must_use]
pub const fn is_squad_leader(entity_id: i32) -> bool {
    entity_id.unsigned_abs().wrapping_sub(1) % SQUAD_SIZE == 0
}

/// Navigation refresh period for this combatant.
#[must_use]
pub const fn path_period(entity_id: i32, close: bool) -> u32 {
    if close {
        CLOSE_PATH_PERIOD
    } else if is_squad_leader(entity_id) {
        LEADER_PATH_PERIOD
    } else {
        FOLLOWER_PATH_PERIOD
    }
}

/// True when this tick should refresh live navigation.
#[must_use]
pub const fn should_refresh_path(entity_id: i32, age: i32, close: bool) -> bool {
    let period = path_period(entity_id, close) as i32;
    if period <= 0 {
        return true;
    }
    let id = entity_id.unsigned_abs() as i32;
    age.rem_euclid(period) == id.rem_euclid(period)
}

/// One destination's flow: each coarse cell stores a step toward `dest`.
#[derive(Clone, Debug)]
pub struct FlowField {
    min_x: i32,
    min_z: i32,
    w: i32,
    h: i32,
    dest_x: f64,
    dest_z: f64,
    next: Vec<u8>,
    blocked: Vec<u8>,
}

impl FlowField {
    /// BFS from `dest` over the town bounds plus camps, with wall cells blocked
    /// except at gates.
    #[must_use]
    pub fn toward(scene: &dyn TownScene, dest: ScenePost, extras: &[ScenePost]) -> Self {
        let mut min_x = scene.bounds().min_x;
        let mut min_z = scene.bounds().min_z;
        let mut max_x = scene.bounds().max_x;
        let mut max_z = scene.bounds().max_z;
        for p in extras
            .iter()
            .copied()
            .chain(scene.raid_camps().iter().copied())
        {
            min_x = min_x.min(p.x);
            min_z = min_z.min(p.z);
            max_x = max_x.max(p.x);
            max_z = max_z.max(p.z);
        }
        min_x = min_x.min(dest.x) - 32;
        min_z = min_z.min(dest.z) - 32;
        max_x = max_x.max(dest.x) + 32;
        max_z = max_z.max(dest.z) + 32;
        Self::build(min_x, min_z, max_x, max_z, dest, scene)
    }

    fn build(
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
        dest: ScenePost,
        scene: &dyn TownScene,
    ) -> Self {
        let min_x = min_x.div_euclid(CELL) * CELL;
        let min_z = min_z.div_euclid(CELL) * CELL;
        let w = ((max_x - min_x).div_euclid(CELL) + 1).max(1);
        let h = ((max_z - min_z).div_euclid(CELL) + 1).max(1);
        let n = (w * h) as usize;
        let mut field = Self {
            min_x,
            min_z,
            w,
            h,
            dest_x: f64::from(dest.x) + 0.5,
            dest_z: f64::from(dest.z) + 0.5,
            next: vec![0; n],
            blocked: vec![0; n],
        };
        field.raster_walls(scene);
        field.open_gates(scene);
        field.bfs(dest);
        field
    }

    fn raster_walls(&mut self, scene: &dyn TownScene) {
        let wall = scene.wall();
        if wall.len() < 2 {
            return;
        }
        for pair in wall.windows(2) {
            let a = pair[0];
            let b = pair[1];
            walk_line(a[0], a[1], b[0], b[1], |x, z| {
                self.mark_blocked_block(x, z);
                self.mark_blocked_block(x + 1, z);
                self.mark_blocked_block(x, z + 1);
            });
        }
        let first = wall[0];
        let last = wall[wall.len() - 1];
        walk_line(last[0], last[1], first[0], first[1], |x, z| {
            self.mark_blocked_block(x, z);
        });
    }

    fn open_gates(&mut self, scene: &dyn TownScene) {
        for g in scene.gates() {
            for dx in -8..=8 {
                for dz in -8..=8 {
                    if let Some(i) = self.idx_block(g.x + dx, g.z + dz) {
                        self.blocked[i] = 0;
                    }
                }
            }
        }
    }

    fn bfs(&mut self, dest: ScenePost) {
        let Some(start) = self.idx_block(dest.x, dest.z) else {
            return;
        };
        self.blocked[start] = 0;
        let mut seen = vec![false; self.next.len()];
        let mut q = VecDeque::new();
        q.push_back(start);
        seen[start] = true;
        while let Some(i) = q.pop_front() {
            let (cx, cz) = self.cell_of(i);
            for (bit, dx, dz) in [(EAST, 1, 0), (WEST, -1, 0), (SOUTH, 0, 1), (NORTH, 0, -1)] {
                let nx = cx + dx;
                let nz = cz + dz;
                let Some(ni) = self.idx_cell(nx, nz) else {
                    continue;
                };
                if seen[ni] || self.blocked[ni] != 0 {
                    continue;
                }
                seen[ni] = true;
                self.next[ni] = opposite(bit);
                q.push_back(ni);
            }
        }
        self.next[start] = 0;
    }

    fn mark_blocked_block(&mut self, x: i32, z: i32) {
        if let Some(i) = self.idx_block(x, z) {
            self.blocked[i] = 1;
        }
    }

    fn idx_block(&self, x: i32, z: i32) -> Option<usize> {
        let cx = (x - self.min_x).div_euclid(CELL);
        let cz = (z - self.min_z).div_euclid(CELL);
        self.idx_cell(cx, cz)
    }

    fn idx_cell(&self, cx: i32, cz: i32) -> Option<usize> {
        if cx < 0 || cz < 0 || cx >= self.w || cz >= self.h {
            return None;
        }
        Some((cz * self.w + cx) as usize)
    }

    fn cell_of(&self, i: usize) -> (i32, i32) {
        let i = i as i32;
        (i % self.w, i / self.w)
    }

    /// True when the coarse cell containing `(x, z)` is a wall.
    #[must_use]
    pub fn is_blocked(&self, x: i32, z: i32) -> bool {
        self.idx_block(x, z).is_some_and(|i| self.blocked[i] != 0)
    }

    /// Cheap LOS: no blocked cell on the line, or already inside `DIRECT_RANGE`.
    #[must_use]
    pub fn line_of_sight(&self, ax: f64, az: f64, bx: f64, bz: f64) -> bool {
        let dx = bx - ax;
        let dz = bz - az;
        if dx.hypot(dz) <= DIRECT_RANGE {
            let mut clear = true;
            walk_line(
                ax.floor() as i32,
                az.floor() as i32,
                bx.floor() as i32,
                bz.floor() as i32,
                |x, z| {
                    if self.is_blocked(x, z) {
                        clear = false;
                    }
                },
            );
            return clear;
        }
        false
    }

    /// Next waypoint along the field, or the destination itself when close /
    /// unreached.
    #[must_use]
    pub fn waypoint(&self, x: f64, z: f64) -> (f64, f64) {
        let dx = self.dest_x - x;
        let dz = self.dest_z - z;
        if dx.hypot(dz) <= DIRECT_RANGE {
            return (self.dest_x, self.dest_z);
        }
        let Some(i) = self.idx_block(x.floor() as i32, z.floor() as i32) else {
            return (self.dest_x, self.dest_z);
        };
        match self.next[i] {
            NORTH => (x, z - f64::from(CELL)),
            EAST => (x + f64::from(CELL), z),
            SOUTH => (x, z + f64::from(CELL)),
            WEST => (x - f64::from(CELL), z),
            _ => (self.dest_x, self.dest_z),
        }
    }

    /// Destination this field was built for.
    #[must_use]
    pub fn dest(&self) -> (f64, f64) {
        (self.dest_x, self.dest_z)
    }
}

const fn opposite(dir: u8) -> u8 {
    match dir {
        NORTH => SOUTH,
        SOUTH => NORTH,
        EAST => WEST,
        WEST => EAST,
        other => other,
    }
}

fn walk_line(x0: i32, z0: i32, x1: i32, z1: i32, mut visit: impl FnMut(i32, i32)) {
    let dx = (x1 - x0).abs();
    let dz = (z1 - z0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sz = if z0 < z1 { 1 } else { -1 };
    let mut err = dx - dz;
    let mut x = x0;
    let mut z = z0;
    loop {
        visit(x, z);
        if x == x1 && z == z1 {
            break;
        }
        let e2 = err * 2;
        if e2 > -dz {
            err -= dz;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            z += sz;
        }
    }
}

/// Unique destinations we bother caching for a town.
#[must_use]
pub fn cache_key(dest: ScenePost) -> (i32, i32) {
    (dest.x, dest.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::TownInfo;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::scene::TownInfoScene;
    use crate::town::town_in_cell;

    fn scene() -> TownInfoScene {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            70,
            8,
            4,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        TownInfoScene::from_info(TownInfo::from_plan(&plan))
    }

    #[test]
    fn field_pulls_toward_the_gate() {
        let scene = scene();
        let gate = scene.gates()[0];
        let dest = ScenePost::new(gate.x, gate.z);
        let camp = scene.raid_camps()[0];
        let field = FlowField::toward(&scene, dest, &[camp]);
        let (wx, wz) = field.waypoint(f64::from(camp.x), f64::from(camp.z));
        let before = dist(
            f64::from(camp.x),
            f64::from(camp.z),
            field.dest_x,
            field.dest_z,
        );
        let after = dist(wx, wz, field.dest_x, field.dest_z);
        assert!(
            after < before,
            "waypoint {wx},{wz} should be closer to gate than camp {camp:?} (before {before} after {after})"
        );
    }

    #[test]
    fn close_range_returns_the_destination() {
        let scene = scene();
        let keep = scene.keep();
        let field = FlowField::toward(&scene, keep, &[]);
        let (wx, wz) = field.waypoint(f64::from(keep.x) + 2.0, f64::from(keep.z) + 1.0);
        assert!((wx - field.dest_x).abs() < 0.01);
        assert!((wz - field.dest_z).abs() < 0.01);
    }

    #[test]
    fn squad_leader_slots() {
        assert!(is_squad_leader(1));
        assert!(!is_squad_leader(2));
        assert!(is_squad_leader(9));
        assert_eq!(path_period(1, false), LEADER_PATH_PERIOD);
        assert_eq!(path_period(2, false), FOLLOWER_PATH_PERIOD);
        assert_eq!(path_period(2, true), CLOSE_PATH_PERIOD);
    }

    fn dist(ax: f64, az: f64, bx: f64, bz: f64) -> f64 {
        (ax - bx).hypot(az - bz)
    }
}
