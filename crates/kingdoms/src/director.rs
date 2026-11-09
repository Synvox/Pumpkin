//! In-process war director: start/stop raids, join kingdoms, query battle state.
//!
//! An HTTP layer can wrap these functions later. Tick is driven from
//! `ServerTickStart`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use pumpkin_core::entity::Entity;
use pumpkin_core::entity::EntityBase;
use pumpkin_core::entity::mob::Mob;
use pumpkin_core::entity::player::{Player, TitleMode};
use pumpkin_core::world::World;
use pumpkin_core::world::bossbar::{Bossbar, BossbarColor, BossbarDivisions};
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
use uuid::Uuid;

use crate::abstract_combat::{
    ABSTRACT_MARCH_TICKS, ABSTRACT_PERIOD, AbstractState, PLAYER_LIVE_RANGE, body_budget,
    near_camp, step_abstract,
};
use crate::chat::{self, ShoutKind};
use crate::faction::{
    Faction, KingdomId, TAG_CAPTAIN, TAG_RAIDER, TAG_ROUTED, TAG_TEMP, faction_of_entity, has_tag,
    is_allied, is_hostile, is_kingdoms_tagged, kingdom_color_rgb, kingdom_tag, role_from_tags,
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
    RaidSide, RaidSpec, WAVE_ADVANCE_REMAINING, WAVE_SPACING_TICKS, WaveMember, compose_wave,
    spawn_offset,
};
use crate::repair::{self, RepairJob};
use crate::roles::{PopulationSpec, VillagerRole, citizen_slots};
use crate::scene::{ScenePost, TownInfoScene, TownScene};
use crate::schedule::{self, CivilianPost};
use crate::siege::{
    self, HOLD_TICKS, MarchJob, TownOwner, WALL_WALK_LIFT, capture_need, in_courtyard, tick_hold_n,
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
const CAPTURE_METER_RANGE: f64 = 64.0;
const DEFAULT_WAR_CLOCK_TICKS: u32 = 2400;
/// Quiet ticks after hold lockout so builders can repair for the new owner.
const REPAIR_WINDOW_TICKS: u32 = 400;
const DEFAULT_WAR_CLOCK_FRONTS: usize = 1;
const DEFAULT_WAR_CLOCK_SIZE: u16 = 24;
const REPAIR_PERIOD: u32 = 40;
const REPAIR_PLACE_PERIOD: u32 = 10;
const REPAIR_REACH: f64 = 2.5;
const REPAIR_QUEUE_CAP: usize = 24;

const COMBAT_LOG_CAP: usize = 512;
const RESPAWN_PERIOD: u32 = 200;
/// Force-load tickets and garrison before the first soldier spawns (~4 s).
const WARM_TICKS: u32 = 80;
/// Tickets applied per director tick during warmup.
const TICKETS_PER_TICK: usize = 12;
/// Soldiers (raiders, extras, garrison) spawned per director tick.
const SPAWNS_PER_TICK: usize = 8;
/// After a flip, skip the opposite hold so leftover garrison cannot retake instantly.
const HOLD_LOCKOUT_TICKS: u32 = 400;

static WORST_MSPT_CENTS: AtomicU32 = AtomicU32::new(0);
static WORST_DIRECTOR_CENTS: AtomicU32 = AtomicU32::new(0);

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

/// Front-line summary for the Sails war map (`GET /war`).
#[derive(Clone, Debug)]
pub struct WarFront {
    pub tick: u32,
    pub scores: HashMap<String, i32>,
    pub clock_enabled: bool,
    pub clock_period: u32,
    pub clock_overridden: bool,
    pub last_clock_tick: u32,
    pub next_raid_id: u32,
    pub fronts: usize,
    pub towns: Vec<WarTownLine>,
}

#[derive(Clone, Debug)]
pub struct WarTownLine {
    pub id: String,
    pub kingdom: String,
    pub owner: String,
    pub alarm: bool,
    pub raid_active: bool,
    pub gate_breached: bool,
    pub keep: (i32, i32),
    pub gate: Option<(i32, i32)>,
    pub occupy_hold: u32,
    pub occupy_need: u32,
    pub defenders: u32,
    pub raiders: u32,
    pub raid_side: Option<String>,
    pub repair_until: u32,
    pub live: bool,
}

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
    pub occupy_hold: u32,
    pub occupy_need: u32,
    pub ram_hp: f32,
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
    Rout,
    RamDestroyed,
    WarClock,
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
    scores: HashMap<String, i32>,
    last_clock_tick: u32,
    last_shout: crate::chat::ShoutLog,
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
    ram: Option<crate::ram::SiegeRam>,
    capture_bar: Option<Uuid>,
    captain_down: bool,
    pending_tickets: Vec<Vector2<i32>>,
    ticket_cursor: usize,
    hold_lockout: u32,
    repair_until: u32,
}

struct Wired {
    faction: Faction,
    captain: bool,
}

