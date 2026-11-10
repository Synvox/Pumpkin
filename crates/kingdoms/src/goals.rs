//! Kingdom-specific goals built on Pumpkin's `Goal` trait.
//!
//! Melee uses [`ThrottledMeleeGoal`] instead of vanilla `MeleeAttackGoal` so
//! large fights do not path every 4 ticks. March / post goals follow a shared
//! flow field; only squad leaders spend the path budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::ai::goal::avoid_entity::AvoidEntityGoal;
use pumpkin_core::entity::ai::goal::bow_attack::BowAttackGoal;
use pumpkin_core::entity::ai::goal::break_door::BreakDoorGoal;
use pumpkin_core::entity::ai::goal::melee_attack::MeleeAttackGoal;
use pumpkin_core::entity::ai::goal::pathfind_to_raid::PathfindToRaidGoal;
use pumpkin_core::entity::ai::goal::ranged_crossbow_attack::RangedCrossbowAttackGoal;
use pumpkin_core::entity::ai::goal::wander_around::WanderAroundGoal;
use pumpkin_core::entity::ai::goal::{Controls, Goal, GoalReach};
use pumpkin_core::entity::ai::pathfinder::NavigatorGoal;
use pumpkin_core::entity::mob::Mob;
use pumpkin_core::entity::mob::patrol::LongDistancePatrolGoal;
use pumpkin_core::entity::mob::raider::{HoldGroundAttackGoal, RaiderMoveThroughVillageGoal};
use pumpkin_core::entity::projectile::arrow::{ArrowEntity, ArrowPickup};
use pumpkin_core::world::World;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, DyedColorImpl, EquipmentSlot};
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_util::Difficulty;
use pumpkin_util::math::vector3::Vector3;

use crate::faction::{
    TAG_BEARER, TAG_CAPTAIN, TAG_CITIZEN, TAG_GOLEM, TAG_RAIDER, TAG_WIRED, kingdom_tag, role_tag,
    town_tag,
};
use crate::flow::{DIRECT_RANGE, is_squad_leader, should_refresh_path};
use crate::roles::VillagerRole;
use crate::scene::ScenePost;
use crate::siege::MarchJob;

const POST_REACH_SQ: f64 = 6.25;
const FLEE_SPEED: f64 = 0.85;
const PATROL_SPEED: f64 = 0.7;
const MARCH_SPEED: f64 = 1.05;
const MELEE_SPEED: f64 = 1.1;
const DIRECT_RANGE_SQ: f64 = DIRECT_RANGE * DIRECT_RANGE;

/// Destination a post-walking goal is heading toward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

fn town_id_of(entity: &pumpkin_core::entity::Entity) -> Option<String> {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    tags.iter().find_map(|t| {
        t.strip_prefix("kingdoms:town:")
            .map(std::string::ToString::to_string)
    })
}

fn dist_sq_xz(pos: Vector3<f64>, dest: Vector3<f64>) -> f64 {
    let dx = pos.x - dest.x;
    let dz = pos.z - dest.z;
    dx * dx + dz * dz
}

fn is_close(pos: Vector3<f64>, dest: Vector3<f64>) -> bool {
    dist_sq_xz(pos, dest) <= DIRECT_RANGE_SQ
}

fn steer_dest(mob: &dyn Mob, dest: PostTarget) -> Vector3<f64> {
    let pos = mob.get_entity().pos.load();
    let raw = dest.vec();
    if is_close(pos, raw) {
        return raw;
    }
    let Some(town) = town_id_of(mob.get_entity()) else {
        return raw;
    };
    let (x, z) = crate::director::flow_waypoint(&town, pos.x, pos.z, dest);
    Vector3::new(x, raw.y, z)
}

fn set_nav(mob: &dyn Mob, dest: Vector3<f64>, speed: f64) {
    let pos = mob.get_entity().pos.load();
    mob.get_mob_entity()
        .steered()
        .navigator()
        .set_progress(NavigatorGoal::new(pos, dest, speed));
}

/// Director-driven walk toward `dest`. Used when vanilla goals lose MOVE or
/// the mob has not been world-ticked yet.
pub fn drive_live(mob: &dyn Mob, dest: PostTarget, in_combat: bool) -> bool {
    let speed = if in_combat { MELEE_SPEED } else { MARCH_SPEED };
    try_path_steered(mob, dest, speed, false)
}

