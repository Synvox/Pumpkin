//! In-process war director: start/stop raids, join kingdoms, query battle state.
//!
//! An HTTP layer can wrap these functions later. Tick is driven from
//! `ServerTickStart`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::mob::Mob;
use pumpkin_core::entity::player::Player;
use pumpkin_core::world::World;
use pumpkin_core::world::scoreboard::{CollisionRule, NameTagVisibility, Team};
use pumpkin_data::damage::DamageType;
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{DataComponentImpl, DyedColorImpl, EquipmentSlot};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

use crate::faction::{
    Faction, KingdomId, TAG_RAIDER, faction_of_entity, is_kingdoms_tagged, kingdom_tag,
    role_from_tags,
};
use crate::flow::{DIRECT_RANGE, FlowField};
use crate::goals::{
    PostTarget, already_wired, bind_citizen, bind_golem, bind_raider, drive_live, shoot_arrow,
};
use crate::info::TownInfo;
use crate::lod::{
    ACQUIRE_RANGE, ALARM_RANGE, DEFAULT_PATH_CAP, LodTier, PathBudget, SpatialBuckets,
};
use crate::plan::plan_from_cache;
use crate::raid::{
    RaidSide, RaidSpec, WAVE_ADVANCE_REMAINING, WAVE_SPACING_TICKS, compose_wave, spawn_offset,
};
use crate::repair::{self, RepairJob};
use crate::roles::{PopulationSpec, VillagerRole, citizen_slots};
use crate::scene::{ScenePost, TownInfoScene, TownScene};
use crate::siege::{
    self, HOLD_TICKS, MarchJob, TownOwner, WALL_WALK_LIFT, in_courtyard, tick_hold,
};
use crate::sim::{SimSnapshot, write_named_snapshots_with_kills};
use crate::town::town_containing;

const FORCELOAD_CAP: usize = 256;
/// Director slide when the mob has not been world-ticked (blocks / tick).
const SLIDE_STEP: f64 = 0.28;
/// Set a vanilla target only this close, so `HoldGroundAttackGoal` does not
/// stop the navigator while they are still marching in.
const SET_TARGET_RANGE: f64 = 12.0;
const LIVE_MELEE_RANGE: f64 = 2.75;
const LIVE_MELEE_DAMAGE: f32 = 4.0;
const LIVE_MELEE_PERIOD: u32 = 10;
const LIVE_RANGED_RANGE: f64 = 16.0;
const LIVE_RANGED_DAMAGE: f32 = 3.0;
const LIVE_RANGED_PERIOD: u32 = 20;
/// Real arrows when a player can see the shot; director damage beyond this.
const PLAYER_ARROW_RANGE: f64 = 32.0;
const RAM_BREACH_RANGE: f64 = 5.0;
const REPAIR_PERIOD: u32 = 40;
const REPAIR_PLACE_PERIOD: u32 = 10;
const REPAIR_REACH: f64 = 2.5;
const REPAIR_QUEUE_CAP: usize = 24;

const COMBAT_LOG_CAP: usize = 512;
const RESPAWN_PERIOD: u32 = 200;

static RUNTIME: OnceLock<Mutex<Director>> = OnceLock::new();

