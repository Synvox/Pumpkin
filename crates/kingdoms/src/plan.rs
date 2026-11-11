//! City-level plan: castle site, walls, roads, lots, farms.
//!
//! Pure function of `(seed, cell, height grid, palette)`. Cached by the decorator.
//! The tick never generates a plan: it reads memory/disk and queues misses.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};

use pumpkin_data::biome::Biome;
use pumpkin_world::biome::BiomeSupplier;
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::noise::router::multi_noise_sampler::MultiNoiseSampler;
use serde::{Deserialize, Serialize};

use crate::buildings::{BuildingKind, Lot};
use crate::castle::{
    CASTLE_MIN, CASTLE_SIZE, CastlePlan, paint_castle, paint_keep_forecourt, site_castle,
};
use crate::cells::{CellKind, Occupancy};
use crate::geom::{Dir, Rect, bresenham};
use crate::height::{HeightGrid, SAMPLE_STEP, world_gen_for};
use crate::info::{GateInfo, GateKind, RoadInfo, TownInfo};
use crate::palette::PaletteKind;
use crate::rng::TownRng;
use crate::scenes::{
    Scene, SceneKind, TREE_STRIP_MARGIN, force_lot, paint_fish_market, paint_harbour_town,
    paint_scene_yards, paint_wind_sails, place_scenes, rect_hits_mill_sail, try_lot, water_body_ok,
};
use crate::town::{
    CELL_SIZE, TownPos, border_crossing, border_goal, town_containing, town_in_cell,
};

/// Pad around castle+town so hinterland farms, hamlets and camps fit.
const HINTERLAND_PAD: i32 = 120;

/// Disk snapshot format. Bump when plan or scene layout changes so files
/// written by older code are not reused.
pub const PLAN_CACHE_VERSION: u32 = 23;

#[derive(Serialize, Deserialize)]
struct PlanCacheFile {
    version: u32,
    info: TownInfo,
}

/// Cached plans, keyed by seed + cell.
fn cache() -> &'static RwLock<HashMap<(u64, i32, i32), Arc<TownPlan>>> {
    static CACHE: OnceLock<RwLock<HashMap<(u64, i32, i32), Arc<TownPlan>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Serializable snapshots so the director can tick before a full `TownPlan` is
/// rebuilt after a restart.
fn info_cache() -> &'static RwLock<HashMap<(u64, i32, i32), TownInfo>> {
    static CACHE: OnceLock<RwLock<HashMap<(u64, i32, i32), TownInfo>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn plans_dir() -> &'static RwLock<Option<PathBuf>> {
    static DIR: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(|| RwLock::new(None))
}

type CellKey = (u64, i32, i32);

struct PlanQueue {
    pending: HashSet<CellKey>,
    jobs: VecDeque<CellKey>,
}

impl PlanQueue {
    fn new() -> Self {
        Self {
            pending: HashSet::new(),
            jobs: VecDeque::new(),
        }
    }
}

fn queue() -> &'static (Mutex<PlanQueue>, Condvar) {
    static Q: OnceLock<(Mutex<PlanQueue>, Condvar)> = OnceLock::new();
    Q.get_or_init(|| (Mutex::new(PlanQueue::new()), Condvar::new()))
}

fn bind_plans_dir(dir: PathBuf) {
    *plans_dir()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(dir);
}

fn disk_path(seed: u64, cell_x: i32, cell_z: i32) -> Option<PathBuf> {
    let guard = plans_dir()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = guard.as_ref()?;
    Some(dir.join(format!("{seed}_{cell_x}_{cell_z}.json")))
}

fn persist_info(info: &TownInfo) {
    let Some(path) = disk_path(info.seed, info.cell[0], info.cell[1]) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = PlanCacheFile {
        version: PLAN_CACHE_VERSION,
        info: info.clone(),
    };
    let Ok(json) = serde_json::to_string_pretty(&file) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn parse_cache_file(bytes: &[u8], seed: u64) -> Option<TownInfo> {
    let file: PlanCacheFile = serde_json::from_slice(bytes).ok()?;
    if file.version != PLAN_CACHE_VERSION {
        return None;
    }
    if file.info.seed != seed {
        return None;
    }
    Some(file.info)
}

fn load_info_from_disk(seed: u64, cell_x: i32, cell_z: i32) -> Option<TownInfo> {
    let path = disk_path(seed, cell_x, cell_z)?;
    let bytes = std::fs::read(path).ok()?;
    let info = parse_cache_file(&bytes, seed)?;
    if info.cell != [cell_x, cell_z] {
        return None;
    }
    Some(info)
}

fn load_all_from_disk(seed: u64) {
    let dir = {
        let guard = plans_dir()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clone()
    };
    let Some(dir) = dir else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut loaded = 0u32;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Some(info) = parse_cache_file(&bytes, seed) else {
            continue;
        };
        let key = (info.seed, info.cell[0], info.cell[1]);
        info_cache()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, info);
        loaded += 1;
    }
    if loaded > 0 {
        tracing::info!(seed, loaded, "kingdoms loaded town-info cache");
    }
}

fn remember_plan(key: CellKey, plan: &Arc<TownPlan>) {
    let info = TownInfo::from_plan(plan);
    info_cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, info.clone());
    persist_info(&info);
}

fn ensure_worker() {
    static START: OnceLock<()> = OnceLock::new();
    START.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("kingdoms-plan".into())
            .spawn(plan_worker);
    });
}

fn plan_worker() {
    loop {
        let job = {
            let (lock, cond) = queue();
            let mut q = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(job) = q.jobs.pop_front() {
                    break job;
                }
                q = cond
                    .wait(q)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let (seed, cell_x, cell_z) = job;
        if cached_plan(seed, cell_x, cell_z).is_some() {
            finish_job(job);
            continue;
        }
        if let Some(world_gen) = world_gen_for(seed) {
            let _ = plan_for(seed, cell_x, cell_z, &world_gen);
            tracing::info!(seed, cell_x, cell_z, "kingdoms plan ready");
        }
        finish_job(job);
    }
}

fn warmup_set() -> &'static Mutex<HashSet<CellKey>> {
    static SET: OnceLock<Mutex<HashSet<CellKey>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

fn finish_job(job: CellKey) {
    let (lock, _) = queue();
    let mut q = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    q.pending.remove(&job);
    drop(q);
    let mut warmup = warmup_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warmup.remove(&job) && warmup.is_empty() {
        tracing::info!("kingdoms plan warmup done");
    }
}

/// Memory-only. Never generates.
#[must_use]
pub fn cached_plan(seed: u64, cell_x: i32, cell_z: i32) -> Option<Arc<TownPlan>> {
    cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(seed, cell_x, cell_z))
        .cloned()
}

/// Memory or disk `TownInfo`. Never generates a plan.
#[must_use]
pub fn cached_town_info(seed: u64, cell_x: i32, cell_z: i32) -> Option<TownInfo> {
    let cached = info_cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(seed, cell_x, cell_z))
        .cloned();
    if let Some(info) = cached {
        return Some(info);
    }
    if let Some(plan) = cached_plan(seed, cell_x, cell_z) {
        let info = TownInfo::from_plan(&plan);
        info_cache()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((seed, cell_x, cell_z), info.clone());
        return Some(info);
    }
    let info = load_info_from_disk(seed, cell_x, cell_z)?;
    info_cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((seed, cell_x, cell_z), info.clone());
    Some(info)
}

/// Snapshots already in the memory info cache for `seed`.
#[must_use]
pub fn cached_infos(seed: u64) -> Vec<TownInfo> {
    info_cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .values()
        .filter(|info| info.seed == seed)
        .cloned()
        .collect()
}

/// Queue background generation. No-op if the full plan is already in memory.
pub fn request_plan(seed: u64, cell_x: i32, cell_z: i32) {
    if cached_plan(seed, cell_x, cell_z).is_some() {
        finish_job((seed, cell_x, cell_z));
        return;
    }
    ensure_worker();
    let job = (seed, cell_x, cell_z);
    let (lock, cond) = queue();
    let mut q = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if q.pending.insert(job) {
        q.jobs.push_back(job);
        cond.notify_one();
    }
}

fn warmup_cells(seed: u64, spawn_x: i32, spawn_z: i32, extra: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let spawn = town_containing(seed, spawn_x, spawn_z);
    let mut cells = HashSet::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            cells.insert((spawn.cell_x + dx, spawn.cell_z + dz));
            cells.insert((dx, dz));
        }
    }
    for &(cx, cz) in extra {
        cells.insert((cx, cz));
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            cells.insert((cx + dx, cz + dz));
        }
    }
    let mut out: Vec<(i32, i32)> = cells.into_iter().collect();
    out.sort_unstable();
    out
}

/// Bind the world folder, load disk snapshots, and queue spawn/front cells.
pub fn bind_and_warmup(
    seed: u64,
    world_root: PathBuf,
    spawn_x: i32,
    spawn_z: i32,
    extra: &[(i32, i32)],
) {
    bind_plans_dir(
        world_root
            .join("kingdoms-plans")
            .join(format!("v{PLAN_CACHE_VERSION}")),
    );
    load_all_from_disk(seed);
    let cells = warmup_cells(seed, spawn_x, spawn_z, extra);
    tracing::info!(
        seed,
        spawn_x,
        spawn_z,
        cells = cells.len(),
        "kingdoms plan warmup queued"
    );
    let mut pending = Vec::new();
    for (cx, cz) in cells {
        if cached_plan(seed, cx, cz).is_none() {
            pending.push((seed, cx, cz));
        }
    }
    if pending.is_empty() {
        tracing::info!("kingdoms plan warmup done");
        return;
    }
    warmup_set()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(pending.iter().copied());
    ensure_worker();
    for (seed, cx, cz) in pending {
        request_plan(seed, cx, cz);
    }
}

#[cfg(test)]
pub(crate) fn bind_plans_dir_for_test(dir: PathBuf) {
    bind_plans_dir(dir);
}

#[cfg(test)]
pub(crate) fn remember_plan_for_test(plan: Arc<TownPlan>) {
    let key = (plan.seed, plan.origin.cell_x, plan.origin.cell_z);
    cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, Arc::clone(&plan));
    remember_plan(key, &plan);
}

#[cfg(test)]
pub(crate) fn forget_memory_for_test(seed: u64, cell_x: i32, cell_z: i32) {
    let key = (seed, cell_x, cell_z);
    cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&key);
    info_cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&key);
}

/// Full planned town.
#[derive(Clone, Debug)]
pub struct TownPlan {
    pub seed: u64,
    pub origin: TownPos,
    pub palette: PaletteKind,
    pub bounds: Rect,
    pub castle: CastlePlan,
    pub market: Rect,
    /// Walled town rectangle (houses, streets, civic square).
    pub town: Rect,
    pub well: (i32, i32),
    /// Town-wall vertices, clockwise, not necessarily closed.
    pub town_wall: Vec<(i32, i32)>,
    pub gates: Vec<GateInfo>,
    pub town_towers: Vec<Rect>,
    pub roads: Vec<RoadInfo>,
    pub lots: Vec<Lot>,
    pub scenes: Vec<Scene>,
    pub occ: Occupancy,
    /// Road columns outside hinterland occupancy, used to stitch cell borders.
    pub border_roads: HashMap<(i32, i32), CellKind>,
    pub heights: HeightGrid,
    /// Spawn cell (0,0) is the royal capital of the seed.
    pub capital: bool,
    /// Capital or a coastal megacity: two-ring curtain, pomerium, fosse.
    pub walled_city: bool,
}

impl TownPlan {
    /// Castle footprint in blocks. Must be at least 128×128.
    #[must_use]
    pub fn castle_footprint(&self) -> (i32, i32) {
        self.castle.footprint()
    }

    /// Scene covering `(x, z)`, if any.
    #[must_use]
    pub fn scene_at(&self, x: i32, z: i32) -> Option<Scene> {
        self.scenes
            .iter()
            .copied()
            .find(|scene| scene.rect.contains(x, z))
    }

    /// True inside any scene footprint plus [`crate::scenes::scene_tree_margin`].
    /// Frontier and ruin yards take extra so hanging Features canopies do
    /// not float over the close-up crop.
    #[must_use]
    pub fn in_scene_yard(&self, x: i32, z: i32) -> bool {
        self.scenes.iter().any(|scene| {
            scene
                .rect
                .inflate(crate::scenes::scene_tree_margin(scene.kind))
                .contains(x, z)
        })
    }

    /// True within [`crate::scenes::BUILDING_TREE_MARGIN`] of a roofed lot.
    #[must_use]
    pub fn near_building(&self, x: i32, z: i32) -> bool {
        self.lots.iter().any(|lot| {
            !matches!(lot.kind, crate::buildings::BuildingKind::Stall)
                && lot
                    .rect
                    .inflate(crate::scenes::BUILDING_TREE_MARGIN)
                    .contains(x, z)
        })
    }

    /// Columns where feature trees and floating leaves are stripped: every
    /// scene yard, the walled town plus curtain/fosse, the market crop, and a
    /// keepout around lots. Planned orchard and garden timber is kept at stamp.
    #[must_use]
    pub fn strips_trees(&self, x: i32, z: i32) -> bool {
        self.in_scene_yard(x, z)
            || self.town.inflate(TREE_STRIP_MARGIN).contains(x, z)
            || self.market.inflate(16).contains(x, z)
            || self.near_building(x, z)
            || matches!(
                self.occ.kind(x, z),
                CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::CityDitch
                    | CellKind::Plaza
                    | CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
            )
    }

