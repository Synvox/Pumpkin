//! Siege explosives: TNT kits, breach charges, sappers, and blast protection.
//!
//! Keep and flag-plate columns are immune so captures still need a hold on the
//! plate. Walls and gates take extra (configurable) damage so a small crew can
//! breach without a ram. Blast holes feed the existing builder-repair queue.

use std::sync::{Mutex, OnceLock};

use crate::buildings::BuildingKind;
use crate::cells::CellKind;
use crate::geom::Rect;
use crate::plan::TownPlan;
use crate::repair::RepairBand;
use crate::siege::{Rank, is_flag_column};

/// Extra Chebyshev radius around a blast that punches wall/gate blocks.
pub const DEFAULT_BREACH_BONUS: i32 = 2;
/// How far down a wall column extra breach punches go.
pub const DEFAULT_BREACH_DEPTH: i32 = 5;
/// Ticks before a sergeant+ can farm another TNT kit (~60 s).
pub const DEFAULT_TNT_KIT_COOLDOWN: u32 = 1200;
/// Emeralds the quartermaster asks for one TNT bundle.
pub const TNT_TRADE_EMERALDS: u8 = 2;
/// TNT in one quartermaster trade.
pub const TNT_TRADE_COUNT: u8 = 4;
/// TNT stacked into each armory / blacksmith chest.
pub const CHEST_TNT_COUNT: u8 = 8;
/// Raids this large field sapper units.
pub const SAPPER_RAID_MIN: u16 = 40;
/// Ticks a planted charge waits before it blows.
pub const SAPPER_FUSE: u32 = 40;
/// How close a sapper must stand to plant. Wider than melee so charges go
/// down at the ram line before the gate is already open.
pub const SAPPER_PLANT_RANGE: f64 = 8.0;
/// Extra cobble-punch radius for an exploding-barrel trebuchet shot.
pub const BARREL_EXTRA_RADIUS: i32 = 1;
/// Extra depth for a barrel impact.
pub const BARREL_EXTRA_DEPTH: i32 = 1;
/// How far a sapper falls back after planting.
pub const SAPPER_RETREAT: i32 = 8;

/// Keep rect + flag column that vanilla TNT must not clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlastGuard {
    pub keep: Rect,
    pub flag: (i32, i32),
}

fn guard_slot() -> &'static Mutex<Option<BlastGuard>> {
    static GUARD: OnceLock<Mutex<Option<BlastGuard>>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(None))
}

/// Remember the keep/flag that the in-flight explosion must not touch.
pub fn arm_blast_guard(guard: Option<BlastGuard>) {
    *guard_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = guard;
}

