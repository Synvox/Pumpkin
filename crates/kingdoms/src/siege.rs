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

#[derive(Default, Serialize, Deserialize)]
struct OwnershipFile {
    towns: HashMap<String, String>,
    #[serde(default)]
    players: HashMap<String, String>,
}

/// World-folder path for the ownership file.
#[must_use]
pub fn ownership_path(world_root: &Path) -> PathBuf {
    world_root.join(OWNERSHIP_FILE)
}

/// Loaded towns and player kingdom membership.
#[derive(Default)]
pub struct SavedState {
    pub towns: HashMap<String, TownOwner>,
    pub players: HashMap<String, String>,
}

/// Load town id → owner from `kingdoms-ownership.json`.
#[must_use]
pub fn load_ownership(path: &Path) -> HashMap<String, TownOwner> {
    load_state(path).towns
}

/// Load towns and players from `kingdoms-ownership.json`.
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
    }
}

/// Persist town ownership. Overwrites the file.
#[allow(clippy::implicit_hasher)]
pub fn save_ownership(path: &Path, owners: &HashMap<String, TownOwner>) -> std::io::Result<()> {
    save_state(path, owners, &HashMap::new())
}

/// Persist towns and player membership. Overwrites the file.
#[allow(clippy::implicit_hasher)]
pub fn save_state(
    path: &Path,
    owners: &HashMap<String, TownOwner>,
    players: &HashMap<String, String>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OwnershipFile {
        towns: owners
            .iter()
            .map(|(id, owner)| (id.clone(), owner.as_str().to_owned()))
            .collect(),
        players: players.clone(),
    };
    let json = serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".to_owned());
    std::fs::write(path, json)
}

/// Advance a courtyard hold. Returns the new hold counter and a flip, if any.
#[must_use]
pub fn tick_hold(
    current: &TownOwner,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
    occupy_hold: u32,
    retake_hold: u32,
) -> HoldUpdate {
    let mut occupy = occupy_hold;
    let mut retake = retake_hold;
    if raiders_in_keep >= 3 && defenders_in_keep == 0 {
        occupy = occupy.saturating_add(1);
        retake = 0;
        if occupy >= HOLD_TICKS && !current.is_pillager() {
            return HoldUpdate {
                occupy_hold: 0,
                retake_hold: 0,
                captured: true,
                retaken: false,
            };
        }
    } else if defenders_in_keep >= 3 && raiders_in_keep == 0 {
        retake = retake.saturating_add(1);
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
        occupy = occupy.saturating_sub(1);
        retake = retake.saturating_sub(1);
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
    tracing::info!(
        "kingdoms ram breached the gate x={} z={} cleared={cleared}",
        gate.x,
        gate.z
    );
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
    fn capture_needs_raiders_in_the_keep_and_no_defenders() {
        let owner = TownOwner::Kingdom("alder".to_owned());
        let mut hold = 0;
        let mut update = tick_hold(&owner, 2, 0, hold, 0);
        assert!(!update.captured);
        hold = update.occupy_hold;
        for _ in 0..HOLD_TICKS {
            update = tick_hold(&owner, 5, 0, hold, 0);
            hold = update.occupy_hold;
        }
        assert!(update.captured);
        assert_eq!(update.occupy_hold, 0);
    }

    #[test]
    fn defenders_in_the_keep_block_capture() {
        let owner = TownOwner::Kingdom("alder".to_owned());
        let update = tick_hold(&owner, 10, 1, HOLD_TICKS - 1, 0);
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
            update = tick_hold(&owner, 0, 4, 0, hold);
            hold = update.retake_hold;
        }
        assert!(update.retaken);
        assert!(!update.captured);
    }

    #[test]
    fn ram_crew_are_every_eighth_plus_vindicators() {
        assert_eq!(MarchJob::assign(0, false), MarchJob::Ram);
        assert_eq!(MarchJob::assign(1, false), MarchJob::Ladder);
        assert_eq!(MarchJob::assign(2, false), MarchJob::Assault);
        assert_eq!(MarchJob::assign(3, true), MarchJob::Ram);
    }
}