    /// Stable FNV-1a of occupancy, lots and scenes. Tests pin this next to
    /// [`PLAN_CACHE_VERSION`] so a layout change cannot ship on a stale cache.
    #[must_use]
    pub fn layout_fingerprint(&self) -> u64 {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0100_0000_01b3;
        let mut hash = OFFSET;
        let mix = |hash: &mut u64, value: u64| {
            *hash ^= value;
            *hash = hash.wrapping_mul(PRIME);
        };
        let mix_i32 = |hash: &mut u64, value: i32| mix(hash, value as u32 as u64);
        mix(
            &mut hash,
            match self.palette {
                PaletteKind::Oak => 1,
                PaletteKind::Spruce => 2,
                PaletteKind::Sandstone => 3,
            },
        );
        mix_i32(&mut hash, self.origin.cell_x);
        mix_i32(&mut hash, self.origin.cell_z);
        mix_i32(&mut hash, self.bounds.min_x);
        mix_i32(&mut hash, self.bounds.min_z);
        mix_i32(&mut hash, self.bounds.max_x);
        mix_i32(&mut hash, self.bounds.max_z);
        mix_i32(&mut hash, self.town.min_x);
        mix_i32(&mut hash, self.town.min_z);
        mix_i32(&mut hash, self.town.max_x);
        mix_i32(&mut hash, self.town.max_z);
        mix_i32(&mut hash, self.market.min_x);
        mix_i32(&mut hash, self.market.min_z);
        mix_i32(&mut hash, self.market.max_x);
        mix_i32(&mut hash, self.market.max_z);
        mix(&mut hash, u64::from(self.walled_city));
        mix(&mut hash, self.lots.len() as u64);
        for lot in &self.lots {
            mix(&mut hash, lot.kind as u8 as u64);
            mix_i32(&mut hash, lot.rect.min_x);
            mix_i32(&mut hash, lot.rect.min_z);
            mix_i32(&mut hash, lot.rect.max_x);
            mix_i32(&mut hash, lot.rect.max_z);
        }
        mix(&mut hash, self.scenes.len() as u64);
        for scene in &self.scenes {
            mix(&mut hash, scene.kind as u8 as u64);
            mix_i32(&mut hash, scene.rect.min_x);
            mix_i32(&mut hash, scene.rect.min_z);
            mix_i32(&mut hash, scene.rect.max_x);
            mix_i32(&mut hash, scene.rect.max_z);
        }
        for z in self.bounds.min_z..=self.bounds.max_z {
            for x in self.bounds.min_x..=self.bounds.max_x {
                mix(&mut hash, u64::from(self.occ.kind(x, z) as u8));
            }
        }
        hash
    }
}

/// Returns the cached plan, building it from `world_gen` if needed.
#[must_use]
pub fn plan_for(seed: u64, cell_x: i32, cell_z: i32, world_gen: &WorldGenerator) -> Arc<TownPlan> {
    let key = (seed, cell_x, cell_z);
    if let Some(plan) = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return Arc::clone(plan);
    }
    let origin = town_in_cell(seed, cell_x, cell_z);
    let min_x = origin.cell_x * CELL_SIZE;
    let min_z = origin.cell_z * CELL_SIZE;
    let grid = HeightGrid::sample(
        world_gen,
        min_x,
        min_z,
        min_x + CELL_SIZE,
        min_z + CELL_SIZE,
        SAMPLE_STEP,
    );
    let palette = sample_palette(world_gen, origin.x, origin.z);
    let plan = Arc::new(build_plan(seed, origin, grid, palette));
    let mut write = cache()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    write.entry(key).or_insert_with(|| Arc::clone(&plan));
    let stored = Arc::clone(write.get(&key).unwrap_or(&plan));
    drop(write);
    remember_plan(key, &stored);
    stored
}

/// Builds or fetches a plan, using a registered generator when the cache misses.
///
/// Samples noise heights only. Does not generate or load chunks.
#[must_use]
pub fn plan_from_cache(seed: u64, cell_x: i32, cell_z: i32) -> Option<Arc<TownPlan>> {
    if let Some(plan) = cache()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(seed, cell_x, cell_z))
    {
        return Some(Arc::clone(plan));
    }
    let world_gen = world_gen_for(seed)?;
    Some(plan_for(seed, cell_x, cell_z, &world_gen))
}

/// Plan for the cell that contains `(block_x, block_z)`. Same as
/// [`plan_from_cache`] after mapping the column to its cell.
#[must_use]
pub fn plan_at(seed: u64, block_x: i32, block_z: i32) -> Option<Arc<TownPlan>> {
    let town = crate::town::town_containing(seed, block_x, block_z);
    plan_from_cache(seed, town.cell_x, town.cell_z)
}

/// Builds a plan from an already-sampled height grid (tests / pregen).
#[must_use]
pub fn build_plan(seed: u64, origin: TownPos, grid: HeightGrid, palette: PaletteKind) -> TownPlan {
    let mut rng = TownRng::salted(seed, origin.cell_x as u64, origin.cell_z as u64);
    let capital = origin.cell_x == 0 && origin.cell_z == 0;
    let castle_rect = site_castle(&grid, origin.x, origin.z, &mut rng, CASTLE_SIZE);
    let gate = downhill_town_dir(&grid, castle_rect);
    let inner_town = driest_town(&grid, castle_rect, gate, 132, 120);
    let coastal = !capital && town_faces_water(&grid, inner_town, castle_rect);
    let walled_city = capital || coastal;
    let town_rect = if walled_city {
        grow_lower_city(&grid, inner_town, castle_rect, gate)
    } else {
        inner_town
    };
    let core = union(castle_rect.inflate(6), town_rect.inflate(18));
    let hinter = union(castle_rect, town_rect).inflate(HINTERLAND_PAD);
    let bounds = clip_to_grid(&grid, union(core, hinter));
    let mut occ = Occupancy::new(bounds);
    let castle = paint_castle(&mut occ, &grid, castle_rect, &mut rng, capital);
    paint_keep_forecourt(
        &mut occ,
        castle.inner,
        castle.keep,
        castle.hall,
        &castle.keep_masses,
    );

    let market = flattest_dry(&grid, inner_town, 28, 28);
    occ.fill(market, CellKind::Market, false);
    let well = market.center();
    occ.set(well.0, well.1, CellKind::Well, true);
    for dz in -1..=1 {
        for dx in -1..=1 {
            occ.set(well.0 + dx, well.1 + dz, CellKind::Well, true);
        }
    }

    let (mut town_wall, mut town_towers) =
        paint_town_wall(&mut occ, &grid, inner_town, castle_rect, gate);
    if walled_city {
        let (outer_wall, outer_towers) =
            paint_city_curtain(&mut occ, &grid, town_rect, castle_rect, gate);
        town_wall.extend(outer_wall);
        town_towers.extend(outer_towers);
    }
    let mut gate_pts = town_gates(town_rect, castle.gatehouse.center(), gate);
    let mut extra_gates = Vec::new();
    if walled_city {
        extra_gates.push(wall_gate(inner_town, gate));
        extra_gates.extend(side_gates(town_rect, gate));
        gate_pts.extend(extra_gates.iter().copied());
    }
    let mut gates = vec![
        GateInfo {
            kind: GateKind::Town,
            x: gate_pts[0].0,
            z: gate_pts[0].1,
            facing: gate_facing_out(town_rect, gate_pts[0].0, gate_pts[0].1),
        },
        GateInfo {
            kind: GateKind::Castle,
            x: gate_pts[1].0,
            z: gate_pts[1].1,
            facing: castle.gate_dir,
        },
    ];
    for &pt in &extra_gates {
        let inner = pt == wall_gate(inner_town, gate);
        let facing = if inner {
            gate_facing_out(inner_town, pt.0, pt.1)
        } else {
            gate_facing_out(town_rect, pt.0, pt.1)
        };
        gates.push(GateInfo {
            kind: GateKind::Town,
            x: pt.0,
            z: pt.1,
            facing,
        });
    }
    for gate_info in &gates {
        if gate_info.kind == GateKind::Castle {
            for dz in -2..=2 {
                for dx in -2..=2 {
                    occ.set(gate_info.x + dx, gate_info.z + dz, CellKind::TownGate, true);
                }
            }
        } else {
            punch_gatehouse(&mut occ, gate_info.x, gate_info.z, gate_info.facing);
        }
    }
    paint_gate_plazas(&mut occ, &gates);
    paint_fountain_ring(&mut occ, well, market);

    let waypoints = vec![well, castle.gatehouse.center(), gate_pts[0]];
    let mut roads = paint_roads(&mut occ, &grid, &waypoints);
    if walled_city {
        let inner_gate = wall_gate(inner_town, gate);
        roads.extend(paint_roads(&mut occ, &grid, &[well, inner_gate]));
    }
    let mut border_roads = HashMap::new();
    paint_outbound_roads(
        &mut occ,
        &mut border_roads,
        &grid,
        origin,
        seed,
        well,
        &gates,
        &mut roads,
    );
    if walled_city {
        punch_road_gates(&mut occ, town_rect, &mut gates);
        paint_pomerium(&mut occ, town_rect);
        paint_city_ditch(&mut occ, town_rect, castle_rect, gate);
    }

    let mut lots = place_lots(
        &mut occ, &grid, inner_town, market, &mut rng, seed, palette, false,
    );
    paint_street_infill(
        &mut occ, &grid, &mut lots, seed, inner_town, market, palette, &mut rng, false,
    );
    paint_civic_square(
        &mut occ, &grid, &mut lots, seed, market, inner_town, palette,
    );
    paint_hedges_and_gardens(&mut occ, town_rect);
    place_farms(&mut occ, &grid, inner_town, castle_rect, &mut rng);
    let mut scenes = place_scenes(
        &mut occ,
        &grid,
        castle_rect,
        town_rect,
        origin,
        &mut lots,
        seed,
        &mut rng,
        palette,
        walled_city,
    );
    if walled_city {
        let mut extra_rng = TownRng::salted(seed, 21, origin.cell_z as u64);
        place_lots_in(
            &mut occ,
            &grid,
            town_rect,
            market,
            &mut extra_rng,
            seed,
            palette,
            &mut lots,
            110,
        );
        paint_street_infill(
            &mut occ,
            &grid,
            &mut lots,
            seed,
            town_rect,
            market,
            palette,
            &mut extra_rng,
            true,
        );
        place_farms(&mut occ, &grid, town_rect, castle_rect, &mut extra_rng);
        if capital {
            paint_capital_districts(
                &mut occ,
                &grid,
                &mut lots,
                seed,
                inner_town,
                town_rect,
                market,
                castle_rect,
                gate,
                palette,
                &mut extra_rng,
                &mut scenes,
            );
        }
    }
    scenes.push(Scene {
        kind: SceneKind::Plaza,
        rect: market,
    });
    paint_water_uses(&mut occ, &grid, bounds, castle_rect);
    paint_harbour_town(
        &mut occ,
        &grid,
        bounds,
        town_rect,
        &mut lots,
        seed,
        &mut rng,
        &mut scenes,
    );
    paint_fish_market(&mut occ, &grid, &mut lots, seed, town_rect);
    if walled_city {
        paint_winding_lanes(
            &mut occ,
            &grid,
            inner_town,
            town_rect,
            castle_rect,
            &mut TownRng::salted(seed, 21, 3),
            &mut roads,
        );
        punch_road_gates(&mut occ, town_rect, &mut gates);
        pack_jettied_city(
            &mut occ,
            &grid,
            &mut lots,
            seed,
            town_rect,
            market,
            palette,
            &mut TownRng::salted(seed, 22, 4),
        );
        paint_pomerium(&mut occ, town_rect);
        paint_street_life(
            &mut occ,
            &grid,
            &mut lots,
            seed,
            town_rect,
            &mut TownRng::salted(seed, 24, 6),
        );
        paint_fish_market(&mut occ, &grid, &mut lots, seed, town_rect);
        paint_extramural(
            &mut occ,
            &grid,
            &mut lots,
            seed,
            town_rect,
            gate,
            &mut TownRng::salted(seed, 23, 5),
            &mut scenes,
        );
        paint_wind_sails(&mut occ, &lots);
    }
    thicken_water_bridges(&mut occ, &grid);
    paint_scene_yards(&mut occ, &grid, &scenes);
    paint_poi_roads(&mut occ, &grid, &gates, &scenes, &mut roads);
    crate::scenes::paint_tree_keepout(&mut occ, &grid, &lots, market, town_rect);
    // Courtyard scenes and tree keepout run after `paint_castle`, so re-site
    // the plinth on leftover inner-bailey court, never on a keep spire.
    let mut castle = castle;
    castle.flag = crate::castle::site_flag_plinth(
        &mut occ,
        castle.inner,
        &castle.inner_ring.verts,
        castle.keep,
        castle.hall,
        castle.gatehouse,
        &castle.keep_masses,
    );

    debug_assert!(castle.bounds.width() >= CASTLE_MIN);
    debug_assert!(castle.bounds.depth() >= CASTLE_MIN);
    let _ = CASTLE_SIZE;

    TownPlan {
        seed,
        origin,
        palette,
        bounds,
        castle,
        market,
        town: town_rect,
        well,
        town_wall,
        gates,
        town_towers,
        roads,
        lots,
        scenes,
        occ,
        border_roads,
        heights: grid,
        capital,
        walled_city,
    }
}

fn sample_palette(world_gen: &WorldGenerator, x: i32, z: i32) -> PaletteKind {
    let WorldGenerator::Noise(vanilla) = world_gen else {
        return PaletteKind::Oak;
    };
    let mut sampler = MultiNoiseSampler::generate(&vanilla.base_router.multi_noise);
    let biome: &Biome = vanilla
        .biome_supplier
        .biome(x >> 2, 16, z >> 2, &mut sampler);
    PaletteKind::from_biome(biome)
}

fn downhill_town_dir(grid: &HeightGrid, castle: Rect) -> Dir {
    let (cx, cz) = castle.center();
    let dirs = [
        (
            Dir::South,
            grid.mean(cx - 40, castle.max_z + 8, cx + 40, castle.max_z + 80),
        ),
        (
            Dir::North,
            grid.mean(cx - 40, castle.min_z - 80, cx + 40, castle.min_z - 8),
        ),
        (
            Dir::East,
            grid.mean(castle.max_x + 8, cz - 40, castle.max_x + 80, cz + 40),
        ),
        (
            Dir::West,
            grid.mean(castle.min_x - 80, cz - 40, castle.min_x - 8, cz + 40),
        ),
    ];
    dirs.iter()
        .min_by_key(|(_, h)| *h)
        .map_or(Dir::South, |(d, _)| *d)
}