struct ActiveRaid {
    id: RaidId,
    spec: RaidSpec,
    wave: u8,
    spawned_this_wave: u16,
    last_wave_tick: u32,
    camp: ScenePost,
    living: Vec<i32>,
    temps: Vec<i32>,
    warm_until: u32,
    pending_members: Vec<WaveMember>,
    pending_retake: u16,
    spawn_cursor: u16,
    extra_left: u16,
    ram_spawned: bool,
    materialized: bool,
    virt_raiders: u16,
    virt_defenders: u16,
    virt_march: u32,
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
            scores: HashMap::new(),
            last_clock_tick: 0,
            last_shout: HashMap::new(),
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
        self.scores = saved.scores;
        if saved.clock_tick > 0 {
            self.tick = saved.clock_tick;
        }
        if saved.next_raid_id > 0 {
            self.next_raid_id = saved.next_raid_id;
        }
        self.last_clock_tick = saved.last_clock_tick;
        crate::damage::restore(&saved.damage);
        self.ownership_path = Some(path);
        tracing::info!(
            tick = self.tick,
            next_raid = self.next_raid_id,
            last_clock = self.last_clock_tick,
            scores = ?self.scores,
            "loaded war state"
        );
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
        let state = siege::SavedState {
            towns: self.owners.clone(),
            players,
            scores: self.scores.clone(),
            clock_tick: self.tick,
            next_raid_id: self.next_raid_id,
            last_clock_tick: self.last_clock_tick,
            damage: crate::damage::snapshot(),
        };
        if let Err(err) = siege::save_state(path, &state) {
            tracing::warn!(%err, "failed to persist kingdoms ownership");
        }
    }

    fn add_score(&mut self, name: &str, delta: i32) {
        *self.scores.entry(name.to_owned()).or_insert(0) += delta;
    }

    fn live_body_count(&self) -> usize {
        self.towns
            .values()
            .map(|w| {
                let Some(raid) = w.raid.as_ref() else {
                    return 0;
                };
                if raid.materialized {
                    w.wired.len() + raid.living.len()
                } else {
                    0
                }
            })
            .sum()
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
            let keep = war.scene.keep();
            let pillager_camp = war.scene.raid_camps().first().copied().unwrap_or(keep);
            let camp = if spec.side.is_kingdom_army() {
                war.scene.road_approach().unwrap_or_else(|| {
                    let mirrored =
                        ScenePost::new(keep.x * 2 - pillager_camp.x, keep.z * 2 - pillager_camp.z);
                    if mirrored.x == keep.x && mirrored.z == keep.z {
                        ScenePost::new(keep.x + 48, keep.z + 48)
                    } else {
                        mirrored
                    }
                })
            } else {
                pillager_camp
            };
            let gate = war.scene.gates().first().map(|g| ScenePost::new(g.x, g.z));
            (camp, gate)
        };
        let id = RaidId(self.next_raid_id);
        self.next_raid_id += 1;
        let tick = self.tick;
        let keep = self.towns.get(town_id).map_or(camp, |w| w.scene.keep());
        let near = player_near_posts(
            world,
            &[camp, keep, gate.unwrap_or(keep)],
            PLAYER_LIVE_RANGE,
        );
        let materialized = near && self.live_body_count() < body_budget();
        let warm_until = if materialized {
            tick.saturating_add(WARM_TICKS)
        } else {
            tick
        };
        let virt_raiders = spec.raiders;
        let virt_defenders = spec.extra_defenders;
        if let Some(war) = self.towns.get_mut(town_id) {
            war.raid = Some(ActiveRaid {
                id,
                spec,
                wave: 0,
                spawned_this_wave: 0,
                last_wave_tick: tick,
                camp,
                living: Vec::new(),
                temps: Vec::new(),
                warm_until,
                pending_members: Vec::new(),
                pending_retake: 0,
                spawn_cursor: 0,
                extra_left: virt_defenders,
                ram_spawned: false,
                materialized,
                virt_raiders,
                virt_defenders,
                virt_march: 0,
            });
            war.alarm = true;
            war.gates_closed = false;
            war.gate_breached = false;
            war.occupy_hold = 0;
            war.retake_hold = 0;
            war.ram = None;
            war.captain_down = false;
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
            warm_until,
            "kingdoms raid started"
        );
        Ok(id)
    }

    pub fn stop_raid(&mut self, town_id: Option<&str>) -> (u32, Vec<Vector2<i32>>, Vec<i32>) {
        let mut stopped = 0;
        let tick = self.tick;
        let mut notes = Vec::new();
        let mut tickets = Vec::new();
        let mut despawn = Vec::new();
        for (id, war) in &mut self.towns {
            if town_id.is_some_and(|want| want != id) {
                continue;
            }
            if let Some(raid) = war.raid.take() {
                stopped += 1;
                notes.push((raid.id.0, id.clone(), raid.living.len() + raid.temps.len()));
                despawn.extend(raid.living);
                despawn.extend(raid.temps);
                if let Some(ram) = war.ram.take() {
                    despawn.extend(ram.parts);
                }
                war.alarm = false;
                war.occupy_hold = 0;
                war.retake_hold = 0;
                tickets.append(&mut war.ticketed);
            }
        }
        for (raid_id, town, n) in notes {
            self.push_event(CombatEvent {
                tick,
                kind: CombatKind::RaidStop,
                actor: raid_id as i32,
                other: Some(n as i32),
                x: 0.0,
                z: 0.0,
                note: format!("stopped raid {raid_id} on {town}, despawned leftover soldiers"),
            });
        }
        (stopped, tickets, despawn)
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
                    if r.materialized {
                        r.living.len() as u32
                    } else {
                        u32::from(r.virt_raiders)
                    },
                )
            });
        let defenders_alive = war.raid.as_ref().map_or_else(
            || {
                war.wired
                    .values()
                    .filter(|w| w.faction.is_kingdom())
                    .count() as u32
            },
            |r| {
                if r.materialized {
                    war.wired
                        .values()
                        .filter(|w| w.faction.is_kingdom())
                        .count() as u32
                } else {
                    u32::from(r.virt_defenders)
                }
            },
        );
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
            occupy_hold: war.occupy_hold,
            occupy_need: war.raid.as_ref().map_or(8, |r| {
                capture_need(if r.materialized {
                    r.living.len() as u32
                } else {
                    u32::from(r.virt_raiders)
                })
            }),
            ram_hp: war.ram.as_ref().map_or(0.0, |r| r.hp),
        })
    }

    #[must_use]
    pub fn all_battles(&self) -> Vec<BattleState> {
        self.towns
            .keys()
            .filter_map(|id| self.battle_state(id))
            .collect()
    }

    #[must_use]
    pub fn war_front(&self) -> WarFront {
        let mut towns: Vec<WarTownLine> = self
            .towns
            .values()
            .map(|war| {
                let keep = war.scene.keep();
                let gate = war
                    .threatened
                    .map(|p| (p.x, p.z))
                    .or_else(|| war.scene.gates().first().map(|g| (g.x, g.z)));
                let live = war.raid.as_ref().is_some_and(|r| r.materialized);
                let raiders = war.raid.as_ref().map_or(0, |r| {
                    if r.materialized {
                        r.living.len() as u32
                    } else {
                        u32::from(r.virt_raiders)
                    }
                });
                let defenders = if live {
                    war.wired
                        .values()
                        .filter(|w| w.faction.is_kingdom())
                        .count() as u32
                } else {
                    war.raid.as_ref().map_or(0, |r| u32::from(r.virt_defenders))
                };
                WarTownLine {
                    id: war.info_id.clone(),
                    kingdom: war.kingdom.as_str().to_owned(),
                    owner: war.owner.as_str().to_owned(),
                    alarm: war.alarm,
                    raid_active: war.raid.is_some(),
                    gate_breached: war.gate_breached,
                    keep: (keep.x, keep.z),
                    gate,
                    occupy_hold: war.occupy_hold,
                    occupy_need: war.raid.as_ref().map_or(8, |r| {
                        capture_need(if r.materialized {
                            r.living.len() as u32
                        } else {
                            u32::from(r.virt_raiders)
                        })
                    }),
                    defenders,
                    raiders,
                    raid_side: war.raid.as_ref().map(|r| r.spec.side.as_str().to_owned()),
                    repair_until: war.repair_until,
                    live,
                }
            })
            .collect();
        towns.sort_by(|a, b| a.id.cmp(&b.id));
        WarFront {
            tick: self.tick,
            scores: self.scores.clone(),
            clock_enabled: war_clock_enabled(),
            clock_period: war_clock_period(),
            clock_overridden: crate::http::external_director_connected(),
            last_clock_tick: self.last_clock_tick,
            next_raid_id: self.next_raid_id,
            fronts: war_clock_fronts(),
            towns,
        }
    }

    pub fn note_death(&mut self, entity_id: i32) {
        let tick = self.tick;
        let mut event = None;
        for war in self.towns.values_mut() {
            if let Some(wired) = war.wired.remove(&entity_id) {
                if wired.captain {
                    war.captain_down = true;
                }
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
        if let Some(kind) = shout_kind(event.kind) {
            let town = shout_town(&event.note);
            let worlds: Vec<Arc<World>> = self.worlds.iter().filter_map(Weak::upgrade).collect();
            for world in worlds {
                chat::try_shout(
                    &world,
                    event.tick,
                    town,
                    kind,
                    event.x,
                    event.z,
                    &mut self.last_shout,
                );
            }
        }
        if self.events.len() >= COMBAT_LOG_CAP {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }
}

fn shout_kind(kind: CombatKind) -> Option<ShoutKind> {
    match kind {
        CombatKind::Breach => Some(ShoutKind::GateBreached),
        CombatKind::RaidStart | CombatKind::WarClock | CombatKind::Alarm => Some(ShoutKind::Rally),
        CombatKind::Rout => Some(ShoutKind::Retreat),
        CombatKind::Victory | CombatKind::Capture | CombatKind::Retake => Some(ShoutKind::Victory),
        _ => None,
    }
}

fn shout_town(note: &str) -> &str {
    note.split_whitespace()
        .find(|w| w.contains(':'))
        .or_else(|| note.split_whitespace().next())
        .unwrap_or("town")
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
            ram: None,
            capture_bar: None,
            captain_down: false,
            pending_tickets: Vec::new(),
            ticket_cursor: 0,
            hold_lockout: 0,
            repair_until: 0,
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
        VillagerRole::Guard => {
            let wall = war.scene.wall();
            let i = schedule::patrol_index(mob.get_entity().entity_id, dir.tick, wall.len());
            if wall.is_empty() {
                pick_home(mob, war.scene.guard_posts(), war.scene.keep())
            } else {
                ScenePost::new(wall[i][0], wall[i][1])
            }
        }
        VillagerRole::Archer => pick_home(mob, war.scene.archer_posts(), war.scene.keep()),
        VillagerRole::Civilian if war.alarm => war.scene.keep(),
        VillagerRole::Civilian => match schedule::civilian_post(false, world_time_for(mob)) {
            CivilianPost::Keep => war.scene.keep(),
            CivilianPost::Work => pick_home(mob, war.scene.workstations(), war.scene.keep()),
            CivilianPost::Home => pick_home(mob, war.scene.civilian_homes(), war.scene.keep()),
        },
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

/// True while a town is alarmed or has a live raid (skip bed respawn).
#[must_use]
pub fn town_is_fighting(town_id: &str) -> bool {
    lock()
        .towns
        .get(town_id)
        .is_some_and(|w| w.alarm || w.raid.is_some())
}

fn world_time_for(mob: &dyn Mob) -> i64 {
    mob.get_entity().world.load().get_time_of_day()
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
    kingdom_color_rgb(kingdom)
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

    let towns_started = Instant::now();
    for town_id in town_ids {
        tick_one_town(world, &town_id, tick);
    }
    let towns_us = towns_started.elapsed().as_micros();
    maybe_war_clock(world, tick);

    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let dir = lock();
    let raid_on = dir.towns.values().any(|w| w.raid.is_some());
    let persist_now = tick.is_multiple_of(200);
    drop(dir);
    if persist_now {
        lock().persist();
    }
    let (tps, mspt) = world
        .server
        .upgrade()
        .map_or((0.0, ms), |s| (s.get_tps(), s.get_mspt()));
    note_hitch(tick, mspt, ms);
    maybe_soak_sample(world, tick, mspt, ms, towns_us);
    maybe_profile(world, tick, mspt, ms, towns_us);
    if tick.is_multiple_of(100) {
        let dir = lock();
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
        let peak_mspt = f64::from(WORST_MSPT_CENTS.load(Ordering::Relaxed)) / 100.0;
        let peak_dir = f64::from(WORST_DIRECTOR_CENTS.load(Ordering::Relaxed)) / 100.0;
        drop(dir);
        if raid_on {
            tracing::info!(
                "kingdoms raid tick tick={tick} director_ms={ms:.2} tps={tps:.1} mspt={mspt:.2} peak_mspt={peak_mspt:.2} peak_director_ms={peak_dir:.2} defenders={def_n} raiders={raid_n} kills={def_k}/{raid_k} gate_dist={gate_d:.1}"
            );
        } else if ms > 1.0 {
            tracing::debug!(ms, tick, "kingdoms director tick");
        }
    }
}

fn maybe_soak_sample(world: &World, tick: u32, mspt: f64, director_ms: f64, towns_us: u128) {
    if !tick.is_multiple_of(20) {
        return;
    }
    let Ok(path) = std::env::var("KINGDOMS_SOAK_LOG") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let entities = world.entities.load().len();
    let dir = lock();
    let tickets: usize = dir.towns.values().map(|w| w.ticketed.len()).sum();
    let raids = dir.towns.values().filter(|w| w.raid.is_some()).count();
    let live_fronts = dir
        .towns
        .values()
        .filter(|w| w.raid.as_ref().is_some_and(|r| r.materialized))
        .count();
    let abstract_fronts = raids.saturating_sub(live_fronts);
    let bodies: usize = dir
        .towns
        .values()
        .map(|w| w.wired.len() + w.raid.as_ref().map_or(0, |r| r.living.len()))
        .sum();
    let live_bodies = dir.live_body_count();
    drop(dir);
    let rss_kb = process_rss_kb();
    let line = format!(
        "{tick},{mspt:.2},{director_ms:.2},{entities},{tickets},{rss_kb},{raids},{bodies},{live_fronts},{abstract_fronts},{live_bodies},{towns_us}\n"
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = std::io::Write::write_all(&mut file, line.as_bytes());
    }
}

fn process_rss_kb() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let resident = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    resident.saturating_mul(4)
}

fn note_hitch(tick: u32, mspt: f64, director_ms: f64) {
    let mspt_cents = (mspt * 100.0).max(0.0) as u32;
    let dir_cents = (director_ms * 100.0).max(0.0) as u32;
    WORST_MSPT_CENTS.fetch_max(mspt_cents, Ordering::Relaxed);
    WORST_DIRECTOR_CENTS.fetch_max(dir_cents, Ordering::Relaxed);
    // Smoothed MSPT stays high after a join stall; only log real director spikes.
    if director_ms >= 50.0 {
        tracing::warn!("kingdoms hitch tick={tick} mspt={mspt:.2} director_ms={director_ms:.2}");
    }
}

fn maybe_profile(world: &World, tick: u32, mspt: f64, director_ms: f64, towns_us: u128) {
    if !tick.is_multiple_of(100) {
        return;
    }
    let profile =
        std::env::var("KINGDOMS_PROFILE").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    if !profile {
        return;
    }
    let entities = world.entities.load().len();
    let dir = lock();
    let tickets: usize = dir.towns.values().map(|w| w.ticketed.len()).sum();
    let live_fronts = dir
        .towns
        .values()
        .filter(|w| w.raid.as_ref().is_some_and(|r| r.materialized))
        .count();
    let abstract_fronts = dir
        .towns
        .values()
        .filter(|w| w.raid.as_ref().is_some_and(|r| !r.materialized))
        .count();
    let live_bodies = dir.live_body_count();
    drop(dir);
    tracing::info!(
        "kingdoms profile tick={tick} director_ms={director_ms:.2} towns_us={towns_us} mspt={mspt:.2} live_fronts={live_fronts} abstract_fronts={abstract_fronts} live_bodies={live_bodies} tickets={tickets} entities={entities} rss_kb={}",
        process_rss_kb()
    );
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
    sync_front_lod(world, town_id, tick);
    let materialized = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .is_some_and(|w| w.raid.as_ref().is_some_and(|r| r.materialized))
    };
    let near = town_has_nearby_player(world, town_id);
    if !materialized {
        tick_abstract_raid(world, town_id, tick);
        if !near {
            return;
        }
    }
    tick_raid_warmup(world, town_id, tick);
    let (bounds, kingdom, alarm_before, camps, gates) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (
            inhabited_bounds(&war.scene),
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

    let mut min_x = bounds.min_x - 48;
    let mut min_z = bounds.min_z - 48;
    let mut max_x = bounds.max_x + 48;
    let mut max_z = bounds.max_z + 48;
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
    let mut hostiles_in_town = false;
    {
        let dir = lock();
        if let Some(war) = dir.towns.get(town_id) {
            let owner_faction = owner_faction(&war.owner, &war.kingdom);
            let bounds = inhabited_bounds(&war.scene);
            let camps: Vec<(i32, i32)> = war
                .scene
                .raid_camps()
                .iter()
                .map(|c| (c.x, c.z))
                .chain(war.raid.as_ref().map(|r| (r.camp.x, r.camp.z)))
                .collect();
            for (entity, faction) in &snapshots {
                if !is_hostile(faction, &owner_faction) {
                    continue;
                }
                let p = entity.get_entity().pos.load();
                let x = p.x.floor() as i32;
                let z = p.z.floor() as i32;
                if bounds.contains(x, z) && !near_camp(&camps, x, z) {
                    hostiles_in_town = true;
                    break;
                }
            }
            let kingdom_faction = Faction::Kingdom(kingdom);
            for gate in war.scene.gates() {
                if buckets.any_hostile_near(
                    f64::from(gate.x),
                    f64::from(gate.z),
                    &kingdom_faction,
                    ALARM_RANGE,
                ) {
                    threatened = Some(ScenePost::new(gate.x, gate.z));
                    break;
                }
            }
            if threatened.is_none() {
                let keep = war.scene.keep();
                if buckets.any_hostile_near(
                    f64::from(keep.x),
                    f64::from(keep.z),
                    &kingdom_faction,
                    ALARM_RANGE * 1.5,
                ) {
                    threatened = Some(keep);
                }
            }
        }
    }

    let mut spawn_soldiers = false;
    let mut pending_events = Vec::new();
    let mut pending_release: Vec<Vector2<i32>> = Vec::new();
    let mut pending_despawn: Vec<i32> = Vec::new();
    let mut dump_plots = false;
    let mut should_close_gates = false;
    {
        let mut dir = lock();
        {
            let Some(war) = dir.towns.get_mut(town_id) else {
                return;
            };
            if hostiles_in_town {
                war.alarm = true;
                if let Some(t) = threatened {
                    war.threatened = Some(t);
                }
            } else {
                let warming = war.raid.as_ref().is_some_and(|r| {
                    tick < r.warm_until
                        || !r.pending_members.is_empty()
                        || r.pending_retake > 0
                        || r.extra_left > 0
                });
                war.alarm = warming;
            }
            if !hostiles_in_town {
                let bounds = inhabited_bounds(&war.scene);
                let camps: Vec<(i32, i32)> = war
                    .scene
                    .raid_camps()
                    .iter()
                    .map(|c| (c.x, c.z))
                    .chain(war.raid.as_ref().map(|r| (r.camp.x, r.camp.z)))
                    .collect();
                let raid_over = war.raid.is_none();
                for (entity, _) in &snapshots {
                    let base = entity.get_entity();
                    let p = base.pos.load();
                    let x = p.x.floor() as i32;
                    let z = p.z.floor() as i32;
                    let outside = !bounds.contains(x, z) || near_camp(&camps, x, z);
                    if (has_tag(base, TAG_ROUTED) && outside)
                        || (raid_over && has_tag(base, TAG_RAIDER))
                    {
                        pending_despawn.push(base.entity_id);
                    }
                }
                if let Some(raid) = &mut war.raid {
                    raid.living.retain(|id| !pending_despawn.contains(id));
                    raid.temps.retain(|id| !pending_despawn.contains(id));
                }
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
                if raid.spec.side.is_kingdom_army() {
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
                let warmed = tick >= raid.warm_until;
                let spacing_ok = tick.saturating_sub(raid.last_wave_tick) >= WAVE_SPACING_TICKS;
                let pending = !raid.pending_members.is_empty()
                    || raid.pending_retake > 0
                    || raid.extra_left > 0;
                if warmed && raid.pending_members.is_empty() && raid.pending_retake == 0 {
                    let first = raid.wave == 0 && raid.spawned_this_wave == 0;
                    let next = raid.spawned_this_wave > 0
                        && raid.wave < raid.spec.waves
                        && ((raid.living.len() as u16) <= WAVE_ADVANCE_REMAINING || spacing_ok);
                    if first || next {
                        let count = raid.spec.raiders_in_wave(raid.wave);
                        if raid.spec.side.is_kingdom_army() {
                            raid.pending_retake = count;
                        } else {
                            raid.pending_members = compose_wave(count, raid.wave);
                        }
                    }
                }
                spawn_soldiers = warmed
                    && (pending
                        || !raid.pending_members.is_empty()
                        || raid.pending_retake > 0
                        || raid.extra_left > 0);
                if raid.wave >= raid.spec.waves
                    && raid.living.is_empty()
                    && raid.spawned_this_wave > 0
                    && raid.pending_members.is_empty()
                    && raid.pending_retake == 0
                    && raid.extra_left == 0
                {
                    finished = true;
                }
            }
            war.wired.clear();
            for (entity, faction) in &snapshots {
                let base = entity.get_entity();
                war.wired.insert(
                    base.entity_id,
                    Wired {
                        faction: faction.clone(),
                        captain: has_tag(base, TAG_CAPTAIN),
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
                if let Some(raid) = war.raid.take() {
                    pending_despawn.extend(raid.living);
                    pending_despawn.extend(raid.temps);
                }
                if let Some(ram) = war.ram.take() {
                    pending_despawn.extend(ram.parts);
                }
                war.alarm = hostiles_in_town;
                pending_release.append(&mut war.ticketed);
                pending_events.push(CombatEvent {
                    tick,
                    kind: CombatKind::Victory,
                    actor: 0,
                    other: None,
                    x: 0.0,
                    z: 0.0,
                    note: format!("{town_id} raid defeated, leftovers despawned"),
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
    despawn_entities(world, &pending_despawn);
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
    let world_time = world.get_time_of_day();
    let (
        alarm,
        march_dest,
        keep_dest,
        ram_dest,
        ladder_dest,
        keep_rect,
        breached,
        owner_pillager,
        owner_faction,
        camp_dest,
        live_ram,
        captain_down,
        work_posts,
        home_posts,
        wall_posts,
    ) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let dest = war.threatened.unwrap_or_else(|| war.scene.keep());
        let y = standing_y(world, dest);
        let keep = war.scene.keep();
        let camp = war.raid.as_ref().map_or(keep, |r| r.camp);
        let ram_post = war.ram.as_ref().map_or_else(
            || war.scene.ram_posts().first().copied().unwrap_or(dest),
            crate::ram::SiegeRam::post,
        );
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
        let wall_posts: Vec<ScenePost> = war
            .scene
            .wall()
            .iter()
            .map(|p| ScenePost::new(p[0], p[1]))
            .collect();
        (
            war.alarm,
            PostTarget::from_post(dest, y),
            PostTarget::from_post(keep, standing_y(world, keep)),
            PostTarget::from_post(ram_post, standing_y(world, ram_post)),
            PostTarget {
                x: ladder.x,
                y: standing_y(world, ladder) + WALL_WALK_LIFT,
                z: ladder.z,
            },
            war.scene.keep_rect(),
            war.gate_breached,
            war.owner.is_pillager(),
            owner_faction(&war.owner, &war.kingdom),
            PostTarget::from_post(camp, standing_y(world, camp)),
            war.ram.clone(),
            war.captain_down,
            war.scene.workstations().to_vec(),
            war.scene.civilian_homes().to_vec(),
            wall_posts,
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
        let routed = has_tag(base, TAG_ROUTED);
        if in_courtyard(keep_rect, pos.x, pos.z) && !routed {
            if is_hostile(faction, &owner_faction) {
                raiders_in_keep += 1;
            } else if is_allied(faction, &owner_faction) {
                defenders_in_keep += 1;
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
            let dest = match (faction, role) {
                (Faction::Pillager, _) => march_dest,
                (Faction::Kingdom(_), _) if is_builder => {
                    assigned_repair_dest(town_id, id, world).unwrap_or(keep_dest)
                }
                (Faction::Kingdom(_), Some(VillagerRole::Civilian)) => schedule_civilian_dest(
                    id,
                    world_time,
                    &work_posts,
                    &home_posts,
                    keep_dest,
                    world,
                ),
                (Faction::Kingdom(_), Some(VillagerRole::Guard)) => {
                    patrol_dest(id, tick, &wall_posts, keep_dest, world)
                }
                (Faction::Kingdom(_), _) => keep_dest,
                (Faction::Neutral, _) => continue,
            };
            let _ = drive_live(mob, dest, in_combat);
        }

        let job = job_of(base);
        let repair_dest = if is_builder {
            assigned_repair_dest(town_id, id, world)
        } else {
            None
        };
        let dest = if routed {
            match faction {
                Faction::Pillager => camp_dest,
                _ => keep_dest,
            }
        } else if job == MarchJob::Ram
            && live_ram.as_ref().is_some_and(|r| !r.destroyed)
            && !breached
        {
            ram_dest
        } else {
            let needs_schedule = !alarm
                && matches!(faction, Faction::Kingdom(_))
                && matches!(role, Some(VillagerRole::Civilian | VillagerRole::Guard));
            let (work_d, home_d, patrol_d) = if needs_schedule {
                (
                    schedule_civilian_dest(
                        id,
                        world_time,
                        &work_posts,
                        &home_posts,
                        keep_dest,
                        world,
                    ),
                    pick_post_dest(id, &home_posts, keep_dest, world),
                    patrol_dest(id, tick, &wall_posts, keep_dest, world),
                )
            } else {
                (keep_dest, keep_dest, keep_dest)
            };
            dest_for(
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
                work_d,
                home_d,
                patrol_d,
                world_time,
            )
        };
        let dest_v = dest.vec();
        let dist = (pos.x - dest_v.x).hypot(pos.z - dest_v.z);
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
    tick_moving_ram(world, town_id, tick, &snapshots, breached, &mut ram_at_gate);
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
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.gate_breached = true;
            }
            if cleared > 0 {
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
    apply_morale(town_id, tick, &snapshots, captain_down, breached);
    apply_capture(
        town_id,
        tick,
        raiders_in_keep,
        defenders_in_keep,
        occupier_from_keep(&snapshots, keep_rect, &owner_faction),
        1,
    );
    update_capture_meter(
        world,
        town_id,
        keep_dest,
        raiders_in_keep,
        defenders_in_keep,
    );

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
    if tick.is_multiple_of(400) && alarm && !soaking() {
        let town = town_id.to_owned();
        std::thread::spawn(move || dump_live_plots(Some(&town)));
    }
    if tick.is_multiple_of(100) && (alarm || n_repair > 0 || n_arrows > 0) {
        let n_entities = snapshots.len();
        tracing::info!(
            "kingdoms raid entity tick town={town_id} ticking={n_ticking} loaded={n_loaded} active={n_active} no_ai={n_no_ai} slid={n_slid} melee={n_melee} ranged={n_ranged} arrows={n_arrows} repair={n_repair} loop_us={loop_us} keep_r={raiders_in_keep} keep_d={defenders_in_keep} entities={n_entities}"
        );
    }

    if spawn_soldiers {
        spawn_soldiers_this_tick(world, town_id, tick);
    }

    if tick.is_multiple_of(RESPAWN_PERIOD) {
        crate::populate::respawn_missing(world, town_id);
    }
}

fn spawn_soldiers_this_tick(world: &Arc<World>, town_id: &str, tick: u32) {
    if lock().live_body_count() >= body_budget() {
        return;
    }
    spawn_extra_defenders(world, town_id, SPAWNS_PER_TICK as u16);
    spawn_wave_batch(world, town_id, tick);
}

fn spawn_wave_batch(world: &Arc<World>, town_id: &str, tick: u32) {
    let (side, camp, raid_id, wave, pending_n, pending_retake) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let Some(raid) = &war.raid else {
            return;
        };
        (
            raid.spec.side,
            raid.camp,
            raid.id,
            raid.wave,
            raid.pending_members.len(),
            raid.pending_retake,
        )
    };
    if pending_n == 0 && pending_retake == 0 {
        return;
    }
    if side.is_kingdom_army() {
        spawn_retake_batch(world, town_id, wave, camp, raid_id, tick);
        return;
    }
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
    let spawn_ram = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .and_then(|w| w.raid.as_ref())
            .is_some_and(|r| wave == 0 && !r.ram_spawned)
            && !rams.is_empty()
    };
    if spawn_ram {
        let origin = rams[0];
        let y = f64::from(standing_y(world, origin));
        let ram = crate::ram::spawn_ram(
            world,
            Vector3::new(f64::from(origin.x) + 0.5, y, f64::from(origin.z) + 0.5),
        );
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ram = Some(ram);
            if let Some(raid) = &mut war.raid {
                raid.ram_spawned = true;
            }
        }
    }
    let batch: Vec<(usize, WaveMember)> = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = &mut war.raid else {
            return;
        };
        let n = SPAWNS_PER_TICK.min(raid.pending_members.len());
        let start = raid.spawn_cursor as usize;
        raid.pending_members
            .drain(..n)
            .enumerate()
            .map(|(i, m)| (start + i, m))
            .collect()
    };
    if batch.is_empty() {
        return;
    }
    let mut spawned_ids = Vec::new();
    for (i, member) in &batch {
        let vindicator = member.entity_type == &EntityType::VINDICATOR;
        let job = MarchJob::assign(*i, vindicator);
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
    let mut wave_done = None;
    if let Some(war) = dir.towns.get_mut(town_id)
        && let Some(raid) = &mut war.raid
        && raid.id == raid_id
    {
        raid.spawn_cursor = raid.spawn_cursor.saturating_add(spawned_ids.len() as u16);
        raid.living.extend(spawned_ids.iter().copied());
        if raid.pending_members.is_empty() {
            let count = raid.spawn_cursor;
            raid.spawned_this_wave = count;
            raid.last_wave_tick = tick;
            raid.wave = raid.wave.saturating_add(1);
            raid.spawn_cursor = 0;
            wave_done = Some((count, raid.camp));
        }
    }
    if let Some((count, camp)) = wave_done {
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
    } else {
        tracing::info!(
            town = town_id,
            spawned = spawned_ids.len(),
            "raid wave spawn batch"
        );
    }
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
    let id = lock().start_raid(world, town_id, spec)?;
    let live = lock()
        .towns
        .get(town_id)
        .and_then(|w| w.raid.as_ref())
        .is_some_and(|r| r.materialized);
    if live {
        queue_raid_warmup(world, town_id);
    }
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

fn queue_raid_warmup(world: &Arc<World>, town_id: &str) {
    let Some((chunks, camp, keep)) = raid_chunk_list(town_id) else {
        return;
    };
    let n = chunks.len();
    {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.pending_tickets = chunks;
            war.ticket_cursor = 0;
            war.ticketed.clear();
            war.live_snaps.clear();
        }
    }
    tracing::info!(
        town = town_id,
        count = n,
        camp_x = camp.x,
        camp_z = camp.z,
        keep_x = keep.x,
        keep_z = keep.z,
        "kingdoms raid warmup queued"
    );
    // Read tick first: `tick_raid_warmup(..., lock().tick)` would keep the
    // director mutex live across the call and deadlock on the inner lock.
    let tick = lock().tick;
    tick_raid_warmup(world, town_id, tick);
}

fn tick_raid_warmup(world: &Arc<World>, town_id: &str, tick: u32) {
    let (batch, remaining, camp, keep) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if war.raid.is_none() || war.ticket_cursor >= war.pending_tickets.len() {
            return;
        }
        let start = war.ticket_cursor;
        let end = (start + TICKETS_PER_TICK).min(war.pending_tickets.len());
        let batch = war.pending_tickets[start..end].to_vec();
        war.ticket_cursor = end;
        war.ticketed.extend(batch.iter().copied());
        let remaining = war.pending_tickets.len().saturating_sub(end);
        let camp = war
            .raid
            .as_ref()
            .map_or_else(|| war.scene.keep(), |r| r.camp);
        let keep = war.scene.keep();
        (batch, remaining, camp, keep)
    };
    if batch.is_empty() {
        return;
    }
    {
        let mut forced = world
            .forced_chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in &batch {
            forced.insert(*pos);
        }
    }
    apply_force_tickets(world, &batch, true);
    world.level.mark_chunks_as_newly_watched(&batch);
    world.update_active_chunks();
    let loaded_world = Arc::clone(world);
    let level = Arc::clone(&world.level);
    let n = batch.len();
    world.level.spawn_task(async move {
        for pos in &batch {
            level.get_or_fetch_chunk(*pos, |_| ()).await;
        }
        loaded_world.update_active_chunks();
    });
    tracing::info!(
        town = town_id,
        tick,
        applied = n,
        remaining,
        camp_x = camp.x,
        camp_z = camp.z,
        keep_x = keep.x,
        keep_z = keep.z,
        "kingdoms raid tickets applied"
    );
    if remaining == 0 {
        tracing::info!(town = town_id, tick, "kingdoms raid chunks warming done");
    }
    spawn_garrison_if_needed(world, town_id, SPAWNS_PER_TICK as u16);
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
    let camp = war
        .raid
        .as_ref()
        .map(|r| r.camp)
        .or_else(|| war.scene.raid_camps().first().copied())
        .or_else(|| war.scene.road_approach())
        .unwrap_or(keep);
    let mut samples = vec![keep, camp];
    // Battle line only: threatened gate plus a thin camp→gate→keep corridor.
    let gate = war
        .threatened
        .or_else(|| war.scene.gates().first().map(|g| ScenePost::new(g.x, g.z)))
        .unwrap_or(keep);
    samples.push(gate);
    let steps = 8;
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
        for dx in -1..=1 {
            for dz in -1..=1 {
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

fn spawn_retake_batch(
    world: &Arc<World>,
    town_id: &str,
    wave: u8,
    camp: ScenePost,
    raid_id: RaidId,
    tick: u32,
) {
    let (kingdom, n, cursor) = {
        let dir = lock();
        dir.towns.get(town_id).map_or_else(
            || (KingdomId::new("alder"), 0u16, 0u16),
            |w| {
                let n = w.raid.as_ref().map_or(0, |r| r.pending_retake);
                let cursor = w.raid.as_ref().map_or(0, |r| r.spawn_cursor);
                let kingdom = w.raid.as_ref().map_or_else(
                    || w.kingdom.clone(),
                    |r| army_kingdom(r.spec.side, &w.kingdom),
                );
                (kingdom, n, cursor)
            },
        )
    };
    if n == 0 {
        return;
    }
    let post = camp;
    let y = standing_y(world, post);
    let batch = n.min(SPAWNS_PER_TICK as u16);
    let mut spawned_ids = Vec::new();
    for i in 0..batch {
        let index = cursor + i;
        let pos = spawn_offset(post, index, y);
        let golem = index % 6 == 5;
        let role = if index % 3 == 2 {
            VillagerRole::Archer
        } else {
            VillagerRole::Guard
        };
        if let Some(id) = spawn_citizen(world, town_id, &kingdom, role, golem, pos) {
            spawned_ids.push(id);
        }
    }
    let mut dir = lock();
    let mut wave_done = None;
    if let Some(war) = dir.towns.get_mut(town_id)
        && let Some(raid) = &mut war.raid
        && raid.id == raid_id
    {
        raid.pending_retake = raid.pending_retake.saturating_sub(batch);
        raid.spawn_cursor = raid.spawn_cursor.saturating_add(spawned_ids.len() as u16);
        raid.living.extend(spawned_ids.iter().copied());
        if raid.pending_retake == 0 {
            let count = raid.spawn_cursor;
            raid.spawned_this_wave = count;
            raid.last_wave_tick = tick;
            raid.wave = raid.wave.saturating_add(1);
            raid.spawn_cursor = 0;
            wave_done = Some(count);
        }
    }
    if let Some(count) = wave_done {
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

fn despawn_entities(world: &World, ids: &[i32]) {
    if ids.is_empty() {
        return;
    }
    let mut n = 0u32;
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(*id) {
            continue;
        }
        if let Some(e) = world.get_entity_by_id(*id) {
            e.get_entity().remove();
            n += 1;
        }
    }
    if n > 0 {
        tracing::info!(despawned = n, "kingdoms leftover soldiers despawned");
    }
}

fn tick_moving_ram(
    world: &World,
    town_id: &str,
    tick: u32,
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    breached: bool,
    ram_at_gate: &mut bool,
) {
    let mut ram = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        war.ram.take()
    };
    let Some(ram_mut) = ram.as_mut() else {
        return;
    };
    if ram_mut.destroyed {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ram = ram;
        }
        return;
    }
    let dest = {
        let dir = lock();
        dir.towns.get(town_id).map_or_else(
            || ScenePost::new(ram_mut.x as i32, ram_mut.z as i32),
            |w| {
                w.threatened.unwrap_or_else(|| {
                    w.scene
                        .gates()
                        .first()
                        .map_or_else(|| w.scene.keep(), |g| ScenePost::new(g.x, g.z))
                })
            },
        )
    };
    let mut crew = Vec::new();
    let mut defenders = Vec::new();
    for (entity, faction) in snapshots {
        let p = entity.get_entity().pos.load();
        match faction {
            Faction::Pillager if job_of(entity.get_entity()) == MarchJob::Ram => {
                crew.push((p.x, p.z));
            }
            Faction::Kingdom(_) => defenders.push((p.x, p.z)),
            _ => {}
        }
    }
    let crew_near = crate::ram::count_near(&crew, ram_mut, crate::ram::crew_range());
    let def_near = crate::ram::count_near(&defenders, ram_mut, crate::ram::defender_range());
    let result = crate::ram::tick_ram(world, ram_mut, dest, crew_near, def_near);
    match result {
        crate::ram::RamTick::AtGate if !breached => *ram_at_gate = true,
        crate::ram::RamTick::Destroyed => {
            let mut dir = lock();
            dir.push_event(CombatEvent {
                tick,
                kind: CombatKind::RamDestroyed,
                actor: crew_near as i32,
                other: Some(def_near as i32),
                x: ram_mut.x,
                z: ram_mut.z,
                note: format!("{town_id} ram destroyed by defenders"),
            });
            tracing::info!("kingdoms ram destroyed town={town_id} defenders_near={def_near} hp=0");
        }
        crate::ram::RamTick::Moving if tick.is_multiple_of(40) => {
            tracing::info!(
                "kingdoms ram moving town={town_id} x={:.1} z={:.1} hp={:.1} crew={crew_near}",
                ram_mut.x,
                ram_mut.z,
                ram_mut.hp
            );
        }
        _ => {}
    }
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        war.ram = ram;
    }
}

fn apply_morale(
    town_id: &str,
    tick: u32,
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    captain_down: bool,
    breached: bool,
) {
    let positions: Vec<(f64, f64, bool)> = snapshots
        .iter()
        .filter_map(|(e, f)| {
            let p = e.get_entity().pos.load();
            match f {
                Faction::Pillager => Some((p.x, p.z, true)),
                Faction::Kingdom(_) => Some((p.x, p.z, false)),
                Faction::Neutral => None,
            }
        })
        .collect();
    let mut routed_n = 0u32;
    let mut rallied_n = 0u32;
    for (entity, faction) in snapshots {
        let base = entity.get_entity();
        if matches!(faction, Faction::Neutral) {
            continue;
        }
        let is_pillager = matches!(faction, Faction::Pillager);
        let pos = base.pos.load();
        let (friends, foes) = crate::morale::local_counts(
            &positions,
            pos.x,
            pos.z,
            crate::morale::MORALE_RANGE,
            is_pillager,
        );
        let captain = has_tag(base, TAG_CAPTAIN);
        // Ram crew stay on the engine; they do not break and run.
        if job_of(base) == MarchJob::Ram {
            continue;
        }
        // Once the gate is down the assault commits to the keep. A captain
        // death must not empty the courtyard before the hold finishes.
        if breached && is_pillager {
            if has_tag(base, TAG_ROUTED) && base.remove_scoreboard_tag(TAG_ROUTED) {
                rallied_n += 1;
            }
            continue;
        }
        if captain_down && !captain && !has_tag(base, TAG_ROUTED) {
            let _ = base.add_scoreboard_tag(TAG_ROUTED);
            routed_n += 1;
            continue;
        }
        if crate::morale::should_rout(friends, foes) && !captain {
            if base.add_scoreboard_tag(TAG_ROUTED) {
                routed_n += 1;
            }
            continue;
        }
        if captain && !has_tag(base, TAG_ROUTED) {
            for (other, other_f) in snapshots {
                if other_f != faction {
                    continue;
                }
                let o = other.get_entity();
                if !has_tag(o, TAG_ROUTED) {
                    continue;
                }
                let op = o.pos.load();
                if (op.x - pos.x).hypot(op.z - pos.z) <= crate::morale::RALLY_RANGE
                    && o.remove_scoreboard_tag(TAG_ROUTED)
                {
                    rallied_n += 1;
                }
            }
        }
    }
    if captain_down {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.captain_down = false;
        }
    }
    if routed_n > 0 {
        let mut dir = lock();
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Rout,
            actor: routed_n as i32,
            other: Some(rallied_n as i32),
            x: 0.0,
            z: 0.0,
            note: format!("{town_id} {routed_n} routed (rallied {rallied_n})"),
        });
        tracing::info!(
            "kingdoms rout town={town_id} routed={routed_n} rallied={rallied_n} captain_down={captain_down}"
        );
    }
}

fn update_capture_meter(
    world: &World,
    town_id: &str,
    keep: PostTarget,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
) {
    let (hold, need, bar, capturing) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let surviving = war.raid.as_ref().map_or(0, |r| r.living.len() as u32);
        let need = capture_need(surviving.max(raiders_in_keep));
        let hold = war.occupy_hold.max(war.retake_hold);
        (
            hold,
            need,
            war.capture_bar,
            war.occupy_hold > 0 || war.retake_hold > 0,
        )
    };
    let players: Vec<Arc<Player>> = world
        .players
        .load()
        .iter()
        .filter(|p| {
            let pos = p.living_entity.entity.pos.load();
            (pos.x - f64::from(keep.x)).hypot(pos.z - f64::from(keep.z)) <= CAPTURE_METER_RANGE
        })
        .cloned()
        .collect();
    if !capturing || hold == 0 {
        if let Some(uuid) = bar {
            for p in &players {
                p.remove_bossbar(uuid);
            }
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.capture_bar = None;
            }
        }
        return;
    }
    let progress = (hold as f32 / HOLD_TICKS as f32).clamp(0.0, 1.0);
    let title = TextComponent::text(format!(
        "Capture {raiders_in_keep}/{need}  hold {hold}/{HOLD_TICKS}  def {defenders_in_keep}"
    ));
    let uuid = bar.unwrap_or_else(Uuid::new_v4);
    if bar.is_none() {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.capture_bar = Some(uuid);
        }
        let mut boss = Bossbar::new(title.clone());
        boss.uuid = uuid;
        boss.health = progress;
        boss.color = BossbarColor::Red;
        boss.division = BossbarDivisions::Notches10;
        for p in &players {
            p.send_bossbar(&boss);
        }
    } else {
        for p in &players {
            p.update_bossbar_health(&uuid, progress);
            p.update_bossbar_title(&uuid, title.clone());
        }
    }
    for p in &players {
        p.show_title(&title, &TitleMode::ActionBar);
    }
}

fn war_clock_enabled() -> bool {
    std::env::var("KINGDOMS_WAR_CLOCK").map_or(true, |v| {
        !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

fn war_clock_period() -> u32 {
    std::env::var("KINGDOMS_WAR_CLOCK_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n: &u32| *n > 0)
        .unwrap_or(DEFAULT_WAR_CLOCK_TICKS)
}

fn war_clock_fronts() -> usize {
    std::env::var("KINGDOMS_WAR_CLOCK_FRONTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_WAR_CLOCK_FRONTS)
        .min(8)
}

fn war_clock_size() -> u16 {
    std::env::var("KINGDOMS_WAR_CLOCK_SIZE")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_WAR_CLOCK_SIZE)
        .min(400)
}

fn war_clock_defenders() -> u16 {
    std::env::var("KINGDOMS_WAR_CLOCK_DEFENDERS")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(0)
        .min(400)
}

fn maybe_war_clock(world: &Arc<World>, tick: u32) {
    if !war_clock_enabled() {
        return;
    }
    if crate::http::external_director_connected() {
        return;
    }
    ensure_border_towns(world);
    let period = war_clock_period();
    let fronts = war_clock_fronts();
    {
        let dir = lock();
        if tick.saturating_sub(dir.last_clock_tick) < period {
            return;
        }
        let active = dir.towns.values().filter(|w| w.raid.is_some()).count();
        if active >= fronts {
            return;
        }
    }
    heal_repaired_towns(tick);
    let mut started = 0u32;
    for _ in 0..fronts {
        let Some((town_id, side)) = pick_clock_front(tick, fronts) else {
            break;
        };
        let spec = RaidSpec::new(war_clock_size())
            .with_extra_defenders(war_clock_defenders())
            .with_side(side);
        match start_raid(world, &town_id, spec) {
            Ok(id) => {
                started += 1;
                let mut dir = lock();
                dir.push_event(CombatEvent {
                    tick,
                    kind: CombatKind::WarClock,
                    actor: id.0 as i32,
                    other: None,
                    x: 0.0,
                    z: 0.0,
                    note: format!(
                        "war clock raid {} side {} on {town_id}",
                        id.0,
                        side.as_str()
                    ),
                });
                drop(dir);
                tracing::info!(
                    raid = id.0,
                    town = town_id.as_str(),
                    side = side.as_str(),
                    "kingdoms war clock started raid"
                );
            }
            Err(err) => {
                tracing::debug!(town = town_id.as_str(), %err, "war clock skipped");
                break;
            }
        }
    }
    if started > 0 {
        let mut dir = lock();
        dir.last_clock_tick = tick;
        dir.persist();
    }
}

fn heal_repaired_towns(tick: u32) {
    let ids: Vec<String> = {
        let dir = lock();
        dir.towns
            .values()
            .filter(|war| war.raid.is_none() && tick >= war.repair_until)
            .filter(|war| crate::damage::damage_state(&war.info_id).is_damaged())
            .map(|war| war.info_id.clone())
            .collect()
    };
    for id in ids {
        crate::damage::set_damage_state(&id, crate::damage::DamageState::Intact);
    }
}

fn pick_clock_front(tick: u32, fronts: usize) -> Option<(String, RaidSide)> {
    let dir = lock();
    let active = dir.towns.values().filter(|w| w.raid.is_some()).count();
    if active >= fronts {
        return None;
    }
    let mut candidates: Vec<(String, TownOwner)> = dir
        .towns
        .values()
        .filter(|w| w.raid.is_none() && tick >= w.repair_until)
        .map(|w| (w.info_id.clone(), w.owner.clone()))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    // `next_raid_id` alone: adding `tick` with a period of 200 made every
    // front land on the same side (`200 ≡ 2 (mod 3)`, `id += 1`).
    let rotate = dir.next_raid_id as usize;
    // Alternate pillager / alder-on-raven / raven-on-alder.
    let mode = rotate % 3;
    let start = rotate % candidates.len();
    let (id, owner) = &candidates[start];
    Some((id.clone(), clock_side(owner, mode)))
}

fn clock_side(owner: &TownOwner, mode: usize) -> RaidSide {
    if owner.is_pillager() {
        return RaidSide::Kingdom;
    }
    match mode % 3 {
        0 => RaidSide::Pillager,
        1 if owner.as_str() != "alder" => RaidSide::Alder,
        1 => RaidSide::Raven,
        _ if owner.as_str() != "raven" => RaidSide::Raven,
        _ => RaidSide::Alder,
    }
}

fn ensure_border_towns(world: &Arc<World>) {
    let fronts = war_clock_fronts();
    if fronts < 2 {
        return;
    }
    let seed = world.level.seed.0;
    let ids: Vec<String> = lock().towns.keys().cloned().collect();
    if ids.len() >= fronts.max(3) {
        return;
    }
    for id in ids {
        let Some((_, cx, cz)) = parse_town_id(&id) else {
            continue;
        };
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nid = crate::info::town_id(seed, cx + dx, cz + dz);
            let _ = lock().ensure_town(world, &nid);
        }
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
    work_dest: PostTarget,
    home_dest: PostTarget,
    patrol_dest: PostTarget,
    world_time: i64,
) -> PostTarget {
    if role == Some(VillagerRole::Builder) {
        return repair_dest.unwrap_or(keep_dest);
    }
    if role == Some(VillagerRole::Civilian) {
        return match schedule::civilian_post(alarm, world_time) {
            CivilianPost::Keep => keep_dest,
            CivilianPost::Work => work_dest,
            CivilianPost::Home => home_dest,
        };
    }
    match faction {
        Faction::Neutral => keep_dest,
        Faction::Kingdom(_) if role == Some(VillagerRole::Archer) => ladder_dest,
        Faction::Kingdom(_) if role == Some(VillagerRole::Guard) && !alarm && !in_combat => {
            patrol_dest
        }
        Faction::Kingdom(_) if alarm || in_combat => {
            if breached {
                keep_dest
            } else {
                march_dest
            }
        }
        Faction::Kingdom(_) => patrol_dest,
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

fn pick_post_dest(
    entity_id: i32,
    posts: &[ScenePost],
    fallback: PostTarget,
    world: &World,
) -> PostTarget {
    if posts.is_empty() {
        return fallback;
    }
    let post = posts[entity_id.unsigned_abs() as usize % posts.len()];
    PostTarget::from_post(post, standing_y(world, post))
}

fn schedule_civilian_dest(
    entity_id: i32,
    world_time: i64,
    work_posts: &[ScenePost],
    home_posts: &[ScenePost],
    keep_dest: PostTarget,
    world: &World,
) -> PostTarget {
    match schedule::civilian_post(false, world_time) {
        CivilianPost::Keep => keep_dest,
        CivilianPost::Work => pick_post_dest(entity_id, work_posts, keep_dest, world),
        CivilianPost::Home => pick_post_dest(entity_id, home_posts, keep_dest, world),
    }
}

fn patrol_dest(
    entity_id: i32,
    tick: u32,
    wall: &[ScenePost],
    fallback: PostTarget,
    world: &World,
) -> PostTarget {
    if wall.is_empty() {
        return fallback;
    }
    let i = schedule::patrol_index(entity_id, tick, wall.len());
    let post = wall[i];
    PostTarget {
        x: post.x,
        y: standing_y(world, post) + WALL_WALK_LIFT,
        z: post.z,
    }
}

fn apply_capture(
    town_id: &str,
    tick: u32,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
    occupier: TownOwner,
    delta: u32,
) {
    let (captured, retaken, kingdom, keep, need, leftover, _hold) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if tick < war.hold_lockout {
            return;
        }
        let owner_fac = owner_faction(&war.owner, &war.kingdom);
        let (surviving, virt_defenders, live_raid) = war.raid.as_ref().map_or((0, 0, true), |r| {
            if r.materialized {
                (r.living.len() as u32, 0, true)
            } else {
                (
                    u32::from(r.virt_raiders),
                    u32::from(r.virt_defenders),
                    false,
                )
            }
        });
        let occupy_need = capture_need(surviving.max(raiders_in_keep));
        let allied = war
            .wired
            .values()
            .filter(|w| is_allied(&w.faction, &owner_fac))
            .count() as u32;
        let retake_need = capture_need(if live_raid { allied } else { virt_defenders });
        let update = tick_hold_n(
            &war.owner,
            raiders_in_keep,
            defenders_in_keep,
            war.occupy_hold,
            war.retake_hold,
            occupy_need,
            retake_need,
            delta,
        );
        war.occupy_hold = update.occupy_hold;
        war.retake_hold = update.retake_hold;
        if war.occupy_hold > 0 && tick.is_multiple_of(20) {
            tracing::info!(
                "kingdoms capture hold town={town_id} keep_r={raiders_in_keep} keep_d={defenders_in_keep} need={occupy_need} hold={}/{HOLD_TICKS}",
                war.occupy_hold
            );
        }
        let leftover = if update.captured || update.retaken {
            let mut ids = Vec::new();
            if let Some(raid) = war.raid.take() {
                ids.extend(raid.living);
                ids.extend(raid.temps);
            }
            if let Some(ram) = war.ram.take() {
                ids.extend(ram.parts);
            }
            war.alarm = false;
            war.hold_lockout = tick.saturating_add(HOLD_LOCKOUT_TICKS);
            war.repair_until = tick.saturating_add(HOLD_LOCKOUT_TICKS + REPAIR_WINDOW_TICKS);
            ids
        } else {
            Vec::new()
        };
        (
            update.captured,
            update.retaken,
            war.kingdom.clone(),
            war.scene.keep(),
            occupy_need,
            leftover,
            war.occupy_hold,
        )
    };
    if captured || retaken {
        let tickets = {
            let mut dir = lock();
            dir.towns
                .get_mut(town_id)
                .map(|w| std::mem::take(&mut w.ticketed))
                .unwrap_or_default()
        };
        let worlds = lock().attached_worlds();
        for world in worlds {
            despawn_entities(&world, &leftover);
            apply_force_tickets(&world, &tickets, false);
        }
    }
    if captured {
        let owner_name = occupier.as_str().to_owned();
        let mut dir = lock();
        dir.add_score(&owner_name, 1);
        dir.persist_owner(town_id, occupier);
        crate::damage::set_damage_state(town_id, crate::damage::DamageState::Scarred);
        dir.persist();
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Capture,
            actor: raiders_in_keep as i32,
            other: Some(need as i32),
            x: f64::from(keep.x),
            z: f64::from(keep.z),
            note: format!(
                "{town_id} captured by {owner_name} raiders_in_keep={raiders_in_keep} need={need} hold={HOLD_TICKS}"
            ),
        });
        drop(dir);
        tracing::info!(
            "kingdoms town captured town={town_id} raiders_in_keep={raiders_in_keep} need={need} hold={HOLD_TICKS} score={owner_name}"
        );
    } else if retaken {
        let owner = TownOwner::kingdom(&kingdom);
        let mut dir = lock();
        dir.add_score(kingdom.as_str(), 1);
        dir.persist_owner(town_id, owner);
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Retake,
            actor: defenders_in_keep as i32,
            other: Some(raiders_in_keep as i32),
            x: f64::from(keep.x),
            z: f64::from(keep.z),
            note: format!("{town_id} retaken by {}", kingdom.as_str()),
        });
        drop(dir);
        tracing::info!(
            "kingdoms town retaken town={town_id} defenders_in_keep={defenders_in_keep} owner={}",
            kingdom.as_str()
        );
    }
}

fn player_near_posts(world: &World, posts: &[ScenePost], range: f64) -> bool {
    world.players.load().iter().any(|p| {
        let pos = p.living_entity.entity.pos.load();
        posts
            .iter()
            .any(|post| (pos.x - f64::from(post.x)).hypot(pos.z - f64::from(post.z)) <= range)
    })
}

fn town_has_nearby_player(world: &World, town_id: &str) -> bool {
    let posts = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return false;
        };
        let keep = war.scene.keep();
        let mut posts = vec![keep];
        posts.extend(war.scene.gates().iter().map(|g| ScenePost::new(g.x, g.z)));
        if let Some(raid) = &war.raid {
            posts.push(raid.camp);
        }
        posts
    };
    player_near_posts(world, &posts, PLAYER_LIVE_RANGE)
}

fn sync_front_lod(world: &Arc<World>, town_id: &str, tick: u32) {
    let (has_raid, was_live) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (
            war.raid.is_some(),
            war.raid.as_ref().is_some_and(|r| r.materialized),
        )
    };
    if !has_raid {
        return;
    }
    let near = town_has_nearby_player(world, town_id);
    let under_budget = {
        let dir = lock();
        dir.live_body_count() < body_budget()
    };
    let want = near && (was_live || under_budget);
    if want && !was_live {
        materialize_raid(world, town_id, tick);
    } else if !want && was_live {
        dematerialize_raid(world, town_id);
    }
}

fn materialize_raid(world: &Arc<World>, town_id: &str, tick: u32) {
    {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = war.raid.as_mut() else {
            return;
        };
        raid.materialized = true;
        raid.warm_until = tick.saturating_add(WARM_TICKS);
        raid.spawned_this_wave = 0;
        raid.wave = 0;
        raid.last_wave_tick = tick;
        raid.extra_left = raid.virt_defenders;
        if raid.spec.side.is_kingdom_army() {
            raid.pending_retake = raid.virt_raiders;
            raid.pending_members.clear();
        } else {
            raid.pending_members = compose_wave(raid.virt_raiders, 0);
            raid.pending_retake = 0;
        }
    }
    queue_raid_warmup(world, town_id);
    tracing::info!(town = town_id, tick, "kingdoms raid materialised");
}

fn dematerialize_raid(world: &Arc<World>, town_id: &str) {
    let (despawn, tickets) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = war.raid.as_mut() else {
            return;
        };
        raid.virt_raiders = u16::try_from(raid.living.len()).unwrap_or(u16::MAX);
        raid.virt_defenders = u16::try_from(
            war.wired
                .values()
                .filter(|w| w.faction.is_kingdom())
                .count(),
        )
        .unwrap_or(u16::MAX);
        if war.occupy_hold > 0 {
            raid.virt_march = ABSTRACT_MARCH_TICKS;
        }
        raid.materialized = false;
        raid.pending_members.clear();
        raid.pending_retake = 0;
        raid.extra_left = 0;
        let mut despawn = std::mem::take(&mut raid.living);
        despawn.append(&mut raid.temps);
        if let Some(ram) = war.ram.take() {
            despawn.extend(ram.parts);
        }
        let tickets = std::mem::take(&mut war.ticketed);
        war.pending_tickets.clear();
        war.ticket_cursor = 0;
        war.wired.clear();
        (despawn, tickets)
    };
    despawn_entities(world, &despawn);
    apply_force_tickets(world, &tickets, false);
    tracing::info!(
        town = town_id,
        leftover = despawn.len(),
        "kingdoms raid dematerialised"
    );
}

