//! Abstract (LOD) combat for fronts with no player nearby.
//!
//! Remote fronts keep virtual raider and defender counts and resolve a
//! Lanchester-style fight on a timer. Entities spawn only when a player is
//! within [`PLAYER_LIVE_RANGE`].

/// Players closer than this (blocks) force a front to materialise.
pub const PLAYER_LIVE_RANGE: f64 = 128.0;
/// Start force-loading the battle line before the front goes live.
pub const PLAYER_APPROACH_RANGE: f64 = 256.0;
/// How often an abstract front steps.
pub const ABSTRACT_PERIOD: u32 = 20;
/// Ticks of marching before courtyard combat starts.
pub const ABSTRACT_MARCH_TICKS: u32 = 400;
/// Default cap on live (materialised) soldiers the war clock respects.
pub const DEFAULT_BODY_BUDGET: usize = 240;
/// Camp pillagers inside this Chebyshev radius do not keep the town alarm up.
pub const CAMP_EXCLUDE: i32 = 32;
/// Engine crews at ram / trebuchet / tower posts do not ring the bell.
pub const ENGINE_EXCLUDE: i32 = 24;
/// Hostiles this close to the keep count as in-town when the wall poly is empty.
pub const KEEP_ALARM_RADIUS: i32 = 48;
/// Hostiles this close to the curtain count as at the walls.
pub const WALL_ALARM_PAD: i32 = 6;

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
    near_posts(camps, x, z, CAMP_EXCLUDE)
}

/// Chebyshev test against a list of posts.
#[must_use]
pub fn near_posts(posts: &[(i32, i32)], x: i32, z: i32, range: i32) -> bool {
    posts
        .iter()
        .any(|(px, pz)| (x - px).abs() <= range && (z - pz).abs() <= range)
}

/// True when `(x, z)` is inside the curtain, on the wall, or in the keep radius.
#[must_use]
pub fn inside_town_for_alarm(keep: (i32, i32), walls: &[(i32, i32)], x: i32, z: i32) -> bool {
    if walls.len() >= 3
        && (crate::geom::point_in_poly(x, z, walls)
            || crate::geom::near_poly_edge(x, z, walls, WALL_ALARM_PAD))
    {
        return true;
    }
    let dx = i64::from(x - keep.0);
    let dz = i64::from(z - keep.1);
    dx * dx + dz * dz <= i64::from(KEEP_ALARM_RADIUS) * i64::from(KEEP_ALARM_RADIUS)
}

/// Town alarm counts only hostiles inside the walls or keep radius.
/// Camp leftovers and distant engine crews are ignored even if a raid is live.
#[must_use]
pub fn hostile_raises_alarm(
    keep: (i32, i32),
    walls: &[(i32, i32)],
    camps: &[(i32, i32)],
    engines: &[(i32, i32)],
    x: i32,
    z: i32,
) -> bool {
    if near_camp(camps, x, z) || near_posts(engines, x, z, ENGINE_EXCLUDE) {
        return false;
    }
    inside_town_for_alarm(keep, walls, x, z)
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
    fn alarm_counts_only_hostiles_inside_walls_or_keep() {
        let keep = (0, 0);
        let walls = [(10, 10), (10, -10), (-10, -10), (-10, 10)];
        let camps = [(80, 0)];
        let engines = [(60, 0)];
        assert!(hostile_raises_alarm(keep, &walls, &camps, &engines, 0, 0));
        assert!(hostile_raises_alarm(keep, &walls, &camps, &engines, 10, 0));
        assert!(!hostile_raises_alarm(keep, &walls, &camps, &engines, 80, 0));
        assert!(!hostile_raises_alarm(keep, &walls, &camps, &engines, 60, 0));
        assert!(!hostile_raises_alarm(keep, &walls, &camps, &engines, 40, 0));
        assert!(inside_town_for_alarm(keep, &[], 0, 0));
        assert!(!inside_town_for_alarm(keep, &[], KEEP_ALARM_RADIUS + 1, 0));
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
