//! Siege jobs, gate close/breach, and town ownership.
//!
//! Capture is a keep-courtyard hold: raiders with no defenders inside the
//! inner bailey for [`HOLD_TICKS`] flip the town to the pillager faction.
//! Villagers can retake the same way. Ownership is a JSON file in the world
//! folder so it survives restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pumpkin_core::world::World;
use pumpkin_data::Block;
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;
use serde::{Deserialize, Serialize};

use crate::faction::KingdomId;
use crate::geom::{Dir, Rect};
use crate::info::GateInfo;
use std::sync::Arc;

/// Ticks raiders (or villagers) must hold the keep courtyard uncontested.
pub const HOLD_TICKS: u32 = 200;
/// Extra Y so ladder crews and archers stand on the wall walk, not the dirt.
pub const WALL_WALK_LIFT: i32 = 6;
/// Scan around a gate column when closing or ramming.
const GATE_SCAN: i32 = 3;
const GATE_HEIGHT: i32 = 5;

/// Who currently holds the town.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TownOwner {
    Kingdom(String),
    Pillager,
}

impl TownOwner {
    #[must_use]
    pub fn kingdom(id: &KingdomId) -> Self {
        Self::Kingdom(id.as_str().to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Kingdom(name) => name.as_str(),
            Self::Pillager => "pillager",
        }
    }

    #[must_use]
    pub fn is_pillager(&self) -> bool {
        matches!(self, Self::Pillager)
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        if raw.eq_ignore_ascii_case("pillager") {
            Self::Pillager
        } else {
            Self::Kingdom(raw.to_ascii_lowercase())
        }
    }
}

/// How a raider contributes to the siege.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarchJob {
    Assault,
    Ram,
    Ladder,
}

impl MarchJob {
    #[must_use]
    pub fn assign(index: usize, vindicator: bool) -> Self {
        if vindicator || index.is_multiple_of(8) {
            Self::Ram
        } else if index % 5 == 1 {
            Self::Ladder
        } else {
            Self::Assault
        }
    }

    #[must_use]
    pub const fn tag(self) -> Option<&'static str> {
        match self {
            Self::Ram => Some(crate::faction::TAG_JOB_RAM),
            Self::Ladder => Some(crate::faction::TAG_JOB_LADDER),
            Self::Assault => None,
        }
    }

    #[must_use]
    pub fn from_tags<'a>(tags: impl IntoIterator<Item = &'a String>) -> Self {
        let mut job = Self::Assault;
        for tag in tags {
            if tag == crate::faction::TAG_JOB_RAM {
                return Self::Ram;
            }
            if tag == crate::faction::TAG_JOB_LADDER {
                job = Self::Ladder;
            }
        }
        job
    }
}

/// File name written next to `level.dat`.
pub const OWNERSHIP_FILE: &str = "kingdoms-ownership.json";

/// Player rank from kills, captures, and defenses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank {
    Levy,
    Freeman,
    Sergeant,
    Knight,
    Banneret,
}

impl Rank {
    #[must_use]
    pub const fn from_renown(renown: u32) -> Self {
        if renown >= 100 {
            Self::Banneret
        } else if renown >= 50 {
            Self::Knight
        } else if renown >= 25 {
            Self::Sergeant
        } else if renown >= 10 {
            Self::Freeman
        } else {
            Self::Levy
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Levy => "levy",
            Self::Freeman => "freeman",
            Self::Sergeant => "sergeant",
            Self::Knight => "knight",
            Self::Banneret => "banneret",
        }
    }
}

/// Persisted war record for one player.
#[derive(Default, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerStats {
    #[serde(default)]
    pub kills: u32,
    #[serde(default)]
    pub captures: u32,
    #[serde(default)]
    pub defenses: u32,
    #[serde(default)]
    pub banner_given: bool,
}

impl PlayerStats {
    #[must_use]
    pub fn renown(&self) -> u32 {
        self.kills
            .saturating_add(self.captures.saturating_mul(5))
            .saturating_add(self.defenses.saturating_mul(3))
    }