fn driest_town(grid: &HeightGrid, castle: Rect, dir: Dir, w: i32, d: i32) -> Rect {
    let mut best = town_beside(castle, dir, w, d);
    let mut best_score = i32::MIN;
    for search_dir in Dir::ALL {
        for &(tw, td) in &[(w, d), (108, 96), (88, 80), (72, 64)] {
            for side in -4..=4 {
                for pull in 0..8 {
                    let rect = town_offset(castle, search_dir, tw, td, side * 16, pull * 12);
                    let dry = grid.dry_ratio(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                    if dry < 55 && search_dir != dir {
                        continue;
                    }
                    let mean = grid.mean(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                    let prefer_downhill = i32::from(search_dir == dir) * 8;
                    let score = dry * 12 + (mean - grid.sea_level).clamp(0, 30) + prefer_downhill
                        - pull
                        - (w - tw) / 3;
                    if score > best_score {
                        best_score = score;
                        best = rect;
                    }
                }
            }
        }
    }
    shrink_to_dry(grid, best)
}

/// Expand the civic town downhill and sideways into a walled lower city.
/// Does not grow toward the castle, so the outer ring can attach to the
/// same uphill flank without a wall between keep and market.
fn grow_lower_city(grid: &HeightGrid, town: Rect, castle: Rect, dir: Dir) -> Rect {
    let grown = match dir {
        Dir::South => Rect::new(
            town.min_x - 12,
            town.min_z,
            town.max_x + 12,
            town.max_z + 56,
        ),
        Dir::North => Rect::new(
            town.min_x - 12,
            town.min_z - 56,
            town.max_x + 12,
            town.max_z,
        ),
        Dir::East => Rect::new(
            town.min_x,
            town.min_z - 12,
            town.max_x + 56,
            town.max_z + 12,
        ),
        Dir::West => Rect::new(
            town.min_x - 56,
            town.min_z - 12,
            town.max_x,
            town.max_z + 12,
        ),
    };
    let grown = clip_off_castle(grown, castle.inflate(4), dir);
    let grown = clip_to_grid(grid, grown);
    // Leave a west corridor so island villages and west harbours stay outside town.
    let grown = if grown.min_x < town.min_x - 12 {
        let shift = (town.min_x - 12) - grown.min_x;
        Rect::new(
            town.min_x - 12,
            grown.min_z,
            grown.max_x + shift,
            grown.max_z,
        )
    } else {
        grown
    };
    let shrunk = shrink_to_dry(grid, grown);
    if shrunk.width() > town.width() + 16 && shrunk.depth() > town.depth() + 12 {
        shrunk
    } else {
        grown
    }
}

fn clip_off_castle(town: Rect, castle: Rect, dir: Dir) -> Rect {
    if !town.intersects(castle) {
        return town;
    }
    match dir {
        Dir::South => Rect::new(
            town.min_x,
            castle.max_z.max(town.min_z),
            town.max_x,
            town.max_z,
        ),
        Dir::North => Rect::new(
            town.min_x,
            town.min_z,
            town.max_x,
            castle.min_z.min(town.max_z),
        ),
        Dir::East => Rect::new(
            castle.max_x.max(town.min_x),
            town.min_z,
            town.max_x,
            town.max_z,
        ),
        Dir::West => Rect::new(
            town.min_x,
            town.min_z,
            castle.min_x.min(town.max_x),
            town.max_z,
        ),
    }
}

fn shrink_to_dry(grid: &HeightGrid, rect: Rect) -> Rect {
    let mut current = rect;
    for _ in 0..10 {
        if grid.dry_ratio(current.min_x, current.min_z, current.max_x, current.max_z) >= 78 {
            return current;
        }
        if current.width() < 56 || current.depth() < 48 {
            return current;
        }
        let north = grid.dry_ratio(
            current.min_x,
            current.min_z,
            current.max_x,
            current.min_z + 8,
        );
        let south = grid.dry_ratio(
            current.min_x,
            current.max_z - 8,
            current.max_x,
            current.max_z,
        );
        let west = grid.dry_ratio(
            current.min_x,
            current.min_z,
            current.min_x + 8,
            current.max_z,
        );
        let east = grid.dry_ratio(
            current.max_x - 8,
            current.min_z,
            current.max_x,
            current.max_z,
        );
        let worst = north.min(south).min(west).min(east);
        current = if worst == north {
            Rect::new(
                current.min_x,
                current.min_z + 8,
                current.max_x,
                current.max_z,
            )
        } else if worst == south {
            Rect::new(
                current.min_x,
                current.min_z,
                current.max_x,
                current.max_z - 8,
            )
        } else if worst == west {
            Rect::new(
                current.min_x + 8,
                current.min_z,
                current.max_x,
                current.max_z,
            )
        } else {
            Rect::new(
                current.min_x,
                current.min_z,
                current.max_x - 8,
                current.max_z,
            )
        };
    }
    current
}

fn town_offset(castle: Rect, dir: Dir, w: i32, d: i32, slide: i32, shift: i32) -> Rect {
    match dir {
        Dir::South => Rect::new(
            castle.min_x - 16 + slide,
            castle.max_z - 8 + shift,
            castle.min_x - 16 + slide + w,
            castle.max_z - 8 + shift + d,
        ),
        Dir::North => Rect::new(
            castle.min_x - 16 + slide,
            castle.min_z - d + 8 - shift,
            castle.min_x - 16 + slide + w,
            castle.min_z + 8 - shift,
        ),
        Dir::East => Rect::new(
            castle.max_x - 8 + shift,
            castle.min_z - 16 + slide,
            castle.max_x - 8 + shift + w,
            castle.min_z - 16 + slide + d,
        ),
        Dir::West => Rect::new(
            castle.min_x - w + 8 - shift,
            castle.min_z - 16 + slide,
            castle.min_x + 8 - shift,
            castle.min_z - 16 + slide + d,
        ),
    }
}

fn town_beside(castle: Rect, dir: Dir, w: i32, d: i32) -> Rect {
    match dir {
        Dir::South => Rect::new(
            castle.min_x - 16,
            castle.max_z - 8,
            castle.min_x - 16 + w,
            castle.max_z - 8 + d,
        ),
        Dir::North => Rect::new(
            castle.min_x - 16,
            castle.min_z - d + 8,
            castle.min_x - 16 + w,
            castle.min_z + 8,
        ),
        Dir::East => Rect::new(
            castle.max_x - 8,
            castle.min_z - 16,
            castle.max_x - 8 + w,
            castle.min_z - 16 + d,
        ),
        Dir::West => Rect::new(
            castle.min_x - w + 8,
            castle.min_z - 16,
            castle.min_x + 8,
            castle.min_z - 16 + d,
        ),
    }
}

fn union(a: Rect, b: Rect) -> Rect {
    Rect::new(
        a.min_x.min(b.min_x),
        a.min_z.min(b.min_z),
        a.max_x.max(b.max_x),
        a.max_z.max(b.max_z),
    )
}

fn clip_to_grid(grid: &HeightGrid, rect: Rect) -> Rect {
    let max_x = grid.min_x + (grid.nx - 1) * grid.step;
    let max_z = grid.min_z + (grid.nz - 1) * grid.step;
    Rect::new(
        rect.min_x.max(grid.min_x),
        rect.min_z.max(grid.min_z),
        rect.max_x.min(max_x),
        rect.max_z.min(max_z),
    )
}

fn paint_gate_plazas(occ: &mut Occupancy, gates: &[GateInfo]) {
    for gate in gates {
        if gate.kind != GateKind::Town {
            continue;
        }
        for dz in -6i32..=6 {
            for dx in -6i32..=6 {
                if dx.abs() <= 2 && dz.abs() <= 2 {
                    continue;
                }
                let x = gate.x + dx;
                let z = gate.z + dz;
                if matches!(
                    occ.kind(x, z),
                    CellKind::Empty | CellKind::Farm | CellKind::FarmEdge | CellKind::Green
                ) {
                    occ.set(x, z, CellKind::Plaza, true);
                }
            }
        }
    }
}

fn paint_civic_square(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    market: Rect,
    town: Rect,
    palette: PaletteKind,
) {
    let cobble = market.inflate(3);
    for z in cobble.min_z..=cobble.max_z {
        for x in cobble.min_x..=cobble.max_x {
            if !town.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Empty | CellKind::Farm | CellKind::FarmEdge | CellKind::Green
            ) {
                occ.set(x, z, CellKind::Plaza, true);
            }
        }
    }
    let (mx, mz) = market.center();
    let hall = Rect::new(mx - 7, market.min_z - 13, mx + 6, market.min_z - 2);
    evict_lots(occ, lots, hall);
    if !civic_lot(
        occ,
        grid,
        lots,
        seed,
        BuildingKind::TownHall,
        hall,
        Dir::South,
        town,
    ) {
        let west = Rect::new(market.min_x - 14, mz - 6, market.min_x - 2, mz + 5);
        evict_lots(occ, lots, west);
        let _ = civic_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::TownHall,
            west,
            Dir::East,
            town,
        );
    }
    paint_market_frontage(occ, grid, lots, seed, market, town, palette);
    let mut stalls = 0;
    let mut z = market.min_z + 1;
    while z + 3 <= market.max_z && stalls < 18 {
        let mut x = market.min_x + 1;
        while x + 3 <= market.max_x && stalls < 18 {
            let rect = Rect::new(x, z, x + 3, z + 3);
            x += 5;
            if rect.inflate(2).contains(mx, mz) {
                continue;
            }
            if force_lot(occ, grid, lots, seed, BuildingKind::Stall, rect, Dir::South).is_some() {
                stalls += 1;
            }
        }
        z += 5;
    }
}

fn evict_lots(occ: &mut Occupancy, lots: &mut Vec<Lot>, rect: Rect) {
    let ids: Vec<u16> = lots
        .iter()
        .filter(|lot| lot.rect.intersects(rect) && lot.kind != BuildingKind::TownHall)
        .map(|lot| lot.id)
        .collect();
    if ids.is_empty() {
        return;
    }
    let gone: Vec<Rect> = lots
        .iter()
        .filter(|lot| ids.contains(&lot.id))
        .map(|lot| lot.rect)
        .collect();
    lots.retain(|lot| !ids.contains(&lot.id));
    for old in gone {
        for z in old.min_z..=old.max_z {
            for x in old.min_x..=old.max_x {
                occ.set(x, z, CellKind::Empty, true);
            }
        }
    }
}

fn civic_lot(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    kind: BuildingKind,
    rect: Rect,
    facing: Dir,
    town: Rect,
) -> bool {
    if !town.contains(rect.min_x, rect.min_z) || !town.contains(rect.max_x, rect.max_z) {
        return false;
    }
    force_lot(occ, grid, lots, seed, kind, rect, facing).is_some()
}

fn paint_market_frontage(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    market: Rect,
    town: Rect,
    palette: PaletteKind,
) {
    let house = if palette == PaletteKind::Sandstone {
        BuildingKind::Townhouse
    } else {
        BuildingKind::Timber
    };
    let (w, d) = house.size();
    let mut x = market.min_x;
    while x + w - 1 <= market.max_x {
        let north = Rect::new(x, market.min_z - d - 1, x + w - 1, market.min_z - 2);
        evict_lots(occ, lots, north);
        let _ = civic_lot(occ, grid, lots, seed, house, north, Dir::South, town);
        let south = Rect::new(x, market.max_z + 2, x + w - 1, market.max_z + d + 1);
        evict_lots(occ, lots, south);
        let _ = civic_lot(occ, grid, lots, seed, house, south, Dir::North, town);
        x += w + 1;
    }
    let mut z = market.min_z;
    while z + w - 1 <= market.max_z {
        let west = Rect::new(market.min_x - d - 1, z, market.min_x - 2, z + w - 1);
        evict_lots(occ, lots, west);
        let _ = civic_lot(occ, grid, lots, seed, house, west, Dir::East, town);
        let east = Rect::new(market.max_x + 2, z, market.max_x + d + 1, z + w - 1);
        evict_lots(occ, lots, east);
        let _ = civic_lot(occ, grid, lots, seed, house, east, Dir::West, town);
        z += w + 1;
    }
    // If a side sits against the town wall, drop a shallower row just inside
    // the square so the frontage still reads on every face.
    fill_inner_frontage(occ, grid, lots, seed, market, house, Dir::North);
    fill_inner_frontage(occ, grid, lots, seed, market, house, Dir::South);
    fill_inner_frontage(occ, grid, lots, seed, market, house, Dir::West);
    fill_inner_frontage(occ, grid, lots, seed, market, house, Dir::East);
}

fn fill_inner_frontage(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    market: Rect,
    house: BuildingKind,
    side: Dir,
) {
    if frontage_on_side(lots, market, side) {
        return;
    }
    let depth = 6;
    let along = 9;
    match side {
        Dir::North => {
            let mut x = market.min_x + 1;
            while x + along - 1 <= market.max_x {
                let rect = Rect::new(x, market.min_z, x + along - 1, market.min_z + depth - 1);
                let _ = force_lot(occ, grid, lots, seed, house, rect, Dir::South);
                x += along + 1;
            }
        }
        Dir::South => {
            let mut x = market.min_x + 1;
            while x + along - 1 <= market.max_x {
                let rect = Rect::new(x, market.max_z - depth + 1, x + along - 1, market.max_z);
                let _ = force_lot(occ, grid, lots, seed, house, rect, Dir::North);
                x += along + 1;
            }
        }
        Dir::West => {
            let mut z = market.min_z + 1;
            while z + along - 1 <= market.max_z {
                let rect = Rect::new(market.min_x, z, market.min_x + depth - 1, z + along - 1);
                let _ = force_lot(occ, grid, lots, seed, house, rect, Dir::East);
                z += along + 1;
            }
        }
        Dir::East => {
            let mut z = market.min_z + 1;
            while z + along - 1 <= market.max_z {
                let rect = Rect::new(market.max_x - depth + 1, z, market.max_x, z + along - 1);
                let _ = force_lot(occ, grid, lots, seed, house, rect, Dir::West);
                z += along + 1;
            }
        }
    }
}

fn frontage_on_side(lots: &[Lot], market: Rect, side: Dir) -> bool {
    lots.iter().any(|lot| {
        if !matches!(
            lot.kind,
            BuildingKind::Timber | BuildingKind::Townhouse | BuildingKind::TownHall
        ) {
            return false;
        }
        match side {
            Dir::North => {
                lot.rect.min_z <= market.min_z + 6
                    && lot.rect.max_z >= market.min_z - 16
                    && lot.rect.max_z <= market.min_z + 8
                    && lot.rect.max_x >= market.min_x
                    && lot.rect.min_x <= market.max_x
            }
            Dir::South => {
                lot.rect.max_z >= market.max_z - 6
                    && lot.rect.min_z <= market.max_z + 16
                    && lot.rect.min_z >= market.max_z - 8
                    && lot.rect.max_x >= market.min_x
                    && lot.rect.min_x <= market.max_x
            }
            Dir::West => {
                lot.rect.min_x <= market.min_x + 6
                    && lot.rect.max_x >= market.min_x - 16
                    && lot.rect.max_x <= market.min_x + 8
                    && lot.rect.max_z >= market.min_z
                    && lot.rect.min_z <= market.max_z
            }
            Dir::East => {
                lot.rect.max_x >= market.max_x - 6
                    && lot.rect.min_x <= market.max_x + 16
                    && lot.rect.min_x >= market.max_x - 8
                    && lot.rect.max_z >= market.min_z
                    && lot.rect.min_z <= market.max_z
            }
        }
    })
}

