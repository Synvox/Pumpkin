//! Kingdom-specific goals built on Pumpkin's `Goal` trait.
//!
//! Combat still uses vanilla [`MeleeAttackGoal`], [`BowAttackGoal`] and
//! [`RangedCrossbowAttackGoal`]. These goals cover posts, alarm flee, the bell,
//! and the builder stub.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_core::entity::ai::goal::avoid_entity::AvoidEntityGoal;
use pumpkin_core::entity::ai::goal::bow_attack::BowAttackGoal;
use pumpkin_core::entity::ai::goal::break_door::BreakDoorGoal;
use pumpkin_core::entity::ai::goal::melee_attack::MeleeAttackGoal;
use pumpkin_core::entity::ai::goal::{Controls, Goal, GoalReach};
use pumpkin_core::entity::ai::pathfinder::NavigatorGoal;
use pumpkin_core::entity::mob::Mob;
use pumpkin_core::world::World;
use pumpkin_data::data_component_impl::EquipmentSlot;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

use crate::faction::{
    TAG_CAPTAIN, TAG_CITIZEN, TAG_GOLEM, TAG_RAIDER, TAG_WIRED, kingdom_tag, role_tag, town_tag,
};
use crate::lod::PathBudget;
use crate::roles::VillagerRole;
use crate::scene::ScenePost;

const POST_REACH_SQ: f64 = 6.25;
const FLEE_SPEED: f64 = 0.85;
const PATROL_SPEED: f64 = 0.7;
const MARCH_SPEED: f64 = 1.05;

/// Destination a post-walking goal is heading toward.
#[derive(Clone, Copy, Debug)]
pub struct PostTarget {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl PostTarget {
    #[must_use]
    pub const fn from_post(post: ScenePost, y: i32) -> Self {
        Self {
            x: post.x,
            y,
            z: post.z,
        }
    }

    #[must_use]
    pub fn vec(self) -> Vector3<f64> {
        Vector3::new(
            f64::from(self.x) + 0.5,
            f64::from(self.y),
            f64::from(self.z) + 0.5,
        )
    }
}

fn surface_y(world: &World, x: i32, z: i32) -> i32 {
    world.sample_surface_height(x, z).unwrap_or(world.sea_level)
}

fn try_path(mob: &dyn Mob, dest: Vector3<f64>, speed: f64, budget: Option<&PathBudget>) -> bool {
    if let Some(budget) = budget
        && !budget.try_acquire()
    {
        return false;
    }
    let pos = mob.get_entity().pos.load();
    mob.get_mob_entity()
        .steered()
        .navigator()
        .set_progress(NavigatorGoal::new(pos, dest, speed));
    true
}

fn dist_sq_xz(pos: Vector3<f64>, dest: Vector3<f64>) -> f64 {
    let dx = pos.x - dest.x;
    let dz = pos.z - dest.z;
    dx * dx + dz * dz
}

/// Walk to a column. Destination is refreshed from a callback so alarm can
/// retarget guards to the threatened gate without rebuilding the goal.
pub struct MoveToPostGoal {
    speed: f64,
    dest: Option<PostTarget>,
    refresh: Arc<dyn Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync>,
}

impl MoveToPostGoal {
    #[must_use]
    pub fn new(
        speed: f64,
        refresh: impl Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync + 'static,
    ) -> Self {
        Self {
            speed,
            dest: None,
            refresh: Arc::new(refresh),
        }
    }

    fn at_post(mob: &dyn Mob, dest: PostTarget) -> bool {
        dist_sq_xz(mob.get_entity().pos.load(), dest.vec()) <= POST_REACH_SQ
    }
}

impl Goal for MoveToPostGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let Some(dest) = (self.refresh)(mob) else {
            return false;
        };
        if Self::at_post(mob, dest) {
            return false;
        }
        self.dest = Some(dest);
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let Some(dest) = (self.refresh)(mob) else {
            return false;
        };
        self.dest = Some(dest);
        !Self::at_post(mob, dest)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest {
            let _ = try_path(mob, dest.vec(), self.speed, None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(dest) = self.dest else {
            return;
        };
        if mob
            .get_entity()
            .age
            .load(std::sync::atomic::Ordering::Relaxed)
            % 10
            == 0
        {
            let _ = try_path(mob, dest.vec(), self.speed, None);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            entities: 0.0,
            players: 0.0,
            speed: self.speed,
            changes_blocks: false,
            links: Vec::new(),
        })
    }
}

/// Civilians run to the keep while the town is alarmed.
pub struct FleeToKeepGoal {
    dest: Option<PostTarget>,
    refresh: Arc<dyn Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync>,
}

impl FleeToKeepGoal {
    #[must_use]
    pub fn new(refresh: impl Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync + 'static) -> Self {
        Self {
            dest: None,
            refresh: Arc::new(refresh),
        }
    }
}