    #[must_use]
    pub fn rank(&self) -> Rank {
        Rank::from_renown(self.renown())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct OwnershipFile {
    towns: HashMap<String, String>,
    #[serde(default)]
    players: HashMap<String, String>,
    #[serde(default)]
    scores: HashMap<String, i32>,
    #[serde(default)]
    clock_tick: u32,
    #[serde(default)]
    next_raid_id: u32,
    #[serde(default)]
    last_clock_tick: u32,
    #[serde(default)]
    damage: HashMap<String, String>,
    #[serde(default)]
    stats: HashMap<String, PlayerStats>,
}

/// World-folder path for the ownership file.
#[must_use]
pub fn ownership_path(world_root: &Path) -> PathBuf {
    world_root.join(OWNERSHIP_FILE)
}

/// Loaded towns, player membership, scores, and war clock.
#[derive(Default, Clone)]
pub struct SavedState {
    pub towns: HashMap<String, TownOwner>,
    pub players: HashMap<String, String>,
    pub scores: HashMap<String, i32>,
    pub clock_tick: u32,
    pub next_raid_id: u32,
    pub last_clock_tick: u32,
    pub damage: HashMap<String, crate::damage::DamageState>,
    pub stats: HashMap<String, PlayerStats>,
}

/// Load town id → owner from `kingdoms-ownership.json`.
#[must_use]
pub fn load_ownership(path: &Path) -> HashMap<String, TownOwner> {
    load_state(path).towns
}

/// Load towns, players, scores, and the war clock from `kingdoms-ownership.json`.
#[must_use]
pub fn load_state(path: &Path) -> SavedState {
    let Ok(bytes) = std::fs::read_to_string(path) else {
        return SavedState::default();
    };
    let Ok(file) = serde_json::from_str::<OwnershipFile>(&bytes) else {
        tracing::warn!(?path, "kingdoms ownership file unreadable");
        return SavedState::default();
    };
    SavedState {
        towns: file
            .towns
            .into_iter()
            .map(|(id, owner)| (id, TownOwner::parse(&owner)))
            .collect(),
        players: file.players,
        scores: file.scores,
        clock_tick: file.clock_tick,
        next_raid_id: file.next_raid_id,
        last_clock_tick: file.last_clock_tick,
        damage: file
            .damage
            .into_iter()
            .map(|(id, raw)| (id, crate::damage::DamageState::parse(&raw)))
            .collect(),
        stats: file.stats,
    }
}

/// Persist town ownership. Overwrites the file.
#[allow(clippy::implicit_hasher)]
pub fn save_ownership(path: &Path, owners: &HashMap<String, TownOwner>) -> std::io::Result<()> {
    save_state(
        path,
        &SavedState {
            towns: owners.clone(),
            ..SavedState::default()
        },
    )
}

/// Persist towns, players, scores, and the war clock. Overwrites the file.
pub fn save_state(path: &Path, state: &SavedState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OwnershipFile {
        towns: state
            .towns
            .iter()
            .map(|(id, owner)| (id.clone(), owner.as_str().to_owned()))
            .collect(),
        players: state.players.clone(),
        scores: state.scores.clone(),
        clock_tick: state.clock_tick,
        next_raid_id: state.next_raid_id,
        last_clock_tick: state.last_clock_tick,
        damage: state
            .damage
            .iter()
            .map(|(id, state)| (id.clone(), state.as_str().to_owned()))
            .collect(),
        stats: state.stats.clone(),
    };
    let json = serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_owned());
    std::fs::write(path, json)
}

/// Write ownership off the director tick when a Tokio runtime is running.
pub fn save_state_async(path: PathBuf, state: SavedState) {
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(async move {
            match tokio::task::spawn_blocking(move || save_state(&path, &state)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(%err, "failed to persist kingdoms ownership"),
                Err(err) => tracing::warn!(%err, "failed to persist kingdoms ownership"),
            }
        });
        return;
    }
    if let Err(err) = save_state(&path, &state) {
        tracing::warn!(%err, "failed to persist kingdoms ownership");
    }
}

/// Attackers needed in the courtyard: at least 8, or 10% of the surviving force.
#[must_use]
pub const fn capture_need(surviving: u32) -> u32 {
    let tenth = surviving.div_ceil(10);
    if tenth < 8 { 8 } else { tenth }
}