fn paint_hedges_and_gardens(occ: &mut Occupancy, town: Rect) {
    for z in town.min_z..=town.max_z {
        for x in town.min_x..=town.max_x {
            if occ.kind(x, z) != CellKind::Empty {
                continue;
            }
            let next_to_house = Dir::ALL
                .iter()
                .any(|dir| occ.kind(x + dir.dx(), z + dir.dz()) == CellKind::Building);
            if !next_to_house {
                continue;
            }
            let mix = (x.wrapping_mul(5) + z.wrapping_mul(11)).rem_euclid(6);
            if mix == 0 {
                occ.set(x, z, CellKind::Hedge, false);
            } else if mix <= 2 {
                occ.set(x, z, CellKind::Green, false);
            }
        }
    }
}

fn paint_fountain_ring(occ: &mut Occupancy, well: (i32, i32), market: Rect) {
    for dz in -4..=4 {
        for dx in -4..=4 {
            let x = well.0 + dx;
            let z = well.1 + dz;
            if !market.contains(x, z) {
                continue;
            }
            if dx.abs() <= 1 && dz.abs() <= 1 {
                continue;
            }
            let ring = dx.abs() + dz.abs() == 3 || (dx.abs() == 3 && dz.abs() == 3);
            if ring && matches!(occ.kind(x, z), CellKind::Market | CellKind::Empty) {
                occ.set(x, z, CellKind::Fountain, true);
            }
        }
    }
}

fn flattest_dry(grid: &HeightGrid, within: Rect, w: i32, d: i32) -> Rect {
    let mut best = Rect::new(
        within.min_x,
        within.min_z,
        within.min_x + w - 1,
        within.min_z + d - 1,
    );
    let mut best_range = i32::MAX;
    let mut z = within.min_z;
    while z + d <= within.max_z {
        let mut x = within.min_x;
        while x + w <= within.max_x {
            let (lo, hi) = grid.range(x, z, x + w - 1, z + d - 1);
            if lo <= grid.sea_level + 1 {
                x += 8;
                continue;
            }
            let r = hi - lo;
            if r < best_range || (r == best_range && (x, z) < (best.min_x, best.min_z)) {
                best_range = r;
                best = Rect::new(x, z, x + w - 1, z + d - 1);
            }
            x += 8;
        }
        z += 8;
    }
    best
}

/// Spacing between interval towers on the lower-city curtain.
const CITY_TOWER_SPACING: usize = 24;

/// True when the civic town or castle hinter sits on a real coast or a
/// large lake, so a non-capital harbour can still get the two-ring curtain.
/// `driest_town` pulls the civic rect inland, so a wet-fraction of the town
/// pad misses the sea; reuse the harbour lake/coast flood instead.
fn town_faces_water(grid: &HeightGrid, town: Rect, castle: Rect) -> bool {
    let search = union(town, castle).inflate(HINTERLAND_PAD);
    let mut z = search.min_z;
    while z <= search.max_z {
        let mut x = search.min_x;
        while x <= search.max_x {
            if !grid.is_dry(x, z) && water_body_ok(grid, search, x, z) {
                return true;
            }
            x += 24;
        }
        z += 24;
    }
    false
}

fn paint_street_life(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    rng: &mut TownRng,
) {
    let mut stalls = 0;
    let mut z = town.min_z + 4;
    while z + 3 <= town.max_z - 4 {
        let mut x = town.min_x + 4;
        while x + 3 <= town.max_x - 4 {
            if stalls >= 18 {
                break;
            }
            let sx = x + rng.gen_range(0, 2);
            let sz = z + rng.gen_range(0, 2);
            x += 8;
            if !matches!(
                occ.kind(sx, sz),
                CellKind::Road | CellKind::Plaza | CellKind::Empty | CellKind::Green
            ) {
                continue;
            }
            let facing = gate_facing_out(town, sx, sz);
            let rect = Rect::new(sx, sz, sx + 2, sz + 2);
            if try_lot(occ, grid, lots, seed, BuildingKind::Stall, rect, facing).is_some() {
                stalls += 1;
            }
        }
        z += 9;
    }
    let mut wells = 0;
    let mut z = town.min_z + 6;
    while z + 2 <= town.max_z - 6 {
        let mut x = town.min_x + 6;
        while x + 2 <= town.max_x - 6 {
            if wells >= 6 {
                break;
            }
            let wx = x;
            x += 11;
            if occ.kind(wx, z) != CellKind::Plaza {
                continue;
            }
            if Dir::ALL
                .iter()
                .any(|dir| occ.kind(wx + dir.dx(), z + dir.dz()) == CellKind::Well)
            {
                continue;
            }
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let kind = occ.kind(wx + dx, z + dz);
                    if matches!(kind, CellKind::Plaza | CellKind::Empty | CellKind::Green) {
                        occ.set(wx + dx, z + dz, CellKind::Well, true);
                    }
                }
            }
            wells += 1;
        }
        z += 13;
    }
}

fn paint_town_wall(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    castle: Rect,
    gate: Dir,
) -> (Vec<(i32, i32)>, Vec<Rect>) {
    let wall = town.inflate(2);
    let verts = [
        (wall.min_x, wall.min_z),
        (wall.max_x, wall.min_z),
        (wall.max_x, wall.max_z),
        (wall.min_x, wall.max_z),
    ];
    for i in 0..4 {
        let a = verts[i];
        let b = verts[(i + 1) % 4];
        // Skip the side that attaches to the castle.
        let skip = match gate {
            Dir::South => a.1 == wall.min_z && b.1 == wall.min_z,
            Dir::North => a.1 == wall.max_z && b.1 == wall.max_z,
            Dir::East => a.0 == wall.min_x && b.0 == wall.min_x,
            Dir::West => a.0 == wall.max_x && b.0 == wall.max_x,
        };
        if skip {
            continue;
        }
        let line: Vec<(i32, i32)> = bresenham(a.0, a.1, b.0, b.1)
            .into_iter()
            .filter(|&(x, z)| grid.is_dry(x, z))
            .collect();
        occ.stroke(&line, 1, CellKind::TownWall, false);
    }
    let mut towers = Vec::new();
    for &(x, z) in &verts {
        if !castle.contains(x, z) && grid.is_dry(x, z) {
            let tower = Rect::new(x - 2, z - 2, x + 2, z + 2);
            towers.push(tower);
            for dz in -2..=2 {
                for dx in -2..=2 {
                    occ.set(x + dx, z + dz, CellKind::TownTower, false);
                }
            }
        }
    }
    (verts.to_vec(), towers)
}

/// Continuous lower-city curtain: thick, contour-following, no dry-filter gaps,
/// interval towers every ~24 blocks. Castle-facing side stays open so the keep
/// attaches to the civic town.
fn paint_city_curtain(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    castle: Rect,
    gate: Dir,
) -> (Vec<(i32, i32)>, Vec<Rect>) {
    let wall = town.inflate(2);
    let verts = [
        (wall.min_x, wall.min_z),
        (wall.max_x, wall.min_z),
        (wall.max_x, wall.max_z),
        (wall.min_x, wall.max_z),
    ];
    let mut ring = Vec::new();
    for i in 0..4 {
        let a = verts[i];
        let b = verts[(i + 1) % 4];
        let skip = match gate {
            Dir::South => a.1 == wall.min_z && b.1 == wall.min_z,
            Dir::North => a.1 == wall.max_z && b.1 == wall.max_z,
            Dir::East => a.0 == wall.min_x && b.0 == wall.min_x,
            Dir::West => a.0 == wall.max_x && b.0 == wall.max_x,
        };
        if skip {
            continue;
        }
        let line = contour_edge(grid, a, b);
        occ.stroke_square(&line, 1, CellKind::TownWall, false);
        ring.extend(line);
    }
    let mut towers = Vec::new();
    let mut i = 0;
    while i < ring.len() {
        let (x, z) = ring[i];
        if !castle.contains(x, z) && !occ.kind(x, z).is_castle() {
            towers.push(place_round_city_tower(occ, x, z));
        }
        i += CITY_TOWER_SPACING;
    }
    (verts.to_vec(), towers)
}

fn place_round_city_tower(occ: &mut Occupancy, x: i32, z: i32) -> Rect {
    let tower = Rect::new(x - 3, z - 3, x + 3, z + 3);
    for dz in -3..=3 {
        for dx in -3..=3 {
            if dx * dx + dz * dz > 10 {
                continue;
            }
            if occ.kind(x + dx, z + dz).is_castle() {
                continue;
            }
            occ.set(x + dx, z + dz, CellKind::TownTower, true);
        }
    }
    tower
}

fn paint_pomerium(occ: &mut Occupancy, town: Rect) {
    for z in town.min_z..=town.max_z {
        for x in town.min_x..=town.max_x {
            let d = (x - town.min_x)
                .min(town.max_x - x)
                .min(z - town.min_z)
                .min(town.max_z - z);
            if d > 2 {
                continue;
            }
            let kind = occ.kind(x, z);
            if kind.is_castle()
                || matches!(
                    kind,
                    CellKind::Building
                        | CellKind::TownWall
                        | CellKind::TownTower
                        | CellKind::TownGate
                        | CellKind::Road
                        | CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW
                        | CellKind::Bridge
                        | CellKind::Market
                        | CellKind::Well
                        | CellKind::Fountain
                        | CellKind::CathedralNave
                        | CellKind::CathedralTransept
                        | CellKind::CathedralChoir
                        | CellKind::CathedralTower
                        | CellKind::CathedralButtress
                        | CellKind::Grave
                        | CellKind::Arcade
                        | CellKind::Lychgate
                        | CellKind::Quay
                        | CellKind::Crane
                        | CellKind::Dock
                        | CellKind::Ship
                        | CellKind::Boat
                )
            {
                continue;
            }
            occ.set(x, z, CellKind::Plaza, true);
        }
    }
}

fn paint_city_ditch(occ: &mut Occupancy, town: Rect, castle: Rect, gate: Dir) {
    let wall = town.inflate(2);
    for z in wall.min_z - 4..=wall.max_z + 4 {
        for x in wall.min_x - 4..=wall.max_x + 4 {
            let skip = match gate {
                Dir::South => z < wall.min_z,
                Dir::North => z > wall.max_z,
                Dir::East => x < wall.min_x,
                Dir::West => x > wall.max_x,
            };
            if skip {
                continue;
            }
            if castle.contains(x, z) || occ.kind(x, z).is_castle() {
                continue;
            }
            let dx = if x < wall.min_x {
                wall.min_x - x
            } else if x > wall.max_x {
                x - wall.max_x
            } else {
                0
            };
            let dz = if z < wall.min_z {
                wall.min_z - z
            } else if z > wall.max_z {
                z - wall.max_z
            } else {
                0
            };
            let out = dx.max(dz);
            if !(1..=4).contains(&out) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::TownWall
                    | CellKind::TownTower
                    | CellKind::TownGate
                    | CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
                    | CellKind::Bridge
                    | CellKind::Harbor
                    | CellKind::Quay
                    | CellKind::Dock
            ) {
                continue;
            }
            occ.set(x, z, CellKind::CityDitch, true);
        }
    }
}

fn contour_edge(grid: &HeightGrid, a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let dx = b.0 - a.0;
    let dz = b.1 - a.1;
    let len = dx.abs().max(dz.abs()).max(1);
    let along_x = dx.abs() >= dz.abs();
    let n = (len / 8).max(1);
    let mut samples = Vec::with_capacity((n as usize) + 1);
    for i in 0..=n {
        let x = a.0 + dx * i / n;
        let z = a.1 + dz * i / n;
        samples.push(contour_sample(grid, x, z, along_x));
    }
    let mut line = Vec::new();
    for window in samples.windows(2) {
        let mut seg = bresenham(window[0].0, window[0].1, window[1].0, window[1].1);
        if !line.is_empty() && !seg.is_empty() {
            seg.remove(0);
        }
        line.extend(seg);
    }
    line
}

fn contour_sample(grid: &HeightGrid, x: i32, z: i32, along_x: bool) -> (i32, i32) {
    let mut best = (x, z);
    let mut best_s = i32::MIN;
    for k in -5..=5 {
        let (nx, nz) = if along_x { (x, z + k) } else { (x + k, z) };
        let dry = i32::from(grid.is_dry(nx, nz)) * 24;
        let score = grid.height(nx, nz) + dry - k.abs();
        if score > best_s {
            best_s = score;
            best = (nx, nz);
        }
    }
    best
}

fn gate_facing_out(town: Rect, x: i32, z: i32) -> Dir {
    let wall = town.inflate(2);
    let dn = (z - wall.min_z).abs();
    let ds = (z - wall.max_z).abs();
    let dw = (x - wall.min_x).abs();
    let de = (x - wall.max_x).abs();
    let m = dn.min(ds).min(dw).min(de);
    if m == ds {
        Dir::South
    } else if m == dn {
        Dir::North
    } else if m == de {
        Dir::East
    } else {
        Dir::West
    }
}

fn punch_gatehouse(occ: &mut Occupancy, x: i32, z: i32, facing: Dir) {
    let (along_x, along_z) = match facing {
        Dir::North | Dir::South => (3, 2),
        Dir::East | Dir::West => (2, 3),
    };
    for dz in -along_z..=along_z {
        for dx in -along_x..=along_x {
            if occ.kind(x + dx, z + dz).is_castle() {
                continue;
            }
            occ.set(x + dx, z + dz, CellKind::TownGate, true);
        }
    }
    let (tx, tz) = match facing {
        Dir::North | Dir::South => (6, 0),
        Dir::East | Dir::West => (0, 6),
    };
    for &(ox, oz) in &[(tx, tz), (-tx, -tz)] {
        let _ = place_round_city_tower(occ, x + ox, z + oz);
    }
}

fn punch_road_gates(occ: &mut Occupancy, town: Rect, gates: &mut Vec<GateInfo>) {
    let wall = town.inflate(2);
    let band = wall.inflate(6);
    let mut hits = Vec::new();
    for z in band.min_z..=band.max_z {
        for x in band.min_x..=band.max_x {
            if !matches!(occ.kind(x, z), CellKind::TownWall | CellKind::TownTower) {
                continue;
            }
            let facing = gate_facing_out(town, x, z);
            let crosses = (1..=4)
                .any(|dist| is_road_kind(occ.kind(x + facing.dx() * dist, z + facing.dz() * dist)));
            if crosses {
                hits.push((x, z));
            }
        }
    }
    for &(x, z) in &hits {
        if gates
            .iter()
            .any(|g| (g.x - x).abs().max((g.z - z).abs()) < 16)
        {
            continue;
        }
        let facing = gate_facing_out(town, x, z);
        punch_gatehouse(occ, x, z, facing);
        gates.push(GateInfo {
            kind: GateKind::Town,
            x,
            z,
            facing,
        });
    }
}