impl Goal for FleeToKeepGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(dest) = (self.refresh)(mob) else {
            return false;
        };
        self.dest = Some(dest);
        dist_sq_xz(mob.get_entity().pos.load(), dest.vec()) > POST_REACH_SQ
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest {
            let _ = try_path(mob, dest.vec(), FLEE_SPEED, None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest
            && mob.get_entity().age.load(Ordering::Relaxed) % 8 == 0
        {
            let _ = try_path(mob, dest.vec(), FLEE_SPEED, None);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            entities: 0.0,
            players: 0.0,
            speed: FLEE_SPEED,
            changes_blocks: false,
            links: Vec::new(),
        })
    }
}

/// Ring once when the town first goes to alert. Plays the vanilla bell sound at
/// the keep; towns may not have a bell block yet.
pub struct RingBellGoal {
    rang: AtomicBool,
    alarmed: Arc<dyn Fn(&dyn Mob) -> bool + Send + Sync>,
}

impl RingBellGoal {
    #[must_use]
    pub fn new(alarmed: impl Fn(&dyn Mob) -> bool + Send + Sync + 'static) -> Self {
        Self {
            rang: AtomicBool::new(false),
            alarmed: Arc::new(alarmed),
        }
    }
}

impl Goal for RingBellGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        (self.alarmed)(mob) && !self.rang.load(Ordering::Relaxed)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.rang.store(true, Ordering::Relaxed);
        let entity = mob.get_entity();
        let world = entity.world.load();
        let pos = entity.pos.load();
        world.play_sound(Sound::BlockBellUse, SoundCategory::Blocks, &pos);
        tracing::info!(
            entity = entity.entity_id,
            x = pos.x,
            z = pos.z,
            "civilian rang the town bell"
        );
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }
}

/// Placeholder so builders have a registered goal. Repair-by-diff comes later.
pub struct BuilderStubGoal {
    ticks: AtomicI32,
}

impl Default for BuilderStubGoal {
    fn default() -> Self {
        Self {
            ticks: AtomicI32::new(0),
        }
    }
}

impl Goal for BuilderStubGoal {
    fn can_start(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        true
    }

    fn tick(&mut self, _mob: &dyn Mob) {
        let n = self.ticks.fetch_add(1, Ordering::Relaxed);
        if n % 200 == 0 {
            tracing::debug!("builder stub tick (repair-by-diff not wired)");
        }
    }

    fn controls(&self) -> Controls {
        Controls::empty()
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach::default())
    }
}

/// Pillagers march on the threatened gate / keep.
pub struct MarchOnTownGoal {
    refresh: Arc<dyn Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync>,
    dest: Option<PostTarget>,
}

impl MarchOnTownGoal {
    #[must_use]
    pub fn new(refresh: impl Fn(&dyn Mob) -> Option<PostTarget> + Send + Sync + 'static) -> Self {
        Self {
            refresh: Arc::new(refresh),
            dest: None,
        }
    }
}

impl Goal for MarchOnTownGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        if mob.get_mob_entity().get_target().is_some() {
            return false;
        }
        let Some(dest) = (self.refresh)(mob) else {
            return false;
        };
        self.dest = Some(dest);
        dist_sq_xz(mob.get_entity().pos.load(), dest.vec()) > POST_REACH_SQ
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest {
            let _ = try_path(mob, dest.vec(), MARCH_SPEED, None);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest
            && mob.get_entity().age.load(Ordering::Relaxed) % 12 == 0
        {
            let _ = try_path(mob, dest.vec(), MARCH_SPEED, None);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            entities: 0.0,
            players: 0.0,
            speed: MARCH_SPEED,
            changes_blocks: false,
            links: Vec::new(),
        })
    }
}

fn equip(mob: &dyn Mob, slot: EquipmentSlot, item: &'static Item) {
    mob.get_mob_entity()
        .set_item_slot(&slot, ItemStack::new(1, item));
}

fn tag(entity: &pumpkin_core::entity::Entity, value: &str) {
    let _ = entity.add_scoreboard_tag(value);
}

/// Strip vanilla flee-from-illager goals so guards actually fight.
fn remove_avoid(mob: &dyn Mob) {
    mob.get_mob_entity()
        .goals_selector
        .remove_goal::<AvoidEntityGoal>(mob);
}