/// Process-wide director. Tests can replace the lock contents.
pub fn lock() -> std::sync::MutexGuard<'static, Director> {
    RUNTIME
        .get_or_init(|| Mutex::new(Director::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RaidId(pub u32);

#[derive(Clone, Debug)]
pub struct BattleState {
    pub town_id: String,
    pub kingdom: String,
    pub alarm: bool,
    pub defenders_alive: u32,
    pub raiders_alive: u32,
    pub defenders_killed: u32,
    pub raiders_killed: u32,
    pub wave: u32,
    pub waves: u32,
    pub raid_active: bool,
    pub threatened_gate: Option<(i32, i32)>,
    pub owner: String,
    pub gate_breached: bool,
}

#[derive(Clone, Debug)]
pub struct CombatEvent {
    pub tick: u32,
    pub kind: CombatKind,
    pub actor: i32,
    pub other: Option<i32>,
    pub x: f64,
    pub z: f64,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CombatKind {
    Alarm,
    Wave,
    Kill,
    RaidStart,
    RaidStop,
    Victory,
    Capture,
    Retake,
    Breach,
}

pub struct Director {
    pub tick: u32,
    next_raid_id: u32,
    towns: HashMap<String, TownWar>,
    player_kingdoms: HashMap<String, KingdomId>,
    worlds: Vec<Weak<World>>,
    pub events: VecDeque<CombatEvent>,
    pub path_budget: PathBudget,
    owners: HashMap<String, TownOwner>,
    ownership_path: Option<PathBuf>,
}

struct TownWar {
    info_id: String,
    kingdom: KingdomId,
    scene: TownInfoScene,
    alarm: bool,
    threatened: Option<ScenePost>,
    raid: Option<ActiveRaid>,
    defenders_killed: u32,
    raiders_killed: u32,
    wired: HashMap<i32, Wired>,
    fields: HashMap<(i32, i32), Arc<FlowField>>,
    raider_gate_dist: f64,
    ticketed: Vec<Vector2<i32>>,
    live_snaps: Vec<SimSnapshot>,
    owner: TownOwner,
    occupy_hold: u32,
    retake_hold: u32,
    gates_closed: bool,
    gate_breached: bool,
    repair_jobs: VecDeque<RepairJob>,
    repair_assigned: HashMap<i32, RepairJob>,
    last_repair_tick: u32,
}

struct Wired {
    faction: Faction,
}

struct ActiveRaid {
    id: RaidId,
    spec: RaidSpec,
    wave: u8,
    spawned_this_wave: u16,
    last_wave_tick: u32,
    camp: ScenePost,
    living: Vec<i32>,
}

impl Director {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: 0,
            next_raid_id: 1,
            towns: HashMap::new(),
            player_kingdoms: HashMap::new(),
            worlds: Vec::new(),
            events: VecDeque::new(),
            path_budget: PathBudget::new(DEFAULT_PATH_CAP),
            owners: HashMap::new(),
            ownership_path: None,
        }
    }

    pub fn attach_world(&mut self, world: &Arc<World>) {
        self.worlds
            .retain(|w| w.upgrade().is_some_and(|o| !Arc::ptr_eq(&o, world)));
        self.worlds.push(Arc::downgrade(world));
        ensure_kingdom_teams(world);
        let path = siege::ownership_path(&world.level.level_folder.root_folder);
        let saved = siege::load_state(&path);
        self.owners = saved.towns;
        for (uuid, name) in saved.players {
            self.player_kingdoms.insert(uuid, KingdomId::new(&name));
        }
        self.ownership_path = Some(path);
        for (id, owner) in &self.owners {
            if let Some(war) = self.towns.get_mut(id) {
                war.owner = owner.clone();
            }
            tracing::info!(
                town = id.as_str(),
                owner = owner.as_str(),
                "loaded town ownership"
            );
        }
    }

    fn persist(&self) {
        let Some(path) = &self.ownership_path else {
            return;
        };
        let players: HashMap<String, String> = self
            .player_kingdoms
            .iter()
            .map(|(uuid, k)| (uuid.clone(), k.as_str().to_owned()))
            .collect();
        if let Err(err) = siege::save_state(path, &self.owners, &players) {
            tracing::warn!(%err, "failed to persist kingdoms ownership");
        }
    }

    fn persist_owner(&mut self, town_id: &str, owner: TownOwner) {
        self.owners.insert(town_id.to_owned(), owner.clone());
        if let Some(war) = self.towns.get_mut(town_id) {
            war.owner = owner;
        }
        self.persist();
    }

    #[must_use]
    pub fn attached_worlds(&self) -> Vec<Arc<World>> {
        self.worlds.iter().filter_map(Weak::upgrade).collect()
    }

    pub fn join_kingdom(&mut self, player: &Player, kingdom: KingdomId) -> KingdomId {
        let uuid = player.gameprofile.id.to_string();
        let entity = player.get_entity();
        let _ = entity.add_scoreboard_tag(&kingdom_tag(&kingdom));
        let world = entity.world.load();
        add_to_team(&world, &kingdom, &player.gameprofile.name);
        self.player_kingdoms.insert(uuid, kingdom.clone());
        self.persist();
        kit_player(player, &kingdom);
        set_keep_respawn(player, &world, &kingdom, &self.towns);
        tracing::info!(
            player = %player.gameprofile.name,
            kingdom = kingdom.as_str(),
            "player joined kingdom"
        );
        crate::http::notify_join(&player.gameprofile.name, kingdom.as_str());
        kingdom
    }

    #[must_use]
    pub fn player_kingdom(&self, uuid: &str) -> Option<&KingdomId> {
        self.player_kingdoms.get(uuid)
    }

    #[must_use]
    pub fn faction_for(&self, entity: &dyn EntityBase) -> Faction {
        let player_k = entity.get_player().and_then(|p| {
            self.player_kingdoms
                .get(&p.gameprofile.id.to_string())
                .cloned()
        });
        faction_of_entity(entity, player_k.as_ref())
    }

    /// Start a raid on `town_id`. Spawns happen on the next ticks.
    pub fn start_raid(
        &mut self,
        world: &Arc<World>,
        town_id: &str,
        spec: RaidSpec,
    ) -> Result<RaidId, String> {
        self.ensure_town(world, town_id)?;
        if let Some(raid) = self.towns.get(town_id).and_then(|w| w.raid.as_ref()) {
            return Err(format!("raid {} already running on {town_id}", raid.id.0));
        }
        let (camp, gate) = {
            let war = self
                .towns
                .get(town_id)
                .ok_or_else(|| format!("missing town {town_id}"))?;
            let camp = war
                .scene
                .raid_camps()
                .first()
                .copied()
                .unwrap_or_else(|| war.scene.keep());
            let gate = war.scene.gates().first().map(|g| ScenePost::new(g.x, g.z));
            (camp, gate)
        };
        let id = RaidId(self.next_raid_id);
        self.next_raid_id += 1;
        let tick = self.tick;
        let last_wave_tick = tick.saturating_sub(WAVE_SPACING_TICKS);
        if let Some(war) = self.towns.get_mut(town_id) {
            war.raid = Some(ActiveRaid {
                id,
                spec,
                wave: 0,
                spawned_this_wave: 0,
                last_wave_tick,
                camp,
                living: Vec::new(),
            });
            war.alarm = true;
            war.gates_closed = false;
            war.gate_breached = false;
            war.occupy_hold = 0;
            war.retake_hold = 0;
            if let Some(g) = gate {
                war.threatened = Some(g);
            }
        }
        self.push_event(CombatEvent {
            tick,
            kind: CombatKind::RaidStart,
            actor: 0,
            other: None,
            x: f64::from(camp.x),
            z: f64::from(camp.z),
            note: format!(
                "raid {} size {} side {:?} on {town_id}",
                id.0, spec.raiders, spec.side
            ),
        });
        tracing::info!(
            raid = id.0,
            town = town_id,
            size = spec.raiders,
            side = ?spec.side,
            waves = spec.waves,
            camp_x = camp.x,
            camp_z = camp.z,
            gate_x = gate.map(|g| g.x),
            gate_z = gate.map(|g| g.z),
            "kingdoms raid started"
        );
        Ok(id)
    }

    pub fn stop_raid(&mut self, town_id: Option<&str>) -> (u32, Vec<Vector2<i32>>) {
        let mut stopped = 0;
        let tick = self.tick;
        let mut notes = Vec::new();
        let mut tickets = Vec::new();
        for (id, war) in &mut self.towns {
            if town_id.is_some_and(|want| want != id) {
                continue;
            }
            if let Some(raid) = war.raid.take() {
                stopped += 1;
                notes.push((raid.id.0, id.clone()));
                war.alarm = false;
                tickets.append(&mut war.ticketed);
            }
        }
        for (raid_id, town) in notes {
            self.push_event(CombatEvent {
                tick,
                kind: CombatKind::RaidStop,
                actor: raid_id as i32,
                other: None,
                x: 0.0,
                z: 0.0,
                note: format!("stopped raid {raid_id} on {town}"),
            });
        }
        (stopped, tickets)
    }

    pub fn set_target_town(&mut self, raid_id: RaidId, town_id: &str) -> Result<(), String> {
        let mut found: Option<ActiveRaid> = None;
        for war in self.towns.values_mut() {
            if war.raid.as_ref().is_some_and(|r| r.id == raid_id) {
                found = war.raid.take();
                break;
            }
        }
        let mut raid = found.ok_or_else(|| format!("no raid {}", raid_id.0))?;
        let war = self
            .towns
            .get_mut(town_id)
            .ok_or_else(|| format!("unknown town {town_id}"))?;
        raid.camp = war
            .scene
            .raid_camps()
            .first()
            .copied()
            .unwrap_or_else(|| war.scene.keep());
        war.raid = Some(raid);
        war.alarm = true;
        Ok(())
    }

    #[must_use]
    pub fn battle_state(&self, town_id: &str) -> Option<BattleState> {
        let war = self.towns.get(town_id)?;
        let (raid_active, wave, waves, raiders_alive) =
            war.raid.as_ref().map_or((false, 0, 0, 0), |r| {
                (
                    true,
                    u32::from(r.wave) + 1,
                    u32::from(r.spec.waves),
                    r.living.len() as u32,
                )
            });
        let defenders_alive = war
            .wired
            .values()
            .filter(|w| w.faction.is_kingdom())
            .count() as u32;
        Some(BattleState {
            town_id: war.info_id.clone(),
            kingdom: war.kingdom.as_str().to_owned(),
            alarm: war.alarm,
            defenders_alive,
            raiders_alive,
            defenders_killed: war.defenders_killed,
            raiders_killed: war.raiders_killed,
            wave,
            waves,
            raid_active,
            threatened_gate: war.threatened.map(|p| (p.x, p.z)),
            owner: war.owner.as_str().to_owned(),
            gate_breached: war.gate_breached,
        })
    }

    #[must_use]
    pub fn all_battles(&self) -> Vec<BattleState> {
        self.towns
            .keys()
            .filter_map(|id| self.battle_state(id))
            .collect()
    }

    pub fn note_death(&mut self, entity_id: i32) {
        let tick = self.tick;
        let mut event = None;
        for war in self.towns.values_mut() {
            if let Some(wired) = war.wired.remove(&entity_id) {
                match wired.faction {
                    Faction::Pillager => {
                        war.raiders_killed += 1;
                        if let Some(raid) = &mut war.raid {
                            raid.living.retain(|id| *id != entity_id);
                        }
                        event = Some(("raider", war.info_id.clone(), war.raiders_killed));
                    }
                    Faction::Kingdom(_) => {
                        war.defenders_killed += 1;
                        event = Some(("defender", war.info_id.clone(), war.defenders_killed));
                    }
                    Faction::Neutral => {}
                }
                break;
            }
            if let Some(raid) = &mut war.raid {
                raid.living.retain(|id| *id != entity_id);
            }
        }
        if let Some((kind, town, n)) = event {
            self.push_event(CombatEvent {
                tick,
                kind: CombatKind::Kill,
                actor: entity_id,
                other: None,
                x: 0.0,
                z: 0.0,
                note: format!("{kind} died in {town} (kills {n})"),
            });
        }
    }

    fn ensure_town(&mut self, world: &World, town_id: &str) -> Result<&mut TownWar, String> {
        if self.towns.contains_key(town_id) {
            return self
                .towns
                .get_mut(town_id)
                .ok_or_else(|| "town vanished".to_owned());
        }
        let seed = world.level.seed.0;
        let info = lookup_town_info(seed, town_id)
            .or_else(|| build_town_info(world, town_id))
            .ok_or_else(|| format!("no planned town {town_id}"))?;
        let scene = TownInfoScene::from_info(info);
        let owner = self
            .owners
            .get(town_id)
            .cloned()
            .unwrap_or_else(|| TownOwner::kingdom(&scene.kingdom()));
        let war = TownWar::from_scene(scene, owner);
        self.towns.insert(town_id.to_owned(), war);
        self.towns
            .get_mut(town_id)
            .ok_or_else(|| "town insert failed".to_owned())
    }

    pub(crate) fn ensure_town_from_info(&mut self, info: TownInfo) {
        let id = info.id.clone();
        let owner = self
            .owners
            .get(&id)
            .cloned()
            .unwrap_or_else(|| TownOwner::kingdom(&KingdomId::for_town(&info)));
        self.towns.entry(id).or_insert_with(|| {
            let scene = TownInfoScene::from_info(info);
            TownWar::from_scene(scene, owner)
        });
    }

    fn push_event(&mut self, event: CombatEvent) {
        crate::http::notify_combat(&event);
        if self.events.len() >= COMBAT_LOG_CAP {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

impl TownWar {
    fn from_scene(scene: TownInfoScene, owner: TownOwner) -> Self {
        Self {
            info_id: scene.town_id().to_owned(),
            kingdom: scene.kingdom(),
            scene,
            alarm: false,
            threatened: None,
            raid: None,
            defenders_killed: 0,
            raiders_killed: 0,
            wired: HashMap::new(),
            fields: HashMap::new(),
            raider_gate_dist: f64::INFINITY,
            ticketed: Vec::new(),
            live_snaps: Vec::new(),
            owner,
            occupy_hold: 0,
            retake_hold: 0,
            gates_closed: false,
            gate_breached: false,
            repair_jobs: VecDeque::new(),
            repair_assigned: HashMap::new(),
            last_repair_tick: 0,
        }
    }

    fn field_toward(&mut self, dest: ScenePost) -> Arc<FlowField> {
        let key = crate::flow::cache_key(dest);
        if let Some(field) = self.fields.get(&key) {
            return Arc::clone(field);
        }
        let extras: Vec<ScenePost> = self.scene.raid_camps().to_vec();
        let field = Arc::new(FlowField::toward(&self.scene, dest, &extras));
        self.fields.insert(key, Arc::clone(&field));
        field
    }
}

impl Default for Director {
    fn default() -> Self {
        Self::new()
    }
}

fn lookup_town_info(seed: u64, town_id: &str) -> Option<TownInfo> {
    if let Some(info) = crate::info::town_info_from_id(seed, town_id) {
        return Some(info);
    }
    // "here" is resolved by commands. Cell form "cx,cz".
    if let Some((cx, cz)) = parse_cell(town_id) {
        return crate::info::town_info(seed, cx, cz);
    }
    None
}

fn parse_cell(s: &str) -> Option<(i32, i32)> {
    // Brigadier single-word tokens cannot contain `,`, so `0:0` is the
    // unquoted form. Quoted `"0,0"` still uses the comma.
    let (a, b) = s.split_once(',').or_else(|| s.split_once(':'))?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn parse_town_id(town_id: &str) -> Option<(u64, i32, i32)> {
    let mut parts = town_id.split(':');
    let seed: u64 = parts.next()?.parse().ok()?;
    let cell_x: i32 = parts.next()?.parse().ok()?;
    let cell_z: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((seed, cell_x, cell_z))
}

fn build_town_info(world: &World, town_id: &str) -> Option<TownInfo> {
    let seed = world.level.seed.0;
    let (use_seed, cell_x, cell_z) = if let Some(cell) = parse_cell(town_id) {
        (seed, cell.0, cell.1)
    } else {
        parse_town_id(town_id)?
    };
    let world_gen = world.level.world_gen();
    let plan = crate::plan::plan_for(use_seed, cell_x, cell_z, world_gen.as_ref());
    Some(TownInfo::from_plan(&plan))
}

/// Public: destination a citizen goal should walk to.
#[must_use]
pub fn post_for_role(mob: &dyn Mob, town_id: &str, role: VillagerRole) -> Option<PostTarget> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    let post = match role {
        VillagerRole::Guard if war.alarm && war.gate_breached => war.scene.keep(),
        VillagerRole::Guard if war.alarm => war.threatened.unwrap_or_else(|| war.scene.keep()),
        VillagerRole::Guard => pick_home(mob, war.scene.guard_posts(), war.scene.keep()),
        VillagerRole::Archer => pick_home(mob, war.scene.archer_posts(), war.scene.keep()),
        VillagerRole::Civilian if war.alarm => war.scene.keep(),
        VillagerRole::Civilian => pick_home(mob, war.scene.civilian_homes(), war.scene.keep()),
        VillagerRole::Builder => war.scene.keep(),
    };
    let world = mob.get_entity().world.load();
    let mut y = standing_y(&world, post);
    if role == VillagerRole::Archer {
        y += WALL_WALK_LIFT;
    }
    Some(PostTarget::from_post(post, y))
}

#[must_use]
pub fn keep_if_alarmed(mob: &dyn Mob, town_id: &str) -> Option<PostTarget> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    if !war.alarm {
        return None;
    }
    let keep = war.scene.keep();
    let world = mob.get_entity().world.load();
    Some(PostTarget::from_post(keep, standing_y(&world, keep)))
}

#[must_use]
pub fn is_alarmed_for(_mob: &dyn Mob, town_id: &str) -> bool {
    lock().towns.get(town_id).is_some_and(|w| w.alarm)
}

#[must_use]
pub fn raid_march_post(mob: &dyn Mob, town_id: &str) -> Option<PostTarget> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    let dest = if war.gate_breached {
        war.scene.keep()
    } else {
        war.threatened.unwrap_or_else(|| war.scene.keep())
    };
    let world = mob.get_entity().world.load();
    Some(PostTarget::from_post(dest, standing_y(&world, dest)))
}

fn standing_y(world: &World, post: ScenePost) -> i32 {
    post.height_or(
        world
            .sample_surface_height(post.x, post.z)
            .unwrap_or(world.sea_level)
            + 1,
    )
}

fn pick_home(mob: &dyn Mob, posts: &[ScenePost], fallback: ScenePost) -> ScenePost {
    if posts.is_empty() {
        return fallback;
    }
    let id = mob.get_entity().entity_id.unsigned_abs() as usize;
    posts[id % posts.len()]
}

fn team_name(kingdom: &KingdomId) -> String {
    let mut name = format!("k_{}", kingdom.as_str());
    name.truncate(16);
    name
}

fn pillager_team_name() -> &'static str {
    "k_pillager"
}

fn ensure_kingdom_teams(world: &World) {
    let mut sb = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (name, color, label) in [
        ("k_alder", NamedColor::Green, "Alder"),
        ("k_raven", NamedColor::Blue, "Raven"),
        (pillager_team_name(), NamedColor::Red, "Pillagers"),
    ] {
        if sb.get_team(name).is_some() {
            continue;
        }
        sb.add_team(
            world,
            Team {
                name: name.to_owned(),
                display_name: TextComponent::text(label),
                options: 0,
                nametag_visibility: NameTagVisibility::Always,
                collision_rule: CollisionRule::PushOtherTeams,
                color,
                player_prefix: TextComponent::empty(),
                player_suffix: TextComponent::empty(),
                players: vec![],
            },
        );
    }
}

fn add_to_team(world: &World, kingdom: &KingdomId, scoreboard_name: &str) {
    ensure_kingdom_teams(world);
    let mut sb = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sb.add_player_to_team(world, &team_name(kingdom), scoreboard_name.to_owned());
}

fn add_pillager_team(world: &World, scoreboard_name: &str) {
    ensure_kingdom_teams(world);
    let mut sb = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sb.add_player_to_team(world, pillager_team_name(), scoreboard_name.to_owned());
}

fn kingdom_dye(kingdom: &KingdomId) -> i32 {
    match kingdom.as_str() {
        "raven" => 0x003C44AA,
        "alder" => 0x005E7C16,
        _ => 0x00B02E26,
    }
}

fn dyed_leather(item: &'static Item, rgb: i32) -> ItemStack {
    let mut stack = ItemStack::new(1, item);
    stack.patch.push((
        DataComponent::DyedColor,
        Some(DyedColorImpl { rgb }.to_dyn()),
    ));
    stack
}

fn kit_player(player: &Player, kingdom: &KingdomId) {
    let rgb = kingdom_dye(kingdom);
    let pieces = [
        (EquipmentSlot::HEAD, &Item::LEATHER_HELMET),
        (EquipmentSlot::CHEST, &Item::LEATHER_CHESTPLATE),
        (EquipmentSlot::LEGS, &Item::LEATHER_LEGGINGS),
        (EquipmentSlot::FEET, &Item::LEATHER_BOOTS),
    ];
    let mut changes = Vec::with_capacity(pieces.len());
    {
        let mut eq = player
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (slot, item) in pieces {
            let stack = dyed_leather(item, rgb);
            eq.put(&slot, stack.clone());
            changes.push((slot, stack));
        }
    }
    player.living_entity.send_equipment_changes(&changes);
}

fn set_keep_respawn(
    player: &Player,
    world: &World,
    kingdom: &KingdomId,
    towns: &HashMap<String, TownWar>,
) {
    let Some(keep) = towns
        .values()
        .find(|w| w.kingdom == *kingdom)
        .map(|w| w.scene.keep())
    else {
        return;
    };
    let y = standing_y(world, keep);
    let _ = player.set_respawn_point(
        world.dimension.clone(),
        BlockPos::new(keep.x, y, keep.z),
        0.0,
        0.0,
        true,
    );
}

/// Re-apply team colour, leather, and keep spawn after a reconnect.
pub fn restore_player(player: &Player) {
    let uuid = player.gameprofile.id.to_string();
    let kingdom = lock().player_kingdom(&uuid).cloned();
    let Some(kingdom) = kingdom else {
        return;
    };
    let entity = player.get_entity();
    let _ = entity.add_scoreboard_tag(&kingdom_tag(&kingdom));
    let world = entity.world.load();
    add_to_team(&world, &kingdom, &player.gameprofile.name);
    kit_player(player, &kingdom);
    let dir = lock();
    set_keep_respawn(player, &world, &kingdom, &dir.towns);
}

/// One server tick: alarm, targets, LOD, raid waves.
pub fn tick_world(world: &Arc<World>) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    let t0 = Instant::now();
    let seed = world.level.seed.0;
    let mut dir = lock();
    dir.tick = dir.tick.wrapping_add(1);
    dir.path_budget.reset();
    let tick = dir.tick;

    // Discover planned towns that have a cached plan (generated chunks).
    let cached: Vec<TownInfo> = nearby_cached_towns(seed, world);
    for info in cached {
        dir.ensure_town_from_info(info);
    }

    let town_ids: Vec<String> = dir.towns.keys().cloned().collect();
    drop(dir);

    for town_id in town_ids {
        tick_one_town(world, &town_id, tick);
    }

    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    if tick.is_multiple_of(100) {
        let dir = lock();
        let raid_on = dir.towns.values().any(|w| w.raid.is_some());
        let (def_k, raid_k, def_n, raid_n, gate_d) = dir.towns.values().fold(
            (0u32, 0u32, 0u32, 0u32, f64::INFINITY),
            |(dk, rk, dn, rn, d), w| {
                (
                    dk + w.defenders_killed,
                    rk + w.raiders_killed,
                    dn + w.wired.values().filter(|e| e.faction.is_kingdom()).count() as u32,
                    rn + w.raid.as_ref().map_or(0, |r| r.living.len() as u32),
                    d.min(w.raider_gate_dist),
                )
            },
        );
        drop(dir);
        if raid_on {
            let (tps, mspt) = world
                .server
                .upgrade()
                .map_or((0.0, ms), |s| (s.get_tps(), s.get_mspt()));
            tracing::info!(
                "kingdoms raid tick tick={tick} director_ms={ms:.2} tps={tps:.1} mspt={mspt:.2} defenders={def_n} raiders={raid_n} kills={def_k}/{raid_k} gate_dist={gate_d:.1}"
            );
        } else if ms > 1.0 {
            tracing::debug!(ms, tick, "kingdoms director tick");
        }
    }
}