fn town_gates(town: Rect, castle_gate: (i32, i32), dir: Dir) -> Vec<(i32, i32)> {
    vec![wall_gate(town, dir), castle_gate]
}

fn wall_gate(town: Rect, dir: Dir) -> (i32, i32) {
    let (cx, cz) = town.center();
    match dir {
        Dir::South => (cx, town.max_z + 2),
        Dir::North => (cx, town.min_z - 2),
        Dir::East => (town.max_x + 2, cz),
        Dir::West => (town.min_x - 2, cz),
    }
}

fn side_gates(town: Rect, dir: Dir) -> Vec<(i32, i32)> {
    let (cx, cz) = town.center();
    match dir {
        Dir::South | Dir::North => {
            vec![(town.min_x - 2, cz), (town.max_x + 2, cz)]
        }
        Dir::East | Dir::West => {
            vec![(cx, town.min_z - 2), (cx, town.max_z + 2)]
        }
    }
}

fn paint_roads(occ: &mut Occupancy, grid: &HeightGrid, waypoints: &[(i32, i32)]) -> Vec<RoadInfo> {
    let mut roads = Vec::new();
    let bounds = occ.bounds;
    let mut unused = HashMap::new();
    let mut push = |path: Vec<(i32, i32)>| {
        if let (Some(&first), Some(&last)) = (path.first(), path.last()) {
            paint_path(occ, &mut unused, grid, &path);
            roads.push(RoadInfo {
                from: [first.0, first.1],
                to: [last.0, last.1],
                points: path.iter().map(|&(x, z)| [x, z]).collect(),
            });
        }
    };
    for pair in waypoints.windows(2) {
        if let Some(path) = astar(grid, bounds, pair[0], pair[1]) {
            push(path);
        } else {
            push(bresenham(pair[0].0, pair[0].1, pair[1].0, pair[1].1));
        }
    }
    let (mx, mz) = waypoints[0];
    for dir in Dir::ALL {
        let end = (mx + dir.dx() * 40, mz + dir.dz() * 40);
        if bounds.contains(end.0, end.1)
            && let Some(path) = astar(grid, bounds, (mx, mz), end)
        {
            push(path);
        }
    }
    roads
}

fn paint_outbound_roads(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    origin: TownPos,
    seed: u64,
    well: (i32, i32),
    gates: &[GateInfo],
    roads: &mut Vec<RoadInfo>,
) {
    let start = gates
        .iter()
        .find(|gate| gate.kind == GateKind::Town)
        .map_or(well, |gate| (gate.x, gate.z));
    for dir in Dir::ALL {
        let neighbor = town_in_cell(seed, origin.cell_x + dir.dx(), origin.cell_z + dir.dz());
        let crossing = border_crossing(seed, origin, neighbor);
        let goal = border_goal(origin, neighbor, crossing);
        let clipped = clip_to_grid(grid, Rect::new(goal.0, goal.1, goal.0, goal.1));
        let route_goal = (clipped.min_x, clipped.min_z);
        if (route_goal.0 - start.0).abs() + (route_goal.1 - start.1).abs() < 28 {
            continue;
        }
        let search = clip_to_grid(
            grid,
            Rect::new(start.0, start.1, route_goal.0, route_goal.1).inflate(80),
        );
        let path = astar(grid, search, start, route_goal)
            .unwrap_or_else(|| bresenham(start.0, start.1, route_goal.0, route_goal.1));
        if let (Some(&first), Some(&last)) = (path.first(), path.last()) {
            paint_path(occ, overlay, grid, &path);
            stamp_road_cell(occ, overlay, grid, goal.0, goal.1, CellKind::Road);
            for dz in -1..=1 {
                for dx in -1..=1 {
                    stamp_road_cell(occ, overlay, grid, goal.0 + dx, goal.1 + dz, CellKind::Road);
                }
            }
            roads.push(RoadInfo {
                from: [first.0, first.1],
                to: [last.0, last.1],
                points: path.iter().map(|&(x, z)| [x, z]).collect(),
            });
        }
    }
}

fn paint_poi_roads(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    gates: &[GateInfo],
    scenes: &[Scene],
    roads: &mut Vec<RoadInfo>,
) {
    let mut overlay = HashMap::new();
    let kinds = [
        SceneKind::RoadsideInn,
        SceneKind::TollBridge,
        SceneKind::RuinedAbbey,
        SceneKind::CastleRuin,
        SceneKind::WalledMarket,
        SceneKind::MillHamlet,
        SceneKind::Beacon,
    ];
    for scene in scenes.iter().filter(|scene| kinds.contains(&scene.kind)) {
        let Some(start) = poi_road_anchor(occ, grid, scene) else {
            continue;
        };
        let Some(goal) = nearest_town_gate(gates, start) else {
            continue;
        };
        if (start.0 - goal.0).abs() + (start.1 - goal.1).abs() < 16 {
            continue;
        }
        let search = clip_to_grid(
            grid,
            Rect::new(start.0, start.1, goal.0, goal.1).inflate(96),
        );
        let path = astar(grid, search, start, goal)
            .unwrap_or_else(|| bresenham(start.0, start.1, goal.0, goal.1));
        let path: Vec<_> = path
            .into_iter()
            .filter(|&(x, z)| {
                occ.bounds.contains(x, z)
                    && !occ.kind(x, z).is_castle()
                    && !matches!(
                        occ.kind(x, z),
                        CellKind::Building
                            | CellKind::Palisade
                            | CellKind::PalisadeGate
                            | CellKind::Ruin
                            | CellKind::Ship
                            | CellKind::Harbor
                            | CellKind::Dock
                            | CellKind::Quay
                            | CellKind::Breakwater
                            | CellKind::Boat
                            | CellKind::Crane
                            | CellKind::Lighthouse
                    )
            })
            .collect();
        if path.len() < 8 {
            continue;
        }
        if let (Some(&first), Some(&last)) = (path.first(), path.last()) {
            paint_country_path(occ, &mut overlay, grid, &path);
            place_roadside_markers(occ, grid, &path);
            roads.push(RoadInfo {
                from: [first.0, first.1],
                to: [last.0, last.1],
                points: path.iter().map(|&(x, z)| [x, z]).collect(),
            });
        }
    }
}

fn poi_road_anchor(occ: &Occupancy, grid: &HeightGrid, scene: &Scene) -> Option<(i32, i32)> {
    let (cx, cz) = scene.rect.center();
    if grid.is_dry(cx, cz) && !occ.kind(cx, cz).is_castle() {
        return Some((cx, cz));
    }
    for z in scene.rect.min_z..=scene.rect.max_z {
        for x in scene.rect.min_x..=scene.rect.max_x {
            if grid.is_dry(x, z)
                && matches!(
                    occ.kind(x, z),
                    CellKind::Green
                        | CellKind::Plaza
                        | CellKind::Road
                        | CellKind::CampCourt
                        | CellKind::Eave
                        | CellKind::Beacon
                )
            {
                return Some((x, z));
            }
        }
    }
    None
}

fn nearest_town_gate(gates: &[GateInfo], start: (i32, i32)) -> Option<(i32, i32)> {
    gates
        .iter()
        .filter(|gate| gate.kind == GateKind::Town)
        .map(|gate| (gate.x, gate.z))
        .min_by_key(|&(x, z)| (x - start.0).abs() + (z - start.1).abs())
}

fn paint_country_path(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    path: &[(i32, i32)],
) {
    for window in path.windows(2) {
        let (x, z) = window[0];
        let (nx, nz) = window[1];
        let dh = grid.height(nx, nz) - grid.height(x, z);
        let stair = if dh.abs() >= 2 {
            if nx > x {
                CellKind::RoadStairE
            } else if nx < x {
                CellKind::RoadStairW
            } else if nz > z {
                CellKind::RoadStairS
            } else {
                CellKind::RoadStairN
            }
        } else {
            CellKind::Road
        };
        stamp_country_road_cell(occ, overlay, grid, x, z, stair);
        if !grid.is_dry(x, z) {
            continue;
        }
        for dir in Dir::ALL {
            let rx = x + dir.dx();
            let rz = z + dir.dz();
            if !grid.is_dry(rx, rz) {
                continue;
            }
            stamp_country_road_cell(occ, overlay, grid, rx, rz, CellKind::Road);
        }
    }
    if let Some(&(x, z)) = path.last() {
        stamp_country_road_cell(occ, overlay, grid, x, z, CellKind::Road);
    }
}

fn place_roadside_markers(occ: &mut Occupancy, grid: &HeightGrid, path: &[(i32, i32)]) {
    for (i, &(x, z)) in path.iter().enumerate() {
        if i % 42 != 21 {
            continue;
        }
        for dir in Dir::ALL {
            let nx = x + dir.dx() * 2;
            let nz = z + dir.dz() * 2;
            if !grid.is_dry(nx, nz) {
                continue;
            }
            let kind = occ.kind(nx, nz);
            if matches!(kind, CellKind::Empty | CellKind::Green | CellKind::Farm) {
                let high = grid.height(nx, nz) >= grid.height(x, z)
                    && Dir::ALL
                        .iter()
                        .all(|n| grid.height(nx + n.dx(), nz + n.dz()) <= grid.height(nx, nz));
                if high && i % 84 == 21 {
                    paint_roadside_beacon(occ, nx, nz);
                } else if i % 84 == 21 {
                    occ.set(nx, nz, CellKind::Shrine, true);
                } else {
                    occ.set(nx, nz, CellKind::FieldWall, true);
                }
                break;
            }
        }
    }
}

fn paint_roadside_beacon(occ: &mut Occupancy, x: i32, z: i32) {
    let pad = Rect::new(x - 1, z - 1, x + 1, z + 1);
    let mut clear = true;
    for zz in pad.min_z..=pad.max_z {
        for xx in pad.min_x..=pad.max_x {
            if !occ.bounds.contains(xx, zz) {
                clear = false;
                break;
            }
            if !matches!(
                occ.kind(xx, zz),
                CellKind::Empty | CellKind::Green | CellKind::Farm | CellKind::FarmEdge
            ) {
                clear = false;
                break;
            }
        }
    }
    if clear {
        occ.fill(pad, CellKind::Beacon, true);
    } else {
        occ.set(x, z, CellKind::Beacon, true);
    }
}

fn paint_path(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    path: &[(i32, i32)],
) {
    for window in path.windows(2) {
        let (x, z) = window[0];
        let (nx, nz) = window[1];
        let dh = grid.height(nx, nz) - grid.height(x, z);
        let stair = if dh.abs() >= 2 {
            if nx > x {
                CellKind::RoadStairE
            } else if nx < x {
                CellKind::RoadStairW
            } else if nz > z {
                CellKind::RoadStairS
            } else {
                CellKind::RoadStairN
            }
        } else {
            CellKind::Road
        };
        for dz in -1..=1 {
            for dx in -1..=1 {
                let kind = if dx == 0 && dz == 0 {
                    stair
                } else {
                    CellKind::Road
                };
                stamp_road_cell(occ, overlay, grid, x + dx, z + dz, kind);
            }
        }
    }
    if let Some(&(x, z)) = path.last() {
        stamp_road_cell(occ, overlay, grid, x, z, CellKind::Road);
        for dz in -1..=1 {
            for dx in -1..=1 {
                stamp_road_cell(occ, overlay, grid, x + dx, z + dz, CellKind::Road);
            }
        }
    }
}

fn paint_narrow_path(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    path: &[(i32, i32)],
) {
    for window in path.windows(2) {
        let (x, z) = window[0];
        let (nx, nz) = window[1];
        let dh = grid.height(nx, nz) - grid.height(x, z);
        let stair = if dh.abs() >= 2 {
            if nx > x {
                CellKind::RoadStairE
            } else if nx < x {
                CellKind::RoadStairW
            } else if nz > z {
                CellKind::RoadStairS
            } else {
                CellKind::RoadStairN
            }
        } else {
            CellKind::Road
        };
        stamp_road_cell(occ, overlay, grid, x, z, stair);
    }
    if let Some(&(x, z)) = path.last() {
        stamp_road_cell(occ, overlay, grid, x, z, CellKind::Road);
    }
}

fn paint_winding_lanes(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    inner: Rect,
    outer: Rect,
    castle: Rect,
    rng: &mut TownRng,
    roads: &mut Vec<RoadInfo>,
) {
    let mut unused = HashMap::new();
    let mut pts = Vec::new();
    let keep = castle.inflate(6);
    for _ in 0..14 {
        let x = rng.gen_range(outer.min_x + 6, outer.max_x - 5);
        let z = rng.gen_range(outer.min_z + 6, outer.max_z - 5);
        if keep.contains(x, z) || inner.deflate(8).contains(x, z) {
            continue;
        }
        if occ.kind(x, z).is_castle() {
            continue;
        }
        if !grid.is_dry(x, z) {
            continue;
        }
        pts.push((x, z));
    }
    for pair in pts.windows(2) {
        let path = astar(grid, outer, pair[0], pair[1])
            .unwrap_or_else(|| bresenham(pair[0].0, pair[0].1, pair[1].0, pair[1].1));
        let path: Vec<_> = path
            .into_iter()
            .filter(|&(x, z)| {
                outer.contains(x, z)
                    && grid.is_dry(x, z)
                    && !occ.kind(x, z).is_castle()
                    && !matches!(
                        occ.kind(x, z),
                        CellKind::Harbor
                            | CellKind::Ship
                            | CellKind::Boat
                            | CellKind::Dock
                            | CellKind::Quay
                            | CellKind::Breakwater
                    )
            })
            .collect();
        if path.len() < 2 {
            continue;
        }
        if let (Some(&first), Some(&last)) = (path.first(), path.last()) {
            paint_narrow_path(occ, &mut unused, grid, &path);
            roads.push(RoadInfo {
                from: [first.0, first.1],
                to: [last.0, last.1],
                points: path.iter().map(|&(x, z)| [x, z]).collect(),
            });
        }
    }
}