/// Wire a town villager or iron golem.
pub fn bind_citizen(
    mob: &dyn Mob,
    town_id: &str,
    kingdom: &crate::faction::KingdomId,
    role: VillagerRole,
) {
    let entity = mob.get_entity();
    mob.get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    tag(entity, TAG_CITIZEN);
    tag(entity, TAG_WIRED);
    tag(entity, &town_tag(town_id));
    tag(entity, &kingdom_tag(kingdom));
    tag(entity, &role_tag(role.as_str()));

    let town = town_id.to_owned();
    let town_flee = town.clone();
    let town_bell = town.clone();
    let town_home = town.clone();
    let role_copy = role;

    match role {
        VillagerRole::Guard => {
            remove_avoid(mob);
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::IRON_SWORD);
            equip(mob, EquipmentSlot::HEAD, &Item::CHAINMAIL_HELMET);
            equip(mob, EquipmentSlot::CHEST, &Item::CHAINMAIL_CHESTPLATE);
            equip(mob, EquipmentSlot::LEGS, &Item::CHAINMAIL_LEGGINGS);
            equip(mob, EquipmentSlot::FEET, &Item::CHAINMAIL_BOOTS);
            mob.get_mob_entity()
                .add_goal(2, MeleeAttackGoal::new(1.1, true));
            mob.get_mob_entity().add_goal(
                3,
                MoveToPostGoal::new(PATROL_SPEED, move |m| {
                    crate::director::post_for_role(m, &town, role_copy)
                }),
            );
        }
        VillagerRole::Archer => {
            remove_avoid(mob);
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::BOW);
            equip(mob, EquipmentSlot::HEAD, &Item::LEATHER_HELMET);
            equip(mob, EquipmentSlot::CHEST, &Item::LEATHER_CHESTPLATE);
            mob.get_mob_entity()
                .add_goal(2, BowAttackGoal::new(1.0, 20, 16.0));
            mob.get_mob_entity().add_goal(
                3,
                MoveToPostGoal::new(PATROL_SPEED, move |m| {
                    crate::director::post_for_role(m, &town, role_copy)
                }),
            );
        }
        VillagerRole::Civilian => {
            mob.get_mob_entity().add_goal(
                1,
                FleeToKeepGoal::new(move |m| crate::director::keep_if_alarmed(m, &town_flee)),
            );
            mob.get_mob_entity().add_goal(
                1,
                RingBellGoal::new(move |m| crate::director::is_alarmed_for(m, &town_bell)),
            );
            mob.get_mob_entity().add_goal(
                4,
                MoveToPostGoal::new(0.5, move |m| {
                    crate::director::post_for_role(m, &town_home, VillagerRole::Civilian)
                }),
            );
        }
        VillagerRole::Builder => {
            mob.get_mob_entity().add_goal(3, BuilderStubGoal::default());
            mob.get_mob_entity().add_goal(
                4,
                MoveToPostGoal::new(0.5, move |m| {
                    crate::director::post_for_role(m, &town, VillagerRole::Builder)
                }),
            );
        }
    }
}

/// Tag and extra goals for a town iron golem. Vanilla melee / monster targeting stay.
pub fn bind_golem(mob: &dyn Mob, town_id: &str, kingdom: &crate::faction::KingdomId) {
    let entity = mob.get_entity();
    mob.get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    tag(entity, TAG_GOLEM);
    tag(entity, TAG_WIRED);
    tag(entity, TAG_CITIZEN);
    tag(entity, &town_tag(town_id));
    tag(entity, &kingdom_tag(kingdom));
    tag(entity, &role_tag(VillagerRole::Guard.as_str()));
    let town = town_id.to_owned();
    mob.get_mob_entity().add_goal(
        3,
        MoveToPostGoal::new(0.8, move |m| {
            crate::director::post_for_role(m, &town, VillagerRole::Guard)
        }),
    );
}

/// Extra raid behaviour on top of vanilla pillager / vindicator goals.
pub fn bind_raider(mob: &dyn Mob, town_id: &str, captain: bool, world: &World, camp: ScenePost) {
    let entity = mob.get_entity();
    mob.get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    tag(entity, TAG_RAIDER);
    tag(entity, TAG_WIRED);
    tag(entity, &town_tag(town_id));
    if captain {
        tag(entity, TAG_CAPTAIN);
        if let Some(raider) = mob.as_raider() {
            raider.set_patrol_leader(true);
        }
        let banner = pumpkin_core::entity::mob::raider::create_ominous_banner();
        mob.get_mob_entity()
            .set_item_slot(&EquipmentSlot::HEAD, banner);
    }
    let y = surface_y(world, camp.x, camp.z);
    if let Some(raider) = mob.as_raider() {
        raider.set_patrol_target(BlockPos::new(camp.x, y, camp.z));
        raider.set_can_join_raid(true);
    }
    let town = town_id.to_owned();
    mob.get_mob_entity().add_goal(
        3,
        MarchOnTownGoal::new(move |m| crate::director::raid_march_post(m, &town)),
    );
    if entity.entity_type == &EntityType::VINDICATOR {
        mob.get_mob_entity().add_goal(
            1,
            BreakDoorGoal::new(Arc::new(|d| {
                d != Difficulty::Peaceful && d != Difficulty::Easy
            })),
        );
    }
}

/// True when we already attached kingdom goals (reload-safe).
#[must_use]
pub fn already_wired(entity: &pumpkin_core::entity::Entity) -> bool {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tags.contains(TAG_WIRED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_target_centers_on_block() {
        let t = PostTarget::from_post(ScenePost::new(10, 20), 64);
        let v = t.vec();
        assert!((v.x - 10.5).abs() < f64::EPSILON);
        assert!((v.z - 20.5).abs() < f64::EPSILON);
    }
}
