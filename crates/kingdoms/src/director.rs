//! In-process war director: start/stop raids, join kingdoms, query battle state.
//!
//! An HTTP layer can wrap these functions later. Tick is driven from
//! `ServerTickStart`.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::mob::Mob;
use pumpkin_core::entity::player::Player;
use pumpkin_core::world::World;
use pumpkin_core::world::scoreboard::{CollisionRule, NameTagVisibility, Team};
use pumpkin_data::dimension::Dimension;
use pumpkin_data::entity::EntityType;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

use crate::faction::{
    Faction, KingdomId, TAG_RAIDER, faction_of_entity, is_kingdoms_tagged, kingdom_tag,
    role_from_tags,
};
use crate::goals::{PostTarget, already_wired, bind_citizen, bind_golem, bind_raider};
use crate::info::TownInfo;
use crate::lod::{
    ACQUIRE_RANGE, ALARM_RANGE, DEFAULT_PATH_CAP, LodTier, PathBudget, SpatialBuckets,
};
use crate::plan::plan_from_cache;
use crate::raid::{
    RaidSpec, WAVE_ADVANCE_REMAINING, WAVE_SPACING_TICKS, compose_wave, spawn_offset,
};
use crate::roles::VillagerRole;
use crate::scene::{ScenePost, TownInfoScene, TownScene};
use crate::town::town_containing;

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
}