fn stamp_country_road_cell(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    x: i32,
    z: i32,
    kind: CellKind,
) {
    let kind = if !grid.is_dry(x, z)
        && matches!(
            kind,
            CellKind::Road
                | CellKind::RoadStairN
                | CellKind::RoadStairS
                | CellKind::RoadStairE
                | CellKind::RoadStairW
        ) {
        CellKind::Bridge
    } else {
        kind
    };
    if occ.bounds.contains(x, z) {
        let cell = occ.kind(x, z);
        if !cell.is_castle() && cell != CellKind::Well {
            occ.set(
                x,
                z,
                kind,
                matches!(
                    cell,
                    CellKind::Empty
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Plaza
                        | CellKind::Green
                ),
            );
        }
    } else {
        overlay.entry((x, z)).or_insert(kind);
    }
}

fn stamp_road_cell(
    occ: &mut Occupancy,
    overlay: &mut HashMap<(i32, i32), CellKind>,
    grid: &HeightGrid,
    x: i32,
    z: i32,
    kind: CellKind,
) {
    let kind = if !grid.is_dry(x, z)
        && matches!(
            kind,
            CellKind::Road
                | CellKind::RoadStairN
                | CellKind::RoadStairS
                | CellKind::RoadStairE
                | CellKind::RoadStairW
        ) {
        CellKind::Bridge
    } else {
        kind
    };
    if occ.bounds.contains(x, z) {
        let cell = occ.kind(x, z);
        if !cell.is_castle() && cell != CellKind::Well {
            occ.set(
                x,
                z,
                kind,
                matches!(
                    cell,
                    CellKind::Empty
                        | CellKind::Farm
                        | CellKind::FarmEdge
                        | CellKind::Market
                        | CellKind::Plaza
                        | CellKind::Green
                        | CellKind::Harbor
                ),
            );
        }
    } else {
        overlay.entry((x, z)).or_insert(kind);
    }
}

fn thicken_water_bridges(occ: &mut Occupancy, grid: &HeightGrid) {
    let bounds = occ.bounds;
    let mut seen = HashSet::new();
    let mut blobs = Vec::new();
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            if occ.kind(x, z) != CellKind::Bridge || !seen.insert((x, z)) {
                continue;
            }
            let mut cells = vec![(x, z)];
            let mut i = 0;
            while i < cells.len() {
                let (cx, cz) = cells[i];
                i += 1;
                for dir in Dir::ALL {
                    let nx = cx + dir.dx();
                    let nz = cz + dir.dz();
                    if occ.kind(nx, nz) == CellKind::Bridge && seen.insert((nx, nz)) {
                        cells.push((nx, nz));
                    }
                }
            }
            blobs.push(cells);
        }
    }
    for cells in blobs {
        let min_x = cells.iter().map(|c| c.0).min().unwrap_or(0);
        let max_x = cells.iter().map(|c| c.0).max().unwrap_or(0);
        let min_z = cells.iter().map(|c| c.1).min().unwrap_or(0);
        let max_z = cells.iter().map(|c| c.1).max().unwrap_or(0);
        let along_x = max_x - min_x >= max_z - min_z;
        let length = if along_x {
            max_x - min_x + 1
        } else {
            max_z - min_z + 1
        };
        if length < 6 {
            continue;
        }
        let half = if length >= 36 { 2 } else { 1 };
        for &(bx, bz) in &cells {
            for d in -half..=half {
                let (nx, nz) = if along_x { (bx, bz + d) } else { (bx + d, bz) };
                if !occ.bounds.contains(nx, nz) {
                    continue;
                }
                let kind = occ.kind(nx, nz);
                if kind.is_castle() {
                    continue;
                }
                let wet = !grid.is_dry(nx, nz)
                    || matches!(kind, CellKind::Harbor | CellKind::Dock | CellKind::Bridge);
                if wet
                    && matches!(
                        kind,
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Farm
                            | CellKind::FarmEdge
                            | CellKind::Harbor
                            | CellKind::Dock
                            | CellKind::Bridge
                            | CellKind::Hedge
                            | CellKind::Orchard
                            | CellKind::Pasture
                    )
                {
                    occ.set(nx, nz, CellKind::Bridge, true);
                }
            }
        }
        let dir = if along_x { Dir::East } else { Dir::South };
        for &(ex, ez, out) in &[(min_x, min_z, dir.opposite()), (max_x, max_z, dir)] {
            for step in 1..=4 {
                let nx = ex + out.dx() * step;
                let nz = ez + out.dz() * step;
                if !occ.bounds.contains(nx, nz) || !grid.is_dry(nx, nz) {
                    break;
                }
                for d in -1..=1 {
                    let (cx, cz) = if along_x { (nx, nz + d) } else { (nx + d, nz) };
                    if !occ.bounds.contains(cx, cz) {
                        continue;
                    }
                    if matches!(
                        occ.kind(cx, cz),
                        CellKind::Empty
                            | CellKind::Green
                            | CellKind::Farm
                            | CellKind::FarmEdge
                            | CellKind::Hedge
                    ) {
                        occ.set(cx, cz, CellKind::Road, true);
                    }
                }
            }
        }
        if length >= 28 {
            let mx = (min_x + max_x) / 2;
            let mz = (min_z + max_z) / 2;
            let pad = Rect::new(mx - 1, mz - 1, mx + 1, mz + 1);
            let mut free = true;
            for z in pad.min_z..=pad.max_z {
                for x in pad.min_x..=pad.max_x {
                    if occ.kind(x, z).is_castle()
                        || matches!(
                            occ.kind(x, z),
                            CellKind::Building | CellKind::TownWall | CellKind::TownGate
                        )
                    {
                        free = false;
                    }
                }
            }
            if free && occ.bounds.contains(pad.min_x, pad.min_z) {
                occ.fill(pad, CellKind::TownTower, true);
            }
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct Node {
    f: i32,
    x: i32,
    z: i32,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .cmp(&self.f)
            .then_with(|| self.x.cmp(&other.x))
            .then_with(|| self.z.cmp(&other.z))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn astar(
    grid: &HeightGrid,
    bounds: Rect,
    start: (i32, i32),
    goal: (i32, i32),
) -> Option<Vec<(i32, i32)>> {
    let step = 4;
    let snap = |v: i32| v / step * step;
    let start = (snap(start.0), snap(start.1));
    let goal = (snap(goal.0), snap(goal.1));
    let mut open = BinaryHeap::new();
    let mut came: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    let mut gscore: HashMap<(i32, i32), i32> = HashMap::new();
    open.push(Node {
        f: heur(start, goal),
        x: start.0,
        z: start.1,
    });
    gscore.insert(start, 0);
    let mut guard = 0u32;
    while let Some(Node { x, z, .. }) = open.pop() {
        guard += 1;
        if guard > 24_000 {
            break;
        }
        if (x, z) == goal {
            return Some(rebuild(&came, start, goal));
        }
        let g = *gscore.get(&(x, z)).unwrap_or(&i32::MAX);
        for dir in Dir::ALL {
            let nx = x + dir.dx() * step;
            let nz = z + dir.dz() * step;
            if !bounds.inflate(8).contains(nx, nz) {
                continue;
            }
            let dh = (grid.height(nx, nz) - grid.height(x, z)).abs();
            let slope = dh * 100 / step;
            // 1:4 is 25. Penalize steeper; allow stairs.
            let mut cost = step + slope.max(0);
            if slope > 25 {
                cost += (slope - 25) * 4;
            }
            if slope > 50 {
                cost += 40;
            }
            if grid.height(nx, nz) <= grid.sea_level + 1 {
                cost += 360;
            }
            let tentative = g.saturating_add(cost);
            if tentative < *gscore.get(&(nx, nz)).unwrap_or(&i32::MAX) {
                came.insert((nx, nz), (x, z));
                gscore.insert((nx, nz), tentative);
                open.push(Node {
                    f: tentative + heur((nx, nz), goal),
                    x: nx,
                    z: nz,
                });
            }
        }
    }
    None
}

fn heur(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs() + (a.1 - b.1).abs()
}

fn rebuild(
    came: &HashMap<(i32, i32), (i32, i32)>,
    start: (i32, i32),
    goal: (i32, i32),
) -> Vec<(i32, i32)> {
    let mut cur = goal;
    let mut points = vec![cur];
    while cur != start {
        if let Some(&prev) = came.get(&cur) {
            let line = bresenham(prev.0, prev.1, cur.0, cur.1);
            points.extend(line.into_iter().rev().skip(1));
            cur = prev;
        } else {
            break;
        }
    }
    points.reverse();
    points
}

fn place_lots(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    market: Rect,
    rng: &mut TownRng,
    seed: u64,
    palette: PaletteKind,
    capital: bool,
) -> Vec<Lot> {
    let mut lots = Vec::new();
    let cap = if capital { 110 } else { 64 };
    place_lots_in(occ, grid, town, market, rng, seed, palette, &mut lots, cap);
    lots
}

fn place_lots_in(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    market: Rect,
    rng: &mut TownRng,
    seed: u64,
    palette: PaletteKind,
    lots: &mut Vec<Lot>,
    cap: usize,
) {
    let kinds = lot_kinds(palette);
    let mut id = lots.iter().map(|lot| lot.id).max().unwrap_or(0) + 1;
    for z in (town.min_z + 5..town.max_z - 5).step_by(7) {
        for x in (town.min_x + 5..town.max_x - 5).step_by(6) {
            let jx = x + rng.gen_range(-2, 3);
            let jz = z + rng.gen_range(-2, 3);
            if market.inflate(3).contains(jx, jz) {
                continue;
            }
            let Some(&kind) = rng.choose(kinds) else {
                continue;
            };
            let (mut w, mut d) = kind.size();
            w += rng.gen_range(0, 2);
            d += rng.gen_range(0, 2);
            if rng.chance(1, 3) {
                std::mem::swap(&mut w, &mut d);
            }
            let rect = Rect::new(jx, jz, jx + w - 1, jz + d - 1);
            if !occ.is_free(rect.inflate(1)) || !town.contains(rect.max_x, rect.max_z) {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            if rect_hits_keep_court(occ, rect) {
                continue;
            }
            if rect_hits_mill_sail(lots, rect) {
                continue;
            }
            let facing = facing_toward_road(occ, rect)
                .unwrap_or(*rng.choose(&Dir::ALL).unwrap_or(&Dir::South));
            let lot = Lot::new(id, kind, rect, facing, seed ^ (u64::from(id) << 8));
            occ.fill_building(rect, id, false);
            paint_eaves(occ, rect);
            lots.push(lot);
            id += 1;
            if lots.len() >= cap {
                return;
            }
        }
    }
}

fn paint_street_infill(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    market: Rect,
    palette: PaletteKind,
    rng: &mut TownRng,
    capital: bool,
) {
    let kinds: &[BuildingKind] = match palette {
        PaletteKind::Sandstone => &[
            BuildingKind::Cottage,
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Blacksmith,
            BuildingKind::Tavern,
            BuildingKind::Cottage,
        ],
        PaletteKind::Spruce => &[
            BuildingKind::Cottage,
            BuildingKind::Longhouse,
            BuildingKind::Timber,
            BuildingKind::Blacksmith,
            BuildingKind::Tavern,
            BuildingKind::Cottage,
        ],
        PaletteKind::Oak => &[
            BuildingKind::Cottage,
            BuildingKind::Timber,
            BuildingKind::Townhouse,
            BuildingKind::Blacksmith,
            BuildingKind::Tavern,
            BuildingKind::Cottage,
        ],
    };
    let cap = if capital { 44 } else { 28 };
    let mut extra = 0;
    let mut z = town.min_z + 4;
    while z <= town.max_z - 4 && extra < cap {
        let mut x = town.min_x + 4;
        while x <= town.max_x - 4 && extra < cap {
            if !matches!(
                occ.kind(x, z),
                CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
            ) {
                x += 5;
                continue;
            }
            let Some(&kind) = rng.choose(kinds) else {
                x += 5;
                continue;
            };
            let (w, d) = kind.size();
            for dir in Dir::ALL {
                let ox = x + dir.dx() * (w / 2 + 2);
                let oz = z + dir.dz() * (d / 2 + 2);
                let rect = Rect::new(
                    ox - w / 2,
                    oz - d / 2,
                    ox - w / 2 + w - 1,
                    oz - d / 2 + d - 1,
                );
                if market.inflate(3).intersects(rect)
                    || !town.contains(rect.min_x, rect.min_z)
                    || !town.contains(rect.max_x, rect.max_z)
                {
                    continue;
                }
                let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
                if hi.saturating_sub(lo) > 6 {
                    continue;
                }
                if try_lot(occ, grid, lots, seed, kind, rect, dir.opposite()).is_some() {
                    extra += 1;
                    break;
                }
            }
            x += 5;
        }
        z += 5;
    }
}

fn lot_kinds(palette: PaletteKind) -> &'static [BuildingKind] {
    match palette {
        PaletteKind::Oak => &[
            BuildingKind::Church,
            BuildingKind::Tavern,
            BuildingKind::Blacksmith,
            BuildingKind::Townhouse,
            BuildingKind::Timber,
            BuildingKind::Timber,
            BuildingKind::Townhouse,
            BuildingKind::Longhouse,
            BuildingKind::Cottage,
            BuildingKind::Cottage,
            BuildingKind::Townhouse,
            BuildingKind::Barn,
            BuildingKind::Stall,
            BuildingKind::Stall,
            BuildingKind::Stall,
        ],
        PaletteKind::Spruce => &[
            BuildingKind::Church,
            BuildingKind::Tavern,
            BuildingKind::Blacksmith,
            BuildingKind::Longhouse,
            BuildingKind::Longhouse,
            BuildingKind::Timber,
            BuildingKind::Townhouse,
            BuildingKind::Longhouse,
            BuildingKind::Cottage,
            BuildingKind::Cottage,
            BuildingKind::Timber,
            BuildingKind::Barn,
            BuildingKind::Stall,
            BuildingKind::Stall,
            BuildingKind::Stall,
        ],
        PaletteKind::Sandstone => &[
            BuildingKind::Church,
            BuildingKind::Tavern,
            BuildingKind::Blacksmith,
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Cottage,
            BuildingKind::Cottage,
            BuildingKind::Cottage,
            BuildingKind::Townhouse,
            BuildingKind::Barn,
            BuildingKind::Stall,
            BuildingKind::Stall,
            BuildingKind::Stall,
        ],
    }
}

fn paint_eaves(occ: &mut Occupancy, rect: Rect) {
    for z in rect.min_z - 1..=rect.max_z + 1 {
        for x in rect.min_x - 1..=rect.max_x + 1 {
            if rect.contains(x, z) {
                continue;
            }
            if matches!(
                occ.kind(x, z),
                CellKind::Empty | CellKind::Plaza | CellKind::Green
            ) {
                occ.set(x, z, CellKind::Eave, true);
            }
        }
    }
}

fn rect_hits_keep_court(occ: &Occupancy, rect: Rect) -> bool {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if occ.kind(x, z).is_castle() {
                return true;
            }
        }
    }
    false
}

fn crafts_row(inner: Rect, outer: Rect, gate: Dir) -> Rect {
    match gate {
        Dir::South => Rect::new(
            inner.min_x + 4,
            inner.max_z - 24,
            inner.max_x - 4,
            outer.max_z - 6,
        ),
        Dir::North => Rect::new(
            inner.min_x + 4,
            outer.min_z + 6,
            inner.max_x - 4,
            inner.min_z + 24,
        ),
        Dir::East => Rect::new(
            inner.max_x - 24,
            inner.min_z + 4,
            outer.max_x - 6,
            inner.max_z - 4,
        ),
        Dir::West => Rect::new(
            outer.min_x + 6,
            inner.min_z + 4,
            inner.min_x + 24,
            inner.max_z - 4,
        ),
    }
}

fn uphill_band(inner: Rect, gate: Dir) -> Rect {
    match gate {
        Dir::South => Rect::new(
            inner.min_x + 4,
            inner.min_z + 2,
            inner.max_x - 4,
            inner.min_z + inner.depth() / 3,
        ),
        Dir::North => Rect::new(
            inner.min_x + 4,
            inner.max_z - inner.depth() / 3,
            inner.max_x - 4,
            inner.max_z - 2,
        ),
        Dir::East => Rect::new(
            inner.min_x + 2,
            inner.min_z + 4,
            inner.min_x + inner.width() / 3,
            inner.max_z - 4,
        ),
        Dir::West => Rect::new(
            inner.max_x - inner.width() / 3,
            inner.min_z + 4,
            inner.max_x - 2,
            inner.max_z - 4,
        ),
    }
}

fn downhill_band(outer: Rect, gate: Dir, depth: i32) -> Rect {
    match gate {
        Dir::South => Rect::new(
            outer.min_x + 6,
            outer.max_z - depth,
            outer.max_x - 6,
            outer.max_z - 4,
        ),
        Dir::North => Rect::new(
            outer.min_x + 6,
            outer.min_z + 4,
            outer.max_x - 6,
            outer.min_z + depth,
        ),
        Dir::East => Rect::new(
            outer.max_x - depth,
            outer.min_z + 6,
            outer.max_x - 4,
            outer.max_z - 6,
        ),
        Dir::West => Rect::new(
            outer.min_x + 4,
            outer.min_z + 6,
            outer.min_x + depth,
            outer.max_z - 6,
        ),
    }
}

fn pack_district(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    rect: Rect,
    kinds: &[BuildingKind],
    rng: &mut TownRng,
    cap: usize,
) {
    if rect.width() < 8 || rect.depth() < 8 {
        return;
    }
    let mut placed = 0;
    let mut z = rect.min_z + 2;
    while z <= rect.max_z - 2 && placed < cap {
        let mut x = rect.min_x + 2;
        while x <= rect.max_x - 2 && placed < cap {
            let Some(&kind) = rng.choose(kinds) else {
                x += 6;
                continue;
            };
            let (w, d) = kind.size();
            let lot_rect = Rect::new(x, z, x + w - 1, z + d - 1);
            x += w.max(6);
            if !rect.contains(lot_rect.max_x, lot_rect.max_z) {
                continue;
            }
            if rect_hits_keep_court(occ, lot_rect)
                || matches!(occ.kind(lot_rect.min_x, lot_rect.min_z), CellKind::WindSail)
            {
                continue;
            }
            let facing = facing_toward_road(occ, lot_rect).unwrap_or(Dir::South);
            if try_lot(occ, grid, lots, seed, kind, lot_rect, facing).is_some() {
                placed += 1;
            }
        }
        z += 8;
    }
}

fn paint_noble_gardens(occ: &mut Occupancy, rect: Rect) {
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if occ.kind(x, z) != CellKind::Empty {
                continue;
            }
            let mix = (x.wrapping_mul(7) + z.wrapping_mul(13)).rem_euclid(5);
            if mix == 0 {
                occ.set(x, z, CellKind::Hedge, false);
            } else {
                occ.set(x, z, CellKind::Green, false);
            }
        }
    }
}

