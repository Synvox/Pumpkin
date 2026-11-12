//! Trebuchets, assault ladders, and siege towers.
//!
//! Camp trebuchets throw cobble along a parabola at the threatened wall.
//! The director fires on a cadence; a player at the anvil (or `/kingdom fire`)
//! can loose a shot too. Impacts punch holes that builders later repair.
//! Ladder crews and a moving siege tower raise climbable ladders on the
//! outer face so raiders are not stuck at the gate.

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::decoration::display::BlockDisplayEntity;
use pumpkin_core::world::World;
use pumpkin_data::Block;
use pumpkin_data::block_properties::{HorizontalFacing, LadderLikeProperties};
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

use crate::geom::Rect;
use crate::scene::ScenePost;
use crate::siege::WALL_WALK_LIFT;

/// Gravity in blocks per tick² for the cobble arc.
pub const TREB_GRAVITY: f64 = 0.08;
/// Ticks the shot spends in the air.
pub const TREB_FLIGHT: u32 = 40;
/// Director looses at most one shot this often.
pub const TREB_PERIOD: u32 = 80;
const IMPACT_RADIUS: i32 = 1;
const IMPACT_DEPTH: i32 = 4;
const LADDER_HEIGHT: i32 = WALL_WALK_LIFT;
const TOWER_STEP: f64 = 0.12;
/// Block-display parts spawned per town tick (the 8-part dump was a 200 ms hitch).
pub const ENGINE_PARTS_PER_TICK: usize = 2;

/// One wall block the cobble cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PunchHit {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub was: u16,
}

/// True when the column is the keep / inner bailey, which trebuchets must not touch.
#[must_use]
pub fn punch_column_allowed(x: i32, z: i32, keep: Rect) -> bool {
    !keep.contains(x, z)
}

/// Chests, barrels, and beds stay put even on a wall column.
#[must_use]
pub fn is_protected_block(block: &Block) -> bool {
    block.id == Block::CHEST.id
        || block.id == Block::TRAPPED_CHEST.id
        || block.id == Block::BARREL.id
        || block.id == Block::ENDER_CHEST.id
        || block.name.ends_with("_bed")
}

/// One flying cobble (a block-display entity).
#[derive(Clone, Debug)]
pub struct TrebShot {
    pub part: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub vx: f64,
    pub vy: f64,
    pub vz: f64,
    pub age: u32,
    pub target_y: f64,
}

/// Covered ladder tower that walks to the wall.
#[derive(Clone, Debug)]
pub struct SiegeTower {
    pub parts: Vec<i32>,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub arrived: bool,
}

/// Horizontal and vertical speed that lands `from` on `to` after `ticks`.
#[must_use]
pub fn aim_arc(
    from: (f64, f64, f64),
    to: (f64, f64, f64),
    ticks: f64,
    gravity: f64,
) -> (f64, f64, f64) {
    let t = ticks.max(1.0);
    let vx = (to.0 - from.0) / t;
    let vz = (to.2 - from.2) / t;
    let vy = (to.1 - from.1) / t + 0.5 * gravity * (t - 1.0);
    (vx, vy, vz)
}

/// Next position after one tick of the arc.
#[must_use]
pub fn step_arc(
    x: f64,
    y: f64,
    z: f64,
    vx: f64,
    vy: f64,
    vz: f64,
    gravity: f64,
) -> (f64, f64, f64, f64) {
    (x + vx, y + vy, z + vz, vy - gravity)
}

/// True when the shot has reached the wall height or timed out.
#[must_use]
pub fn shot_done(age: u32, y: f64, target_y: f64, flight: u32) -> bool {
    age >= flight || y <= target_y
}