fn tick_abstract_raid(world: &Arc<World>, town_id: &str, tick: u32) {
    if !tick.is_multiple_of(ABSTRACT_PERIOD) {
        return;
    }
    let (state, side, kingdom) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let Some(raid) = &war.raid else {
            return;
        };
        if raid.materialized {
            return;
        }
        (
            AbstractState {
                raiders: raid.virt_raiders,
                defenders: raid.virt_defenders,
                march: raid.virt_march,
            },
            raid.spec.side,
            war.kingdom.clone(),
        )
    };
    let next = step_abstract(state, ABSTRACT_PERIOD);
    {
        let mut dir = lock();
        if let Some(raid) = dir.towns.get_mut(town_id).and_then(|w| w.raid.as_mut()) {
            raid.virt_raiders = next.raiders;
            raid.virt_defenders = next.defenders;
            raid.virt_march = next.march;
        }
    }
    if next.raiders == 0 {
        finish_abstract_victory(world, town_id, tick);
        return;
    }
    let raiders_in_keep = if next.march >= ABSTRACT_MARCH_TICKS && next.defenders == 0 {
        u32::from(next.raiders)
    } else {
        0
    };
    let defenders_in_keep = if next.march >= ABSTRACT_MARCH_TICKS {
        u32::from(next.defenders)
    } else {
        0
    };
    apply_capture(
        town_id,
        tick,
        raiders_in_keep,
        defenders_in_keep,
        occupier_from_side(side, &kingdom),
        ABSTRACT_PERIOD,
    );
}