fn paint_capital_districts(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    inner: Rect,
    outer: Rect,
    market: Rect,
    castle: Rect,
    gate: Dir,
    palette: PaletteKind,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let market_q = Rect::new(
        market.min_x - 8,
        market.min_z - 8,
        market.max_x + 8,
        market.max_z + 8,
    );
    scenes.push(Scene {
        kind: SceneKind::MarketQuarter,
        rect: market_q,
    });

    let crafts = crafts_row(inner, outer, gate);
    let crafts_kinds: &[BuildingKind] = match palette {
        PaletteKind::Sandstone => &[
            BuildingKind::Blacksmith,
            BuildingKind::Barn,
            BuildingKind::Cottage,
            BuildingKind::Townhouse,
        ],
        _ => &[
            BuildingKind::Blacksmith,
            BuildingKind::Blacksmith,
            BuildingKind::Barn,
            BuildingKind::Cottage,
            BuildingKind::Timber,
        ],
    };
    pack_district(occ, grid, lots, seed, crafts, crafts_kinds, rng, 10);
    scenes.push(Scene {
        kind: SceneKind::CraftsmenRow,
        rect: crafts,
    });

    let mut noble = uphill_band(inner, gate);
    if noble.intersects(castle) {
        noble = clip_off_castle(noble, castle.inflate(2), gate);
    }
    let noble_kinds: &[BuildingKind] = match palette {
        PaletteKind::Sandstone => &[BuildingKind::Townhouse, BuildingKind::Townhouse],
        _ => &[
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Timber,
        ],
    };
    pack_district(occ, grid, lots, seed, noble, noble_kinds, rng, 8);
    paint_noble_gardens(occ, noble);
    if noble.width() >= 10 && noble.depth() >= 8 {
        scenes.push(Scene {
            kind: SceneKind::NobleQuarter,
            rect: noble,
        });
    }

    let docks = downhill_band(outer, gate, 36);
    let dock_kinds: &[BuildingKind] = match palette {
        PaletteKind::Sandstone => &[
            BuildingKind::Tavern,
            BuildingKind::Barn,
            BuildingKind::Cottage,
            BuildingKind::Townhouse,
        ],
        _ => &[
            BuildingKind::Tavern,
            BuildingKind::Barn,
            BuildingKind::Cottage,
            BuildingKind::Timber,
        ],
    };
    pack_district(occ, grid, lots, seed, docks, dock_kinds, rng, 8);
    if docks.width() >= 10 && docks.depth() >= 8 {
        scenes.push(Scene {
            kind: SceneKind::DocksQuarter,
            rect: docks,
        });
    }
}

fn is_road_kind(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::Road
            | CellKind::RoadStairN
            | CellKind::RoadStairS
            | CellKind::RoadStairE
            | CellKind::RoadStairW
            | CellKind::Bridge
    )
}

fn packable_city_lot(
    occ: &Occupancy,
    grid: &HeightGrid,
    town: Rect,
    market: Rect,
    rect: Rect,
) -> bool {
    if !town.contains(rect.min_x, rect.min_z) || !town.contains(rect.max_x, rect.max_z) {
        return false;
    }
    let edge = (rect.min_x - town.min_x)
        .min(town.max_x - rect.max_x)
        .min(rect.min_z - town.min_z)
        .min(town.max_z - rect.max_z);
    if edge <= 2 {
        return false;
    }
    if market.inflate(2).intersects(rect) {
        return false;
    }
    if rect_hits_keep_court(occ, rect) {
        return false;
    }
    if !grid.rect_is_buildable(rect) {
        return false;
    }
    for z in rect.min_z..=rect.max_z {
        for x in rect.min_x..=rect.max_x {
            if !matches!(
                occ.kind(x, z),
                CellKind::Empty
                    | CellKind::Green
                    | CellKind::Hedge
                    | CellKind::Farm
                    | CellKind::FarmEdge
            ) {
                return false;
            }
            if near_gothic_pack(occ, x, z) {
                return false;
            }
        }
    }
    true
}

fn near_gothic_pack(occ: &Occupancy, x: i32, z: i32) -> bool {
    for dz in -2..=2 {
        for dx in -2..=2 {
            if matches!(
                occ.kind(x + dx, z + dz),
                CellKind::CathedralNave
                    | CellKind::CathedralTransept
                    | CellKind::CathedralChoir
                    | CellKind::CathedralTower
                    | CellKind::CathedralButtress
            ) {
                return true;
            }
        }
    }
    false
}

/// Wall-to-wall 2–3 storey jettied houses along lanes, then leftover gardens
/// and a few small squares. Aimed at well over 60% built cover inside the wall.
fn pack_jettied_city(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    market: Rect,
    palette: PaletteKind,
    rng: &mut TownRng,
) {
    let kinds: &[BuildingKind] = match palette {
        PaletteKind::Sandstone => &[
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
        ],
        PaletteKind::Spruce => &[
            BuildingKind::Timber,
            BuildingKind::Townhouse,
            BuildingKind::Timber,
        ],
        PaletteKind::Oak => &[
            BuildingKind::Townhouse,
            BuildingKind::Townhouse,
            BuildingKind::Timber,
        ],
    };
    let cap = lots.len() + 280;
    let mut z = town.min_z + 2;
    while z <= town.max_z - 2 && lots.len() < cap {
        let mut x = town.min_x + 2;
        while x <= town.max_x - 2 && lots.len() < cap {
            if !is_road_kind(occ.kind(x, z)) {
                x += 1;
                continue;
            }
            let Some(&kind) = rng.choose(kinds) else {
                x += 1;
                continue;
            };
            let (w, d) = kind.size();
            for dir in Dir::ALL {
                let ox = x + dir.dx() * (w / 2 + 1);
                let oz = z + dir.dz() * (d / 2 + 1);
                let rect = Rect::new(
                    ox - w / 2,
                    oz - d / 2,
                    ox - w / 2 + w - 1,
                    oz - d / 2 + d - 1,
                );
                if !packable_city_lot(occ, grid, town, market, rect) {
                    continue;
                }
                if try_lot(occ, grid, lots, seed, kind, rect, dir.opposite()).is_some() {
                    break;
                }
            }
            x += 4;
        }
        z += 3;
    }
    let mut z = town.min_z + 3;
    while z + 8 <= town.max_z - 2 && lots.len() < cap {
        let mut x = town.min_x + 3;
        while x + 9 <= town.max_x - 2 && lots.len() < cap {
            let Some(&kind) = rng.choose(kinds) else {
                x += 9;
                continue;
            };
            let (mut w, mut d) = kind.size();
            if rng.chance(1, 3) {
                std::mem::swap(&mut w, &mut d);
            }
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            if packable_city_lot(occ, grid, town, market, rect) {
                let facing = facing_toward_road(occ, rect).unwrap_or(Dir::South);
                let _ = try_lot(occ, grid, lots, seed, kind, rect, facing);
            }
            x += w.max(8);
        }
        z += 8;
    }
    paint_small_squares(occ, town, market);
    paint_hedges_and_gardens(occ, town);
}

fn paint_extramural(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    lots: &mut Vec<Lot>,
    seed: u64,
    town: Rect,
    gate: Dir,
    rng: &mut TownRng,
    scenes: &mut Vec<Scene>,
) {
    let start = wall_gate(town, gate);
    let mut cover = Rect::new(start.0, start.1, start.0, start.1);
    let mut placed = 0;
    let facing = gate.opposite();
    let (hw, hd) = BuildingKind::Church.size();
    let hospital = extramural_rect(start, gate, 22, 0, hw, hd);
    if !town.inflate(6).intersects(hospital)
        && force_lot(
            occ,
            grid,
            lots,
            seed,
            BuildingKind::Church,
            hospital,
            facing,
        )
        .is_some()
    {
        cover = union(cover, hospital);
        placed += 1;
    }
    let (bw, bd) = BuildingKind::Barn.size();
    let tannery = find_tannery_site(occ, grid, town, start, gate)
        .unwrap_or_else(|| extramural_rect(start, gate, 32, 12, bw, bd));
    if !town.inflate(6).intersects(tannery)
        && (try_lot(occ, grid, lots, seed, BuildingKind::Barn, tannery, facing).is_some()
            || force_lot(occ, grid, lots, seed, BuildingKind::Barn, tannery, facing).is_some())
    {
        cover = union(cover, tannery);
        placed += 1;
    }
    let (cw, cd) = BuildingKind::Cottage.size();
    for i in 0..12 {
        let dist = 14 + i * 5 + rng.gen_range(0, 3);
        let side = if rng.chance(1, 2) { 10 } else { -11 };
        let rect = extramural_rect(start, gate, dist, side, cw, cd);
        if town.inflate(6).intersects(rect) {
            continue;
        }
        let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
        if hi - lo > 10 {
            continue;
        }
        if try_lot(occ, grid, lots, seed, BuildingKind::Cottage, rect, facing).is_some()
            || force_lot(occ, grid, lots, seed, BuildingKind::Cottage, rect, facing).is_some()
        {
            cover = union(cover, rect);
            placed += 1;
        }
    }
    if placed >= 2 {
        scenes.push(Scene {
            kind: SceneKind::Extramural,
            rect: cover.inflate(4),
        });
    }
}

fn extramural_rect(start: (i32, i32), gate: Dir, dist: i32, side: i32, w: i32, d: i32) -> Rect {
    let along = match gate {
        Dir::North | Dir::South => (side, 0),
        Dir::East | Dir::West => (0, side),
    };
    let x = start.0 + gate.dx() * dist + along.0;
    let z = start.1 + gate.dz() * dist + along.1;
    match gate {
        Dir::South => Rect::new(x - w / 2, z, x - w / 2 + w - 1, z + d - 1),
        Dir::North => Rect::new(x - w / 2, z - d + 1, x - w / 2 + w - 1, z),
        Dir::East => Rect::new(x, z - d / 2, x + w - 1, z - d / 2 + d - 1),
        Dir::West => Rect::new(x - w + 1, z - d / 2, x, z - d / 2 + d - 1),
    }
}