/// Live navigator update. Close / follower steering is free; only a far-away
/// squad leader spends the shared path budget.
fn try_path_steered(mob: &dyn Mob, dest: PostTarget, speed: f64, force: bool) -> bool {
    let pos = mob.get_entity().pos.load();
    let id = mob.get_entity().entity_id;
    let age = mob.get_entity().age.load(Ordering::Relaxed);
    let close = is_close(pos, dest.vec());
    if !force && !should_refresh_path(id, age, close) {
        return false;
    }
    if !close && is_squad_leader(id) && !crate::director::try_take_path_budget() {
        return false;
    }
    set_nav(mob, steer_dest(mob, dest), speed);
    true
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
            let _ = try_path_steered(mob, dest, self.speed, true);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(dest) = self.dest else {
            return;
        };
        let _ = try_path_steered(mob, dest, self.speed, false);
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
            let _ = try_path_steered(mob, dest, FLEE_SPEED, true);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest {
            let _ = try_path_steered(mob, dest, FLEE_SPEED, false);
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

/// Idle animation; the director walks builders to repair jobs after a siege.
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
            let _ = try_path_steered(mob, dest, MARCH_SPEED, true);
        }
    }

    fn tick(&mut self, mob: &dyn Mob) {
        if let Some(dest) = self.dest {
            let _ = try_path_steered(mob, dest, MARCH_SPEED, false);
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

/// Melee that path-refreshes on the squad cadence, not every 4 ticks.
///
/// Vanilla `MeleeAttackGoal` would spend the whole path budget at 200 vs 200.
/// This copy still swings the same way, but walks straight once the target is
/// within [`DIRECT_RANGE`].
pub struct ThrottledMeleeGoal {
    speed: f64,
    cooldown: i32,
}

impl ThrottledMeleeGoal {
    #[must_use]
    pub const fn new(speed: f64) -> Self {
        Self { speed, cooldown: 0 }
    }
}

impl Goal for ThrottledMeleeGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|t| t.get_entity().is_alive())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn start(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().set_attacking(true);
        self.cooldown = 0;
        if let Some(target) = mob.get_mob_entity().get_target() {
            set_nav(mob, target.get_entity().pos.load(), self.speed);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        mob.get_mob_entity().set_attacking(false);
        mob.get_mob_entity().steered().navigator().stop();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let target_pos = target.get_entity().pos.load();
        let eye = target.get_entity().get_eye_pos();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(eye.x, eye.y, eye.z, 30.0, 30.0);

        let pos = mob.get_entity().pos.load();
        let close = is_close(pos, target_pos);
        let los = close || mob.has_line_of_sight(target.get_entity());
        let dest = if close && los {
            target_pos
        } else if let Some(town) = town_id_of(mob.get_entity()) {
            let post = PostTarget {
                x: target_pos.x.floor() as i32,
                y: target_pos.y.floor() as i32,
                z: target_pos.z.floor() as i32,
            };
            let (x, z) = crate::director::flow_waypoint(&town, pos.x, pos.z, post);
            Vector3::new(x, target_pos.y, z)
        } else {
            target_pos
        };
        let id = mob.get_entity().entity_id;
        let age = mob.get_entity().age.load(Ordering::Relaxed);
        if should_refresh_path(id, age, close || los) {
            let far_leader = !close && !los && is_squad_leader(id);
            let allowed = !far_leader || crate::director::try_take_path_budget();
            if allowed {
                set_nav(mob, dest, self.speed);
            }
        }

        self.cooldown = (self.cooldown - 1).max(0);
        if self.cooldown <= 0
            && mob.get_mob_entity().is_in_attack_range(target.as_ref())
            && mob.has_line_of_sight(target.get_entity())
        {
            self.cooldown = 20;
            if let Some(golem) = mob.as_iron_golem() {
                golem.trigger_attack_animation();
            }
            mob.get_mob_entity().living_entity.swing_hand();
            let damaged = mob
                .get_mob_entity()
                .try_attack(mob.get_entity(), target.as_ref());
            if damaged {
                mob.on_attack(target.as_ref());
            }
        }
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn controls(&self) -> Controls {
        Controls::MOVE | Controls::LOOK
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            entities: ACQUIRE_AS_REACH,
            players: 0.0,
            speed: self.speed,
            changes_blocks: false,
            links: Vec::new(),
        })
    }
}