/// Spawn a cobble display at `origin` aimed at `target`.
///
/// `visible` is the display entity. A far shot still flies and punches; spawning
/// the cobble at an unloaded camp was the 700 ms first-town engines hitch.
pub fn fire_trebuchet(
    world: &Arc<World>,
    origin: Vector3<f64>,
    target: Vector3<f64>,
    visible: bool,
) -> TrebShot {
    let (vx, vy, vz) = aim_arc(
        (origin.x, origin.y, origin.z),
        (target.x, target.y, target.z),
        f64::from(TREB_FLIGHT),
        TREB_GRAVITY,
    );
    let part = if visible {
        let entity = Entity::new(world.clone(), origin, &EntityType::BLOCK_DISPLAY);
        let display = BlockDisplayEntity::new(entity);
        display.set_block_state(i32::from(Block::COBBLESTONE.default_state.id.as_u16()));
        display.display.set_teleport_duration(1);
        let id = display.get_entity().entity_id;
        if world.spawn_entity(display) { id } else { -1 }
    } else {
        -1
    };
    TrebShot {
        part,
        x: origin.x,
        y: origin.y,
        z: origin.z,
        vx,
        vy,
        vz,
        age: 0,
        target_y: target.y,
    }
}

/// Advance the shot. On impact, punch a hole in solid wall blocks.
pub fn tick_shot(world: &Arc<World>, shot: &mut TrebShot, keep: Rect) -> Option<Vec<PunchHit>> {
    let (x, y, z, vy) = step_arc(
        shot.x,
        shot.y,
        shot.z,
        shot.vx,
        shot.vy,
        shot.vz,
        TREB_GRAVITY,
    );
    shot.x = x;
    shot.y = y;
    shot.z = z;
    shot.vy = vy;
    shot.age = shot.age.saturating_add(1);
    if shot.part >= 0
        && let Some(e) = world.get_entity_by_id(shot.part)
    {
        e.get_entity().set_pos(Vector3::new(x, y, z));
    }
    if !shot_done(shot.age, shot.y, shot.target_y, TREB_FLIGHT) {
        return None;
    }
    let top = shot.target_y.floor() as i32;
    let hits = collect_punch_hits(
        world,
        shot.x.floor() as i32,
        shot.z.floor() as i32,
        keep,
        top,
    );
    despawn_shot(world, shot);
    tracing::info!(
        x = shot.x,
        z = shot.z,
        cleared = hits.len(),
        "kingdoms trebuchet impact"
    );
    Some(hits)
}

fn despawn_shot(world: &World, shot: &TrebShot) {
    if shot.part >= 0
        && let Some(e) = world.get_entity_by_id(shot.part)
    {
        e.get_entity().remove();
    }
}

/// Blocks the cobble would clear. Writes happen later, a few per tick.
/// `top_y` is the aimed wall height; do not noise-sample the column here.
#[must_use]
pub fn collect_punch_hits(world: &World, x: i32, z: i32, keep: Rect, top_y: i32) -> Vec<PunchHit> {
    let mut hits = Vec::new();
    for dz in -IMPACT_RADIUS..=IMPACT_RADIUS {
        for dx in -IMPACT_RADIUS..=IMPACT_RADIUS {
            let cx = x + dx;
            let cz = z + dz;
            if !punch_column_allowed(cx, cz, keep) {
                continue;
            }
            for dy in -IMPACT_DEPTH..=0 {
                let pos = BlockPos::new(cx, top_y + dy, cz);
                if !world.is_loaded(&pos) {
                    continue;
                }
                let (block, state) = world.get_block_and_state(&pos);
                if is_protected_block(block) || !state.is_solid() {
                    continue;
                }
                hits.push(PunchHit {
                    x: cx,
                    y: pos.0.y,
                    z: cz,
                    was: state.id.as_u16(),
                });
            }
        }
    }
    hits
}

/// Replace one punched block with air. Returns whether the world changed.
pub fn apply_punch_hit(world: &Arc<World>, hit: PunchHit) -> bool {
    let pos = BlockPos::new(hit.x, hit.y, hit.z);
    if !world.is_loaded(&pos) {
        return false;
    }
    let state = world.get_block_state(&pos);
    if !state.is_solid() {
        return false;
    }
    world.set_block_state(
        &pos,
        Block::AIR.default_state.id,
        crate::siege::SIEGE_PLACE_FLAGS,
    );
    true
}

