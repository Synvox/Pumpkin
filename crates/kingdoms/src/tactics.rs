//! Medieval siege tactics: ladders, rams, and gate breaking.
//!
//! Pure per-tick helpers so the director can pick an approach, chip a gate,
//! and raise or topple ladders without pathing every bot. Capture still means
//! holding the keep flag plate; these rules never touch that column.

use crate::geom::Rect;
use crate::info::GateKind;
use crate::scene::{ScenePost, TownScene};
use crate::siege::MarchJob;

/// Hits a closed gate can take before it breaks.
pub const GATE_MAX_HP: u32 = 24;
/// Damage one ram stroke deals when two or more crew are at the gatehouse.
pub const RAM_HIT_DAMAGE: u32 = 4;
/// Ticks between ram strokes at a closed gate.
pub const RAM_HIT_PERIOD: u32 = 20;
/// Extra ram-frame damage per defender on the walk above the gatehouse.
pub const MURDER_HOLE_CHIP: f32 = 1.6;
/// How close a ladder crew must stand to plant.
pub const LADDER_PLACE_RANGE: f64 = 3.5;
/// How close a wall-walk defender must stand to shove a ladder.
pub const LADDER_TOPPLE_RANGE: f64 = 2.75;
/// Ladder blocks a crew plants per tick (amortised writes).
pub const LADDER_BLOCKS_PER_TICK: u32 = 2;
/// Attackers this close count as approaching a gate.
pub const GATE_APPROACH_RANGE: f64 = 16.0;
/// Defenders this close to a ram are sallying.
pub const SALLY_RANGE: f64 = 6.0;
/// Crew needed on a ram before it may strike.
pub const RAM_CREW_MIN: u32 = 2;
/// Attackers at a closed gate that make defenders drop the portcullis.
pub const CLOSE_ON_APPROACH: u32 = 3;
/// Defenders at the ram that open a sally port.
pub const SALLY_MIN: u32 = 2;

/// How the director spends this siege.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiegeApproach {
    /// Carry ladders to low curtain points and fight on the wall walk.
    Ladder,
    /// Crew a covered ram against the gatehouse.
    Ram,
    /// Plant breach charges (round 22 sappers).
    Sapper,
}

impl SiegeApproach {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ladder => "ladder",
            Self::Ram => "ram",
            Self::Sapper => "sapper",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "ladder" => Self::Ladder,
            "sapper" => Self::Sapper,
            _ => Self::Ram,
        }
    }
}

/// Portcullis / gate leaf as exposed on `/state` and `/war`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    /// Leaves open (peacetime, or a sally port).
    Open,
    /// Iron-bar portcullis down.
    Closed,
    /// Ram (or storm) has broken the leaves. Builders restore from the plan.
    Broken,
}

impl GateState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Broken => "broken",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "closed" => Self::Closed,
            "broken" => Self::Broken,
            _ => Self::Open,
        }
    }
}

/// Cheap read of a town's defences, used once when a raid starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DefenceProfile {
    pub gate_count: u32,
    pub tower_count: u32,
    pub wall_samples: u32,
    pub wall_height_min: i32,
    pub wall_height_median: i32,
    pub low_points: u32,
    pub capital: bool,
}

/// One live assault ladder on the outer face of a curtain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveLadder {
    pub wall_x: i32,
    pub wall_z: i32,
    pub foot_x: i32,
    pub foot_z: i32,
    pub floor_y: i32,
    pub remaining: u32,
}

/// Result of one cheap gate tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GateTick {
    pub state: GateState,
    pub hp: u32,
    pub ram_hit: bool,
    pub broken: bool,
    pub closed: bool,
    pub opened_for_sally: bool,
}

/// Sample wall-walk posts into a defence profile.
#[must_use]
pub fn defence_profile(
    wall_posts: &[(i32, i32, i32)],
    gate_count: usize,
    tower_count: usize,
    capital: bool,
) -> DefenceProfile {
    if wall_posts.is_empty() {
        return DefenceProfile {
            gate_count: gate_count as u32,
            tower_count: tower_count as u32,
            wall_samples: 0,
            wall_height_min: 0,
            wall_height_median: 0,
            low_points: 0,
            capital,
        };
    }
    let mut heights: Vec<i32> = wall_posts.iter().map(|p| p.1).collect();
    heights.sort_unstable();
    let mid = heights[heights.len() / 2];
    let min = heights[0];
    let low = heights.iter().filter(|h| **h + 4 <= mid).count() as u32;
    DefenceProfile {
        gate_count: gate_count as u32,
        tower_count: tower_count as u32,
        wall_samples: wall_posts.len() as u32,
        wall_height_min: min,
        wall_height_median: mid,
        low_points: low,
        capital,
    }
}