/// Current keep/flag guard, if an explosion is in flight.
#[must_use]
pub fn blast_guard() -> Option<BlastGuard> {
    *guard_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Cheap per-block filter used from pumpkin-core's explosion raycast.
#[must_use]
pub fn should_block_explode(x: i32, _y: i32, z: i32) -> bool {
    blast_guard().is_none_or(|g| !is_blast_protected(x, z, g.keep, g.flag))
}

/// True when the column is the keep mass or the flag plinth.
#[must_use]
pub fn is_blast_protected(x: i32, z: i32, keep: Rect, flag: (i32, i32)) -> bool {
    keep.contains(x, z) || is_flag_column(flag, x, z)
}

/// Wall, tower, and gate columns that extra breach damage may punch.
#[must_use]
pub fn is_breach_column(kind: CellKind) -> bool {
    matches!(
        kind,
        CellKind::TownWall
            | CellKind::TownTower
            | CellKind::TownGate
            | CellKind::CastleWall
            | CellKind::CastleTower
            | CellKind::CastleGate
            | CellKind::CastleBarbican
            | CellKind::CastleInnerWall
            | CellKind::FortressWall
            | CellKind::FortressTower
    )
}

/// Repair band for a punched column.
#[must_use]
pub fn band_for_kind(kind: CellKind) -> RepairBand {
    match kind {
        CellKind::TownGate | CellKind::CastleGate | CellKind::CastleBarbican => RepairBand::Gate,
        CellKind::CastleKeep | CellKind::CastleHall | CellKind::CastleCourt => RepairBand::Keep,
        CellKind::TownWall
        | CellKind::TownTower
        | CellKind::CastleWall
        | CellKind::CastleTower
        | CellKind::CastleInnerWall
        | CellKind::FortressWall
        | CellKind::FortressTower => RepairBand::Wall,
        _ => RepairBand::House,
    }
}

/// TNT a rank gets on a wartime respawn. Sergeant 2, knight 3, banneret 4.
#[must_use]
pub const fn tnt_kit_count(rank: Rank) -> u8 {
    match rank {
        Rank::Levy | Rank::Freeman => 0,
        Rank::Sergeant => 2,
        Rank::Knight => 3,
        Rank::Banneret => 4,
    }
}

/// True when `now` is far enough after the last kit grant.
#[must_use]
pub fn kit_ready(last_tick: Option<u32>, now: u32, cooldown: u32) -> bool {
    last_tick.is_none_or(|t| now.saturating_sub(t) >= cooldown)
}

/// Extra Chebyshev radius. `KINGDOMS_BREACH_BONUS` overrides the default.
#[must_use]
pub fn breach_bonus() -> i32 {
    env_i32("KINGDOMS_BREACH_BONUS")
        .unwrap_or(DEFAULT_BREACH_BONUS)
        .max(0)
}

/// Extra column depth. `KINGDOMS_BREACH_DEPTH` overrides the default.
#[must_use]
pub fn breach_depth() -> i32 {
    env_i32("KINGDOMS_BREACH_DEPTH")
        .unwrap_or(DEFAULT_BREACH_DEPTH)
        .max(1)
}

/// Kit cooldown in ticks. `KINGDOMS_TNT_KIT_COOLDOWN` overrides the default.
#[must_use]
pub fn tnt_kit_cooldown() -> u32 {
    env_u32("KINGDOMS_TNT_KIT_COOLDOWN").unwrap_or(DEFAULT_TNT_KIT_COOLDOWN)
}

/// Exploding-barrel trebuchet shots. On unless `KINGDOMS_TREB_BARREL=0`.
#[must_use]
pub fn barrel_shot_enabled() -> bool {
    std::env::var("KINGDOMS_TREB_BARREL").map_or(true, |v| {
        matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}

/// Sappers in a wave of `raid_size`. Zero below [`SAPPER_RAID_MIN`].
#[must_use]
pub fn sapper_count(raid_size: u16) -> usize {
    if raid_size < SAPPER_RAID_MIN {
        0
    } else {
        (usize::from(raid_size) / 20).clamp(1, 4)
    }
}

/// Blacksmith lots plus the castle barracks (the town armoury).
#[must_use]
pub fn armory_rects(plan: &TownPlan) -> Vec<Rect> {
    let mut out: Vec<Rect> = plan
        .lots
        .iter()
        .filter(|lot| lot.kind == BuildingKind::Blacksmith)
        .map(|lot| lot.rect)
        .collect();
    if plan.castle.barracks.width() > 2 && plan.castle.barracks.depth() > 2 {
        out.push(plan.castle.barracks);
    }
    out
}

/// Columns inside `rect` that overlap chunk `(chunk_x, chunk_z)`.
#[must_use]
pub fn chest_scan_columns(rect: Rect, chunk_x: i32, chunk_z: i32) -> Vec<(i32, i32)> {
    let min_x = (chunk_x * 16).max(rect.min_x + 1);
    let max_x = (chunk_x * 16 + 15).min(rect.max_x - 1);
    let min_z = (chunk_z * 16).max(rect.min_z + 1);
    let max_z = (chunk_z * 16 + 15).min(rect.max_z - 1);
    if min_x > max_x || min_z > max_z {
        return Vec::new();
    }
    let mut out = Vec::new();
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            out.push((x, z));
        }
    }
    out
}

/// Chebyshev radius used to extra-punch walls after a blast of `power`.
#[must_use]
pub fn blast_radius(power: f32) -> i32 {
    (power.ceil() as i32)
        .max(1)
        .saturating_add(breach_bonus())
        .min(6)
}

/// XZ columns within Chebyshev `radius` of `(x, z)`.
#[must_use]
pub fn columns_in_radius(x: i32, z: i32, radius: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            out.push((x + dx, z + dz));
        }
    }
    out
}

