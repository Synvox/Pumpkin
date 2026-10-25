//! Coarse height field used by the planner.
//!
//! Samples come from vanilla's noise column sampler so the plan is a function of
//! seed + cell, not of which chunks have already been generated.

use std::sync::{Arc, OnceLock, RwLock, Weak};

use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::structure::structures::HeightSampler;

/// Spacing of planner height samples, in blocks.
pub const SAMPLE_STEP: i32 = 8;

static GENERATORS: OnceLock<RwLock<Vec<(u64, Weak<WorldGenerator>)>>> = OnceLock::new();

fn generators() -> &'static RwLock<Vec<(u64, Weak<WorldGenerator>)>> {
    GENERATORS.get_or_init(|| RwLock::new(Vec::new()))
}

/// Remembers a world generator so the decorator can sample heights later.
pub fn register_world_gen(world_gen: Arc<WorldGenerator>) {
    let seed = world_gen.seed();
    let weak = Arc::downgrade(&world_gen);
    let mut guard = generators()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.retain(|(s, existing)| *s != seed && existing.strong_count() > 0);
    guard.push((seed, weak));
}

/// Upgrades a previously registered generator for `seed`.
#[must_use]
pub fn world_gen_for(seed: u64) -> Option<Arc<WorldGenerator>> {
    let guard = generators()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .iter()
        .find(|(s, _)| *s == seed)
        .and_then(|(_, weak)| weak.upgrade())
}

/// Sampled heights on a regular XZ grid.
#[derive(Clone, Debug)]
pub struct HeightGrid {
    pub min_x: i32,
    pub min_z: i32,
    pub step: i32,
    pub nx: i32,
    pub nz: i32,
    pub heights: Vec<i32>,
    pub sea_level: i32,
}

impl HeightGrid {
    /// Samples `world_gen` on `[min_x, max_x] × [min_z, max_z]` every `step` blocks.
    #[must_use]
    pub fn sample(
        world_gen: &WorldGenerator,
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
        step: i32,
    ) -> Self {
        let step = step.max(1);
        let nx = ((max_x - min_x) / step) + 1;
        let nz = ((max_z - min_z) / step) + 1;
        let mut heights = vec![64; (nx * nz) as usize];
        let sea_level = match world_gen {
            WorldGenerator::Noise(vanilla) => {
                let mut sampler = vanilla.surface_height_sampler();
                for iz in 0..nz {
                    for ix in 0..nx {
                        let x = min_x + ix * step;
                        let z = min_z + iz * step;
                        heights[(iz * nx + ix) as usize] = sampler.estimate_height(x, z);
                    }
                }
                vanilla.settings.sea_level
            }
            _ => {
                heights.fill(64);
                63
            }
        };
        Self {
            min_x,
            min_z,
            step,
            nx,
            nz,
            heights,
            sea_level,
        }
    }

    /// Builds a synthetic sloping grid (tests).
    #[must_use]
    pub fn synthetic_slope(
        min_x: i32,
        min_z: i32,
        max_x: i32,
        max_z: i32,
        step: i32,
        base: i32,
        rise_x: i32,
        rise_z: i32,
    ) -> Self {
        let step = step.max(1);
        let nx = ((max_x - min_x) / step) + 1;
        let nz = ((max_z - min_z) / step) + 1;
        let mut heights = Vec::with_capacity((nx * nz) as usize);
        for iz in 0..nz {
            for ix in 0..nx {
                let y = base + ix * rise_x / nx.max(1) + iz * rise_z / nz.max(1);
                heights.push(y);
            }
        }
        Self {
            min_x,
            min_z,
            step,
            nx,
            nz,
            heights,
            sea_level: 63,
        }
    }

    fn index(&self, ix: i32, iz: i32) -> Option<usize> {
        if ix < 0 || iz < 0 || ix >= self.nx || iz >= self.nz {
            return None;
        }
        Some((iz * self.nx + ix) as usize)
    }