fn finish_abstract_victory(world: &Arc<World>, town_id: &str, tick: u32) {
    let tickets = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = war.raid.take() else {
            return;
        };
        war.alarm = false;
        let tickets = std::mem::take(&mut war.ticketed);
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Victory,
            actor: 0,
            other: None,
            x: 0.0,
            z: 0.0,
            note: format!(
                "{town_id} abstract raid defeated raiders=0 defenders={}",
                raid.virt_defenders
            ),
        });
        tickets
    };
    apply_force_tickets(world, &tickets, false);
    tracing::info!(town = town_id, "kingdoms abstract raid defeated");
}

fn occupier_from_side(side: RaidSide, home: &KingdomId) -> TownOwner {
    match side {
        RaidSide::Pillager => TownOwner::Pillager,
        RaidSide::Alder => TownOwner::Kingdom("alder".to_owned()),
        RaidSide::Raven => TownOwner::Kingdom("raven".to_owned()),
        RaidSide::Kingdom => TownOwner::kingdom(home),
    }
}

/// Walled town plus a short pad. Hinterland camps sit outside this, so leftover
/// palisade pillagers do not keep the alarm ringing after a raid.
fn inhabited_bounds(scene: &TownInfoScene) -> crate::geom::Rect {
    const PAD: i32 = 24;
    let keep = scene.keep();
    let mut min_x = keep.x;
    let mut max_x = keep.x;
    let mut min_z = keep.z;
    let mut max_z = keep.z;
    for gate in scene.gates() {
        min_x = min_x.min(gate.x);
        max_x = max_x.max(gate.x);
        min_z = min_z.min(gate.z);
        max_z = max_z.max(gate.z);
    }
    for post in scene.wall() {
        min_x = min_x.min(post[0]);
        max_x = max_x.max(post[0]);
        min_z = min_z.min(post[1]);
        max_z = max_z.max(post[1]);
    }
    crate::geom::Rect::new(min_x - PAD, min_z - PAD, max_x + PAD, max_z + PAD)
}