fn find_tannery_site(
    occ: &Occupancy,
    grid: &HeightGrid,
    town: Rect,
    start: (i32, i32),
    gate: Dir,
) -> Option<Rect> {
    let (w, d) = BuildingKind::Barn.size();
    let mut best = None;
    let mut best_s = i32::MIN;
    for dist in 14..48 {
        for side in [-12, -6, 6, 12] {
            let (ox, oz) = match gate {
                Dir::North | Dir::South => (side, 0),
                Dir::East | Dir::West => (0, side),
            };
            let x = start.0 + gate.dx() * dist + ox;
            let z = start.1 + gate.dz() * dist + oz;
            let rect = Rect::new(x, z, x + w - 1, z + d - 1);
            if town.inflate(4).intersects(rect) {
                continue;
            }
            if !occ.bounds.contains(rect.min_x, rect.min_z)
                || !occ.bounds.contains(rect.max_x, rect.max_z)
            {
                continue;
            }
            if !grid.rect_is_dry(rect) {
                continue;
            }
            let mut wet = 0;
            for dz in -4..=d + 3 {
                for dx in -4..=w + 3 {
                    if !grid.is_dry(x + dx, z + dz) {
                        wet += 1;
                    }
                }
            }
            if wet < 4 {
                continue;
            }
            let score = wet * 4 - dist;
            if score > best_s {
                best_s = score;
                best = Some(rect);
            }
        }
    }
    best
}

fn paint_small_squares(occ: &mut Occupancy, town: Rect, market: Rect) {
    let mut painted = 0;
    let mut z = town.min_z + 4;
    while z + 5 <= town.max_z - 4 && painted < 8 {
        let mut x = town.min_x + 4;
        while x + 5 <= town.max_x - 4 && painted < 8 {
            let rect = Rect::new(x, z, x + 5, z + 5);
            x += 11;
            if market.inflate(4).intersects(rect) {
                continue;
            }
            let mut empty = 0;
            let mut n = 0;
            let mut road = false;
            for zz in rect.min_z..=rect.max_z {
                for xx in rect.min_x..=rect.max_x {
                    n += 1;
                    let kind = occ.kind(xx, zz);
                    if kind == CellKind::Empty {
                        empty += 1;
                    }
                    if is_road_kind(kind) {
                        road = true;
                    }
                }
            }
            if road || empty * 5 < n * 4 {
                continue;
            }
            for zz in rect.min_z..=rect.max_z {
                for xx in rect.min_x..=rect.max_x {
                    if occ.kind(xx, zz) == CellKind::Empty {
                        occ.set(xx, zz, CellKind::Plaza, true);
                    }
                }
            }
            painted += 1;
        }
        z += 11;
    }
}

fn facing_toward_road(occ: &Occupancy, rect: Rect) -> Option<Dir> {
    let (cx, cz) = rect.center();
    for dist in 2..=8 {
        for dir in Dir::ALL {
            let x = cx + dir.dx() * dist;
            let z = cz + dir.dz() * dist;
            if matches!(
                occ.kind(x, z),
                CellKind::Road
                    | CellKind::RoadStairN
                    | CellKind::RoadStairS
                    | CellKind::RoadStairE
                    | CellKind::RoadStairW
                    | CellKind::Market
            ) {
                return Some(dir);
            }
        }
    }
    None
}

fn place_farms(
    occ: &mut Occupancy,
    grid: &HeightGrid,
    town: Rect,
    castle: Rect,
    rng: &mut TownRng,
) {
    let ring = town.inflate(36);
    let mut placed = 0;
    let mut z = ring.min_z;
    while z < ring.max_z && placed < 8 {
        let mut x = ring.min_x;
        while x < ring.max_x && placed < 8 {
            let rect = Rect::new(x, z, x + 18, z + 12);
            if castle.inflate(8).intersects(rect) || town.intersects(rect) {
                x += 20;
                continue;
            }
            let (lo, hi) = grid.range(rect.min_x, rect.min_z, rect.max_x, rect.max_z);
            let dry = lo > grid.sea_level + 1;
            let gentle = hi - lo <= 6;
            if dry && gentle && occ.is_free(rect) && rng.chance(2, 3) {
                occ.fill(rect.deflate(1), CellKind::Farm, false);
                occ.fill(rect, CellKind::FarmEdge, false);
                occ.fill(rect.deflate(1), CellKind::Farm, true);
                placed += 1;
            }
            x += 22;
        }
        z += 16;
    }
}

fn paint_water_uses(occ: &mut Occupancy, grid: &HeightGrid, bounds: Rect, castle: Rect) {
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            if grid.is_dry(x, z) {
                continue;
            }
            let kind = occ.kind(x, z);
            if castle.contains(x, z)
                && !matches!(
                    kind,
                    CellKind::Empty
                        | CellKind::Harbor
                        | CellKind::Dock
                        | CellKind::CastleMoat
                        | CellKind::Bridge
                )
            {
                continue;
            }
            match kind {
                CellKind::Empty
                | CellKind::Green
                | CellKind::Farm
                | CellKind::FarmEdge
                | CellKind::Orchard
                | CellKind::Pasture
                | CellKind::Grave
                | CellKind::Plaza
                | CellKind::Fountain => {
                    let dock = Dir::ALL.iter().any(|dir| {
                        let n = occ.kind(x + dir.dx(), z + dir.dz());
                        matches!(
                            n,
                            CellKind::Road
                                | CellKind::RoadStairN
                                | CellKind::RoadStairS
                                | CellKind::RoadStairE
                                | CellKind::RoadStairW
                                | CellKind::Market
                                | CellKind::Building
                                | CellKind::TownGate
                        )
                    });
                    occ.set(
                        x,
                        z,
                        if dock {
                            CellKind::Dock
                        } else {
                            CellKind::Harbor
                        },
                        true,
                    );
                }
                CellKind::Road
                | CellKind::RoadStairN
                | CellKind::RoadStairS
                | CellKind::RoadStairE
                | CellKind::RoadStairW => {
                    occ.set(x, z, CellKind::Bridge, true);
                }
                CellKind::Eave | CellKind::Hedge | CellKind::Shrine | CellKind::FieldWall => {
                    occ.set(x, z, CellKind::Harbor, true);
                }
                CellKind::WindSail | CellKind::Building => {}
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::town::town_in_cell;

    #[test]
    fn plan_is_deterministic_and_castle_is_large() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            6,
        );
        let a = build_plan(20_261_025, origin, grid.clone(), PaletteKind::Oak);
        let b = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert_eq!(a.castle.bounds, b.castle.bounds);
        assert_eq!(a.well, b.well);
        assert_eq!(a.lots.len(), b.lots.len());
        let (w, d) = a.castle_footprint();
        assert!(w >= 128, "castle width {w}");
        assert!(d >= 128, "castle depth {d}");
        assert_eq!(a.occ.kind(a.well.0, a.well.1), CellKind::Well);
        let keep = a.castle.keep.center();
        let col_a = a.planned_column(keep.0, keep.1);
        let col_b = b.planned_column(keep.0, keep.1);
        assert!(!col_a.is_empty(), "keep column should have intended blocks");
        assert_eq!(col_a, col_b);
        let sample = col_a[col_a.len() / 2];
        assert_eq!(
            a.planned_block(sample.x, sample.y, sample.z)
                .map(|p| p.state),
            Some(sample.state)
        );
        assert_eq!(
            a.planned_at(pumpkin_util::math::position::BlockPos::new(
                sample.x, sample.y, sample.z
            ))
            .map(|p| p.state),
            Some(sample.state)
        );
        assert!(a.planned_keep_blocks().len() >= col_a.len());
        let lot = a.lots.first().expect("town should have a building lot");
        let house = a.planned_building_blocks(lot.id);
        assert!(
            !house.is_empty(),
            "civilian lot {} should have intended blocks",
            lot.id
        );
        assert_eq!(house, a.planned_blocks_in(lot.rect));
        let mid = house[house.len() / 2];
        assert_eq!(
            a.planned_block(mid.x, mid.y, mid.z).map(|p| p.state),
            Some(mid.state)
        );
    }

    #[test]
    fn lots_stay_on_dry_land() {
        let origin = town_in_cell(5, 0, 0);
        let mut grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            58,
            24,
            10,
        );
        grid.sea_level = 63;
        let plan = build_plan(5, origin, grid, PaletteKind::Oak);
        for lot in &plan.lots {
            assert!(
                plan.heights.rect_is_dry(lot.rect),
                "lot {} sited on water",
                lot.id
            );
        }
        for z in plan.bounds.min_z..=plan.bounds.max_z {
            for x in plan.bounds.min_x..=plan.bounds.max_x {
                if matches!(
                    plan.occ.kind(x, z),
                    CellKind::Building
                        | CellKind::Farm
                        | CellKind::Orchard
                        | CellKind::Pasture
                        | CellKind::Grave
                ) {
                    assert!(
                        plan.heights.is_dry(x, z)
                            || matches!(
                                plan.occ.kind(x, z),
                                CellKind::Harbor | CellKind::Dock | CellKind::Bridge
                            ),
                        "occupied wet column {x},{z} is not a water use"
                    );
                }
            }
        }
        let town_dry = plan.heights.dry_ratio(
            plan.market.min_x - 8,
            plan.market.min_z - 8,
            plan.market.max_x + 8,
            plan.market.max_z + 8,
        );
        assert!(
            town_dry >= 70,
            "market neighborhood should be mostly dry, got {town_dry}"
        );
    }

    #[test]
    fn outbound_roads_meet_the_cell_border() {
        let seed = 7u64;
        let origin = town_in_cell(seed, 0, 0);
        let grid = HeightGrid::synthetic_slope(0, 0, CELL_SIZE, CELL_SIZE, 8, 70, 2, 1);
        let plan = build_plan(seed, origin, grid, PaletteKind::Oak);
        let neighbor = town_in_cell(seed, 1, 0);
        let cross = border_crossing(seed, origin, neighbor);
        let goal = border_goal(origin, neighbor, cross);
        let mut found = false;
        for dz in -4..=4 {
            for dx in -4..=4 {
                let x = goal.0 + dx;
                let z = goal.1 + dz;
                let kind = plan
                    .border_roads
                    .get(&(x, z))
                    .copied()
                    .unwrap_or_else(|| plan.occ.kind(x, z));
                if matches!(
                    kind,
                    CellKind::Road
                        | CellKind::RoadStairN
                        | CellKind::RoadStairS
                        | CellKind::RoadStairE
                        | CellKind::RoadStairW
                ) {
                    found = true;
                }
            }
        }
        assert!(
            found,
            "expected a stitched road at the hashed crossing {goal:?}"
        );
        assert_eq!(cross.0, CELL_SIZE);
    }

    #[test]
    fn cached_plan_does_not_generate() {
        assert!(cached_plan(9_999_001, 0, 0).is_none());
        assert!(cached_town_info(9_999_001, 0, 0).is_none());
    }

    #[test]
    fn town_info_disk_round_trip_without_regenerating() {
        let dir = std::env::temp_dir().join(format!(
            "kingdoms-plans-test-{}-{}",
            std::process::id(),
            9_999_002
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        bind_plans_dir_for_test(dir.clone());
        let origin = town_in_cell(9_999_002, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            6,
        );
        let plan = Arc::new(build_plan(9_999_002, origin, grid, PaletteKind::Oak));
        remember_plan_for_test(Arc::clone(&plan));
        forget_memory_for_test(9_999_002, 0, 0);
        assert!(cached_plan(9_999_002, 0, 0).is_none());
        let info = cached_town_info(9_999_002, 0, 0).expect("disk snapshot");
        assert_eq!(info.id, "9999002:0:0");
        assert_eq!(info.cell, [0, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_plan_cache_version_is_ignored() {
        assert_eq!(PLAN_CACHE_VERSION, 23);
        let dir = std::env::temp_dir().join(format!(
            "kingdoms-plans-test-{}-{}",
            std::process::id(),
            9_999_003
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        bind_plans_dir_for_test(dir.clone());
        let origin = town_in_cell(9_999_003, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            6,
        );
        let plan = Arc::new(build_plan(9_999_003, origin, grid, PaletteKind::Oak));
        let info = TownInfo::from_plan(&plan);
        let path = dir.join("9999003_0_0.json");
        let stale = PlanCacheFile {
            version: 1,
            info: info.clone(),
        };
        std::fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();
        forget_memory_for_test(9_999_003, 0, 0);
        assert!(cached_town_info(9_999_003, 0, 0).is_none());
        std::fs::write(&path, info.to_json().unwrap()).unwrap();
        forget_memory_for_test(9_999_003, 0, 0);
        assert!(cached_town_info(9_999_003, 0, 0).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Occupancy fingerprint of `build_plan(20_261_025, cell 0, Oak slope)`.
    /// When plan output changes, bump [`PLAN_CACHE_VERSION`] (and this pair).
    const PLAN_LAYOUT_VERSION: u32 = 23;
    const PLAN_LAYOUT_HASH: u64 = 0x5436_a4d8_c072_5c22;

    #[test]
    fn plan_cache_version_tracks_layout() {
        let origin = town_in_cell(20_261_025, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            80,
            18,
            6,
        );
        let plan = build_plan(20_261_025, origin, grid, PaletteKind::Oak);
        assert_eq!(
            plan.occ.kind(plan.castle.flag.0, plan.castle.flag.1),
            crate::cells::CellKind::FlagPlinth,
            "flag plinth must be stamped into occupancy"
        );
        assert!(
            !plan
                .castle
                .gatehouse
                .contains(plan.castle.flag.0, plan.castle.flag.1),
            "flag must not sit in the gatehouse"
        );
        assert!(
            plan.castle
                .keep_mass_at(plan.castle.flag.0, plan.castle.flag.1)
                .is_none(),
            "flag must not sit inside a keep mass flag={:?}",
            plan.castle.flag
        );
        let (kx, kz) = plan.castle.keep.center();
        let (hx, hz) = plan.castle.hall.center();
        let (fx, fz) = plan.castle.flag;
        let near_keep = (fx - kx).abs().max((fz - kz).abs()) <= 18;
        let near_hall = (fx - hx).abs().max((fz - hz).abs()) <= 18;
        assert!(
            near_keep && near_hall,
            "flag must sit on the keep-hall court flag={:?} keep_c={:?} hall_c={:?}",
            plan.castle.flag,
            (kx, kz),
            (hx, hz),
        );
        let hash = plan.layout_fingerprint();
        assert_eq!(
            PLAN_CACHE_VERSION, PLAN_LAYOUT_VERSION,
            "bump PLAN_CACHE_VERSION when plan output changes"
        );
        assert_eq!(
            hash, PLAN_LAYOUT_HASH,
            "plan output changed; bump PLAN_CACHE_VERSION (now {PLAN_CACHE_VERSION}) and PLAN_LAYOUT_HASH to {hash:#x}"
        );
    }
}