/// Profile from a town scene. Uses archer/guard Y as wall-walk samples.
#[must_use]
pub fn profile_from_scene(scene: &dyn TownScene) -> DefenceProfile {
    let keep_y = scene.keep().y;
    let mut posts: Vec<(i32, i32, i32)> = scene
        .archer_posts()
        .iter()
        .chain(scene.guard_posts())
        .map(|p| (p.x, p.height_or(keep_y), p.z))
        .collect();
    if posts.is_empty() {
        posts = scene
            .wall()
            .iter()
            .step_by(8)
            .map(|w| (w[0], keep_y, w[1]))
            .collect();
    }
    let capital = scene.town_id().ends_with(":0:0");
    defence_profile(
        &posts,
        scene.gates().len(),
        scene
            .archer_posts()
            .len()
            .max(scene.guard_posts().len() / 2),
        capital,
    )
}

/// Lowest curtain posts, camp-side, for ladder crews.
#[must_use]
pub fn low_wall_points(posts: &[(i32, i32, i32)], max: usize) -> Vec<(i32, i32, i32)> {
    if posts.is_empty() || max == 0 {
        return Vec::new();
    }
    let mut heights: Vec<i32> = posts.iter().map(|p| p.1).collect();
    heights.sort_unstable();
    let mid = heights[heights.len() / 2];
    let mut low: Vec<(i32, i32, i32)> = posts
        .iter()
        .copied()
        .filter(|p| p.1 + 4 <= mid || p.1 == heights[0])
        .collect();
    if low.is_empty() {
        low = posts.to_vec();
    }
    low.sort_by_key(|p| p.1);
    low.truncate(max);
    low
}

/// Mix so neighbouring towns do not all pick the same stroke.
#[must_use]
pub fn mix_from_id(id: &str) -> u32 {
    let mut h = 2_166_136_261u32;
    for b in id.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(16_777_619);
    }
    h
}

/// Pick ladder, ram, or sapper from the town's defences and raid size.
#[must_use]
pub fn pick_approach(profile: &DefenceProfile, raid_size: u16, mix: u32) -> SiegeApproach {
    let sapper_ok = raid_size >= crate::blast::SAPPER_RAID_MIN;
    let high_walls = profile.wall_height_median >= profile.wall_height_min.saturating_add(6)
        && profile.low_points < 3;
    let strong_gate = profile.gate_count >= 2 && profile.tower_count >= 4;
    if sapper_ok && (profile.capital || high_walls) && profile.low_points < 2 && mix % 3 == 0 {
        return SiegeApproach::Sapper;
    }
    if profile.low_points >= 3 || (!high_walls && profile.low_points >= 1 && mix % 2 == 0) {
        return SiegeApproach::Ladder;
    }
    if strong_gate || profile.gate_count > 0 {
        return SiegeApproach::Ram;
    }
    if sapper_ok && mix % 2 == 1 {
        SiegeApproach::Sapper
    } else {
        SiegeApproach::Ladder
    }
}

/// Attackers may occupy the keep plate. A ram siege waits until the gate is broken.
#[must_use]
pub fn plate_open(approach: SiegeApproach, gate_breached: bool) -> bool {
    gate_breached || approach != SiegeApproach::Ram
}

/// Ladder crew size for a wave on a ladder approach.
#[must_use]
pub fn ladder_crew_size(count: usize) -> usize {
    if count < 8 {
        0
    } else {
        (count / 8).clamp(2, 8)
    }
}

/// Extra sappers when the primary approach is sapper.
#[must_use]
pub fn sapper_crew_size(raid_size: u16) -> usize {
    crate::blast::sapper_count(raid_size).max(if raid_size >= crate::blast::SAPPER_RAID_MIN {
        3
    } else {
        0
    })
}

