//! A moving covered battering ram made of block-display entities.
//!
//! Ram-job raiders push it along the road. Nearby defenders chip its HP.
//! It breaches the gate on contact or is destroyed before it arrives.

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::decoration::display::BlockDisplayEntity;
use pumpkin_core::world::World;
use pumpkin_data::Block;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::vector3::Vector3;
use std::sync::Arc;

use crate::scene::ScenePost;

/// Hit points. Defenders at the gate can wreck it; a clear road lets it land.
pub const RAM_MAX_HP: f32 = 80.0;
/// Display parts spawned per town tick (the 6-part dump hitch).
pub const RAM_PARTS_PER_TICK: usize = 2;
const RAM_STEP: f64 = 0.16;
const CREW_RANGE: f64 = 8.0;
const DEFENDER_RANGE: f64 = 4.5;
/// Damage applied per nearby defender each ram tick.
pub const DAMAGE_PER_DEFENDER: f32 = 0.8;

/// Live ram in the world.
#[derive(Clone, Debug)]
pub struct SiegeRam {
    pub parts: Vec<i32>,
    pub hp: f32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub destroyed: bool,
}

impl SiegeRam {
    #[must_use]
    pub fn post(&self) -> ScenePost {
        ScenePost::new(self.x.floor() as i32, self.z.floor() as i32)
    }
}

/// Offsets of the covered ram relative to its origin (logs, roof, sides).
#[must_use]
pub fn part_offsets() -> [(f64, f64, f64, &'static Block); 6] {
    [
        (0.0, 0.0, 0.0, &Block::DARK_OAK_LOG),
        (0.0, 0.0, 1.0, &Block::DARK_OAK_LOG),
        (0.0, 0.0, -1.0, &Block::DARK_OAK_LOG),
        (0.0, 1.0, 0.0, &Block::OAK_SLAB),
        (1.0, 0.5, 0.0, &Block::DARK_OAK_FENCE),
        (-1.0, 0.5, 0.0, &Block::DARK_OAK_FENCE),
    ]
}

/// Empty ram at `origin`; parts are appended a few per tick.
#[must_use]
pub fn empty_ram(origin: Vector3<f64>) -> SiegeRam {
    SiegeRam {
        parts: Vec::new(),
        hp: RAM_MAX_HP,
        x: origin.x,
        y: origin.y,
        z: origin.z,
        destroyed: false,
    }
}

/// True when every display part has been spawned.
#[must_use]
pub fn ram_complete(ram: &SiegeRam) -> bool {
    ram.parts.len() >= part_offsets().len()
}

/// Spawn up to `budget` remaining ram displays. Returns how many were added.
pub fn append_ram_parts(world: &Arc<World>, ram: &mut SiegeRam, budget: usize) -> usize {
    if budget == 0 {
        return 0;
    }
    let mut added = 0usize;
    let offsets = part_offsets();
    while ram.parts.len() < offsets.len() && added < budget {
        let (dx, dy, dz, block) = offsets[ram.parts.len()];
        let pos = Vector3::new(ram.x + dx, ram.y + dy, ram.z + dz);
        let entity = Entity::new(world.clone(), pos, &EntityType::BLOCK_DISPLAY);
        let display = BlockDisplayEntity::new(entity);
        display.set_block_state(i32::from(block.default_state.id.as_u16()));
        display.display.set_teleport_duration(2);
        let id = display.get_entity().entity_id;
        if world.spawn_entity(display) {
            ram.parts.push(id);
        } else {
            break;
        }
        added += 1;
    }
    added
}

/// Spawn the display-entity ram at `origin`.
pub fn spawn_ram(world: &Arc<World>, origin: Vector3<f64>) -> SiegeRam {
    let mut ram = empty_ram(origin);
    append_ram_parts(world, &mut ram, usize::MAX);
    tracing::info!(
        x = origin.x,
        z = origin.z,
        parts = ram.parts.len(),
        "kingdoms ram spawned"
    );
    ram
}

/// Result of one ram tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RamTick {
    Moving,
    AtGate,
    Destroyed,
    Idle,
}

/// Apply defender chips and, if the ram is still up, the crew push.
#[must_use]
pub fn apply_damage(hp: f32, defenders_near: u32) -> f32 {
    hp - DAMAGE_PER_DEFENDER * defenders_near as f32
}

/// Push the ram toward `dest` when crew are close; apply defender damage.
pub fn tick_ram(
    world: &World,
    ram: &mut SiegeRam,
    dest: ScenePost,
    crew_near: u32,
    defenders_near: u32,
) -> RamTick {
    if ram.destroyed {
        return RamTick::Destroyed;
    }
    ram.hp = apply_damage(ram.hp, defenders_near);
    if ram.hp <= 0.0 {
        destroy_ram(world, ram);
        return RamTick::Destroyed;
    }
    let dx = f64::from(dest.x) + 0.5 - ram.x;
    let dz = f64::from(dest.z) + 0.5 - ram.z;
    let dist = dx.hypot(dz);
    if dist < 5.0 {
        return RamTick::AtGate;
    }
    if crew_near < 2 {
        return RamTick::Idle;
    }
    let step = RAM_STEP.min(dist);
    ram.x += dx / dist * step;
    ram.z += dz / dist * step;
    move_parts(world, ram);
    RamTick::Moving
}

/// Remove ram display entities.
pub fn destroy_ram(world: &World, ram: &mut SiegeRam) {
    for id in ram.parts.drain(..) {
        if let Some(e) = world.get_entity_by_id(id) {
            e.get_entity().remove();
        }
    }
    ram.destroyed = true;
    ram.hp = 0.0;
    tracing::info!("kingdoms ram destroyed x={:.1} z={:.1}", ram.x, ram.z);
}

fn move_parts(world: &World, ram: &SiegeRam) {
    for (i, id) in ram.parts.iter().enumerate() {
        let Some(&(dx, dy, dz, _)) = part_offsets().get(i) else {
            continue;
        };
        if let Some(e) = world.get_entity_by_id(*id) {
            e.get_entity()
                .set_pos(Vector3::new(ram.x + dx, ram.y + dy, ram.z + dz));
        }
    }
}

/// Count bodies within `range` of the ram.
#[must_use]
pub fn count_near(positions: &[(f64, f64)], ram: &SiegeRam, range: f64) -> u32 {
    positions
        .iter()
        .filter(|(x, z)| (x - ram.x).hypot(z - ram.z) <= range)
        .count() as u32
}

#[must_use]
pub const fn crew_range() -> f64 {
    CREW_RANGE
}

#[must_use]
pub const fn defender_range() -> f64 {
    DEFENDER_RANGE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_dies_when_hp_hits_zero() {
        let hp = apply_damage(2.0, 3);
        assert!(hp <= 0.0);
        let hp = apply_damage(RAM_MAX_HP, 1);
        assert!(hp > 0.0);
        assert_eq!(hp, RAM_MAX_HP - DAMAGE_PER_DEFENDER);
    }

    #[test]
    fn six_parts_make_a_covered_ram() {
        assert_eq!(part_offsets().len(), 6);
        assert_eq!(RAM_PARTS_PER_TICK, 2);
        assert!(part_offsets().len() > RAM_PARTS_PER_TICK);
        let empty = empty_ram(Vector3::new(0.0, 64.0, 0.0));
        assert!(!ram_complete(&empty));
        const { assert!(RAM_MAX_HP > 0.0) };
    }

    #[test]
    fn crew_must_stand_nearer_than_defenders_chip_range() {
        assert!(crew_range() > defender_range());
    }
}