/// Wall and gate columns extra breach damage would punch around `(bx, bz)`.
/// Keep mass and the flag plate are never included.
#[must_use]
pub fn extra_damage_columns(
    plan: &TownPlan,
    bx: i32,
    bz: i32,
    keep: Rect,
    flag: (i32, i32),
    radius: i32,
) -> Vec<(i32, i32)> {
    columns_in_radius(bx, bz, radius)
        .into_iter()
        .filter(|&(x, z)| {
            !is_blast_protected(x, z, keep, flag) && is_breach_column(plan.occ.kind(x, z))
        })
        .collect()
}

/// Extra punch offsets around an impact: `(dx, dy, dz)` in Chebyshev radius.
#[must_use]
pub fn breach_offsets(radius: i32, depth: i32) -> Vec<(i32, i32, i32)> {
    let mut out = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            for dy in -depth..=0 {
                out.push((dx, dy, dz));
            }
        }
    }
    out
}

/// Sapper walk / plant / fuse / blow cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SapperPhase {
    March,
    Plant,
    Fuse,
    Detonate,
    Done,
}

/// Next sapper phase. `dist` is horizontal distance to the wall/gate column.
#[must_use]
pub fn sapper_phase(dist: f64, planted: bool, fuse_left: u32) -> SapperPhase {
    if !planted {
        if dist <= SAPPER_PLANT_RANGE {
            SapperPhase::Plant
        } else {
            SapperPhase::March
        }
    } else if fuse_left == 0 {
        SapperPhase::Detonate
    } else if fuse_left < SAPPER_FUSE {
        SapperPhase::Fuse
    } else {
        SapperPhase::Done
    }
}

/// Retreat column camp-side of a planted charge.
#[must_use]
pub fn sapper_retreat(plant: (i32, i32), camp: (i32, i32), dist: i32) -> (i32, i32) {
    let dx = (camp.0 - plant.0).signum();
    let dz = (camp.1 - plant.1).signum();
    if dx == 0 && dz == 0 {
        (plant.0, plant.1 + dist)
    } else {
        (plant.0 + dx * dist, plant.1 + dz * dist)
    }
}