/// Job mix for one wave under a chosen approach. Ram/ladder crews come first.
#[must_use]
pub fn job_for_wave(index: usize, count: usize, approach: SiegeApproach) -> MarchJob {
    match approach {
        SiegeApproach::Ram => MarchJob::for_wave_index(index, count),
        SiegeApproach::Ladder => {
            let ladder_n = ladder_crew_size(count);
            if index < ladder_n {
                return MarchJob::Ladder;
            }
            let sapper_n = crate::blast::sapper_count(count as u16);
            if index < ladder_n + sapper_n {
                return MarchJob::Sapper;
            }
            let rest = index - ladder_n - sapper_n;
            if rest % 3 == 0 {
                MarchJob::Assault
            } else {
                MarchJob::Crossbow
            }
        }
        SiegeApproach::Sapper => {
            let sapper_n = sapper_crew_size(count as u16);
            if index < sapper_n {
                return MarchJob::Sapper;
            }
            let rest = index - sapper_n;
            if rest % 3 == 0 {
                MarchJob::Assault
            } else {
                MarchJob::Crossbow
            }
        }
    }
}

/// `(ram, trebuchet, tower)` engines this approach actually fields.
#[must_use]
pub fn engines_for_approach(raiders: u16, approach: SiegeApproach) -> (bool, bool, bool) {
    let (ram, treb) = crate::engines::engines_for_raid(raiders);
    match approach {
        SiegeApproach::Ram => (ram, treb, false),
        SiegeApproach::Ladder => (false, treb, raiders >= 8),
        SiegeApproach::Sapper => (false, treb, false),
    }
}

/// True when a ladder crew standing `dist` from the foot may plant.
#[must_use]
pub fn can_place_ladder(dist: f64, remaining: u32, height: u32) -> bool {
    dist <= LADDER_PLACE_RANGE && remaining < height
}

/// Blocks a defender shove knocks off this tick.
#[must_use]
pub fn topple_blocks(defenders_on_walk: u32, remaining: u32) -> u32 {
    if defenders_on_walk == 0 || remaining == 0 {
        0
    } else {
        remaining.min(LADDER_BLOCKS_PER_TICK).min(defenders_on_walk)
    }
}

/// True when a climber may step onto the wall walk.
#[must_use]
pub fn climb_ready(remaining: u32, height: u32) -> bool {
    remaining >= height && height > 0
}

/// Outer-face foot of a wall column, toward the camp.
#[must_use]
pub fn ladder_foot(wall: (i32, i32), camp: (i32, i32)) -> (i32, i32, i32, i32) {
    let mut fx = (camp.0 - wall.0).signum();
    let mut fz = (camp.1 - wall.1).signum();
    if fx == 0 && fz == 0 {
        fz = 1;
    } else if fx.abs() >= fz.abs() {
        fz = 0;
    } else {
        fx = 0;
    }
    (wall.0 + fx, wall.1 + fz, fx, fz)
}

/// Chebyshev distance on XZ.
#[must_use]
pub fn chebyshev(ax: i32, az: i32, bx: i32, bz: i32) -> i32 {
    (ax - bx).abs().max((az - bz).abs())
}

/// Push a raid camp out of the keep so rams and ladders approach a real gate.
#[must_use]
pub fn camp_outside(
    camp: ScenePost,
    keep: ScenePost,
    keep_rect: Rect,
    gate: Option<ScenePost>,
) -> ScenePost {
    if chebyshev(camp.x, camp.z, keep.x, keep.z) >= 32 && !keep_rect.contains(camp.x, camp.z) {
        return camp;
    }
    if let Some(gate) = gate {
        let mut fx = (gate.x - keep.x).signum();
        let fz = (gate.z - keep.z).signum();
        if fx == 0 && fz == 0 {
            fx = 1;
        }
        return ScenePost::new(gate.x + fx * 40, gate.z + fz * 40);
    }
    ScenePost::new(keep.x + 48, keep.z + 48)
}

/// Apply one ram stroke. `crew` must meet [`RAM_CREW_MIN`].
#[must_use]
pub fn apply_ram_hit(hp: u32, crew: u32, murder_holes: u32) -> (u32, u32) {
    if crew < RAM_CREW_MIN || hp == 0 {
        return (hp, 0);
    }
    let extra = murder_holes.min(3);
    let dmg = RAM_HIT_DAMAGE.saturating_add(extra);
    let next = hp.saturating_sub(dmg);
    (next, dmg)
}