    /// Nearest sample, or `None` if outside the grid.
    #[must_use]
    pub fn sample_at(&self, x: i32, z: i32) -> Option<i32> {
        let ix = (x - self.min_x).div_euclid(self.step);
        let iz = (z - self.min_z).div_euclid(self.step);
        self.index(ix, iz).map(|i| self.heights[i])
    }

    /// Bilinear height at any block in range; clamps to the edge.
    #[must_use]
    pub fn height(&self, x: i32, z: i32) -> i32 {
        let fx = x.saturating_sub(self.min_x).max(0);
        let fz = z.saturating_sub(self.min_z).max(0);
        let ix0 = (fx / self.step).min(self.nx - 1);
        let iz0 = (fz / self.step).min(self.nz - 1);
        let ix1 = (ix0 + 1).min(self.nx - 1);
        let iz1 = (iz0 + 1).min(self.nz - 1);
        let tx = fx - ix0 * self.step;
        let tz = fz - iz0 * self.step;
        let h00 = self.heights[self.index(ix0, iz0).unwrap_or(0)];
        let h10 = self.heights[self.index(ix1, iz0).unwrap_or(0)];
        let h01 = self.heights[self.index(ix0, iz1).unwrap_or(0)];
        let h11 = self.heights[self.index(ix1, iz1).unwrap_or(0)];
        let hx0 = h00 * (self.step - tx) + h10 * tx;
        let hx1 = h01 * (self.step - tx) + h11 * tx;
        (hx0 * (self.step - tz) + hx1 * tz) / (self.step * self.step).max(1)
    }

    /// Mean height inside `rect`.
    #[must_use]
    pub fn mean(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> i32 {
        let mut sum = 0i64;
        let mut n = 0i64;
        let mut z = min_z;
        while z <= max_z {
            let mut x = min_x;
            while x <= max_x {
                sum += i64::from(self.height(x, z));
                n += 1;
                x += self.step;
            }
            z += self.step;
        }
        if n == 0 { 64 } else { (sum / n) as i32 }
    }

    /// Height range inside `rect`.
    #[must_use]
    pub fn range(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> (i32, i32) {
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        let mut z = min_z;
        while z <= max_z {
            let mut x = min_x;
            while x <= max_x {
                let h = self.height(x, z);
                lo = lo.min(h);
                hi = hi.max(h);
                x += self.step;
            }
            z += self.step;
        }
        if lo == i32::MAX { (64, 64) } else { (lo, hi) }
    }

    /// Dry enough to host a street or lot. One block of freeboard above sea.
    #[must_use]
    pub fn is_dry(&self, x: i32, z: i32) -> bool {
        self.height(x, z) > self.sea_level + 1
    }

    /// Share of samples in `rect` that are dry, as 0..=100.
    #[must_use]
    pub fn dry_ratio(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> i32 {
        let mut dry = 0i32;
        let mut n = 0i32;
        let mut z = min_z;
        while z <= max_z {
            let mut x = min_x;
            while x <= max_x {
                if self.is_dry(x, z) {
                    dry += 1;
                }
                n += 1;
                x += self.step;
            }
            z += self.step;
        }
        if n == 0 { 100 } else { dry * 100 / n }
    }

    /// Whether every corner and the center of `rect` is dry.
    #[must_use]
    pub fn rect_is_dry(&self, rect: crate::geom::Rect) -> bool {
        let (cx, cz) = rect.center();
        self.is_dry(rect.min_x, rect.min_z)
            && self.is_dry(rect.max_x, rect.min_z)
            && self.is_dry(rect.min_x, rect.max_z)
            && self.is_dry(rect.max_x, rect.max_z)
            && self.is_dry(cx, cz)
            && self.dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z) >= 85
    }

    /// Highest sample in `rect`.
    #[must_use]
    pub fn peak(&self, min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> (i32, i32, i32) {
        let mut best = (min_x, min_z, i32::MIN);
        let mut z = min_z;
        while z <= max_z {
            let mut x = min_x;
            while x <= max_x {
                let h = self.height(x, z);
                if h > best.2 || (h == best.2 && (x, z) < (best.0, best.1)) {
                    best = (x, z, h);
                }
                x += self.step;
            }
            z += self.step;
        }
        best
    }
}