fn owner_faction(owner: &TownOwner, _home: &KingdomId) -> Faction {
    match owner {
        TownOwner::Pillager => Faction::Pillager,
        TownOwner::Kingdom(name) => Faction::Kingdom(KingdomId::new(name)),
    }
}

fn army_kingdom(side: RaidSide, home: &KingdomId) -> KingdomId {
    match side {
        RaidSide::Alder => KingdomId::new("alder"),
        RaidSide::Raven => KingdomId::new("raven"),
        RaidSide::Kingdom | RaidSide::Pillager => home.clone(),
    }
}

fn occupier_from_keep(
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    keep_rect: crate::geom::Rect,
    owner: &Faction,
) -> TownOwner {
    let mut pillager = 0u32;
    let mut alder = 0u32;
    let mut raven = 0u32;
    for (entity, faction) in snapshots {
        let base = entity.get_entity();
        if has_tag(base, TAG_ROUTED) {
            continue;
        }
        let pos = base.pos.load();
        if !in_courtyard(keep_rect, pos.x, pos.z) {
            continue;
        }
        if !is_hostile(faction, owner) {
            continue;
        }
        match faction {
            Faction::Pillager => pillager += 1,
            Faction::Kingdom(k) if k.as_str() == "alder" => alder += 1,
            Faction::Kingdom(_) => raven += 1,
            Faction::Neutral => {}
        }
    }
    if alder >= pillager && alder >= raven && alder > 0 {
        TownOwner::kingdom(&KingdomId::new("alder"))
    } else if raven >= pillager && raven > 0 {
        TownOwner::kingdom(&KingdomId::new("raven"))
    } else {
        TownOwner::Pillager
    }
}