pub struct Director {
    pub tick: u32,
    next_raid_id: u32,
    towns: HashMap<String, TownWar>,
    player_kingdoms: HashMap<String, KingdomId>,
    worlds: Vec<Weak<World>>,
    pub events: VecDeque<CombatEvent>,
    pub path_budget: PathBudget,
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
        }
    }

    pub fn attach_world(&mut self, world: &Arc<World>) {
        self.worlds
            .retain(|w| w.upgrade().is_some_and(|o| !Arc::ptr_eq(&o, world)));
        self.worlds.push(Arc::downgrade(world));
        ensure_kingdom_teams(world);
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
        tracing::info!(
            player = %player.gameprofile.name,
            kingdom = kingdom.as_str(),
            "player joined kingdom"
        );
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
            note: format!("raid {} size {} on {town_id}", id.0, spec.raiders),
        });
        tracing::info!(
            raid = id.0,
            town = town_id,
            size = spec.raiders,
            waves = spec.waves,
            "kingdoms raid started"
        );
        Ok(id)
    }

    pub fn stop_raid(&mut self, town_id: Option<&str>) -> u32 {
        let mut stopped = 0;
        let tick = self.tick;
        let mut notes = Vec::new();
        for (id, war) in &mut self.towns {
            if town_id.is_some_and(|want| want != id) {
                continue;
            }
            if let Some(raid) = war.raid.take() {
                stopped += 1;
                notes.push((raid.id.0, id.clone()));
                war.alarm = false;
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
        stopped
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
        let war = TownWar {
            info_id: scene.town_id().to_owned(),
            kingdom: scene.kingdom(),
            scene,
            alarm: false,
            threatened: None,
            raid: None,
            defenders_killed: 0,
            raiders_killed: 0,
            wired: HashMap::new(),
        };
        self.towns.insert(town_id.to_owned(), war);
        self.towns
            .get_mut(town_id)
            .ok_or_else(|| "town insert failed".to_owned())
    }

    pub(crate) fn ensure_town_from_info(&mut self, info: TownInfo) {
        let id = info.id.clone();
        self.towns.entry(id).or_insert_with(|| {
            let scene = TownInfoScene::from_info(info);
            TownWar {
                info_id: scene.town_id().to_owned(),
                kingdom: scene.kingdom(),
                scene,
                alarm: false,
                threatened: None,
                raid: None,
                defenders_killed: 0,
                raiders_killed: 0,
                wired: HashMap::new(),
            }
        });
    }

    fn push_event(&mut self, event: CombatEvent) {
        if self.events.len() >= COMBAT_LOG_CAP {
            self.events.pop_front();
        }
        self.events.push_back(event);
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
    let (a, b) = s.split_once(',')?;
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
        VillagerRole::Guard if war.alarm => war.threatened.unwrap_or_else(|| war.scene.keep()),
        VillagerRole::Guard => pick_home(mob, war.scene.guard_posts(), war.scene.keep()),
        VillagerRole::Archer => pick_home(mob, war.scene.archer_posts(), war.scene.keep()),
        VillagerRole::Civilian if war.alarm => war.scene.keep(),
        VillagerRole::Civilian => pick_home(mob, war.scene.civilian_homes(), war.scene.keep()),
        VillagerRole::Builder => war.scene.keep(),
    };
    let world = mob.get_entity().world.load();
    let y = world
        .sample_surface_height(post.x, post.z)
        .unwrap_or(world.sea_level);
    Some(PostTarget::from_post(post, y + 1))
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
    let y = world
        .sample_surface_height(keep.x, keep.z)
        .unwrap_or(world.sea_level);
    Some(PostTarget::from_post(keep, y + 1))
}

#[must_use]
pub fn is_alarmed_for(_mob: &dyn Mob, town_id: &str) -> bool {
    lock().towns.get(town_id).is_some_and(|w| w.alarm)
}

#[must_use]
pub fn raid_march_post(mob: &dyn Mob, town_id: &str) -> Option<PostTarget> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    let dest = war.threatened.unwrap_or_else(|| war.scene.keep());
    let world = mob.get_entity().world.load();
    let y = world
        .sample_surface_height(dest.x, dest.z)
        .unwrap_or(world.sea_level);
    Some(PostTarget::from_post(dest, y + 1))
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
    if tick.is_multiple_of(200) && ms > 1.0 {
        tracing::debug!(ms, tick, "kingdoms director tick");
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
    let (bounds, kingdom, alarm_before) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (war.scene.bounds(), war.kingdom.clone(), war.alarm)
    };

    let aabb = BoundingBox::new(
        Vector3::new(
            f64::from(bounds.min_x - 160),
            -64.0,
            f64::from(bounds.min_z - 160),
        ),
        Vector3::new(
            f64::from(bounds.max_x + 160),
            320.0,
            f64::from(bounds.max_z + 160),
        ),
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

            // Refresh living raid ids from snapshots.
            let mut finished = false;
            if let Some(raid) = &mut war.raid {
                let living: Vec<i32> = snapshots
                    .iter()
                    .filter(|(_, f)| matches!(f, Faction::Pillager))
                    .map(|(e, _)| e.get_entity().entity_id)
                    .collect();
                raid.living = living;
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
            if finished {
                war.raid = None;
                war.alarm = pillagers_near;
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
            }
        }
        for event in pending_events {
            dir.push_event(event);
        }
    }

    // Assign targets + LOD without holding the director lock across nav.
    for (entity, faction) in &snapshots {
        let Some(mob) = entity.get_mob() else {
            continue;
        };
        let pos = entity.get_entity().pos.load();
        let hostile = buckets.nearest_hostile(pos.x, pos.z, faction, ACQUIRE_RANGE);
        let in_combat = hostile.is_some();
        let alarm = {
            let dir = lock();
            dir.towns.get(town_id).is_some_and(|w| w.alarm)
        };
        let tier = if in_combat {
            LodTier::Combat
        } else if alarm {
            LodTier::Alert
        } else {
            LodTier::Idle
        };
        let id = entity.get_entity().entity_id;
        let skip = !tier.should_tick(id, tick);
        mob.get_mob_entity().set_no_ai(skip);
        if skip {
            continue;
        }
        if let Some(h) = hostile
            && let Some(target) = world.get_entity_by_id(h.entity_id)
        {
            mob.set_mob_target(Some(target));
        } else if !in_combat {
            mob.set_mob_target(None);
        }
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
    let count = spec.raiders_in_wave(wave);
    let members = compose_wave(count, wave);
    let y = world
        .sample_surface_height(camp.x, camp.z)
        .unwrap_or(world.sea_level);
    let mut spawned_ids = Vec::new();
    for (i, member) in members.iter().enumerate() {
        let pos = spawn_offset(camp, i as u16, y);
        let entity = Entity::new(world.clone(), pos, member.entity_type);
        let base: Arc<dyn EntityBase> = if member.entity_type == &EntityType::VINDICATOR {
            pumpkin_core::entity::mob::vindicator::VindicatorEntity::new(entity)
                as Arc<dyn EntityBase>
        } else {
            pumpkin_core::entity::mob::pillager::PillagerEntity::new(entity) as Arc<dyn EntityBase>
        };
        if let Some(mob) = base.get_mob() {
            bind_raider(mob, town_id, member.captain, world, camp);
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
        bind_raider(mob, &town_id, false, &base.world.load(), camp);
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
    lock().start_raid(world, town_id, spec)
}

pub fn stop_raid(town_id: Option<&str>) -> u32 {
    lock().stop_raid(town_id)
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
        assert_eq!(dir.stop_raid(None), 0);
        let spec = RaidSpec::new(40);
        assert_eq!(spec.waves, 1);
        assert!(is_hostile(
            &Faction::Pillager,
            &Faction::Kingdom(KingdomId::new("alder"))
        ));
        dir.tick = 10;
        assert_eq!(dir.next_raid_id, 1);
    }
}