fn nearby_cached_towns(seed: u64, world: &World) -> Vec<TownInfo> {
    // Players tell us which cells are hot. Console-only worlds still get raids
    // via /raid start, which caches the town explicitly.
    let mut out = Vec::new();
    let players = world.players.load();
    for player in players.iter() {
        let pos = player.living_entity.entity.pos.load();
        let town = town_containing(seed, pos.x.floor() as i32, pos.z.floor() as i32);
        if let Some(info) = crate::info::town_info(seed, town.cell_x, town.cell_z) {
            out.push(info);
        } else if let Some(plan) = plan_from_cache(seed, town.cell_x, town.cell_z) {
            out.push(TownInfo::from_plan(&plan));
        }
    }
    out
}

fn tick_one_town(world: &Arc<World>, town_id: &str, tick: u32) {
    let (bounds, kingdom, alarm_before, camps, gates) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (
            war.scene.bounds(),
            war.kingdom.clone(),
            war.alarm,
            war.scene.raid_camps().to_vec(),
            war.scene
                .gates()
                .iter()
                .map(|g| ScenePost::new(g.x, g.z))
                .collect::<Vec<_>>(),
        )
    };

    let mut min_x = bounds.min_x - 160;
    let mut min_z = bounds.min_z - 160;
    let mut max_x = bounds.max_x + 160;
    let mut max_z = bounds.max_z + 160;
    for p in camps.iter().chain(gates.iter()) {
        min_x = min_x.min(p.x - 48);
        min_z = min_z.min(p.z - 48);
        max_x = max_x.max(p.x + 48);
        max_z = max_z.max(p.z + 48);
    }
    let aabb = BoundingBox::new(
        Vector3::new(f64::from(min_x), -64.0, f64::from(min_z)),
        Vector3::new(f64::from(max_x), 320.0, f64::from(max_z)),
    );
    let entities = world.get_all_at_box(&aabb);

    let mut buckets = SpatialBuckets::new();
    let mut snapshots: Vec<(Arc<dyn EntityBase>, Faction)> = Vec::new();

    {
        let dir = lock();
        for entity in &entities {
            let faction = dir.faction_for(entity.as_ref());
            if matches!(faction, Faction::Neutral) && !is_kingdoms_tagged(entity.get_entity()) {
                continue;
            }
            let pos = entity.get_entity().pos.load();
            buckets.insert(crate::lod::BucketEntry {
                entity_id: entity.get_entity().entity_id,
                x: pos.x,
                z: pos.z,
                faction: faction.clone(),
            });
            snapshots.push((entity.clone(), faction));
        }
    }

    let mut threatened = None;
    let mut pillagers_near = false;
    {
        let dir = lock();
        if let Some(war) = dir.towns.get(town_id) {
            let kingdom_faction = Faction::Kingdom(kingdom);
            for gate in war.scene.gates() {
                if buckets.any_hostile_near(
                    f64::from(gate.x),
                    f64::from(gate.z),
                    &kingdom_faction,
                    ALARM_RANGE,
                ) {
                    pillagers_near = true;
                    threatened = Some(ScenePost::new(gate.x, gate.z));
                    break;
                }
            }
            if !pillagers_near {
                let keep = war.scene.keep();
                pillagers_near = buckets.any_hostile_near(
                    f64::from(keep.x),
                    f64::from(keep.z),
                    &kingdom_faction,
                    ALARM_RANGE * 1.5,
                );
            }
        }
    }

    let mut wave_to_spawn: Option<(u8, RaidSpec, ScenePost, RaidId)> = None;
    let mut pending_events = Vec::new();
    let mut pending_release: Vec<Vector2<i32>> = Vec::new();
    let mut dump_plots = false;
    let mut should_close_gates = false;
    {
        let mut dir = lock();
        {
            let Some(war) = dir.towns.get_mut(town_id) else {
                return;
            };
            if pillagers_near {
                war.alarm = true;
                if let Some(t) = threatened {
                    war.threatened = Some(t);
                }
            } else if war.raid.is_none() {
                war.alarm = false;
            }
            let now_alarm = war.alarm;
            if now_alarm && !alarm_before {
                let x = threatened.map_or(0.0, |p| f64::from(p.x));
                let z = threatened.map_or(0.0, |p| f64::from(p.z));
                pending_events.push(CombatEvent {
                    tick,
                    kind: CombatKind::Alarm,
                    actor: 0,
                    other: None,
                    x,
                    z,
                    note: format!("{town_id} alarm"),
                });
                tracing::info!(town = town_id, "town alarm");
            }
            if war.alarm && !war.gates_closed {
                should_close_gates = true;
                war.gates_closed = true;
            }

            // Refresh living raid ids from snapshots.
            let mut finished = false;
            if let Some(raid) = &mut war.raid {
                if raid.spec.side == RaidSide::Kingdom {
                    let alive: std::collections::HashSet<i32> = snapshots
                        .iter()
                        .map(|(e, _)| e.get_entity().entity_id)
                        .collect();
                    raid.living.retain(|id| alive.contains(id));
                } else {
                    let living: Vec<i32> = snapshots
                        .iter()
                        .filter(|(_, f)| matches!(f, Faction::Pillager))
                        .map(|(e, _)| e.get_entity().entity_id)
                        .collect();
                    raid.living = living;
                }
                let should_advance = raid.spawned_this_wave > 0
                    && (raid.living.len() as u16) <= WAVE_ADVANCE_REMAINING
                    || tick.saturating_sub(raid.last_wave_tick) >= WAVE_SPACING_TICKS
                        && raid.spawned_this_wave == 0
                    || tick.saturating_sub(raid.last_wave_tick) >= WAVE_SPACING_TICKS
                        && raid.wave == 0
                        && raid.spawned_this_wave == 0;
                if raid.spawned_this_wave == 0 && raid.wave < raid.spec.waves && should_advance {
                    wave_to_spawn = Some((raid.wave, raid.spec, raid.camp, raid.id));
                } else if raid.wave >= raid.spec.waves
                    && raid.living.is_empty()
                    && raid.spawned_this_wave > 0
                {
                    finished = true;
                }
            }
            war.wired.clear();
            for (entity, faction) in &snapshots {
                war.wired.insert(
                    entity.get_entity().entity_id,
                    Wired {
                        faction: faction.clone(),
                    },
                );
            }
            let gate = war.threatened.unwrap_or_else(|| war.scene.keep());
            war.raider_gate_dist = snapshots
                .iter()
                .filter(|(_, f)| matches!(f, Faction::Pillager))
                .map(|(e, _)| {
                    let p = e.get_entity().pos.load();
                    (p.x - f64::from(gate.x)).hypot(p.z - f64::from(gate.z))
                })
                .fold(f64::INFINITY, f64::min);
            if finished {
                war.raid = None;
                war.alarm = pillagers_near;
                pending_release.append(&mut war.ticketed);
                pending_events.push(CombatEvent {
                    tick,
                    kind: CombatKind::Victory,
                    actor: 0,
                    other: None,
                    x: 0.0,
                    z: 0.0,
                    note: format!("{town_id} raid defeated"),
                });
                tracing::info!(town = town_id, "raid defeated");
                dump_plots = true;
            }
        }
        for event in pending_events {
            dir.push_event(event);
        }
    }
    if !pending_release.is_empty() {
        apply_force_tickets(world, &pending_release, false);
    }
    if dump_plots {
        dump_live_plots(Some(town_id));
    }
    if should_close_gates {
        let gates = {
            let dir = lock();
            dir.towns
                .get(town_id)
                .map(|w| w.scene.gates().to_vec())
                .unwrap_or_default()
        };
        siege::close_gates(world, &gates, |x, z| {
            standing_y(world, ScenePost::new(x, z))
        });
    }

    // Director-owned combat: NoAI + flow-field slide. Vanilla melee/bow/path
    // goals stay off during a raid so 200 vs 200 does not A* or shove.
    let loop_started = Instant::now();
    let (alarm, march_dest, keep_dest, ram_dest, ladder_dest, keep_rect, breached, owner_pillager) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let dest = war.threatened.unwrap_or_else(|| war.scene.keep());
        let y = standing_y(world, dest);
        let keep = war.scene.keep();
        let ram = war.scene.ram_posts().first().copied().unwrap_or(dest);
        let ladder = war
            .scene
            .ladder_posts()
            .first()
            .copied()
            .or_else(|| {
                war.scene
                    .wall()
                    .iter()
                    .min_by_key(|p| (p[0] - dest.x).abs() + (p[1] - dest.z).abs())
                    .map(|p| ScenePost::new(p[0], p[1]))
            })
            .unwrap_or(dest);
        (
            war.alarm,
            PostTarget::from_post(dest, y),
            PostTarget::from_post(keep, standing_y(world, keep)),
            PostTarget::from_post(ram, standing_y(world, ram)),
            PostTarget {
                x: ladder.x,
                y: standing_y(world, ladder) + WALL_WALK_LIFT,
                z: ladder.z,
            },
            war.scene.keep_rect(),
            war.gate_breached,
            war.owner.is_pillager(),
        )
    };
    maybe_refill_repair(world, town_id, tick, alarm);

    let mut n_ticking = 0u32;
    let mut n_loaded = 0u32;
    let mut n_active = 0u32;
    let mut n_no_ai = 0u32;
    let mut n_slid = 0u32;
    let mut n_melee = 0u32;
    let mut n_ranged = 0u32;
    let mut n_arrows = 0u32;
    let mut n_repair = 0u32;
    let mut ram_at_gate = false;
    let mut raiders_in_keep = 0u32;
    let mut defenders_in_keep = 0u32;
    let active_chunks = world
        .active_chunks
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for (entity, faction) in &snapshots {
        let base = entity.get_entity();
        let chunk = base.chunk_pos.load();
        if world.level.is_chunk_loaded(&chunk) {
            n_loaded += 1;
        }
        if active_chunks.contains(&chunk) {
            n_active += 1;
        }
        if base.tick_count.load(Ordering::Relaxed) > 0 {
            n_ticking += 1;
        }
        let Some(mob) = entity.get_mob() else {
            continue;
        };
        let pos = base.pos.load();
        if in_courtyard(keep_rect, pos.x, pos.z) {
            match faction {
                Faction::Pillager => raiders_in_keep += 1,
                Faction::Kingdom(_) => defenders_in_keep += 1,
                Faction::Neutral => {}
            }
        }
        let hostile = buckets.nearest_hostile(pos.x, pos.z, faction, ACQUIRE_RANGE);
        let in_combat = hostile.is_some();
        let melee_hostile = buckets.nearest_hostile(pos.x, pos.z, faction, LIVE_MELEE_RANGE);
        let ranged_hostile = buckets.nearest_hostile(pos.x, pos.z, faction, LIVE_RANGED_RANGE);
        let role = role_from_tags(base).and_then(|s| VillagerRole::parse(&s));
        let is_builder = role == Some(VillagerRole::Builder);
        let tier = if in_combat {
            LodTier::Combat
        } else if alarm {
            LodTier::Alert
        } else {
            LodTier::Idle
        };
        let id = base.entity_id;
        // Raid soldiers never run vanilla AI. The director slides them.
        let skip = if alarm || is_builder {
            false
        } else {
            !tier.should_tick(id, tick)
        };
        mob.get_mob_entity().set_no_ai(alarm || skip);
        if alarm {
            n_no_ai += 1;
        } else if skip {
            n_no_ai += 1;
            continue;
        }
        if !alarm {
            if let Some(h) = buckets.nearest_hostile(pos.x, pos.z, faction, SET_TARGET_RANGE)
                && let Some(target) = world.get_entity_by_id(h.entity_id)
            {
                mob.set_mob_target(Some(target));
            } else {
                mob.set_mob_target(None);
            }
            let dest = match faction {
                Faction::Pillager => march_dest,
                Faction::Kingdom(_) if is_builder => {
                    assigned_repair_dest(town_id, id, world).unwrap_or(keep_dest)
                }
                Faction::Kingdom(_) => keep_dest,
                Faction::Neutral => continue,
            };
            let _ = drive_live(mob, dest, in_combat);
        }

        let job = job_of(base);
        let repair_dest = if is_builder {
            assigned_repair_dest(town_id, id, world)
        } else {
            None
        };
        let dest = dest_for(
            faction,
            job,
            role,
            alarm,
            in_combat,
            breached,
            repair_dest,
            if owner_pillager {
                keep_dest
            } else {
                march_dest
            },
            keep_dest,
            ram_dest,
            ladder_dest,
        );
        let dest_v = dest.vec();
        let dist = (pos.x - dest_v.x).hypot(pos.z - dest_v.z);
        if job == MarchJob::Ram && dist < RAM_BREACH_RANGE && !breached {
            ram_at_gate = true;
        }
        let period = slide_period(dist, in_combat);
        if alarm && tick.is_multiple_of(period) {
            slide_along_field(base, town_id, dest);
            n_slid += 1;
        } else if !alarm {
            let far = dist > DIRECT_RANGE;
            let unticked = base.tick_count.load(Ordering::Relaxed) == 0;
            if unticked || far {
                slide_along_field(base, town_id, dest);
                n_slid += 1;
            }
        }
        if tick.is_multiple_of(LIVE_MELEE_PERIOD)
            && let Some(h) = melee_hostile
            && let Some(target) = world.get_entity_by_id(h.entity_id)
        {
            let _ = target.damage_with_context(
                target.as_ref(),
                LIVE_MELEE_DAMAGE,
                DamageType::MOB_ATTACK,
                None,
                Some(entity.as_ref()),
                Some(entity.as_ref()),
            );
            n_melee += 1;
        } else if tick.is_multiple_of(LIVE_RANGED_PERIOD)
            && is_ranged_fighter(base, role)
            && let Some(h) = ranged_hostile
            && let Some(target) = world.get_entity_by_id(h.entity_id)
        {
            if player_near(world, pos.x, pos.z, PLAYER_ARROW_RANGE) {
                shoot_arrow(mob, &target);
                n_arrows += 1;
            } else {
                let _ = target.damage_with_context(
                    target.as_ref(),
                    LIVE_RANGED_DAMAGE,
                    DamageType::ARROW,
                    None,
                    Some(entity.as_ref()),
                    Some(entity.as_ref()),
                );
            }
            n_ranged += 1;
        }
        if is_builder
            && repair_dest.is_some()
            && dist < REPAIR_REACH
            && try_place_repair(world, town_id, id, tick)
        {
            mob.get_mob_entity().living_entity.swing_hand();
            n_repair += 1;
        }
    }
    let loop_us = loop_started.elapsed().as_micros();
    if ram_at_gate && !breached {
        let gate = {
            let dir = lock();
            dir.towns.get(town_id).and_then(|w| {
                w.threatened
                    .or_else(|| w.scene.gates().first().map(|g| ScenePost::new(g.x, g.z)))
                    .zip(w.scene.gates().first().copied())
            })
        };
        if let Some((post, gate)) = gate {
            let cleared = siege::breach_gate(world, gate, standing_y(world, post));
            if cleared > 0 {
                let mut dir = lock();
                if let Some(war) = dir.towns.get_mut(town_id) {
                    war.gate_breached = true;
                }
                dir.push_event(CombatEvent {
                    tick,
                    kind: CombatKind::Breach,
                    actor: 0,
                    other: Some(cleared as i32),
                    x: f64::from(gate.x),
                    z: f64::from(gate.z),
                    note: format!("{town_id} ram breached gate"),
                });
            }
        }
    }
    apply_capture(town_id, tick, raiders_in_keep, defenders_in_keep);

    if alarm && tick.is_multiple_of(20) {
        let snap = SimSnapshot {
            tick,
            positions: snapshots
                .iter()
                .map(|(e, f)| {
                    let p = e.get_entity().pos.load();
                    (
                        e.get_entity().entity_id,
                        p.x,
                        p.z,
                        matches!(f, Faction::Pillager),
                    )
                })
                .collect(),
        };
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            if war.live_snaps.len() >= 64 {
                war.live_snaps.remove(0);
            }
            war.live_snaps.push(snap);
        }
    }
    if tick.is_multiple_of(400) && alarm {
        let town = town_id.to_owned();
        std::thread::spawn(move || dump_live_plots(Some(&town)));
    }
    if tick.is_multiple_of(100) && (alarm || n_repair > 0 || n_arrows > 0) {
        let n_entities = snapshots.len();
        tracing::info!(
            "kingdoms raid entity tick town={town_id} ticking={n_ticking} loaded={n_loaded} active={n_active} no_ai={n_no_ai} slid={n_slid} melee={n_melee} ranged={n_ranged} arrows={n_arrows} repair={n_repair} loop_us={loop_us} keep_r={raiders_in_keep} keep_d={defenders_in_keep} entities={n_entities}"
        );
    }

    if let Some((wave, spec, camp, raid_id)) = wave_to_spawn {
        spawn_wave(world, town_id, wave, spec, camp, raid_id, tick);
    }

    if tick.is_multiple_of(RESPAWN_PERIOD) {
        crate::populate::respawn_missing(world, town_id);
    }
}