fn soaking() -> bool {
    std::env::var("KINGDOMS_SOAK_LOG").is_ok_and(|s| !s.is_empty())
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

fn spawn_extra_defenders(world: &Arc<World>, town_id: &str, max: u16) {
    if max == 0 {
        return;
    }
    let (kingdom, gate, extra, cursor) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let home = Faction::Kingdom(war.kingdom.clone());
        let have = war
            .wired
            .values()
            .filter(|w| is_allied(&w.faction, &home))
            .count() as u16;
        let Some(raid) = war.raid.as_mut() else {
            return;
        };
        if raid.temps.is_empty() {
            raid.extra_left = raid.extra_left.saturating_sub(have);
        }
        let gate = war.threatened.unwrap_or_else(|| war.scene.keep());
        (
            war.kingdom.clone(),
            gate,
            raid.extra_left,
            raid.temps.len() as u16,
        )
    };
    let extra = extra.min(max);
    if extra == 0 {
        return;
    }
    let y = standing_y(world, gate);
    let mut spawned = 0u32;
    let mut extras = Vec::new();
    for i in 0..extra {
        let pos = spawn_offset(gate, cursor + i, y);
        let role = if i % 3 == 2 {
            VillagerRole::Archer
        } else {
            VillagerRole::Guard
        };
        if let Some(id) = spawn_citizen(world, town_id, &kingdom, role, false, pos) {
            if let Some(entity) = world.get_entity_by_id(id) {
                let _ = entity.get_entity().add_scoreboard_tag(TAG_TEMP);
            }
            extras.push(id);
            spawned += 1;
        }
    }
    if spawned > 0 {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id)
            && let Some(raid) = &mut war.raid
        {
            raid.temps.extend(extras.iter().copied());
            raid.extra_left = raid.extra_left.saturating_sub(spawned as u16);
        }
        tracing::info!(
            town = town_id,
            spawned,
            remaining = extra.saturating_sub(spawned as u16),
            "spawned extra defenders for raid"
        );
    }
}