/// Extra ram-frame chip from murder holes / machicolations.
#[must_use]
pub fn murder_hole_chip(defenders_above: u32) -> f32 {
    MURDER_HOLE_CHIP * defenders_above as f32
}

/// Gate leaf after approach, sally, and ram damage.
#[must_use]
pub fn tick_gate(
    state: GateState,
    attackers_near: u32,
    sallying: u32,
    hp: u32,
    ram_at_gate: bool,
    ram_crew: u32,
    murder_holes: u32,
) -> GateTick {
    if state == GateState::Broken || hp == 0 {
        return GateTick {
            state: GateState::Broken,
            hp: 0,
            ram_hit: false,
            broken: state != GateState::Broken,
            closed: false,
            opened_for_sally: false,
        };
    }
    if ram_at_gate && ram_crew >= RAM_CREW_MIN {
        let (next_hp, dmg) = apply_ram_hit(hp, ram_crew, murder_holes);
        if next_hp == 0 {
            return GateTick {
                state: GateState::Broken,
                hp: 0,
                ram_hit: dmg > 0,
                broken: true,
                closed: false,
                opened_for_sally: false,
            };
        }
        return GateTick {
            state: GateState::Closed,
            hp: next_hp,
            ram_hit: dmg > 0,
            broken: false,
            closed: state != GateState::Closed,
            opened_for_sally: false,
        };
    }
    if sallying >= SALLY_MIN && attackers_near < CLOSE_ON_APPROACH.saturating_mul(2) {
        return GateTick {
            state: GateState::Open,
            hp,
            ram_hit: false,
            broken: false,
            closed: false,
            opened_for_sally: state != GateState::Open,
        };
    }
    if attackers_near >= CLOSE_ON_APPROACH {
        return GateTick {
            state: GateState::Closed,
            hp,
            ram_hit: false,
            broken: false,
            closed: state != GateState::Closed,
            opened_for_sally: false,
        };
    }
    GateTick {
        state,
        hp,
        ram_hit: false,
        broken: false,
        closed: false,
        opened_for_sally: false,
    }
}

/// True when builders have put back enough of a broken gate to close it.
#[must_use]
pub fn gate_restored(broken: bool, gate_jobs_left: u32, placed_gate_blocks: u32) -> bool {
    broken && gate_jobs_left == 0 && placed_gate_blocks > 0
}

/// Keep and flag columns stay off-limits for ram punches, same as blasts.
#[must_use]
pub fn ram_punch_allowed(x: i32, z: i32, keep: Rect, flag: (i32, i32)) -> bool {
    !crate::blast::is_blast_protected(x, z, keep, flag)
}

/// Town or castle gate columns a ram may chip.
#[must_use]
pub fn is_gate_column(kind: GateKind) -> bool {
    matches!(kind, GateKind::Town | GateKind::Castle)
}

/// Dist from `(x,z)` to a post.
#[must_use]
pub fn dist_xz(x: f64, z: f64, post: ScenePost) -> f64 {
    (x - f64::from(post.x)).hypot(z - f64::from(post.z))
}

/// Count bodies within `range` of `post`.
#[must_use]
pub fn count_near_post(positions: &[(f64, f64)], post: ScenePost, range: f64) -> u32 {
    positions
        .iter()
        .filter(|(x, z)| dist_xz(*x, *z, post) <= range)
        .count() as u32
}

