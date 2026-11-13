//! Inter-town supply carts: roads, ambush, and cutoff morale.

/// One cart at a time per kingdom unless more towns need a run.
pub const SUPPLY_CAP: usize = 3;
/// Spawn a new cart this often (~60 s).
pub const CART_PERIOD: u32 = 1200;
/// No delivery for this long counts as cut off (~5 min).
pub const CUTOFF_TICKS: u32 = 6000;
/// Drain 1 morale this often while cut off (~10 s).
pub const CUTOFF_MORALE_PERIOD: u32 = 200;
pub const CUTOFF_MORALE_STEP: i32 = 1;
/// Hostile this close to the cart can ambush it.
pub const AMBUSH_RANGE: f64 = 8.0;
/// Ticks a hostile must stay in range before the cart is lost.
pub const AMBUSH_TICKS: u32 = 20;
/// Waypoint reached.
pub const WAYPOINT_RANGE: f64 = 4.0;
/// Same slide as the escort bounty.
pub const CART_STEP: f64 = crate::bounty::CART_STEP;
/// 8-neighbour offsets so checkerboard kingdoms still share a corner road.
pub const NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Cell of `seed:x:z`.
#[must_use]
pub fn town_cell(town_id: &str) -> Option<(u64, i32, i32)> {
    let mut parts = town_id.split(':');
    let seed: u64 = parts.next()?.parse().ok()?;
    let x: i32 = parts.next()?.parse().ok()?;
    let z: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((seed, x, z))
}

/// Same-kingdom towns that share a side or a corner.
#[must_use]
pub fn neighbor_ids(town_id: &str) -> Vec<String> {
    let Some((seed, x, z)) = town_cell(town_id) else {
        return Vec::new();
    };
    NEIGHBOR_OFFSETS
        .iter()
        .map(|(dx, dz)| format!("{seed}:{}:{}", x + dx, z + dz))
        .collect()
}

/// True when an adjacent town is still held by `kingdom`.
#[must_use]
pub fn has_friendly_neighbor(town_id: &str, owner_of: &[(String, String)], kingdom: &str) -> bool {
    let neighbors = neighbor_ids(town_id);
    owner_of
        .iter()
        .any(|(id, owner)| owner == kingdom && neighbors.iter().any(|n| n == id))
}

/// Cut off when there is no friendly neighbour, the inbound cart was
/// destroyed, or no cart has arrived for [`CUTOFF_TICKS`].
#[must_use]
pub fn is_cutoff(
    has_neighbor: bool,
    inbound_lost: bool,
    in_flight: bool,
    delivery_age: u32,
) -> bool {
    if !has_neighbor {
        return true;
    }
    if in_flight {
        return false;
    }
    inbound_lost || delivery_age >= CUTOFF_TICKS
}

#[must_use]
pub const fn recruit_hit(base: usize, cutoff: bool) -> usize {
    if cutoff { base.saturating_sub(1) } else { base }
}

#[must_use]
pub fn morale_after_cutoff(morale: i32, cutoff: bool, due: bool) -> i32 {
    if cutoff && due {
        morale.saturating_sub(CUTOFF_MORALE_STEP).max(0)
    } else {
        morale
    }
}

/// Keep → road → road → keep. Empty optional posts are skipped.
#[must_use]
pub fn route(
    from_keep: (i32, i32),
    from_road: Option<(i32, i32)>,
    to_road: Option<(i32, i32)>,
    to_keep: (i32, i32),
) -> Vec<(i32, i32)> {
    let mut pts = vec![from_keep];
    for extra in [from_road, to_road, Some(to_keep)] {
        let Some(p) = extra else {
            continue;
        };
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    pts
}

#[must_use]
pub fn step_towards(x: f64, z: f64, tx: f64, tz: f64, step: f64) -> (f64, f64) {
    let dx = tx - x;
    let dz = tz - z;
    let dist = dx.hypot(dz);
    if dist <= step || dist < 0.001 {
        return (tx, tz);
    }
    (x + dx / dist * step, z + dz / dist * step)
}

#[must_use]
pub const fn should_ambush(hostile_near: bool, ticks_near: u32) -> bool {
    hostile_near && ticks_near >= AMBUSH_TICKS
}

#[must_use]
pub fn waypoint_reached(x: f64, z: f64, dest: (i32, i32)) -> bool {
    (x - f64::from(dest.0)).hypot(z - f64::from(dest.1)) <= WAYPOINT_RANGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_same_kingdom_towns_count_as_neighbours() {
        let ids = neighbor_ids("20261025:0:-1");
        assert!(ids.contains(&"20261025:-1:0".to_owned()));
        assert!(ids.contains(&"20261025:0:0".to_owned()));
        let owners = vec![
            ("20261025:0:-1".to_owned(), "raven".to_owned()),
            ("20261025:-1:0".to_owned(), "raven".to_owned()),
            ("20261025:0:0".to_owned(), "alder".to_owned()),
        ];
        assert!(has_friendly_neighbor("20261025:0:-1", &owners, "raven"));
        assert!(!has_friendly_neighbor("20261025:0:0", &owners, "alder"));
    }

    #[test]
    fn cutoff_when_isolated_destroyed_or_stale() {
        assert!(is_cutoff(false, false, false, 0));
        assert!(is_cutoff(true, true, false, 0));
        assert!(!is_cutoff(true, true, true, CUTOFF_TICKS));
        assert!(is_cutoff(true, false, false, CUTOFF_TICKS));
        assert!(!is_cutoff(true, false, false, CUTOFF_TICKS - 1));
        assert_eq!(recruit_hit(2, true), 1);
        assert_eq!(recruit_hit(0, true), 0);
        assert_eq!(recruit_hit(2, false), 2);
        assert_eq!(morale_after_cutoff(50, true, true), 49);
        assert_eq!(morale_after_cutoff(0, true, true), 0);
        assert_eq!(morale_after_cutoff(50, true, false), 50);
    }

    #[test]
    fn cart_follows_the_road_then_the_keep() {
        let pts = route((0, 0), Some((10, 0)), Some((10, 10)), (20, 10));
        assert_eq!(pts, vec![(0, 0), (10, 0), (10, 10), (20, 10)]);
        let (x, z) = step_towards(0.0, 0.0, 10.0, 0.0, 2.0);
        assert!((x - 2.0).abs() < 1e-6 && z.abs() < 1e-6);
        assert!(waypoint_reached(10.0, 0.0, (10, 0)));
        assert!(!waypoint_reached(0.0, 0.0, (10, 0)));
        assert!(should_ambush(true, AMBUSH_TICKS));
        assert!(!should_ambush(true, AMBUSH_TICKS - 1));
        assert!(!should_ambush(false, AMBUSH_TICKS));
    }
}