fn spawn_garrison_if_needed(world: &Arc<World>, town_id: &str, max: u16) -> u16 {
    if max == 0 {
        return 0;
    }
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
    let mut spawned = 0u16;
    for slot in slots {
        if spawned >= max {
            break;
        }
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
    existing as u16 + spawned
}

pub fn stop_raid(town_id: Option<&str>) -> u32 {
    let worlds = lock().attached_worlds();
    dump_live_plots(town_id);
    let (n, tickets, despawn) = lock().stop_raid(town_id);
    for world in worlds {
        despawn_entities(&world, &despawn);
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
pub fn war_front() -> WarFront {
    lock().war_front()
}

/// Console `/war` summary: scores, clock, and each cached town.
#[must_use]
pub fn format_war_status() -> String {
    let front = war_front();
    let mut scores: Vec<String> = front
        .scores
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    scores.sort();
    let mut lines = vec![format!(
        "tick={} scores [{}] clock period={} last={} fronts={} next_raid={}",
        front.tick,
        scores.join(" "),
        front.clock_period,
        front.last_clock_tick,
        front.fronts,
        front.next_raid_id
    )];
    for t in &front.towns {
        lines.push(format!(
            "{} owner={} home={} alarm={} raid={} live={} side={} hold={}/{} repair_until={}",
            t.id,
            t.owner,
            t.kingdom,
            t.alarm,
            t.raid_active,
            t.live,
            t.raid_side.as_deref().unwrap_or("-"),
            t.occupy_hold,
            t.occupy_need,
            t.repair_until
        ));
    }
    if front.towns.is_empty() {
        lines.push("no cached towns".to_owned());
    }
    lines.join(" | ")
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
    fn war_front_includes_scores_and_clock() {
        let mut dir = Director::new();
        dir.tick = 42;
        dir.scores.insert("pillager".to_owned(), 2);
        dir.last_clock_tick = 24;
        let front = dir.war_front();
        assert_eq!(front.tick, 42);
        assert_eq!(front.scores.get("pillager"), Some(&2));
        assert_eq!(front.last_clock_tick, 24);
        assert!(front.towns.is_empty());
        assert!(front.clock_period > 0);
        assert!(front.fronts >= 1);
    }

    #[test]
    fn clock_side_never_attacks_the_current_owner() {
        let alder = TownOwner::kingdom(&KingdomId::new("alder"));
        let raven = TownOwner::kingdom(&KingdomId::new("raven"));
        assert_eq!(clock_side(&TownOwner::Pillager, 0), RaidSide::Kingdom);
        assert_eq!(clock_side(&alder, 0), RaidSide::Pillager);
        assert_eq!(clock_side(&raven, 1), RaidSide::Alder);
        assert_eq!(clock_side(&alder, 1), RaidSide::Raven);
        assert_eq!(clock_side(&alder, 2), RaidSide::Raven);
        assert_eq!(clock_side(&raven, 2), RaidSide::Alder);
        assert_eq!(REPAIR_WINDOW_TICKS, 400);
        assert_eq!(HOLD_LOCKOUT_TICKS + REPAIR_WINDOW_TICKS, 800);
    }

    #[test]
    fn war_front_marks_unmaterialised_raids_as_not_live() {
        let mut dir = Director::new();
        dir.tick = 10;
        let front = dir.war_front();
        assert!(front.towns.iter().all(|t| !t.live));
        assert_eq!(crate::abstract_combat::DEFAULT_BODY_BUDGET, 240);
        assert_eq!(crate::abstract_combat::PLAYER_LIVE_RANGE, 128.0);
    }

    #[test]
    fn dest_for_sends_civilians_to_keep_when_alarmed() {
        let keep = PostTarget { x: 1, y: 64, z: 1 };
        let work = PostTarget { x: 9, y: 64, z: 9 };
        let home = PostTarget { x: 3, y: 64, z: 3 };
        let patrol = PostTarget { x: 5, y: 70, z: 5 };
        let march = PostTarget { x: 7, y: 64, z: 7 };
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Civilian),
            true,
            false,
            false,
            None,
            march,
            keep,
            march,
            march,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (keep.x, keep.z));
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Civilian),
            false,
            false,
            false,
            None,
            march,
            keep,
            march,
            march,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (work.x, work.z));
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Guard),
            false,
            false,
            false,
            None,
            march,
            keep,
            march,
            march,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (patrol.x, patrol.z));
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
            occupy_hold: 40,
            occupy_need: 8,
            ram_hp: 12.0,
        };
        assert_eq!(state.owner, "pillager");
        assert!(state.gate_breached);
        assert_eq!(state.occupy_need, 8);
    }
}
