//! Morale: units rout when badly outnumbered or their captain dies.
//! A living captain rallies routed soldiers in range.
//!
//! Kingdom shock: losing a town cuts recruitment and raises desertion until
//! the capital is held again.

/// Foes per friend that triggers a rout, once at least [`ROUT_MIN_FOES`] are close.
pub const ROUT_RATIO: u32 = 3;
/// Do not rout a 2-vs-6 skirmish; need a real press.
pub const ROUT_MIN_FOES: u32 = 8;
/// Captains rally routed soldiers inside this radius.
pub const RALLY_RANGE: f64 = 16.0;
/// Local count radius for the outnumbered check.
pub const MORALE_RANGE: f64 = 16.0;
/// How long a lost-town shock lasts (~5 min) unless the capital restores it.
pub const SHOCK_TICKS: u32 = 6000;
/// Recruitment drop per lost town during shock.
pub const LOST_TOWN_HIT: f32 = 0.2;
/// Floor so a kingdom is never empty of levies.
pub const MIN_RECRUITMENT: f32 = 0.4;

/// Shock state for one kingdom.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KingdomShock {
    pub lost_towns: u32,
    pub shock_until: u32,
}

/// Snapshot for `GET /state`.
#[derive(Clone, Debug, PartialEq)]
pub struct MoraleState {
    pub kingdom: String,
    pub recruitment: f32,
    pub desertion: f32,
    pub lost_towns: u32,
    pub shock_until: u32,
    pub holds_capital: bool,
}

/// True when this cluster should break and run.
#[must_use]
pub const fn should_rout(friends: u32, foes: u32) -> bool {
    foes >= ROUT_MIN_FOES && foes >= friends.saturating_mul(ROUT_RATIO)
}

/// Count friends and foes of `side_is_pillager` inside `range` of `(x, z)`.
#[must_use]
pub fn local_counts(
    positions: &[(f64, f64, bool)],
    x: f64,
    z: f64,
    range: f64,
    side_is_pillager: bool,
) -> (u32, u32) {
    let mut friends = 0u32;
    let mut foes = 0u32;
    for (px, pz, is_pillager) in positions {
        if (px - x).hypot(pz - z) > range {
            continue;
        }
        if *is_pillager == side_is_pillager {
            friends += 1;
        } else {
            foes += 1;
        }
    }
    (friends, foes)
}

/// Apply a lost-town shock.
#[must_use]
pub fn on_lost_town(shock: KingdomShock, tick: u32) -> KingdomShock {
    KingdomShock {
        lost_towns: shock.lost_towns.saturating_add(1),
        shock_until: tick.saturating_add(SHOCK_TICKS),
    }
}

/// Holding the capital clears shock once the timer elapses, or at once on retake.
#[must_use]
pub fn on_capital_held(shock: KingdomShock, tick: u32, retake: bool) -> KingdomShock {
    if retake || tick >= shock.shock_until {
        KingdomShock::default()
    } else {
        shock
    }
}

/// 1.0 = full levy. Holding the capital recovers toward 1 during shock.
#[must_use]
pub fn recruitment(shock: &KingdomShock, tick: u32, holds_capital: bool) -> f32 {
    if shock.lost_towns == 0 || tick >= shock.shock_until {
        return 1.0;
    }
    let hit = LOST_TOWN_HIT * shock.lost_towns as f32;
    let base = (1.0 - hit).max(MIN_RECRUITMENT);
    if !holds_capital {
        return base;
    }
    let remain = shock.shock_until.saturating_sub(tick);
    let t = remain as f32 / SHOCK_TICKS as f32;
    base + (1.0 - base) * (1.0 - t.clamp(0.0, 1.0))
}

/// Inverse of recruitment during shock.
#[must_use]
pub fn desertion(shock: &KingdomShock, tick: u32, holds_capital: bool) -> f32 {
    (1.0 - recruitment(shock, tick, holds_capital)).clamp(0.0, 1.0)
}

/// How many citizen respawns this tick (0..=2).
#[must_use]
pub fn recruit_budget(shock: &KingdomShock, tick: u32, holds_capital: bool) -> usize {
    let r = recruitment(shock, tick, holds_capital);
    if r >= 0.85 { 2 } else { usize::from(r >= 0.55) }
}

/// True when this soldier should desert this tick (deterministic, no RNG).
#[must_use]
pub fn should_desert(entity_id: i32, tick: u32, desertion: f32) -> bool {
    if desertion <= 0.05 {
        return false;
    }
    // At most one check per 40 ticks per id; threshold scales with desertion.
    if !tick.is_multiple_of(40) {
        return false;
    }
    let bucket = ((desertion * 10.0).ceil() as u32).clamp(1, 10);
    entity_id.unsigned_abs() % 10 < bucket && (entity_id.unsigned_abs() + tick) % 7 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_to_one_with_enough_foes_routes() {
        assert!(should_rout(4, 12));
        assert!(should_rout(10, 30));
        assert!(!should_rout(4, 7));
        assert!(!should_rout(20, 40));
        assert!(!should_rout(0, 7));
        assert!(should_rout(1, 8));
    }

    #[test]
    fn local_counts_split_sides() {
        let pos = vec![
            (0.0, 0.0, true),
            (1.0, 0.0, true),
            (2.0, 0.0, false),
            (100.0, 0.0, false),
        ];
        let (friends, foes) = local_counts(&pos, 0.0, 0.0, 8.0, true);
        assert_eq!(friends, 2);
        assert_eq!(foes, 1);
    }

    #[test]
    fn lost_town_cuts_recruitment_until_capital_restores() {
        let hit = on_lost_town(KingdomShock::default(), 100);
        assert_eq!(hit.lost_towns, 1);
        assert_eq!(hit.shock_until, 100 + SHOCK_TICKS);
        let r = recruitment(&hit, 100, false);
        assert!((r - 0.8).abs() < 0.001);
        assert!(desertion(&hit, 100, false) > 0.15);
        assert_eq!(recruit_budget(&hit, 100, false), 1);
        let restored = on_capital_held(hit.clone(), 100, true);
        assert_eq!(restored.lost_towns, 0);
        assert!((recruitment(&restored, 100, true) - 1.0).abs() < f32::EPSILON);
        let expired = on_capital_held(hit, 100 + SHOCK_TICKS, false);
        assert_eq!(expired.lost_towns, 0);
    }

    #[test]
    fn desertion_is_rare_and_deterministic() {
        assert!(!should_desert(1, 1, 0.0));
        assert!(!should_desert(1, 40, 0.0));
        let a = should_desert(3, 40, 0.6);
        let b = should_desert(3, 40, 0.6);
        assert_eq!(a, b);
    }
}
