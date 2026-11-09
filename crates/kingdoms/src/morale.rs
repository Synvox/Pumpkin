//! Morale: units rout when badly outnumbered or their captain dies.
//! A living captain rallies routed soldiers in range.

/// Foes per friend that triggers a rout, once at least [`ROUT_MIN_FOES`] are close.
pub const ROUT_RATIO: u32 = 3;
/// Do not rout a 2-vs-6 skirmish; need a real press.
pub const ROUT_MIN_FOES: u32 = 8;
/// Captains rally routed soldiers inside this radius.
pub const RALLY_RANGE: f64 = 16.0;
/// Local count radius for the outnumbered check.
pub const MORALE_RANGE: f64 = 16.0;

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
}
