//! Civilian and guard routines between raids.
//!
//! Day: civilians walk to a workstation. Night: they go home to a bed.
//! Alarm: everyone flees to the keep. Guards patrol the wall when idle.

/// Vanilla day length in ticks.
pub const DAY_TICKS: i64 = 24_000;
/// Work posts between morning and late afternoon.
pub const WORK_START: i64 = 2_000;
pub const WORK_END: i64 = 11_000;
/// Night begins at 12000 (dusk) in vanilla.
pub const NIGHT_START: i64 = 12_000;
/// Guards step to the next wall post this often.
pub const PATROL_PERIOD: u32 = 40;

/// Time of day in `0..DAY_TICKS`.
#[must_use]
pub const fn time_of_day(world_time: i64) -> i64 {
    let t = world_time % DAY_TICKS;
    if t < 0 { t + DAY_TICKS } else { t }
}

#[must_use]
pub const fn is_night(world_time: i64) -> bool {
    time_of_day(world_time) >= NIGHT_START
}

#[must_use]
pub const fn is_work_hours(world_time: i64) -> bool {
    let t = time_of_day(world_time);
    t >= WORK_START && t < WORK_END
}

/// Where a civilian should walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CivilianPost {
    Keep,
    Work,
    Home,
}

/// Alarm overrides the clock: flee to the keep. Otherwise work by day, home at night.
#[must_use]
pub const fn civilian_post(alarm: bool, world_time: i64) -> CivilianPost {
    if alarm {
        CivilianPost::Keep
    } else if is_work_hours(world_time) {
        CivilianPost::Work
    } else {
        CivilianPost::Home
    }
}

/// Wall-walk index that cycles while idle.
#[must_use]
pub fn patrol_index(entity_id: i32, tick: u32, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let step = tick / PATROL_PERIOD;
    (entity_id.unsigned_abs() / 7 + step) as usize % n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_night_split_at_dusk() {
        assert!(!is_night(0));
        assert!(!is_night(6_000));
        assert!(is_night(12_000));
        assert!(is_night(18_000));
        assert!(!is_night(24_000));
        assert!(is_work_hours(6_000));
        assert!(!is_work_hours(13_000));
        assert!(!is_work_hours(0));
    }

    #[test]
    fn civilians_flee_when_the_bell_rings() {
        assert_eq!(civilian_post(true, 6_000), CivilianPost::Keep);
        assert_eq!(civilian_post(false, 6_000), CivilianPost::Work);
        assert_eq!(civilian_post(false, 14_000), CivilianPost::Home);
    }

    #[test]
    fn patrol_walks_the_wall() {
        assert_eq!(patrol_index(0, 0, 4), 0);
        assert_ne!(patrol_index(0, 0, 8), patrol_index(0, PATROL_PERIOD, 8));
        assert_eq!(patrol_index(3, 10, 0), 0);
    }
}