fn spawn_wave(
    world: &Arc<World>,
    town_id: &str,
    wave: u8,
    spec: RaidSpec,
    camp: ScenePost,
    raid_id: RaidId,
    tick: u32,
) {
    if spec.side == RaidSide::Kingdom {
        spawn_retake_wave(world, town_id, wave, spec, camp, raid_id, tick);
        return;
    }
    let count = spec.raiders_in_wave(wave);
    let members = compose_wave(count, wave);
    let (tents, rams, ladders): (Vec<ScenePost>, Vec<ScenePost>, Vec<ScenePost>) = {
        let dir = lock();
        dir.towns.get(town_id).map_or_else(
            || (vec![camp], Vec::new(), Vec::new()),
            |w| {
                let mut tents = w.scene.raid_camps().to_vec();
                if tents.is_empty() {
                    tents.push(camp);
                }
                (
                    tents,
                    w.scene.ram_posts().to_vec(),
                    w.scene.ladder_posts().to_vec(),
                )
            },
        )
    };
    let mut spawned_ids = Vec::new();
    for (i, member) in members.iter().enumerate() {
        let vindicator = member.entity_type == &EntityType::VINDICATOR;
        let job = MarchJob::assign(i, vindicator);
        let post = match job {
            MarchJob::Ram if !rams.is_empty() => rams[i % rams.len()],
            MarchJob::Ladder if !ladders.is_empty() => ladders[i % ladders.len()],
            _ => tents[i % tents.len()],
        };
        let y = standing_y(world, post);
        let ring = (i / tents.len().max(1)) as u16;
        let pos = spawn_offset(post, ring, y);
        let entity = Entity::new(world.clone(), pos, member.entity_type);
        let base: Arc<dyn EntityBase> = if vindicator {
            pumpkin_core::entity::mob::vindicator::VindicatorEntity::new(entity)
                as Arc<dyn EntityBase>
        } else {
            pumpkin_core::entity::mob::pillager::PillagerEntity::new(entity) as Arc<dyn EntityBase>
        };
        if let Some(mob) = base.get_mob() {
            bind_raider(mob, town_id, member.captain, job, world, post);
            add_pillager_team(world, &base.get_scoreboard_name());
        }
        let id = base.get_entity().entity_id;
        if world.spawn_entity(base) {
            spawned_ids.push(id);
        }
    }
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id)
        && let Some(raid) = &mut war.raid
        && raid.id == raid_id
    {
        raid.spawned_this_wave = count;
        raid.last_wave_tick = tick;
        raid.living.extend(spawned_ids.iter().copied());
        raid.wave = raid.wave.saturating_add(1);
    }
    dir.push_event(CombatEvent {
        tick,
        kind: CombatKind::Wave,
        actor: i32::from(wave) + 1,
        other: Some(i32::from(count)),
        x: f64::from(camp.x),
        z: f64::from(camp.z),
        note: format!("{town_id} wave {} spawned {count}", u32::from(wave) + 1),
    });
    tracing::info!(
        town = town_id,
        wave = u32::from(wave) + 1,
        count,
        "raid wave spawned"
    );
}