/// Put ladders on the outer face of a wall column, camp-side.
pub fn place_assault_ladders(
    world: &Arc<World>,
    wall: ScenePost,
    camp: ScenePost,
    floor_y: i32,
) -> u32 {
    let mut fx = (camp.x - wall.x).signum();
    let mut fz = (camp.z - wall.z).signum();
    if fx == 0 && fz == 0 {
        fz = 1;
    } else if fx.abs() >= fz.abs() {
        fz = 0;
    } else {
        fx = 0;
    }
    let lx = wall.x + fx;
    let lz = wall.z + fz;
    let mut placed = 0u32;
    for dy in 1..=LADDER_HEIGHT {
        if placed >= 2 {
            break;
        }
        let pos = BlockPos::new(lx, floor_y + dy, lz);
        if !world.is_loaded(&pos) {
            continue;
        }
        let (block, _) = world.get_block_and_state(&pos);
        if block.id != Block::AIR.id && block.id != Block::LADDER.id {
            continue;
        }
        if block.id == Block::LADDER.id {
            continue;
        }
        world.set_block_state(&pos, ladder_facing(fx, fz), crate::siege::SIEGE_PLACE_FLAGS);
        placed += 1;
    }
    if placed > 0 {
        tracing::info!(x = lx, z = lz, placed, "kingdoms assault ladders raised");
    }
    placed
}

fn ladder_facing(fx: i32, fz: i32) -> pumpkin_data::BlockStateId {
    let facing = match (fx.signum(), fz.signum()) {
        (1, _) => HorizontalFacing::East,
        (-1, _) => HorizontalFacing::West,
        (_, 1) => HorizontalFacing::South,
        _ => HorizontalFacing::North,
    };
    LadderLikeProperties {
        facing,
        waterlogged: false,
    }
    .to_state_id(&Block::LADDER)
}