/// Advance a courtyard hold. Returns the new hold counter and a flip, if any.
///
/// Attackers must meet [`capture_need`] in the keep with defenders there wiped
/// out or routed (`defenders_in_keep == 0`).
#[must_use]
pub fn tick_hold(
    current: &TownOwner,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
    occupy_hold: u32,
    retake_hold: u32,
    occupy_need: u32,
    retake_need: u32,
) -> HoldUpdate {
    tick_hold_n(
        current,
        raiders_in_keep,
        defenders_in_keep,
        occupy_hold,
        retake_hold,
        occupy_need,
        retake_need,
        1,
    )
}

/// Same as [`tick_hold`], adding `delta` ticks (abstract fronts step in batches).
#[must_use]
pub fn tick_hold_n(
    current: &TownOwner,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
    occupy_hold: u32,
    retake_hold: u32,
    occupy_need: u32,
    retake_need: u32,
    delta: u32,
) -> HoldUpdate {
    let mut occupy = occupy_hold;
    let mut retake = retake_hold;
    if raiders_in_keep >= occupy_need && defenders_in_keep == 0 {
        occupy = occupy.saturating_add(delta);
        retake = 0;
        // Kingdom armies (and pillagers) both flip a town by holding the keep.
        // Skipping pillager-owned towns here left abstract KvK / retake fronts
        // stuck at occupy_hold >> HOLD_TICKS.
        if occupy >= HOLD_TICKS {
            return HoldUpdate {
                occupy_hold: 0,
                retake_hold: 0,
                captured: true,
                retaken: false,
            };
        }
    } else if defenders_in_keep >= retake_need && raiders_in_keep == 0 {
        retake = retake.saturating_add(delta);
        occupy = 0;
        if retake >= HOLD_TICKS && current.is_pillager() {
            return HoldUpdate {
                occupy_hold: 0,
                retake_hold: 0,
                captured: false,
                retaken: true,
            };
        }
    } else {
        occupy = occupy.saturating_sub(delta);
        retake = retake.saturating_sub(delta);
    }
    HoldUpdate {
        occupy_hold: occupy,
        retake_hold: retake,
        captured: false,
        retaken: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HoldUpdate {
    pub occupy_hold: u32,
    pub retake_hold: u32,
    pub captured: bool,
    pub retaken: bool,
}

/// Close the portcullis: fill air in each gate opening with iron bars.
pub fn close_gates(world: &Arc<World>, gates: &[GateInfo], floor_y: impl Fn(i32, i32) -> i32) {
    for gate in gates {
        let y0 = floor_y(gate.x, gate.z);
        for pos in gate_cells(gate, y0) {
            let (block, _) = world.get_block_and_state(&pos);
            if block.id == Block::AIR.id {
                world.set_block_state(
                    &pos,
                    Block::IRON_BARS.default_state.id,
                    BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_DROPS,
                );
            }
        }
        tracing::info!("kingdoms gate closed x={} z={}", gate.x, gate.z);
    }
}

/// Ram a threatened gate: iron bars become air so the courtyard is open.
pub fn breach_gate(world: &Arc<World>, gate: GateInfo, floor_y: i32) -> u32 {
    let mut cleared = 0u32;
    for pos in gate_cells(&gate, floor_y) {
        let (block, _) = world.get_block_and_state(&pos);
        if block.id == Block::IRON_BARS.id {
            world.set_block_state(
                &pos,
                Block::AIR.default_state.id,
                BlockFlags::NOTIFY_LISTENERS | BlockFlags::SKIP_DROPS,
            );
            cleared += 1;
        }
    }
    if cleared > 0 {
        tracing::info!(
            "kingdoms ram breached the gate x={} z={} cleared={cleared}",
            gate.x,
            gate.z
        );
    }
    cleared
}

fn gate_cells(gate: &GateInfo, floor_y: i32) -> Vec<BlockPos> {
    let mut out = Vec::new();
    let (dx, dz) = match gate.facing {
        Dir::North | Dir::South => (GATE_SCAN, 1),
        Dir::East | Dir::West => (1, GATE_SCAN),
    };
    for y in floor_y + 1..=floor_y + GATE_HEIGHT {
        for x in gate.x - dx..=gate.x + dx {
            for z in gate.z - dz..=gate.z + dz {
                out.push(BlockPos::new(x, y, z));
            }
        }
    }
    out
}

/// True when `(x, z)` is inside the keep courtyard (inner bailey).
#[must_use]
pub fn in_courtyard(rect: Rect, x: f64, z: f64) -> bool {
    let ix = x.floor() as i32;
    let iz = z.floor() as i32;
    ix >= rect.min_x && ix <= rect.max_x && iz >= rect.min_z && iz <= rect.max_z
}

/// One column to try as a respawn stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StandCandidate {
    pub x: i32,
    pub z: i32,
}

/// Standable column: solid underfoot, two air blocks for the player.
#[must_use]
pub fn column_is_standable(
    x: i32,
    z: i32,
    floor_y: i32,
    solid: impl Fn(i32, i32, i32) -> bool,
) -> bool {
    let feet = floor_y;
    solid(x, feet - 1, z) && !solid(x, feet, z) && !solid(x, feet + 1, z)
}

/// Pick a standable keep courtyard column, then the nearest friendly fallback.
/// Always returns a column: last resort is the keep itself at `floor_y`.
#[must_use]
pub fn pick_safe_stand(
    keep: StandCandidate,
    courtyard: Rect,
    fallbacks: &[StandCandidate],
    floor_y: impl Fn(i32, i32) -> i32,
    solid: impl Fn(i32, i32, i32) -> bool,
) -> (i32, i32, i32) {
    let mut candidates = Vec::new();
    let min_x = courtyard.min_x.min(keep.x - 8);
    let max_x = courtyard.max_x.max(keep.x + 8);
    let min_z = courtyard.min_z.min(keep.z - 8);
    let max_z = courtyard.max_z.max(keep.z + 8);
    for radius in 1i32..=12 {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs() != radius && dz.abs() != radius {
                    continue;
                }
                let x = keep.x + dx;
                let z = keep.z + dz;
                if x < min_x || x > max_x || z < min_z || z > max_z {
                    continue;
                }
                candidates.push(StandCandidate { x, z });
            }
        }
    }
    candidates.extend(fallbacks.iter().copied());
    let mut court_best: Option<(i32, i32, i32)> = None;
    let mut other_best: Option<(i32, i32, i32)> = None;
    for c in candidates {
        if courtyard.contains(c.x, c.z) && on_courtyard_edge(courtyard, c.x, c.z) {
            continue;
        }
        let y = floor_y(c.x, c.z);
        if !column_is_standable(c.x, c.z, y, &solid) {
            continue;
        }
        let slot = if courtyard.contains(c.x, c.z) {
            &mut court_best
        } else {
            &mut other_best
        };
        match *slot {
            None => *slot = Some((c.x, y, c.z)),
            Some((_, by, _)) if y < by => *slot = Some((c.x, y, c.z)),
            _ => {}
        }
    }
    if let Some(stand) = court_best.or(other_best) {
        return stand;
    }
    if let Some(c) = fallbacks.first() {
        return (c.x, floor_y(c.x, c.z), c.z);
    }
    let y = floor_y(keep.x, keep.z);
    (keep.x, y, keep.z)
}