/// Spawn one citizen at `pos`.
pub fn spawn_citizen(
    world: &Arc<World>,
    town_id: &str,
    kingdom: &KingdomId,
    role: VillagerRole,
    golem: bool,
    pos: Vector3<f64>,
) -> Option<i32> {
    let base: Arc<dyn EntityBase> = if golem {
        let entity = Entity::new(world.clone(), pos, &EntityType::IRON_GOLEM);
        pumpkin_core::entity::passive::iron_golem::IronGolemEntity::new(entity)
            as Arc<dyn EntityBase>
    } else {
        let entity = Entity::new(world.clone(), pos, &EntityType::VILLAGER);
        pumpkin_core::entity::passive::villager::VillagerEntity::new(entity) as Arc<dyn EntityBase>
    };
    if let Some(mob) = base.get_mob() {
        if golem {
            bind_golem(mob, town_id, kingdom);
        } else {
            bind_citizen(mob, town_id, kingdom, role);
        }
        add_to_team(world, kingdom, &base.get_scoreboard_name());
    }
    let id = base.get_entity().entity_id;
    world.spawn_entity(base).then_some(id)
}

/// Re-bind AI on tagged entities that were loaded from disk.
pub fn rebind_if_needed(entity: &dyn EntityBase) {
    let base = entity.get_entity();
    if already_wired(base) {
        return;
    }
    if !is_kingdoms_tagged(base) {
        return;
    }
    let Some(mob) = entity.get_mob() else {
        return;
    };
    let role = role_from_tags(base).and_then(|s| VillagerRole::parse(&s));
    let tags = base
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let town_id = tags.iter().find_map(|t| {
        t.strip_prefix("kingdoms:town:")
            .map(std::string::ToString::to_string)
    });
    let kingdom = tags
        .iter()
        .find_map(|t| t.strip_prefix("kingdoms:kingdom:").map(KingdomId::new));
    let is_raider = tags.iter().any(|t| t == TAG_RAIDER);
    let is_golem = base.entity_type == &EntityType::IRON_GOLEM;
    drop(tags);
    let Some(town_id) = town_id else {
        return;
    };
    if is_raider {
        let camp = ScenePost::new(base.block_pos.load().0.x, base.block_pos.load().0.z);
        let job = {
            let tags = base
                .scoreboard_tags
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            MarchJob::from_tags(tags.iter())
        };
        bind_raider(mob, &town_id, false, job, &base.world.load(), camp);
        return;
    }
    let Some(kingdom) = kingdom else {
        return;
    };
    if is_golem {
        bind_golem(mob, &town_id, &kingdom);
    } else if let Some(role) = role {
        bind_citizen(mob, &town_id, &kingdom, role);
    }
}

