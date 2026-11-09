//! Abstract (LOD) combat for fronts with no player nearby.
//!
//! Remote fronts keep virtual raider and defender counts and resolve a
//! Lanchester-style fight on a timer. Entities spawn only when a player is
//! within [`PLAYER_LIVE_RANGE`].

/// Players closer than this (blocks) force a front to materialise.
pub const PLAYER_LIVE_RANGE: f64 = 128.0;
/// How often an abstract front steps.
pub const ABSTRACT_PERIOD: u32 = 20;
/// Ticks of marching before courtyard combat starts.
pub const ABSTRACT_MARCH_TICKS: u32 = 400;
/// Default cap on live (materialised) soldiers the war clock respects.
pub const DEFAULT_BODY_BUDGET: usize = 240;
/// Camp pillagers inside this Chebyshev radius do not keep the town alarm up.
pub const CAMP_EXCLUDE: i32 = 32;

/// Process-wide live-body cap. Override with `KINGDOMS_BODY_BUDGET`.
#[must_use]
pub fn body_budget() -> usize {
    std::env::var("KINGDOMS_BODY_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &usize| *n > 0)
        .unwrap_or(DEFAULT_BODY_BUDGET)
}

/// Virtual army for a remote front.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbstractState {
    pub raiders: u16,
    pub defenders: u16,
    pub march: u32,
}

/// One timer step: road attrition until [`ABSTRACT_MARCH_TICKS`], then
/// Lanchester casualties. Occupy / capture is applied by the director from
/// the resulting keep counts.
#[must_use]
pub fn step_abstract(mut state: AbstractState, period: u32) -> AbstractState {
    state.march = state.march.saturating_add(period);
    if state.march < ABSTRACT_MARCH_TICKS {
        state.raiders = state.raiders.saturating_sub(1);
        state.defenders = state.defenders.saturating_sub(1);
        return state;
    }
    if state.raiders > 0 && state.defenders > 0 {
        let r_loss = u16::try_from((u32::from(state.defenders) / 8).max(1)).unwrap_or(1);
        let d_loss = u16::try_from((u32::from(state.raiders) / 6).max(1)).unwrap_or(1);
        state.raiders = state.raiders.saturating_sub(r_loss);
        state.defenders = state.defenders.saturating_sub(d_loss);
    }
    state
}

/// True when leftover camp soldiers at `(x, z)` should not ring the town bell.
#[must_use]
pub fn near_camp(camps: &[(i32, i32)], x: i32, z: i32) -> bool {
    camps
        .iter()
        .any(|(cx, cz)| (x - cx).abs() <= CAMP_EXCLUDE && (z - cz).abs() <= CAMP_EXCLUDE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::siege::{HOLD_TICKS, TownOwner, capture_need, tick_hold_n};

    #[test]
    fn hundred_vs_hundred_captures_after_the_march() {
        let mut state = AbstractState {
            raiders: 100,
            defenders: 100,
            march: 0,
        };
        let owner = TownOwner::kingdom(&crate::faction::KingdomId::new("alder"));
        let mut occupy = 0;
        let mut captured = false;
        for _ in 0..80 {
            state = step_abstract(state, ABSTRACT_PERIOD);
            if state.raiders == 0 {
                break;
            }
            let in_keep = u32::from(state.raiders);
            let def_keep = u32::from(state.defenders);
            let need = capture_need(in_keep);
            let update = tick_hold_n(
                &owner,
                in_keep,
                def_keep,
                occupy,
                0,
                need,
                8,
                ABSTRACT_PERIOD,
            );
            occupy = update.occupy_hold;
            if update.captured {
                captured = true;
                break;
            }
        }
        assert!(captured, "raiders={state:?} occupy={occupy}");
        assert!(state.march >= ABSTRACT_MARCH_TICKS);
        assert_eq!(state.defenders, 0);
    }

    #[test]
    fn defenders_can_wipe_the_raid() {
        let mut state = AbstractState {
            raiders: 8,
            defenders: 80,
            march: ABSTRACT_MARCH_TICKS,
        };
        for _ in 0..40 {
            state = step_abstract(state, ABSTRACT_PERIOD);
            if state.raiders == 0 {
                break;
            }
        }
        assert_eq!(state.raiders, 0);
        assert!(state.defenders > 0);
    }

    #[test]
    fn march_does_not_occupy_the_keep() {
        let state = step_abstract(
            AbstractState {
                raiders: 40,
                defenders: 40,
                march: 0,
            },
            ABSTRACT_PERIOD,
        );
        assert!(state.march < ABSTRACT_MARCH_TICKS);
        let owner = TownOwner::kingdom(&crate::faction::KingdomId::new("alder"));
        let update = tick_hold_n(
            &owner,
            0,
            u32::from(state.defenders),
            0,
            0,
            8,
            8,
            ABSTRACT_PERIOD,
        );
        assert!(!update.captured);
        assert_eq!(update.occupy_hold, 0);
    }

    #[test]
    fn camp_hostiles_are_excluded_from_the_alarm() {
        let camps = [(100, 100), (200, 50)];
        assert!(near_camp(&camps, 110, 90));
        assert!(!near_camp(&camps, 0, 0));
        assert!(near_camp(&camps, 100 + CAMP_EXCLUDE, 100));
        assert!(!near_camp(&camps, 100 + CAMP_EXCLUDE + 1, 100));
    }

    #[test]
    fn occupy_jumps_by_the_abstract_period() {
        let owner = TownOwner::kingdom(&crate::faction::KingdomId::new("alder"));
        let mut occupy = HOLD_TICKS.saturating_sub(ABSTRACT_PERIOD);
        let update = tick_hold_n(&owner, 40, 0, occupy, 0, 8, 8, ABSTRACT_PERIOD);
        assert!(update.captured);
        occupy = 0;
        let update = tick_hold_n(&owner, 40, 0, occupy, 0, 8, 8, ABSTRACT_PERIOD);
        assert!(!update.captured);
        assert_eq!(update.occupy_hold, ABSTRACT_PERIOD);
    }
}
