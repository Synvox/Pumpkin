//! Captain bounties posted when a town is threatened.
//!
//! The town captain asks nearby enlisted players to destroy a ram, hold the
//! flag plate, or escort a supply cart. Completing one adds rank progress and
//! raises town morale.

/// Ticks an enlisted player must stand on the plate (~30 s).
pub const HOLD_PLATE_TICKS: u32 = 600;
/// Player this close to the cart counts as escorting it.
pub const ESCORT_RANGE: f64 = 16.0;
/// Cart has arrived when this close to the keep.
pub const CART_KEEP_RANGE: f64 = 8.0;
/// Blocks the cart slides toward the keep each tick while escorted.
pub const CART_STEP: f64 = 0.18;
/// Morale granted on a completed bounty.
pub const BOUNTY_MORALE: i32 = 15;
/// Default town morale before any bounty.
pub const TOWN_MORALE_START: i32 = 50;
/// Cap after bounties.
pub const TOWN_MORALE_MAX: i32 = 100;
/// Renown granted per completed bounty (see [`crate::siege::PlayerStats::renown`]).
pub const BOUNTY_RENOWN: u32 = 8;

/// One posted job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BountyKind {
    DestroyRam,
    HoldPlate,
    EscortCart,
}

impl BountyKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DestroyRam => "destroy_ram",
            Self::HoldPlate => "hold_plate",
            Self::EscortCart => "escort_cart",
        }
    }

    #[must_use]
    pub const fn line(self) -> &'static str {
        match self {
            Self::DestroyRam => "Bring down that ram before it splits the gate.",
            Self::HoldPlate => "Hold the flag plate for thirty breaths. Do not yield.",
            Self::EscortCart => "Escort the supply cart to the keep. Stay with it.",
        }
    }
}

/// Choose the job that matches the fight in front of the town.
#[must_use]
pub const fn pick(ram_alive: bool, plate_contested: bool) -> BountyKind {
    if ram_alive {
        BountyKind::DestroyRam
    } else if plate_contested {
        BountyKind::HoldPlate
    } else {
        BountyKind::EscortCart
    }
}

/// True when the ram this bounty named is gone.
#[must_use]
pub const fn ram_complete(ram_alive: bool) -> bool {
    !ram_alive
}

/// True when the player has held the plate long enough.
#[must_use]
pub const fn plate_complete(held_ticks: u32) -> bool {
    held_ticks >= HOLD_PLATE_TICKS
}

/// True when the escorted cart has reached the keep.
#[must_use]
pub fn cart_complete(dist_keep: f64) -> bool {
    dist_keep <= CART_KEEP_RANGE
}

/// Raise town morale, capped.
#[must_use]
pub const fn apply_morale(current: i32) -> i32 {
    let next = current.saturating_add(BOUNTY_MORALE);
    if next > TOWN_MORALE_MAX {
        TOWN_MORALE_MAX
    } else {
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_prefers_the_live_ram() {
        assert_eq!(pick(true, true), BountyKind::DestroyRam);
        assert_eq!(pick(false, true), BountyKind::HoldPlate);
        assert_eq!(pick(false, false), BountyKind::EscortCart);
        assert_eq!(BountyKind::DestroyRam.as_str(), "destroy_ram");
        assert!(BountyKind::HoldPlate.line().contains("plate"));
        assert!(BountyKind::EscortCart.line().contains("cart"));
    }

    #[test]
    fn complete_checks_match_the_posted_job() {
        assert!(!ram_complete(true));
        assert!(ram_complete(false));
        assert!(!plate_complete(HOLD_PLATE_TICKS - 1));
        assert!(plate_complete(HOLD_PLATE_TICKS));
        assert!(!cart_complete(CART_KEEP_RANGE + 0.1));
        assert!(cart_complete(CART_KEEP_RANGE));
        assert!(cart_complete(0.0));
    }

    #[test]
    fn bounty_raises_town_morale_to_the_cap() {
        assert_eq!(
            apply_morale(TOWN_MORALE_START),
            TOWN_MORALE_START + BOUNTY_MORALE
        );
        assert_eq!(apply_morale(TOWN_MORALE_MAX), TOWN_MORALE_MAX);
        assert_eq!(apply_morale(TOWN_MORALE_MAX - 1), TOWN_MORALE_MAX);
    }
}