fn on_courtyard_edge(rect: Rect, x: i32, z: i32) -> bool {
    x == rect.min_x || x == rect.max_x || z == rect.min_z || z == rect.max_z
}

/// Lowest open bailey column around the keep, using plan occupancy.
///
/// Inner-ward terraces sit next to the keep at roof height; the real court is
/// the lower grass. Prefer that Y, then the nearest column at that floor.
#[must_use]
pub fn pick_open_court(
    keep: StandCandidate,
    is_open: impl Fn(i32, i32) -> bool,
    floor_y: impl Fn(i32, i32) -> i32,
    radius: i32,
) -> Option<(i32, i32, i32)> {
    let mut best = None;
    let mut best_key = (i32::MAX, i32::MAX);
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            if dx.abs() < 4 && dz.abs() < 4 {
                continue;
            }
            let x = keep.x + dx;
            let z = keep.z + dz;
            if !is_open(x, z) {
                continue;
            }
            let y = floor_y(x, z);
            let d = dx.abs() + dz.abs();
            let key = (y, d);
            if key < best_key {
                best_key = key;
                best = Some((x, y, z));
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("kingdoms-own-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = ownership_path(&dir);
        let mut owners = HashMap::new();
        owners.insert("20261025:0:0".to_owned(), TownOwner::Pillager);
        owners.insert(
            "20261025:1:0".to_owned(),
            TownOwner::Kingdom("alder".to_owned()),
        );
        save_ownership(&path, &owners).unwrap();
        let back = load_ownership(&path);
        assert_eq!(back.get("20261025:0:0"), Some(&TownOwner::Pillager));
        assert_eq!(
            back.get("20261025:1:0"),
            Some(&TownOwner::Kingdom("alder".to_owned()))
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn scores_and_clock_survive_a_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("kingdoms-war-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = ownership_path(&dir);
        let mut towns = HashMap::new();
        towns.insert("20261025:0:0".to_owned(), TownOwner::Pillager);
        let mut scores = HashMap::new();
        scores.insert("pillager".to_owned(), 3);
        scores.insert("alder".to_owned(), 1);
        save_state(
            &path,
            &SavedState {
                towns,
                players: HashMap::new(),
                scores,
                clock_tick: 4800,
                next_raid_id: 7,
                last_clock_tick: 2400,
                damage: HashMap::new(),
                stats: HashMap::new(),
            },
        )
        .unwrap();
        let back = load_state(&path);
        assert_eq!(back.towns.get("20261025:0:0"), Some(&TownOwner::Pillager));
        assert_eq!(back.scores.get("pillager"), Some(&3));
        assert_eq!(back.clock_tick, 4800);
        assert_eq!(back.next_raid_id, 7);
        assert_eq!(back.last_clock_tick, 2400);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn keep_respawn_skips_a_walled_column_and_uses_the_courtyard() {
        let keep = StandCandidate { x: 10, z: 10 };
        let court = Rect::new(8, 8, 14, 14);
        let solid = |x: i32, y: i32, z: i32| {
            if x == 10 && z == 10 {
                return (79..=82).contains(&y);
            }
            y == 79
        };
        let floor = |_x: i32, _z: i32| 80;
        let (x, y, z) = pick_safe_stand(keep, court, &[], floor, solid);
        assert_ne!((x, z), (10, 10), "walled keep column must not be chosen");
        assert!(column_is_standable(x, z, y, solid));
        assert_eq!(y, 80);
        // Keep roof is standable too; courtyard still wins.
        let roof_ok = |x: i32, y: i32, _z: i32| y == 79 || (x == 10 && y == 174);
        let roof_y = |x: i32, _z: i32| if x == 10 { 175 } else { 80 };
        let (x, y, z) = pick_safe_stand(keep, court, &[], roof_y, roof_ok);
        assert_ne!((x, z), (10, 10), "keep roof must not beat the courtyard");
        assert_eq!(y, 80);
    }

    #[test]
    fn keep_respawn_skips_the_curtain_ring() {
        let keep = StandCandidate { x: 10, z: 10 };
        let court = Rect::new(4, 4, 16, 16);
        let solid = |x: i32, y: i32, z: i32| {
            let wall = x == 4 || x == 16 || z == 4 || z == 16;
            if wall {
                return (79..=85).contains(&y);
            }
            y == 79
        };
        let floor = |x: i32, z: i32| {
            if x == 4 || x == 16 || z == 4 || z == 16 {
                86
            } else {
                80
            }
        };
        let (x, y, z) = pick_safe_stand(keep, court, &[], floor, solid);
        assert_ne!(x, 4);
        assert_ne!(x, 16);
        assert_ne!(z, 4);
        assert_ne!(z, 16);
        assert!(court.contains(x, z));
        assert!(column_is_standable(x, z, y, solid));
        assert_eq!(y, 80);
    }

    #[test]
    fn keep_respawn_prefers_courtyard_over_world_spawn() {
        let keep = StandCandidate { x: 100, z: 100 };
        let court = Rect::new(90, 90, 110, 110);
        let spawn = StandCandidate { x: 0, z: 0 };
        let solid = |_x: i32, y: i32, _z: i32| y == 79 || y == 63;
        let floor = |x: i32, z: i32| {
            if x == 0 && z == 0 { 64 } else { 80 }
        };
        let (x, y, z) = pick_safe_stand(keep, court, &[spawn], floor, solid);
        assert!(
            court.contains(x, z),
            "respawn {x},{y},{z} must stay in the court"
        );
        assert_ne!((x, z), (0, 0));
        assert_eq!(y, 80);
        assert!(column_is_standable(x, z, y, solid));
    }

    #[test]
    fn pick_open_court_skips_the_keep_mass() {
        let keep = StandCandidate { x: 10, z: 10 };
        let is_open = |x: i32, z: i32| (x - 10).abs() >= 6 || (z - 10).abs() >= 6;
        let floor = |_x: i32, _z: i32| 80;
        let (x, y, z) = pick_open_court(keep, is_open, floor, 12).expect("open bailey");
        assert!((x - 10).abs() >= 6 || (z - 10).abs() >= 6);
        assert_ne!((x, z), (10, 10));
        assert_eq!(y, 80);
    }

    #[test]
    fn pick_open_court_prefers_the_lower_bailey_over_the_keep_terrace() {
        let keep = StandCandidate { x: 10, z: 10 };
        let is_open = |_x: i32, _z: i32| true;
        let floor = |x: i32, z: i32| {
            if (x - 10).abs() <= 12 && (z - 10).abs() <= 12 {
                173
            } else {
                80
            }
        };
        let (x, y, z) = pick_open_court(keep, is_open, floor, 20).expect("lower bailey");
        assert_eq!(y, 80, "respawn {x},{y},{z} must not stay on the keep roof");
        assert!((x - 10).abs() > 12 || (z - 10).abs() > 12);
        assert_ne!((x, z), (10, 10));
    }

    #[test]
    fn keep_respawn_picks_the_lower_courtyard_over_the_keep_roof() {
        let keep = StandCandidate { x: 10, z: 10 };
        let court = Rect::new(0, 0, 20, 20);
        let solid = |x: i32, y: i32, z: i32| {
            if x == 10 && z == 10 {
                return y <= 173;
            }
            y == 79
        };
        let floor = |x: i32, z: i32| {
            if x == 10 && z == 10 { 174 } else { 80 }
        };
        let (x, y, z) = pick_safe_stand(keep, court, &[], floor, solid);
        assert_ne!((x, z), (10, 10));
        assert_eq!(y, 80);
        assert!(court.contains(x, z));
    }

    #[test]
    fn keep_respawn_falls_back_to_the_nearest_friendly_town() {
        let keep = StandCandidate { x: 0, z: 0 };
        let court = Rect::new(0, 0, 0, 0);
        let other = StandCandidate { x: 40, z: 2 };
        let solid = |x: i32, y: i32, _z: i32| {
            if x == 40 {
                return y == 63;
            }
            true
        };
        let floor = |x: i32, _z: i32| if x == 40 { 64 } else { 80 };
        let (x, y, z) = pick_safe_stand(keep, court, &[other], floor, solid);
        assert_eq!((x, y, z), (40, 64, 2));
    }

    #[test]
    fn capture_need_is_max_of_eight_and_ten_percent() {
        assert_eq!(capture_need(0), 8);
        assert_eq!(capture_need(50), 8);
        assert_eq!(capture_need(80), 8);
        assert_eq!(capture_need(400), 40);
    }

    #[test]
    fn four_raiders_do_not_flip_a_town() {
        let owner = TownOwner::Kingdom("alder".to_owned());
        let need = capture_need(400);
        let mut hold = 0;
        let mut update = HoldUpdate {
            occupy_hold: 0,
            retake_hold: 0,
            captured: false,
            retaken: false,
        };
        for _ in 0..HOLD_TICKS {
            update = tick_hold(&owner, 4, 0, hold, 0, need, 8);
            hold = update.occupy_hold;
        }
        assert!(!update.captured);
        assert_eq!(update.occupy_hold, 0);
    }

    #[test]
    fn capture_needs_raiders_in_the_keep_and_no_defenders() {
        let owner = TownOwner::Kingdom("alder".to_owned());
        let need = capture_need(80);
        let mut hold = 0;
        let mut update = tick_hold(&owner, 2, 0, hold, 0, need, 8);
        assert!(!update.captured);
        hold = update.occupy_hold;
        for _ in 0..HOLD_TICKS {
            update = tick_hold(&owner, need, 0, hold, 0, need, 8);
            hold = update.occupy_hold;
        }
        assert!(update.captured);
        assert_eq!(update.occupy_hold, 0);
    }

    #[test]
    fn defenders_in_the_keep_block_capture() {
        let owner = TownOwner::Kingdom("alder".to_owned());
        let update = tick_hold(&owner, 40, 1, HOLD_TICKS - 1, 0, 8, 8);
        assert!(!update.captured);
        assert_eq!(update.occupy_hold, HOLD_TICKS - 2);
    }

    #[test]
    fn villagers_retake_a_pillager_town() {
        let owner = TownOwner::Pillager;
        let mut hold = 0;
        let mut update = HoldUpdate {
            occupy_hold: 0,
            retake_hold: 0,
            captured: false,
            retaken: false,
        };
        for _ in 0..HOLD_TICKS {
            update = tick_hold(&owner, 0, 8, 0, hold, 8, 8);
            hold = update.retake_hold;
        }
        assert!(update.retaken);
        assert!(!update.captured);
    }

    #[test]
    fn kingdom_army_can_occupy_a_pillager_town() {
        let owner = TownOwner::Pillager;
        let mut hold = 0;
        let mut update = HoldUpdate {
            occupy_hold: 0,
            retake_hold: 0,
            captured: false,
            retaken: false,
        };
        for _ in 0..HOLD_TICKS {
            update = tick_hold(&owner, 8, 0, hold, 0, 8, 8);
            hold = update.occupy_hold;
        }
        assert!(update.captured);
        assert!(!update.retaken);
    }

    #[test]
    fn ram_crew_are_every_eighth_plus_vindicators() {
        assert_eq!(MarchJob::assign(0, false), MarchJob::Ram);
        assert_eq!(MarchJob::assign(1, false), MarchJob::Ladder);
        assert_eq!(MarchJob::assign(2, false), MarchJob::Assault);
        assert_eq!(MarchJob::assign(3, true), MarchJob::Ram);
    }

    #[test]
    fn renown_ranks_from_kills_captures_and_defenses() {
        let levy = PlayerStats::default();
        assert_eq!(levy.renown(), 0);
        assert_eq!(levy.rank(), Rank::Levy);
        let freeman = PlayerStats {
            kills: 10,
            ..PlayerStats::default()
        };
        assert_eq!(freeman.rank(), Rank::Freeman);
        let sergeant = PlayerStats {
            captures: 5,
            ..PlayerStats::default()
        };
        assert_eq!(sergeant.renown(), 25);
        assert_eq!(sergeant.rank(), Rank::Sergeant);
        let knight = PlayerStats {
            kills: 20,
            captures: 4,
            defenses: 4,
            ..PlayerStats::default()
        };
        assert_eq!(knight.renown(), 52);
        assert_eq!(knight.rank(), Rank::Knight);
        let banneret = PlayerStats {
            captures: 20,
            ..PlayerStats::default()
        };
        assert_eq!(banneret.rank(), Rank::Banneret);
    }

    #[test]
    fn player_stats_survive_a_file_round_trip() {
        let dir = std::env::temp_dir().join(format!("kingdoms-stats-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = ownership_path(&dir);
        let mut stats = HashMap::new();
        stats.insert(
            "uuid-1".to_owned(),
            PlayerStats {
                kills: 3,
                captures: 1,
                defenses: 2,
                banner_given: true,
            },
        );
        save_state(
            &path,
            &SavedState {
                stats,
                ..SavedState::default()
            },
        )
        .unwrap();
        let back = load_state(&path);
        let got = back.stats.get("uuid-1").expect("stats");
        assert_eq!(got.kills, 3);
        assert_eq!(got.captures, 1);
        assert_eq!(got.defenses, 2);
        assert!(got.banner_given);
        assert_eq!(got.renown(), 14);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