/// Take `want` of `item_id` from `stacks`. Returns how many were removed.
pub fn take_from_stacks(stacks: &mut [u8], item_match: &[bool], want: u8) -> u8 {
    let mut left = want;
    for (i, count) in stacks.iter_mut().enumerate() {
        if left == 0 {
            break;
        }
        if !item_match.get(i).copied().unwrap_or(false) || *count == 0 {
            continue;
        }
        let take = left.min(*count);
        *count -= take;
        left -= take;
    }
    want - left
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::height::HeightGrid;
    use crate::info::GateKind;
    use crate::palette::PaletteKind;
    use crate::plan::build_plan;
    use crate::town::town_in_cell;

    #[test]
    fn sergeant_and_up_get_two_to_four_tnt() {
        assert_eq!(tnt_kit_count(Rank::Levy), 0);
        assert_eq!(tnt_kit_count(Rank::Freeman), 0);
        assert_eq!(tnt_kit_count(Rank::Sergeant), 2);
        assert_eq!(tnt_kit_count(Rank::Knight), 3);
        assert_eq!(tnt_kit_count(Rank::Banneret), 4);
        assert!(kit_ready(None, 10, DEFAULT_TNT_KIT_COOLDOWN));
        assert!(!kit_ready(Some(0), 100, DEFAULT_TNT_KIT_COOLDOWN));
        assert!(kit_ready(
            Some(0),
            DEFAULT_TNT_KIT_COOLDOWN,
            DEFAULT_TNT_KIT_COOLDOWN
        ));
    }

    #[test]
    fn keep_and_flag_are_blast_protected_walls_are_not() {
        let keep = Rect::new(10, 10, 20, 20);
        let flag = (12, 12);
        assert!(is_blast_protected(15, 15, keep, flag));
        assert!(is_blast_protected(12, 12, keep, flag));
        assert!(is_blast_protected(13, 13, keep, flag));
        assert!(!is_blast_protected(40, 40, keep, flag));
        assert!(is_breach_column(CellKind::TownWall));
        assert!(is_breach_column(CellKind::TownGate));
        assert!(is_breach_column(CellKind::CastleGate));
        assert!(!is_breach_column(CellKind::CastleKeep));
        assert!(!is_breach_column(CellKind::Green));
        assert_eq!(band_for_kind(CellKind::TownGate), RepairBand::Gate);
        assert_eq!(band_for_kind(CellKind::TownWall), RepairBand::Wall);
        assert_eq!(band_for_kind(CellKind::CastleKeep), RepairBand::Keep);
        arm_blast_guard(Some(BlastGuard { keep, flag }));
        assert!(!should_block_explode(15, 80, 15));
        assert!(should_block_explode(40, 80, 40));
        arm_blast_guard(None);
        assert!(should_block_explode(15, 80, 15));
    }

    #[test]
    fn larger_raids_field_sappers_small_ones_do_not() {
        assert_eq!(sapper_count(12), 0);
        assert_eq!(sapper_count(24), 0);
        assert_eq!(sapper_count(40), 2);
        assert_eq!(sapper_count(80), 4);
        assert_eq!(sapper_count(200), 4);
        assert_eq!(sapper_phase(9.0, false, SAPPER_FUSE), SapperPhase::March);
        assert_eq!(sapper_phase(8.0, false, SAPPER_FUSE), SapperPhase::Plant);
        assert_eq!(sapper_phase(1.0, false, SAPPER_FUSE), SapperPhase::Plant);
        assert_eq!(sapper_phase(1.0, true, 10), SapperPhase::Fuse);
        assert_eq!(sapper_phase(1.0, true, 0), SapperPhase::Detonate);
        let back = sapper_retreat((10, 10), (10, 0), SAPPER_RETREAT);
        assert_eq!(back, (10, 2));
    }

    #[test]
    fn breach_offsets_cover_the_bonus_radius() {
        let offs = breach_offsets(1, 2);
        assert!(offs.contains(&(0, 0, 0)));
        assert!(offs.contains(&(1, -2, 1)));
        assert!(!offs.contains(&(2, 0, 0)));
        assert_eq!(TNT_TRADE_EMERALDS, 2);
        assert_eq!(TNT_TRADE_COUNT, 4);
        assert_eq!(CHEST_TNT_COUNT, 8);
        let cols = columns_in_radius(10, 20, 1);
        assert_eq!(cols.len(), 9);
        assert!(cols.contains(&(10, 20)));
        assert!(cols.contains(&(11, 21)));
        assert!(blast_radius(4.0) >= DEFAULT_BREACH_BONUS);
        assert!(blast_radius(4.0) <= 6);
    }

    #[test]
    fn take_from_stacks_stops_at_the_want() {
        let mut stacks = [3u8, 4, 1];
        let matches = [true, true, false];
        assert_eq!(take_from_stacks(&mut stacks, &matches, 5), 5);
        assert_eq!(stacks, [0, 2, 1]);
        assert_eq!(take_from_stacks(&mut stacks, &matches, 9), 2);
        assert_eq!(stacks, [0, 0, 1]);
    }

    fn sample_plan() -> TownPlan {
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
        build_plan(3, origin, grid, PaletteKind::Oak)
    }

    #[test]
    fn keep_and_flag_survive_tnt_walls_and_gates_take_extra_damage() {
        let plan = sample_plan();
        let keep = plan.castle.inner;
        let flag = plan.castle.flag;
        arm_blast_guard(Some(BlastGuard { keep, flag }));

        let (kx, kz) = keep.center();
        assert!(
            is_blast_protected(kx, kz, keep, flag),
            "keep center must be blast-protected"
        );
        assert!(
            !should_block_explode(kx, 80, kz),
            "TNT must not destroy keep blocks"
        );
        assert!(
            is_blast_protected(flag.0, flag.1, keep, flag),
            "flag mast must be blast-protected"
        );
        assert!(
            !should_block_explode(flag.0, 80, flag.1),
            "TNT must not destroy the flag plate"
        );
        for dz in -2..=2 {
            for dx in -2..=2 {
                assert!(
                    !should_block_explode(flag.0 + dx, 80, flag.1 + dz),
                    "flag plate column ({}, {}) must survive TNT",
                    flag.0 + dx,
                    flag.1 + dz
                );
            }
        }

        let radius = DEFAULT_BREACH_BONUS.max(1);
        let at_flag = extra_damage_columns(&plan, flag.0, flag.1, keep, flag, radius);
        assert!(
            at_flag.is_empty(),
            "flag-plate TNT must not extra-punch keep or plate, got {at_flag:?}"
        );
        let at_keep = extra_damage_columns(&plan, kx, kz, keep, flag, radius);
        assert!(
            at_keep.is_empty(),
            "keep TNT must not extra-punch the keep, got {at_keep:?}"
        );

        let wall = (plan.town.min_x..=plan.town.max_x)
            .flat_map(|x| (plan.town.min_z..=plan.town.max_z).map(move |z| (x, z)))
            .find(|&(x, z)| {
                matches!(
                    plan.occ.kind(x, z),
                    CellKind::TownWall | CellKind::CastleWall
                ) && !is_blast_protected(x, z, keep, flag)
            })
            .expect("planned town has a wall column");
        assert!(
            should_block_explode(wall.0, 80, wall.1),
            "vanilla TNT is allowed to hit the wall"
        );
        let wall_hits = extra_damage_columns(&plan, wall.0, wall.1, keep, flag, radius);
        assert!(
            wall_hits.contains(&wall),
            "wall column must take extra breach damage"
        );
        assert!(
            wall_hits
                .iter()
                .all(|&(x, z)| is_breach_column(plan.occ.kind(x, z))
                    && !is_blast_protected(x, z, keep, flag)),
            "extra damage is walls/gates only"
        );

        let gate = plan
            .gates
            .iter()
            .find(|g| {
                g.kind == GateKind::Town
                    && is_breach_column(plan.occ.kind(g.x, g.z))
                    && !is_blast_protected(g.x, g.z, keep, flag)
            })
            .or_else(|| {
                plan.gates.iter().find(|g| {
                    is_breach_column(plan.occ.kind(g.x, g.z))
                        && !is_blast_protected(g.x, g.z, keep, flag)
                })
            })
            .expect("planned town has a gate");
        assert!(
            should_block_explode(gate.x, 80, gate.z),
            "vanilla TNT is allowed to hit the gate"
        );
        let gate_hits = extra_damage_columns(&plan, gate.x, gate.z, keep, flag, radius);
        assert!(
            gate_hits.contains(&(gate.x, gate.z)),
            "gate column must take extra breach damage"
        );
        assert!(
            gate_hits.iter().any(|&(x, z)| {
                matches!(
                    plan.occ.kind(x, z),
                    CellKind::TownGate | CellKind::CastleGate | CellKind::CastleBarbican
                )
            }),
            "gate blast extra-punches a gate column"
        );
        arm_blast_guard(None);
    }

    #[test]
    fn write_round22_breach_iso_when_artifacts_exist() {
        let out = std::path::Path::new("/opt/cursor/artifacts");
        if !out.is_dir() {
            return;
        }
        let plan = sample_plan();
        let volume = crate::stamp::fill_volume(&plan);
        crate::render::write_breach_sequence(&plan, &volume, out, "round22-breach")
            .expect("breach iso sequence");
        for name in [
            "round22-breach-before.png",
            "round22-breach-after.png",
            "round22-breach-repaired.png",
        ] {
            let path = out.join(name);
            assert!(
                path.metadata().is_ok_and(|m| m.len() > 800),
                "{name} missing or tiny"
            );
        }
    }

    #[test]
    fn armory_rects_include_blacksmith_and_barracks() {
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
        let rects = armory_rects(&plan);
        assert!(
            rects.iter().any(|r| plan
                .lots
                .iter()
                .any(|lot| lot.kind == BuildingKind::Blacksmith && lot.rect == *r)),
            "blacksmith lot missing from armory rects"
        );
        assert!(
            rects.contains(&plan.castle.barracks),
            "castle barracks missing from armory rects"
        );
        let chunk_x = plan.castle.barracks.min_x >> 4;
        let chunk_z = plan.castle.barracks.min_z >> 4;
        let cols = chest_scan_columns(plan.castle.barracks, chunk_x, chunk_z);
        assert!(!cols.is_empty());
    }
}