fn tower_offsets() -> [(f64, f64, f64, &'static Block); 8] {
    [
        (0.0, 0.0, 0.0, &Block::DARK_OAK_LOG),
        (0.0, 1.0, 0.0, &Block::DARK_OAK_FENCE),
        (0.0, 2.0, 0.0, &Block::DARK_OAK_FENCE),
        (0.0, 3.0, 0.0, &Block::DARK_OAK_FENCE),
        (0.0, 4.0, 0.0, &Block::DARK_OAK_FENCE),
        (0.0, 5.0, 0.0, &Block::OAK_SLAB),
        (1.0, 1.0, 0.0, &Block::LADDER),
        (-1.0, 1.0, 0.0, &Block::LADDER),
    ]
}

/// Empty tower at `origin`; parts are appended a few per tick.
#[must_use]
pub fn empty_tower(origin: Vector3<f64>) -> SiegeTower {
    SiegeTower {
        parts: Vec::new(),
        x: origin.x,
        y: origin.y,
        z: origin.z,
        arrived: false,
    }
}

/// True when every display part has been spawned.
#[must_use]
pub fn tower_complete(tower: &SiegeTower) -> bool {
    tower.parts.len() >= tower_offsets().len()
}

/// Spawn up to `budget` remaining tower displays. Returns how many were added.
pub fn append_tower_parts(world: &Arc<World>, tower: &mut SiegeTower, budget: usize) -> usize {
    if budget == 0 {
        return 0;
    }
    let mut added = 0usize;
    let offsets = tower_offsets();
    while tower.parts.len() < offsets.len() && added < budget {
        let (dx, dy, dz, block) = offsets[tower.parts.len()];
        let pos = Vector3::new(tower.x + dx, tower.y + dy, tower.z + dz);
        let entity = Entity::new(world.clone(), pos, &EntityType::BLOCK_DISPLAY);
        let display = BlockDisplayEntity::new(entity);
        display.set_block_state(i32::from(block.default_state.id.as_u16()));
        display.display.set_teleport_duration(2);
        let id = display.get_entity().entity_id;
        if world.spawn_entity(display) {
            tower.parts.push(id);
        } else {
            break;
        }
        added += 1;
    }
    added
}

/// Spawn a walking siege tower at `origin`.
pub fn spawn_tower(world: &Arc<World>, origin: Vector3<f64>) -> SiegeTower {
    let mut tower = empty_tower(origin);
    append_tower_parts(world, &mut tower, usize::MAX);
    tracing::info!(
        x = origin.x,
        z = origin.z,
        parts = tower.parts.len(),
        "kingdoms siege tower spawned"
    );
    tower
}

/// Push the tower toward `dest`. Returns true once it is against the wall.
///
/// `visible` updates display entities. Far towns still march the origin so
/// arrival and ladders stay on schedule without a tracker hitch.
pub fn tick_tower(world: &World, tower: &mut SiegeTower, dest: ScenePost, visible: bool) -> bool {
    if tower.arrived {
        return true;
    }
    let dx = f64::from(dest.x) + 0.5 - tower.x;
    let dz = f64::from(dest.z) + 0.5 - tower.z;
    let dist = dx.hypot(dz);
    if dist < 4.0 {
        tower.arrived = true;
        return true;
    }
    let step = TOWER_STEP.min(dist);
    tower.x += dx / dist * step;
    tower.z += dz / dist * step;
    if !visible {
        return false;
    }
    for (i, id) in tower.parts.iter().enumerate() {
        let Some(&(ox, oy, oz, _)) = tower_offsets().get(i) else {
            continue;
        };
        if let Some(e) = world.get_entity_by_id(*id) {
            e.get_entity()
                .set_pos(Vector3::new(tower.x + ox, tower.y + oy, tower.z + oz));
        }
    }
    false
}

/// Remove tower display entities.
pub fn destroy_tower(world: &World, tower: &mut SiegeTower) {
    for id in tower.parts.drain(..) {
        if let Some(e) = world.get_entity_by_id(id) {
            e.get_entity().remove();
        }
    }
}

/// True when `pos` is within `range` of a trebuchet post.
#[must_use]
pub fn near_controls(pos: Vector3<f64>, posts: &[ScenePost], range: f64) -> bool {
    posts
        .iter()
        .any(|p| (pos.x - f64::from(p.x)).hypot(pos.z - f64::from(p.z)) <= range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_lands_on_the_wall_after_flight_ticks() {
        let from = (0.0, 80.0, 0.0);
        let to = (40.0, 86.0, 10.0);
        let (vx, vy, vz) = aim_arc(from, to, f64::from(TREB_FLIGHT), TREB_GRAVITY);
        let mut x = from.0;
        let mut y = from.1;
        let mut z = from.2;
        let mut speed_y = vy;
        for _ in 0..TREB_FLIGHT {
            let next = step_arc(x, y, z, vx, speed_y, vz, TREB_GRAVITY);
            x = next.0;
            y = next.1;
            z = next.2;
            speed_y = next.3;
        }
        assert!((x - to.0).abs() < 0.01, "x {x} vs {}", to.0);
        assert!((z - to.2).abs() < 0.01, "z {z} vs {}", to.2);
        assert!((y - to.1).abs() < 0.01, "y {y} vs {}", to.1);
    }

    #[test]
    fn shot_done_when_below_target_or_timed_out() {
        assert!(shot_done(TREB_FLIGHT, 90.0, 80.0, TREB_FLIGHT));
        assert!(shot_done(10, 79.0, 80.0, TREB_FLIGHT));
        assert!(!shot_done(10, 90.0, 80.0, TREB_FLIGHT));
    }

    #[test]
    fn tower_has_climb_face() {
        assert!(
            tower_offsets()
                .iter()
                .any(|(_, _, _, b)| b.id == Block::LADDER.id)
        );
        assert_eq!(ENGINE_PARTS_PER_TICK, 2);
        assert!(tower_offsets().len() > ENGINE_PARTS_PER_TICK);
        let empty = empty_tower(Vector3::new(0.0, 64.0, 0.0));
        assert!(!tower_complete(&empty));
    }

    #[test]
    fn trebuchet_skips_keep_and_loot_blocks() {
        let keep = Rect::new(10, 10, 20, 20);
        assert!(!punch_column_allowed(12, 15, keep));
        assert!(punch_column_allowed(0, 0, keep));
        assert!(punch_column_allowed(21, 15, keep));
        assert!(is_protected_block(&Block::CHEST));
        assert!(is_protected_block(&Block::TRAPPED_CHEST));
        assert!(is_protected_block(&Block::BARREL));
        assert!(is_protected_block(&Block::WHITE_BED));
        assert!(is_protected_block(&Block::RED_BED));
        assert!(!is_protected_block(&Block::STONE_BRICKS));
        assert!(!is_protected_block(&Block::COBBLESTONE));
        assert!(!is_protected_block(&Block::LADDER));
    }
}