const ACQUIRE_AS_REACH: f64 = 28.0;
const BOW_RANGE: f64 = 16.0;
const BOW_RANGE_SQ: f64 = BOW_RANGE * BOW_RANGE;

/// Look-at-target bow. Does not path; the director slides the archer.
#[derive(Default)]
pub struct ThrottledBowGoal {
    cooldown: i32,
}

impl Goal for ThrottledBowGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity()
            .get_target()
            .is_some_and(|t| t.get_entity().is_alive())
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        self.can_start(mob)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(target) = mob.get_mob_entity().get_target() else {
            return;
        };
        let eye = target.get_entity().get_eye_pos();
        mob.get_mob_entity()
            .look_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .look_at_with_range(eye.x, eye.y, eye.z, 30.0, 30.0);
        let pos = mob.get_entity().pos.load();
        let tpos = target.get_entity().pos.load();
        let dist_sq = (pos.x - tpos.x).hypot(pos.z - tpos.z).powi(2);
        self.cooldown = (self.cooldown - 1).max(0);
        if self.cooldown <= 0 && dist_sq <= BOW_RANGE_SQ {
            self.cooldown = 20;
            mob.get_mob_entity().living_entity.swing_hand();
        }
    }

    fn controls(&self) -> Controls {
        Controls::LOOK
    }

    fn tick_reach(&self, _mob: &dyn Mob, _running: bool) -> Option<GoalReach> {
        Some(GoalReach {
            entities: BOW_RANGE,
            players: 0.0,
            speed: 0.0,
            changes_blocks: false,
            links: Vec::new(),
        })
    }
}

/// Drop vanilla pathing combat so 200 soldiers do not each A* every few ticks.
fn strip_vanilla_combat(mob: &dyn Mob) {
    let goals = &mob.get_mob_entity().goals_selector;
    goals.remove_goal::<MeleeAttackGoal>(mob);
    goals.remove_goal::<BowAttackGoal>(mob);
    goals.remove_goal::<RangedCrossbowAttackGoal>(mob);
    goals.remove_goal::<HoldGroundAttackGoal>(mob);
    goals.remove_goal::<LongDistancePatrolGoal>(mob);
    goals.remove_goal::<PathfindToRaidGoal>(mob);
    goals.remove_goal::<RaiderMoveThroughVillageGoal>(mob);
    goals.remove_goal::<WanderAroundGoal>(mob);
}

/// Put `item` in `slot` and sync equipment to watchers.
pub fn equip(mob: &dyn Mob, slot: EquipmentSlot, item: &'static Item) {
    mob.get_mob_entity()
        .set_item_slot(&slot, ItemStack::new(1, item));
}

fn dyed_leather(item: &'static Item, rgb: i32) -> ItemStack {
    let mut stack = ItemStack::new(1, item);
    stack.patch.push((
        DataComponent::DyedColor,
        Some(DyedColorImpl { rgb }.to_dyn()),
    ));
    stack
}

fn kit_kingdom(mob: &dyn Mob, kingdom: &crate::faction::KingdomId) {
    let rgb = crate::faction::kingdom_color_rgb(kingdom);
    let pieces = [
        (EquipmentSlot::HEAD, &Item::LEATHER_HELMET),
        (EquipmentSlot::CHEST, &Item::LEATHER_CHESTPLATE),
        (EquipmentSlot::LEGS, &Item::LEATHER_LEGGINGS),
        (EquipmentSlot::FEET, &Item::LEATHER_BOOTS),
    ];
    for (slot, item) in pieces {
        mob.get_mob_entity()
            .set_item_slot(&slot, dyed_leather(item, rgb));
    }
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
    mob.get_mob_entity().set_director_driven(true);
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
            strip_vanilla_combat(mob);
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::IRON_SWORD);
            kit_kingdom(mob, kingdom);
            mob.get_mob_entity()
                .add_goal(2, ThrottledMeleeGoal::new(MELEE_SPEED));
            mob.get_mob_entity().add_goal(
                3,
                MoveToPostGoal::new(PATROL_SPEED, move |m| {
                    crate::director::post_for_role(m, &town, role_copy)
                }),
            );
        }
        VillagerRole::Archer => {
            remove_avoid(mob);
            strip_vanilla_combat(mob);
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::BOW);
            kit_kingdom(mob, kingdom);
            mob.get_mob_entity()
                .add_goal(2, ThrottledBowGoal::default());
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
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::IRON_AXE);
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
    mob.get_mob_entity().set_director_driven(true);
    tag(entity, TAG_GOLEM);
    tag(entity, TAG_WIRED);
    tag(entity, TAG_CITIZEN);
    tag(entity, &town_tag(town_id));
    tag(entity, &kingdom_tag(kingdom));
    tag(entity, &role_tag(VillagerRole::Guard.as_str()));
    strip_vanilla_combat(mob);
    mob.get_mob_entity()
        .add_goal(2, ThrottledMeleeGoal::new(MELEE_SPEED));
    let town = town_id.to_owned();
    mob.get_mob_entity().add_goal(
        3,
        MoveToPostGoal::new(0.8, move |m| {
            crate::director::post_for_role(m, &town, VillagerRole::Guard)
        }),
    );
}