/// Wall-walk Y from a standing height.
#[must_use]
pub fn wall_walk_y(floor_y: i32) -> i32 {
    floor_y.saturating_add(crate::siege::WALL_WALK_LIFT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::TownInfo;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::scene::TownInfoScene;
    use crate::town::town_in_cell;

    fn sample_scene() -> TownInfoScene {
        let origin = town_in_cell(3, 0, 0);
        let grid = HeightGrid::synthetic_slope(
            origin.x - 360,
            origin.z - 360,
            origin.x + 360,
            origin.z + 360,
            8,
            70,
            8,
            4,
        );
        let plan = build_plan(3, origin, grid, PaletteKind::Oak);
        TownInfoScene::from_info(TownInfo::from_plan(&plan))
    }

    #[test]
    fn low_curtain_picks_ladders_tall_gatehouse_picks_ram() {
        let low = DefenceProfile {
            gate_count: 1,
            tower_count: 2,
            wall_samples: 8,
            wall_height_min: 70,
            wall_height_median: 74,
            low_points: 4,
            capital: false,
        };
        assert_eq!(pick_approach(&low, 24, 0), SiegeApproach::Ladder);
        let tall = DefenceProfile {
            gate_count: 3,
            tower_count: 8,
            wall_samples: 16,
            wall_height_min: 80,
            wall_height_median: 90,
            low_points: 0,
            capital: false,
        };
        assert_eq!(pick_approach(&tall, 24, 1), SiegeApproach::Ram);
        let capital = DefenceProfile {
            gate_count: 4,
            tower_count: 10,
            wall_samples: 20,
            wall_height_min: 86,
            wall_height_median: 96,
            low_points: 0,
            capital: true,
        };
        assert_eq!(pick_approach(&capital, 120, 0), SiegeApproach::Sapper);
        assert_eq!(pick_approach(&capital, 120, 1), SiegeApproach::Ram);
        assert!(!plate_open(SiegeApproach::Ram, false));
        assert!(plate_open(SiegeApproach::Ram, true));
        assert!(plate_open(SiegeApproach::Ladder, false));
        assert!(plate_open(SiegeApproach::Sapper, false));
    }

    #[test]
    fn ladder_jobs_replace_the_ram_crew() {
        let n = 12usize;
        assert_eq!(job_for_wave(0, n, SiegeApproach::Ladder), MarchJob::Ladder);
        assert_eq!(job_for_wave(1, n, SiegeApproach::Ladder), MarchJob::Ladder);
        assert_ne!(job_for_wave(0, n, SiegeApproach::Ram), MarchJob::Ladder);
        assert_eq!(job_for_wave(0, n, SiegeApproach::Ram), MarchJob::Ram);
        let sapper_n = sapper_crew_size(40);
        assert!(sapper_n >= 3);
        assert_eq!(job_for_wave(0, 40, SiegeApproach::Sapper), MarchJob::Sapper);
        assert_eq!(
            job_for_wave(sapper_n, 40, SiegeApproach::Sapper),
            MarchJob::Assault
        );
        let engines = engines_for_approach(16, SiegeApproach::Ladder);
        assert!(!engines.0);
        assert!(engines.1);
        assert!(engines.2);
        let ram = engines_for_approach(16, SiegeApproach::Ram);
        assert!(ram.0);
        assert!(!ram.2);
    }

    #[test]
    fn ram_hits_break_a_closed_gate_after_the_threshold() {
        let mut hp = GATE_MAX_HP;
        let mut hits = 0u32;
        let mut state = GateState::Closed;
        for _ in 0..16 {
            let tick = tick_gate(state, 8, 0, hp, true, 4, 0);
            hp = tick.hp;
            state = tick.state;
            if tick.ram_hit {
                hits += 1;
            }
            if tick.broken {
                break;
            }
        }
        assert!(hits >= 4, "hits {hits}");
        assert_eq!(state, GateState::Broken);
        assert_eq!(hp, 0);
        assert!(gate_restored(true, 0, 6));
        assert!(!gate_restored(true, 3, 6));
        assert!(!gate_restored(false, 0, 6));
    }

    #[test]
    fn defenders_close_on_approach_and_open_for_a_sally() {
        let closed = tick_gate(GateState::Open, 4, 0, GATE_MAX_HP, false, 0, 0);
        assert_eq!(closed.state, GateState::Closed);
        assert!(closed.closed);
        let sally = tick_gate(GateState::Closed, 2, 3, GATE_MAX_HP, false, 0, 0);
        assert_eq!(sally.state, GateState::Open);
        assert!(sally.opened_for_sally);
        let keep = tick_gate(GateState::Broken, 10, 10, 0, true, 6, 2);
        assert_eq!(keep.state, GateState::Broken);
        assert!(!keep.broken);
    }

    #[test]
    fn murder_holes_chip_the_ram_and_add_gate_damage() {
        let (hp, dmg) = apply_ram_hit(GATE_MAX_HP, 4, 2);
        assert_eq!(dmg, RAM_HIT_DAMAGE + 2);
        assert_eq!(hp, GATE_MAX_HP - dmg);
        assert_eq!(apply_ram_hit(10, 1, 4), (10, 0));
        assert!((murder_hole_chip(2) - 3.2).abs() < f32::EPSILON);
    }

    #[test]
    fn ladders_plant_at_low_points_and_topple() {
        let posts = [(0, 80, 0), (4, 70, 0), (8, 82, 0), (12, 71, 0), (16, 90, 0)];
        let low = low_wall_points(&posts, 2);
        assert_eq!(low.len(), 2);
        assert!(low.iter().all(|p| p.1 <= 71));
        let (fx, fz, dir_x, _) = ladder_foot((10, 10), (10, 0));
        assert_eq!((fx, fz), (10, 9));
        assert_eq!(dir_x, 0);
        assert!(can_place_ladder(2.0, 0, 6));
        assert!(!can_place_ladder(9.0, 0, 6));
        assert_eq!(topple_blocks(3, 6), 2);
        assert_eq!(topple_blocks(0, 6), 0);
        assert!(climb_ready(6, 6));
        assert!(!climb_ready(3, 6));
        let profile = defence_profile(&posts, 2, 4, false);
        assert_eq!(profile.wall_height_min, 70);
        assert!(profile.low_points >= 1);
    }

    #[test]
    fn keep_and_flag_are_off_limits_to_ram_punches() {
        let keep = Rect::new(10, 10, 20, 20);
        let flag = (12, 12);
        assert!(!ram_punch_allowed(15, 15, keep, flag));
        assert!(!ram_punch_allowed(12, 12, keep, flag));
        assert!(ram_punch_allowed(40, 40, keep, flag));
        assert!(is_gate_column(GateKind::Town));
        assert!(is_gate_column(GateKind::Castle));
        assert_eq!(wall_walk_y(80), 86);
        assert_eq!(GateState::Broken.as_str(), "broken");
        assert_eq!(SiegeApproach::parse("ladder"), SiegeApproach::Ladder);
        let keep = ScenePost::new(0, 0);
        let gate = ScenePost::new(20, 0);
        let inside = camp_outside(keep, keep, Rect::new(-8, -8, 8, 8), Some(gate));
        assert_eq!(inside, ScenePost::new(60, 0));
        let outside = camp_outside(
            ScenePost::new(80, 0),
            keep,
            Rect::new(-8, -8, 8, 8),
            Some(gate),
        );
        assert_eq!(outside, ScenePost::new(80, 0));
    }

    #[test]
    fn scene_profile_does_not_touch_the_flag_plate() {
        let scene = sample_scene();
        let profile = profile_from_scene(&scene);
        assert!(profile.gate_count >= 1);
        assert!(profile.wall_samples >= 1);
        let flag = scene.flag();
        let keep = scene.keep_rect();
        assert!(
            crate::blast::is_blast_protected(flag.x, flag.z, keep, (flag.x, flag.z)),
            "flag plate must stay off siege punches"
        );
        let mix = mix_from_id(scene.town_id());
        let approach = pick_approach(&profile, 40, mix);
        assert!(matches!(
            approach,
            SiegeApproach::Ladder | SiegeApproach::Ram | SiegeApproach::Sapper
        ));
        assert_ne!(mix_from_id("a"), mix_from_id("b"));
    }

    #[test]
    fn capture_still_means_holding_the_plate() {
        assert_eq!(crate::siege::HOLD_TICKS, 200);
        assert_eq!(crate::siege::capture_need(80), 4);
        let owner = crate::siege::TownOwner::Kingdom("alder".to_owned());
        let mut hold = 0;
        let mut update = crate::siege::HoldUpdate::default();
        for _ in 0..crate::siege::HOLD_TICKS {
            update = crate::siege::tick_hold(&owner, 4, 0, hold, 0, 4, 8);
            hold = update.occupy_hold;
        }
        assert!(update.captured);
        update = crate::siege::tick_hold(&owner, 4, 4, 40, 0, 4, 8);
        assert!(!update.captured);
        assert!(update.paused);
        assert_eq!(update.occupy_hold, 40);
    }
}