/// Public start-raid used by commands and a future HTTP director.
pub fn start_raid(world: &Arc<World>, town_id: &str, spec: RaidSpec) -> Result<RaidId, String> {
    let extra = spec.extra_defenders;
    let id = lock().start_raid(world, town_id, spec)?;
    prepare_raid_chunks(world, town_id);
    let have = spawn_garrison_if_needed(world, town_id);
    spawn_extra_defenders(world, town_id, extra, have);
    Ok(id)
}

/// Spend one slot of this tick's live navigator budget. Close / follower
/// steering must not call this.
pub fn try_take_path_budget() -> bool {
    lock().path_budget.try_acquire()
}

/// O(1) flow-field waypoint toward `dest` for this town.
#[must_use]
pub fn flow_waypoint(town_id: &str, x: f64, z: f64, dest: PostTarget) -> (f64, f64) {
    let mut dir = lock();
    let Some(war) = dir.towns.get_mut(town_id) else {
        let v = dest.vec();
        return (v.x, v.z);
    };
    let field = war.field_toward(ScenePost::new(dest.x, dest.z));
    field.waypoint(x, z)
}

fn prepare_raid_chunks(world: &Arc<World>, town_id: &str) {
    let Some((chunks, camp, keep)) = raid_chunk_list(town_id) else {
        return;
    };
    {
        let mut forced = world
            .forced_chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in &chunks {
            forced.insert(*pos);
        }
    }
    // `forced_chunks` only puts the column in `World::active_chunks`. The
    // chunk scheduler still unloads block data unless a FULL ticket stays
    // on it — same pair game tests use (`forced_chunks` + `add_force_ticket`).
    // `get_or_fetch_chunk` adds a ticket then *removes* it, so a fetch
    // without this lease is not enough.
    apply_force_tickets(world, &chunks, true);
    {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ticketed.clone_from(&chunks);
            war.live_snaps.clear();
        }
    }
    world.level.mark_chunks_as_newly_watched(&chunks);
    world.update_active_chunks();
    let loaded_world = Arc::clone(world);
    let level = Arc::clone(&world.level);
    let n = chunks.len();
    world.level.spawn_task(async move {
        for pos in &chunks {
            level.get_or_fetch_chunk(*pos, |_| ()).await;
        }
        loaded_world.update_active_chunks();
        tracing::info!(
            count = n,
            camp_x = camp.x,
            camp_z = camp.z,
            keep_x = keep.x,
            keep_z = keep.z,
            "kingdoms raid chunks loaded"
        );
    });
}