/// Extra raid behaviour on top of vanilla pillager / vindicator goals.
pub fn bind_raider(
    mob: &dyn Mob,
    town_id: &str,
    captain: bool,
    job: MarchJob,
    _world: &World,
    _camp: ScenePost,
) {
    let entity = mob.get_entity();
    mob.get_mob_entity()
        .persistence_required
        .store(true, Ordering::Relaxed);
    mob.get_mob_entity().set_director_driven(true);
    tag(entity, TAG_RAIDER);
    tag(entity, TAG_WIRED);
    tag(entity, &town_tag(town_id));
    if let Some(job_tag) = job.tag() {
        tag(entity, job_tag);
    }
    strip_vanilla_combat(mob);
    if captain {
        tag(entity, TAG_CAPTAIN);
        tag(entity, TAG_BEARER);
        if let Some(raider) = mob.as_raider() {
            raider.set_patrol_leader(true);
        }
        let banner = pumpkin_core::entity::mob::raider::create_ominous_banner();
        mob.get_mob_entity()
            .set_item_slot(&EquipmentSlot::HEAD, banner);
    }
    if let Some(raider) = mob.as_raider() {
        raider.set_can_join_raid(true);
    }
    if entity.entity_type == &EntityType::VINDICATOR {
        mob.get_mob_entity()
            .add_goal(2, ThrottledMeleeGoal::new(MELEE_SPEED));
        mob.get_mob_entity().add_goal(
            1,
            BreakDoorGoal::new(Arc::new(|d| {
                d != Difficulty::Peaceful && d != Difficulty::Easy
            })),
        );
    } else {
        mob.get_mob_entity()
            .add_goal(2, ThrottledBowGoal::default());
    }
    let town = town_id.to_owned();
    mob.get_mob_entity().add_goal(
        3,
        MarchOnTownGoal::new(move |m| crate::director::raid_march_post(m, &town)),
    );
}

const ARROW_SPEED: f64 = 1.6;

/// Spawn a real arrow at `target`. Used when a player is close enough to see it.
pub fn shoot_arrow(mob: &dyn Mob, target: &Arc<dyn EntityBase>) {
    let entity = mob.get_entity();
    let world = entity.world.load();
    let world_full = entity.world.load_full();
    let arrow_entity = Entity::new(world.clone(), entity.pos.load(), &EntityType::ARROW);
    let projectile = ItemStack::new(1, &Item::ARROW);
    let bow = ItemStack::new(1, &Item::BOW);
    let arrow = ArrowEntity::new_shot_with_weapon(
        arrow_entity,
        entity,
        &projectile,
        &bow,
        ArrowPickup::Disallowed,
    );
    let difficulty = world.level_info.load().difficulty as i32;
    arrow.set_base_damage_from_mob(ARROW_SPEED, difficulty);
    let mob_pos = entity.pos.load();
    let target_entity = target.get_entity();
    let target_pos = target_entity.pos.load();
    let dx = target_pos.x - mob_pos.x;
    let dy = (target_pos.y + f64::from(target_entity.entity_dimension.load().height) / 3.0)
        - arrow.entity.pos.load().y;
    let dz = target_pos.z - mob_pos.z;
    let horizontal_distance = dx.hypot(dz);
    let divergence = f64::from(14 - difficulty * 4);
    arrow.set_velocity(
        dx,
        horizontal_distance.mul_add(0.2, dy),
        dz,
        ARROW_SPEED,
        divergence,
    );
    world.play_sound(Sound::EntityArrowShoot, SoundCategory::Hostile, &mob_pos);
    let arrow: Arc<dyn EntityBase> = Arc::new(arrow);
    world_full.spawn_entity(arrow);
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
