//! Per-town damage state for capture and repair.
//!
//! The director sets [`DamageState::Scarred`] (or `Ruined`) on capture.
//! [`crate::stamp::planned_block`] then returns the damaged variant until
//! builders finish and the state is set back to [`DamageState::Intact`].

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// How intact a town's walls and houses currently are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DamageState {
    /// Undamaged plan. Builders have finished, or no siege has landed.
    #[default]
    Intact,
    /// Capture aftermath: gaps, scorch, rubble. The frontier scene is this look.
    Scarred,
    /// Heavier collapse. Director may set this for a sacked town.
    Ruined,
}

impl DamageState {
    /// Parse a stored string. Unknown values are intact.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "scarred" => Self::Scarred,
            "ruined" => Self::Ruined,
            _ => Self::Intact,
        }
    }

    /// Stable name for JSON / HTTP.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intact => "intact",
            Self::Scarred => "scarred",
            Self::Ruined => "ruined",
        }
    }

    /// True when planned blocks should punch holes and drop rubble.
    #[must_use]
    pub const fn is_damaged(self) -> bool {
        !matches!(self, Self::Intact)
    }
}

fn table() -> &'static RwLock<HashMap<String, DamageState>> {
    static TABLE: OnceLock<RwLock<HashMap<String, DamageState>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Current damage for `town_id` (`seed:cell_x:cell_z`). Defaults to intact.
#[must_use]
pub fn damage_state(town_id: &str) -> DamageState {
    table()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(town_id)
        .copied()
        .unwrap_or(DamageState::Intact)
}

/// Intact → Scarred on the first capture; Scarred → Ruined if taken again.
pub fn note_capture(town_id: &str) {
    let next = match damage_state(town_id) {
        DamageState::Intact => DamageState::Scarred,
        DamageState::Scarred | DamageState::Ruined => DamageState::Ruined,
    };
    set_damage_state(town_id, next);
}

/// One step toward intact: Ruined → Scarred, Scarred → Intact.
pub fn step_repair(town_id: &str) -> DamageState {
    let next = match damage_state(town_id) {
        DamageState::Ruined => DamageState::Scarred,
        DamageState::Scarred | DamageState::Intact => DamageState::Intact,
    };
    set_damage_state(town_id, next);
    next
}

/// Director / repair hook. `planned_block` reads this on the next stamp.
pub fn set_damage_state(town_id: impl Into<String>, state: DamageState) {
    let id = town_id.into();
    let mut guard = table()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state == DamageState::Intact {
        guard.remove(&id);
    } else {
        guard.insert(id, state);
    }
}

/// Snapshot for `kingdoms-ownership.json`.
#[must_use]
pub fn snapshot() -> HashMap<String, DamageState> {
    table()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Restore a snapshot loaded from disk.
#[allow(clippy::implicit_hasher)]
pub fn restore(states: &HashMap<String, DamageState>) {
    let mut guard = table()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.clear();
    for (id, state) in states {
        if state.is_damaged() {
            guard.insert(id.clone(), *state);
        }
    }
}

/// Damage for a cached plan's town id.
#[must_use]
pub fn damage_state_for_plan(plan: &crate::plan::TownPlan) -> DamageState {
    damage_state(&crate::info::town_id(
        plan.seed,
        plan.origin.cell_x,
        plan.origin.cell_z,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_round_trip_names() {
        assert_eq!(DamageState::parse("scarred"), DamageState::Scarred);
        assert_eq!(DamageState::parse("RUINED"), DamageState::Ruined);
        assert_eq!(DamageState::parse("nope"), DamageState::Intact);
        assert_eq!(DamageState::Scarred.as_str(), "scarred");
    }

    #[test]
    fn director_can_set_and_clear() {
        let id = "test-damage-state:0:0";
        set_damage_state(id, DamageState::Scarred);
        assert_eq!(damage_state(id), DamageState::Scarred);
        set_damage_state(id, DamageState::Ruined);
        assert_eq!(damage_state(id), DamageState::Ruined);
        set_damage_state(id, DamageState::Intact);
        assert_eq!(damage_state(id), DamageState::Intact);
    }

    #[test]
    fn capture_scars_then_ruins_and_repair_steps_back() {
        let id = "test-damage-capture:1:0";
        set_damage_state(id, DamageState::Intact);
        note_capture(id);
        assert_eq!(damage_state(id), DamageState::Scarred);
        note_capture(id);
        assert_eq!(damage_state(id), DamageState::Ruined);
        note_capture(id);
        assert_eq!(damage_state(id), DamageState::Ruined);
        assert_eq!(step_repair(id), DamageState::Scarred);
        assert_eq!(step_repair(id), DamageState::Intact);
        assert_eq!(step_repair(id), DamageState::Intact);
    }
}