fn apply_force_tickets(world: &World, chunks: &[Vector2<i32>], add: bool) {
    if chunks.is_empty() {
        return;
    }
    let mut loading = world
        .level
        .chunk_loading
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for pos in chunks {
        if add {
            loading.add_force_ticket(*pos);
        } else {
            loading.remove_force_ticket(*pos);
        }
    }
    loading.send_change();
}

fn raid_chunk_list(town_id: &str) -> Option<(Vec<Vector2<i32>>, ScenePost, ScenePost)> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    let keep = war.scene.keep();
    let camp = war.scene.raid_camps().first().copied().unwrap_or(keep);
    let mut samples = vec![keep, camp];
    samples.extend(war.scene.raid_camps().iter().copied());
    for g in war.scene.gates() {
        samples.push(ScenePost::new(g.x, g.z));
    }
    // Corridor from camp to the threatened gate so marchers tick.
    let gate = war.threatened.unwrap_or(keep);
    let steps = 16;
    for i in 0..=steps {
        let t = f64::from(i) / f64::from(steps);
        samples.push(ScenePost::new(
            camp.x + ((f64::from(gate.x - camp.x) * t) as i32),
            camp.z + ((f64::from(gate.z - camp.z) * t) as i32),
        ));
        samples.push(ScenePost::new(
            gate.x + ((f64::from(keep.x - gate.x) * t) as i32),
            gate.z + ((f64::from(keep.z - gate.z) * t) as i32),
        ));
    }
    drop(dir);
    let mut set = std::collections::BTreeSet::new();
    for p in samples {
        let cx = p.x >> 4;
        let cz = p.z >> 4;
        for dx in -2..=2 {
            for dz in -2..=2 {
                set.insert((cx + dx, cz + dz));
            }
        }
    }
    let mut chunks: Vec<Vector2<i32>> = set.into_iter().map(|(x, z)| Vector2::new(x, z)).collect();
    chunks.truncate(FORCELOAD_CAP);
    Some((chunks, camp, keep))
}

fn slide_along_field(entity: &Entity, town_id: &str, dest: PostTarget) {
    let pos = entity.pos.load();
    let (wx, wz) = flow_waypoint(town_id, pos.x, pos.z, dest);
    let dx = wx - pos.x;
    let dz = wz - pos.z;
    let len = dx.hypot(dz);
    let climb = (f64::from(dest.y) - pos.y).clamp(-0.45, 0.45);
    if len < 0.05 {
        if climb.abs() < 0.02 {
            return;
        }
        entity.set_pos(Vector3::new(pos.x, pos.y + climb, pos.z));
        return;
    }
    let step = SLIDE_STEP.min(len);
    entity.set_pos(Vector3::new(
        pos.x + dx / len * step,
        pos.y + climb,
        pos.z + dz / len * step,
    ));
}

fn job_of(entity: &Entity) -> MarchJob {
    let tags = entity
        .scoreboard_tags
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    MarchJob::from_tags(tags.iter())
}

fn is_ranged_fighter(entity: &Entity, role: Option<VillagerRole>) -> bool {
    role == Some(VillagerRole::Archer) || entity.entity_type == &EntityType::PILLAGER
}

fn player_near(world: &World, x: f64, z: f64, range: f64) -> bool {
    world.players.load().iter().any(|p| {
        let pos = p.living_entity.entity.pos.load();
        (pos.x - x).hypot(pos.z - z) <= range
    })
}

fn maybe_refill_repair(world: &World, town_id: &str, tick: u32, alarm: bool) {
    if alarm || !tick.is_multiple_of(REPAIR_PERIOD) {
        return;
    }
    let (scene, threatened, queued) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (
            war.scene.clone(),
            war.threatened.map(|p| (p.x, p.z)),
            war.repair_jobs.len() + war.repair_assigned.len(),
        )
    };
    if queued >= 8 {
        return;
    }
    let Some((_, cx, cz)) = parse_town_id(town_id) else {
        return;
    };
    let seed = world.level.seed.0;
    let Some(plan) = plan_from_cache(seed, cx, cz) else {
        return;
    };
    let cols = repair::repair_columns(&scene, threatened);
    let jobs = repair::collect_jobs(world, &plan, &cols, REPAIR_QUEUE_CAP);
    if jobs.is_empty() {
        return;
    }
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        war.repair_jobs = jobs.into_iter().collect();
        tracing::info!(
            town = town_id,
            queued = war.repair_jobs.len(),
            "kingdoms builders queued repair jobs"
        );
    }
}

fn assigned_repair_dest(town_id: &str, entity_id: i32, world: &World) -> Option<PostTarget> {
    let mut dir = lock();
    let war = dir.towns.get_mut(town_id)?;
    if let Some(job) = war.repair_assigned.get(&entity_id).copied() {
        return Some(PostTarget {
            x: job.x,
            y: job.y,
            z: job.z,
        });
    }
    let job = war.repair_jobs.pop_front()?;
    war.repair_assigned.insert(entity_id, job);
    drop(dir);
    let y = standing_y(world, ScenePost::new(job.x, job.z)).max(job.y);
    Some(PostTarget {
        x: job.x,
        y,
        z: job.z,
    })
}

fn try_place_repair(world: &Arc<World>, town_id: &str, entity_id: i32, tick: u32) -> bool {
    let job = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return false;
        };
        if tick.saturating_sub(war.last_repair_tick) < REPAIR_PLACE_PERIOD {
            return false;
        }
        let Some(job) = war.repair_assigned.remove(&entity_id) else {
            return false;
        };
        war.last_repair_tick = tick;
        job
    };
    if repair::place_one(world, job) {
        tracing::info!(
            "kingdoms builder placed block x={} y={} z={}",
            job.x,
            job.y,
            job.z
        );
        true
    } else {
        false
    }
}

fn spawn_retake_wave(
    world: &Arc<World>,
    town_id: &str,
    wave: u8,
    spec: RaidSpec,
    camp: ScenePost,
    raid_id: RaidId,
    tick: u32,
) {
    let count = spec.raiders_in_wave(wave);
    let (kingdom, keep, gate) = {
        let dir = lock();
        dir.towns.get(town_id).map_or_else(
            || (KingdomId::new("alder"), camp, camp),
            |w| {
                (
                    w.kingdom.clone(),
                    w.scene.keep(),
                    w.scene
                        .gates()
                        .first()
                        .map_or_else(|| w.scene.keep(), |g| ScenePost::new(g.x, g.z)),
                )
            },
        )
    };
    let post = ScenePost::new(keep.x * 2 - camp.x, keep.z * 2 - camp.z);
    let post = if post.x == keep.x && post.z == keep.z {
        ScenePost::new(gate.x + 48, gate.z + 48)
    } else {
        post
    };
    let y = standing_y(world, post);
    let mut spawned_ids = Vec::new();
    for i in 0..count {
        let pos = spawn_offset(post, i, y);
        let golem = i % 6 == 5;
        let role = if i % 3 == 2 {
            VillagerRole::Archer
        } else {
            VillagerRole::Guard
        };
        if let Some(id) = spawn_citizen(world, town_id, &kingdom, role, golem, pos) {
            spawned_ids.push(id);
        }
    }
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id)
        && let Some(raid) = &mut war.raid
        && raid.id == raid_id
    {
        raid.spawned_this_wave = count;
        raid.last_wave_tick = tick;
        raid.living.extend(spawned_ids.iter().copied());
        raid.wave = raid.wave.saturating_add(1);
    }
    dir.push_event(CombatEvent {
        tick,
        kind: CombatKind::Wave,
        actor: i32::from(wave) + 1,
        other: Some(i32::from(count)),
        x: f64::from(post.x),
        z: f64::from(post.z),
        note: format!(
            "{town_id} retake wave {} spawned {count}",
            u32::from(wave) + 1
        ),
    });
    tracing::info!(
        town = town_id,
        wave = u32::from(wave) + 1,
        count,
        "retake wave spawned"
    );
}

fn slide_period(dist: f64, in_combat: bool) -> u32 {
    if in_combat || dist < 24.0 {
        1
    } else if dist < 64.0 {
        2
    } else {
        4
    }
}

fn dest_for(
    faction: &Faction,
    job: MarchJob,
    role: Option<VillagerRole>,
    alarm: bool,
    in_combat: bool,
    breached: bool,
    repair_dest: Option<PostTarget>,
    march_dest: PostTarget,
    keep_dest: PostTarget,
    _ram_dest: PostTarget,
    ladder_dest: PostTarget,
) -> PostTarget {
    if role == Some(VillagerRole::Builder) {
        return repair_dest.unwrap_or(keep_dest);
    }
    match faction {
        Faction::Neutral => keep_dest,
        Faction::Kingdom(_) if role == Some(VillagerRole::Archer) => ladder_dest,
        Faction::Kingdom(_) if alarm || in_combat => {
            if breached {
                keep_dest
            } else {
                march_dest
            }
        }
        Faction::Kingdom(_) => keep_dest,
        Faction::Pillager => {
            if job == MarchJob::Ladder && !breached {
                ladder_dest
            } else if breached {
                keep_dest
            } else {
                march_dest
            }
        }
    }
}

fn apply_capture(town_id: &str, tick: u32, raiders_in_keep: u32, defenders_in_keep: u32) {
    let (captured, retaken, kingdom, keep) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let update = tick_hold(
            &war.owner,
            raiders_in_keep,
            defenders_in_keep,
            war.occupy_hold,
            war.retake_hold,
        );
        war.occupy_hold = update.occupy_hold;
        war.retake_hold = update.retake_hold;
        (
            update.captured,
            update.retaken,
            war.kingdom.clone(),
            war.scene.keep(),
        )
    };
    if captured {
        lock().persist_owner(town_id, TownOwner::Pillager);
        lock().push_event(CombatEvent {
            tick,
            kind: CombatKind::Capture,
            actor: raiders_in_keep as i32,
            other: Some(defenders_in_keep as i32),
            x: f64::from(keep.x),
            z: f64::from(keep.z),
            note: format!("{town_id} captured by pillagers after {HOLD_TICKS} ticks"),
        });
        tracing::info!(
            "kingdoms town captured town={town_id} raiders_in_keep={raiders_in_keep} hold={HOLD_TICKS}"
        );
    } else if retaken {
        let owner = TownOwner::kingdom(&kingdom);
        lock().persist_owner(town_id, owner);
        lock().push_event(CombatEvent {
            tick,
            kind: CombatKind::Retake,
            actor: defenders_in_keep as i32,
            other: Some(raiders_in_keep as i32),
            x: f64::from(keep.x),
            z: f64::from(keep.z),
            note: format!("{town_id} retaken by villagers"),
        });
        tracing::info!(
            "kingdoms town retaken town={town_id} defenders_in_keep={defenders_in_keep}"
        );
    }
}

fn plot_dir() -> Option<PathBuf> {
    let raw = std::env::var("KINGDOMS_PLOT_DIR").ok()?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// Write start/mid/end top-down frames from live entity positions.
pub fn dump_live_plots(town_id: Option<&str>) {
    let Some(dir) = plot_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let towns: Vec<(String, TownInfoScene, Vec<SimSnapshot>, u32, u32)> = {
        let locked = lock();
        locked
            .towns
            .iter()
            .filter(|(id, _)| town_id.is_none_or(|want| want == *id))
            .filter(|(_, war)| !war.live_snaps.is_empty())
            .map(|(id, war)| {
                (
                    id.clone(),
                    war.scene.clone(),
                    war.live_snaps.clone(),
                    war.raiders_killed,
                    war.defenders_killed,
                )
            })
            .collect()
    };
    for (id, scene, snaps, raider_kills, defender_kills) in towns {
        let stem = format!(
            "live-{}-d{defender_kills}-r{raider_kills}",
            id.replace(':', "_")
        );
        match write_named_snapshots_with_kills(
            &scene,
            &snaps,
            &dir,
            &stem,
            defender_kills,
            raider_kills,
        ) {
            Ok(paths) => tracing::info!(town = id, ?paths, "wrote live raid plots"),
            Err(err) => tracing::warn!(town = id, %err, "live raid plot failed"),
        }
    }
}

fn spawn_extra_defenders(world: &Arc<World>, town_id: &str, want: u16, have: u16) {
    if want == 0 {
        return;
    }
    let (kingdom, gate) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let gate = war.threatened.unwrap_or_else(|| war.scene.keep());
        (war.kingdom.clone(), gate)
    };
    let extra = want.saturating_sub(have);
    if extra == 0 {
        return;
    }
    let y = standing_y(world, gate);
    let mut spawned = 0u32;
    for i in 0..extra {
        let pos = spawn_offset(gate, i, y);
        let role = if i % 3 == 2 {
            VillagerRole::Archer
        } else {
            VillagerRole::Guard
        };
        if spawn_citizen(world, town_id, &kingdom, role, false, pos).is_some() {
            spawned += 1;
        }
    }
    if spawned > 0 {
        tracing::info!(
            town = town_id,
            spawned,
            want,
            "spawned extra defenders for raid"
        );
    }
}

fn spawn_garrison_if_needed(world: &Arc<World>, town_id: &str) -> u16 {
    let Some((chunks, _, _)) = raid_chunk_list(town_id) else {
        return 0;
    };
    let hot: std::collections::HashSet<(i32, i32)> = chunks.iter().map(|c| (c.x, c.y)).collect();
    let (scene, kingdom, existing) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return 0;
        };
        let existing = war
            .wired
            .values()
            .filter(|w| w.faction.is_kingdom())
            .count();
        (war.scene.clone(), war.kingdom.clone(), existing)
    };
    let spec = PopulationSpec::from_scene(&scene);
    if existing >= spec.total() as usize / 2 {
        return existing as u16;
    }
    let slots = citizen_slots(&scene, spec);
    let mut spawned = 0u32;
    for slot in slots {
        if !hot.contains(&(slot.home.x >> 4, slot.home.z >> 4)) {
            continue;
        }
        let y = standing_y(world, slot.home);
        let pos = Vector3::new(
            f64::from(slot.home.x) + 0.5,
            f64::from(y),
            f64::from(slot.home.z) + 0.5,
        );
        if spawn_citizen(world, town_id, &kingdom, slot.role, slot.is_golem, pos).is_some() {
            spawned += 1;
        }
    }
    if spawned > 0 {
        tracing::info!(town = town_id, spawned, "spawned town garrison for raid");
    }
    existing as u16 + spawned as u16
}

pub fn stop_raid(town_id: Option<&str>) -> u32 {
    let worlds = lock().attached_worlds();
    dump_live_plots(town_id);
    let (n, tickets) = lock().stop_raid(town_id);
    for world in worlds {
        apply_force_tickets(&world, &tickets, false);
    }
    n
}

pub fn stop_raid_by_id(id: RaidId) -> u32 {
    let town = {
        let dir = lock();
        dir.towns.iter().find_map(|(town_id, war)| {
            war.raid
                .as_ref()
                .is_some_and(|r| r.id == id)
                .then(|| town_id.clone())
        })
    };
    town.map_or(0, |town_id| stop_raid(Some(&town_id)))
}

pub fn set_target_town(raid_id: RaidId, town_id: &str) -> Result<(), String> {
    lock().set_target_town(raid_id, town_id)
}

#[must_use]
pub fn battle_state(town_id: &str) -> Option<BattleState> {
    lock().battle_state(town_id)
}

#[must_use]
pub fn query_battle_state(town_id: &str) -> Option<BattleState> {
    battle_state(town_id)
}

pub fn join_kingdom(player: &Player, name: &str) -> KingdomId {
    lock().join_kingdom(player, KingdomId::new(name))
}

/// Resolve a town argument from a command (`here`, `cx,cz`, id, kingdom name).
#[must_use]
pub fn resolve_town_id(world: &World, pos: Vector3<f64>, arg: &str) -> Option<String> {
    let seed = world.level.seed.0;
    if arg == "here" || arg == "~" {
        let t = town_containing(seed, pos.x.floor() as i32, pos.z.floor() as i32);
        return Some(crate::info::town_id(seed, t.cell_x, t.cell_z));
    }
    if let Some((cx, cz)) = parse_cell(arg) {
        return Some(crate::info::town_id(seed, cx, cz));
    }
    if arg.contains(':') {
        return Some(arg.to_owned());
    }
    let want = KingdomId::new(arg);
    let t = town_containing(seed, pos.x.floor() as i32, pos.z.floor() as i32);
    let nearest = crate::info::town_id(seed, t.cell_x, t.cell_z);
    if KingdomId::for_cell(t.cell_x, t.cell_z) == want {
        return Some(nearest);
    }
    Some(nearest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faction::is_hostile;

    #[test]
    fn raid_spec_round_trip_on_director_state() {
        let mut dir = Director::new();
        assert_eq!(dir.stop_raid(None).0, 0);
        let spec = RaidSpec::new(40);
        assert_eq!(spec.waves, 1);
        assert!(is_hostile(
            &Faction::Pillager,
            &Faction::Kingdom(KingdomId::new("alder"))
        ));
        dir.tick = 10;
        assert_eq!(dir.next_raid_id, 1);
    }

    #[test]
    fn battle_state_exposes_owner_and_breach() {
        let state = BattleState {
            town_id: "20261025:0:0".to_owned(),
            kingdom: "alder".to_owned(),
            alarm: true,
            defenders_alive: 0,
            raiders_alive: 12,
            defenders_killed: 20,
            raiders_killed: 3,
            wave: 1,
            waves: 1,
            raid_active: true,
            threatened_gate: Some((1, 2)),
            owner: "pillager".to_owned(),
            gate_breached: true,
        };
        assert_eq!(state.owner, "pillager");
        assert!(state.gate_breached);
    }
}
