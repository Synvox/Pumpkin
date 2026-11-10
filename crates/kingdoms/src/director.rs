//! In-process war director: start/stop raids, join kingdoms, query battle state.
//!
//! An HTTP layer can wrap these functions later. Tick is driven from
//! `ServerTickStart`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
use pumpkin_util::Hand;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;
use uuid::Uuid;

use crate::abstract_combat::{
    ABSTRACT_MARCH_TICKS, ABSTRACT_PERIOD, AbstractState, PLAYER_APPROACH_RANGE, PLAYER_LIVE_RANGE,
    body_budget, near_camp, step_abstract,
};
use crate::chat::{self, ShoutKind};
use crate::engines;
use crate::faction::{
    Faction, KingdomId, TAG_ARMED, TAG_BEARER, TAG_CAPTAIN, TAG_MILITIA, TAG_NPC_BLACKSMITH,
    TAG_NPC_PRIEST, TAG_RAIDER, TAG_ROUTED, TAG_TEMP, faction_of_entity, has_tag, is_allied,
    is_hostile, is_kingdoms_tagged, kingdom_color_rgb, kingdom_tag, role_from_tags,
};
use crate::flow::{DIRECT_RANGE, FlowField, is_squad_leader, should_refresh_path};
use crate::geom::Rect;
use crate::goals::{
    PostTarget, already_wired, bind_citizen, bind_golem, bind_raider, drive_live, equip,
    shoot_arrow,
};
use crate::info::{GateInfo, TownInfo};
use crate::lod::{
    ACQUIRE_RANGE, ALARM_RANGE, CIVILIAN_SLEEP_RANGE, DEFAULT_PATH_CAP, LodTier, PLAYER_CULL_RANGE,
    PLAYER_SIM_RANGE, PathBudget, SpatialBuckets,
};
use crate::raid::{
    RaidSide, RaidSpec, WAVE_ADVANCE_REMAINING, WAVE_SPACING_TICKS, WaveMember, compose_wave,
    spawn_offset,
};
use crate::repair::{self, RepairJob};
use crate::roles::{PopulationSpec, VillagerRole, citizen_slots};
use crate::scene::{ScenePost, TownInfoScene, TownScene};
use crate::schedule::{self, CivilianPost};
use crate::siege::{
    self, FLAG_MAST_FENCES, FLAG_PANEL_HEIGHT, FLAG_PANEL_WIDTH, FlagPresence, HOLD_TICKS,
    MarchJob, TownOwner, WALL_WALK_LIFT, capture_need, hold_ticks, in_capture_zone, in_courtyard,
    near_flag, on_flag_plate, owner_banner, owner_wool, tick_flag_hold,
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
const REPAIR_WINDOW_TICKS: u32 = 2400;
const DEFAULT_WAR_CLOCK_FRONTS: usize = 1;
const DEFAULT_WAR_CLOCK_SIZE: u16 = 24;
const REPAIR_PERIOD: u32 = 40;
const REPAIR_PLACE_PER_TICK: u32 = 2;
const REPAIR_REACH: f64 = 2.5;
const REPAIR_QUEUE_CAP: usize = 24;
/// Columns inspected for missing blocks per repair pass.
const REPAIR_SCAN_COLUMNS: usize = 4;
/// No flag-distance progress for this long counts as a stuck column.
const STUCK_TICKS: u32 = 200;

const COMBAT_LOG_CAP: usize = 512;
const RESPAWN_PERIOD: u32 = 200;
/// Force-load tickets and garrison before the first soldier spawns (~5 s).
const WARM_TICKS: u32 = 120;
/// Tickets applied per town per director tick during warmup.
const TICKETS_PER_TICK: usize = 1;
/// One ticket per live front; idle towns must not consume these.
const TICKETS_PER_WORLD_TICK: u32 = 3;
/// Ownership flips (persist, webhook, credit) committed per director tick.
const CAPTURES_PER_TICK: usize = 1;
/// Soldiers (raiders, extras, garrison) spawned per town per director tick.
const SPAWNS_PER_TICK: usize = 3;
/// Cap across every town so three live fronts cannot spawn 9 bodies at once.
const SPAWNS_PER_WORLD_TICK: u32 = 12;
/// Scoreboard rout/rally tag writes per town per tick.
const ROUT_TAGS_PER_TICK: usize = 8;
/// Leftover raid bodies despawned per town per tick.
const DESPAWNS_PER_TICK: usize = 12;
/// Far wave members turned into sliding ghosts per town per tick.
const GHOST_SPAWNS_PER_TICK: usize = 8;
/// Nearby players credited on a capture or successful defense.
const CREDIT_RANGE: f64 = 64.0;
/// Minimum ticks between any soldier/captain line on the whole director.
const GLOBAL_CHAT_GAP: u32 = 40;
/// After a flip, skip the opposite hold so leftover garrison cannot retake instantly.
const HOLD_LOCKOUT_TICKS: u32 = 400;
/// Trebuchet-hole blocks restored per director tick (the 45-block dump was a 629 ms hitch).
const PUNCH_REPAIR_PER_TICK: usize = 2;
const PUNCH_BREAK_PER_TICK: usize = 2;
/// Iron-bar portcullis cells written per town per tick.
const PORTCULLIS_BARS_PER_TICK: usize = 2;
/// Civilians kitted from the armoury per tick when the bell rings.
const ARM_PER_TICK: usize = 4;
/// Log a director phase that ran longer than this.
const PHASE_WARN_MS: f64 = 20.0;
/// Ranked-player squad order range.
const SQUAD_RANGE: f64 = 48.0;
/// Goat-horn command cooldown.
const HORN_COOLDOWN_TICKS: u32 = 40;
/// First live ticks skip engines so join does not dump 200 ms of town work.
const JOIN_AMORTIZE_TICKS: u32 = 40;
/// AI bodies per amortised town tick (plate counts still cover everyone).
const JOIN_ENTITY_BUDGET: usize = 64;
/// Item / XP orbs despawned per town tick during a live raid.
const DROPS_PER_TICK: usize = 8;
/// Engines / punch / repair-scan towns per world tick.
const HEAVY_TOWNS_PER_TICK: u32 = 1;
/// Raiders within this range of a gate count as storming it.
const STORMING_RANGE: f64 = 16.0;
/// This many pillagers at the portcullis count as a breach even without the ram.
const STORMING_COUNT: usize = 8;
/// Stuck columns breach if this many raiders are within [`STUCK_STORM_RANGE`].
const STUCK_STORM_COUNT: usize = 3;
const STUCK_STORM_RANGE: f64 = 24.0;

static WORST_MSPT_CENTS: AtomicU32 = AtomicU32::new(0);
static WORST_DIRECTOR_CENTS: AtomicU32 = AtomicU32::new(0);
static SPAWNS_THIS_TICK: AtomicU32 = AtomicU32::new(0);
static TICKETS_THIS_TICK: AtomicU32 = AtomicU32::new(0);
static REPAIR_SCANS_THIS_TICK: AtomicU32 = AtomicU32::new(0);
static HEAVY_THIS_TICK: AtomicU32 = AtomicU32::new(0);
static REPAIR_SCAN_BUSY: AtomicBool = AtomicBool::new(false);

fn post_y_cache() -> &'static Mutex<HashMap<(i32, i32), i32>> {
    static CACHE: OnceLock<Mutex<HashMap<(i32, i32), i32>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

static RUNTIME: OnceLock<Mutex<Director>> = OnceLock::new();

/// Process-wide director. Tests can replace the lock contents.
pub fn lock() -> std::sync::MutexGuard<'static, Director> {
    RUNTIME
        .get_or_init(|| Mutex::new(Director::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Cell coordinates of towns already in the ownership file (for plan warmup).
#[must_use]
pub fn owned_cells() -> Vec<(i32, i32)> {
    lock()
        .owners
        .keys()
        .filter_map(|id| parse_town_id(id).map(|(_, cx, cz)| (cx, cz)))
        .collect()
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
    pub flag: (i32, i32),
    pub flag_status: String,
    pub flag_paused: bool,
    pub flag_attackers: u32,
    pub flag_defenders: u32,
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
    pub flag: (i32, i32),
    pub flag_status: String,
    pub flag_paused: bool,
    pub flag_attackers: u32,
    pub flag_defenders: u32,
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
    Trebuchet,
    LaddersRaised,
    Portcullis,
    CaptureStarted,
    CaptureProgress,
    CapturePaused,
    FlagChanged,
}

/// Order a ranked player can give nearby kingdom soldiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SquadOrder {
    Follow,
    Hold,
    Charge,
}

impl SquadOrder {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Hold => "hold",
            Self::Charge => "charge",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "follow" | "rally" => Some(Self::Follow),
            "hold" | "halt" => Some(Self::Hold),
            "charge" | "advance" => Some(Self::Charge),
            _ => None,
        }
    }

    const fn shout(self) -> ShoutKind {
        match self {
            Self::Follow => ShoutKind::Rally,
            Self::Hold => ShoutKind::Hold,
            Self::Charge => ShoutKind::Charge,
        }
    }

    const fn line(self) -> &'static str {
        match self {
            Self::Follow => "On me. Form up.",
            Self::Hold => "Hold this ground.",
            Self::Charge => "With me. Charge!",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Follow => Self::Hold,
            Self::Hold => Self::Charge,
            Self::Charge => Self::Follow,
        }
    }
}

#[derive(Clone)]
struct SquadCommand {
    order: SquadOrder,
    kingdom: KingdomId,
    hold: PostTarget,
    x: f64,
    y: f64,
    z: f64,
    at_tick: u32,
}

/// Named commander the Sails director can drive per kingdom.
#[derive(Clone, Debug)]
pub struct Hero {
    pub kingdom: String,
    pub name: String,
    pub title: String,
    pub line: String,
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
    player_stats: HashMap<String, siege::PlayerStats>,
    last_hit: HashMap<i32, String>,
    heroes: HashMap<String, Hero>,
    persist_dirty: bool,
    last_any_shout: u32,
    clock_filling: bool,
    pending_flips: VecDeque<PendingFlip>,
    pending_keep_tp: HashSet<String>,
    squad_orders: HashMap<String, SquadCommand>,
}

struct PendingFlip {
    town_id: String,
    captured: bool,
    occupier: TownOwner,
    kingdom: KingdomId,
    keep: ScenePost,
    raiders_in_keep: u32,
    defenders_in_keep: u32,
    need: u32,
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
    builders: Vec<i32>,
    builders_scanned: bool,
    last_repair_tick: u32,
    ram: Option<crate::ram::SiegeRam>,
    treb_shot: Option<crate::engines::TrebShot>,
    siege_tower: Option<crate::engines::SiegeTower>,
    ladders_placed: bool,
    last_treb_tick: u32,
    punch_repair: Vec<RepairJob>,
    punch_repair_at: u32,
    punch_break: Vec<engines::PunchHit>,
    pending_bars: Vec<BlockPos>,
    pending_breach: Vec<BlockPos>,
    crossed_curtain: bool,
    logged_archer: bool,
    logged_sally: bool,
    capture_bar: Option<Uuid>,
    captain_down: bool,
    pending_tickets: Vec<Vector2<i32>>,
    ticket_cursor: usize,
    hold_lockout: u32,
    repair_until: u32,
    pending_despawn: Vec<i32>,
    pending_unticket: Vec<Vector2<i32>>,
    repair_scan_cursor: usize,
    named_npcs: bool,
    live_age: u32,
    /// Entity ids collected across join-snapshot strips so the first live
    /// tick does not walk the whole capital AABB.
    snapshot_ids: HashSet<i32>,
    snapshot_cursor: u32,
    flag_paused: bool,
    flag_attackers: u32,
    flag_defenders: u32,
    last_flag_event: u32,
    last_progress_dist: f64,
    last_progress_tick: u32,
    repair_placed: u32,
    repair_started: u32,
    repair_placed_tick: u32,
    repair_placed_this_tick: u32,
}

struct Wired {
    faction: Faction,
    captain: bool,
}

/// Despawned raid body that still slides and counts toward the plate.
#[derive(Clone)]
struct GhostSoldier {
    faction: Faction,
    x: f64,
    y: f64,
    z: f64,
    bearer: bool,
    raider: bool,
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
    tower_spawned: bool,
    materialized: bool,
    virt_raiders: u16,
    virt_defenders: u16,
    virt_march: u32,
    ghosts: Vec<GhostSoldier>,
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
            player_stats: HashMap::new(),
            last_hit: HashMap::new(),
            heroes: HashMap::new(),
            persist_dirty: false,
            last_any_shout: 0,
            clock_filling: false,
            pending_flips: VecDeque::new(),
            pending_keep_tp: HashSet::new(),
            squad_orders: HashMap::new(),
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
        self.player_stats = saved.stats;
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
            stats: self.player_stats.clone(),
        };
        siege::save_state_async(path.clone(), state);
    }

    fn mark_dirty(&mut self) {
        self.persist_dirty = true;
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
                    // Soldiers only. Militia and named civilians sit in `wired`
                    // after the bell and must not eat KINGDOMS_BODY_BUDGET.
                    // Ghosts still occupy a budget slot so cull does not refill.
                    raid.living.len() + raid.temps.len() + raid.ghosts.len()
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
        self.mark_dirty();
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
        self.player_kingdoms.insert(uuid.clone(), kingdom.clone());
        let rank = self.player_stats.entry(uuid.clone()).or_default().rank();
        kit_player(player, &kingdom, rank, true);
        maybe_give_banner(player, &kingdom, self.player_stats.get_mut(&uuid));
        self.mark_dirty();
        let seed = world.level.seed.0;
        let pos = entity.pos.load();
        let near = crate::town::nearest_town(seed, pos.x.floor() as i32, pos.z.floor() as i32);
        let tid = crate::info::town_id(seed, near.cell_x, near.cell_z);
        let _ = self.ensure_town(&world, &tid);
        set_keep_respawn(player, &world, &kingdom, &self.towns);
        if pick_keep_stand(&world, &kingdom, &self.towns).is_none() {
            self.pending_keep_tp.insert(uuid);
        }
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
        let materialized = (near || soaking()) && self.live_body_count() < body_budget();
        // Always warm tickets, including abstract raids, so first contact
        // does not apply a town's tickets on the same tick bodies spawn.
        let warm_until = tick.saturating_add(WARM_TICKS);
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
                tower_spawned: false,
                materialized,
                virt_raiders,
                virt_defenders,
                virt_march: 0,
                ghosts: Vec::new(),
            });
            war.alarm = true;
            war.gates_closed = false;
            war.gate_breached = false;
            war.ladders_placed = false;
            war.last_treb_tick = 0;
            war.punch_repair.clear();
            war.punch_repair_at = 0;
            war.punch_break.clear();
            war.pending_bars.clear();
            war.pending_breach.clear();
            war.crossed_curtain = false;
            war.logged_archer = false;
            war.logged_sally = false;
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
                if let Some(tower) = war.siege_tower.take() {
                    despawn.extend(tower.parts);
                }
                if let Some(shot) = war.treb_shot.take()
                    && shot.part >= 0
                {
                    despawn.push(shot.part);
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
            occupy_need: war.raid.as_ref().map_or(3, |r| {
                capture_need(if r.materialized {
                    r.living.len() as u32
                } else {
                    u32::from(r.virt_raiders)
                })
            }),
            ram_hp: war.ram.as_ref().map_or(0.0, |r| r.hp),
            flag: (war.scene.flag().x, war.scene.flag().z),
            flag_status: flag_status_of(war).to_owned(),
            flag_paused: war.flag_paused,
            flag_attackers: war.flag_attackers,
            flag_defenders: war.flag_defenders,
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
                    occupy_need: war.raid.as_ref().map_or(3, |r| {
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
                    flag: (war.scene.flag().x, war.scene.flag().z),
                    flag_status: flag_status_of(war).to_owned(),
                    flag_paused: war.flag_paused,
                    flag_attackers: war.flag_attackers,
                    flag_defenders: war.flag_defenders,
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

    pub fn note_hit(&mut self, victim: i32, attacker_uuid: String) {
        if self.last_hit.len() > 512 {
            self.last_hit.clear();
        }
        self.last_hit.insert(victim, attacker_uuid);
    }

    fn note_death(&mut self, entity_id: i32) -> Option<(String, siege::Rank)> {
        let tick = self.tick;
        let killer = self.last_hit.remove(&entity_id);
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
        let mut ranked = None;
        if let Some((kind, town, n)) = event {
            if let Some(uuid) = killer {
                let (before, after) = {
                    let stats = self.player_stats.entry(uuid.clone()).or_default();
                    let before = stats.rank();
                    stats.kills = stats.kills.saturating_add(1);
                    (before, stats.rank())
                };
                self.mark_dirty();
                if after != before {
                    ranked = Some((uuid, after));
                }
            }
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
        ranked
    }

    fn ensure_town(&mut self, world: &World, town_id: &str) -> Result<&mut TownWar, String> {
        if self.towns.contains_key(town_id) {
            return self
                .towns
                .get_mut(town_id)
                .ok_or_else(|| "town vanished".to_owned());
        }
        let seed = world.level.seed.0;
        let info = lookup_town_info(seed, town_id).ok_or_else(|| {
            if let Some((s, cx, cz)) = parse_town_id(town_id) {
                crate::plan::request_plan(s, cx, cz);
            } else if let Some((cx, cz)) = parse_cell(town_id) {
                crate::plan::request_plan(seed, cx, cz);
            }
            format!("town plan not ready {town_id}")
        })?;
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
            let kingdom = self.towns.get(town).map(|w| w.kingdom.as_str().to_owned());
            let hero = kingdom.as_deref().and_then(|k| self.heroes.get(k).cloned());
            let speaker = hero
                .as_ref()
                .map(|h| h.name.as_str())
                .filter(|n| !n.is_empty())
                .unwrap_or("captain");
            let spoken = hero.as_ref().and_then(|h| {
                let line = h.line.trim();
                if line.is_empty() {
                    None
                } else {
                    Some(line.to_owned())
                }
            });
            let worlds: Vec<Arc<World>> = self.worlds.iter().filter_map(Weak::upgrade).collect();
            let gap_ok = event.tick.saturating_sub(self.last_any_shout) >= GLOBAL_CHAT_GAP;
            if gap_ok {
                for world in worlds {
                    if chat::try_shout_as(
                        &world,
                        event.tick,
                        town,
                        kind,
                        event.x,
                        event.z,
                        &mut self.last_shout,
                        speaker,
                        spoken.as_deref(),
                    ) {
                        self.last_any_shout = event.tick;
                    }
                }
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
        CombatKind::Victory
        | CombatKind::Capture
        | CombatKind::Retake
        | CombatKind::FlagChanged => Some(ShoutKind::Victory),
        CombatKind::CaptureStarted => Some(ShoutKind::Charge),
        CombatKind::CapturePaused => Some(ShoutKind::Hold),
        _ => None,
    }
}

fn flag_status_of(war: &TownWar) -> &'static str {
    if war.flag_paused {
        "paused"
    } else if war.occupy_hold > 0 {
        "capturing"
    } else if war.retake_hold > 0 {
        "reclaiming"
    } else {
        "idle"
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
            builders: Vec::new(),
            builders_scanned: false,
            last_repair_tick: 0,
            ram: None,
            treb_shot: None,
            siege_tower: None,
            ladders_placed: false,
            last_treb_tick: 0,
            punch_repair: Vec::new(),
            punch_repair_at: 0,
            punch_break: Vec::new(),
            pending_bars: Vec::new(),
            pending_breach: Vec::new(),
            crossed_curtain: false,
            logged_archer: false,
            logged_sally: false,
            capture_bar: None,
            captain_down: false,
            pending_tickets: Vec::new(),
            ticket_cursor: 0,
            hold_lockout: 0,
            repair_until: 0,
            pending_despawn: Vec::new(),
            pending_unticket: Vec::new(),
            repair_scan_cursor: 0,
            named_npcs: false,
            live_age: 0,
            snapshot_ids: HashSet::new(),
            snapshot_cursor: 0,
            flag_paused: false,
            flag_attackers: 0,
            flag_defenders: 0,
            last_flag_event: 0,
            last_progress_dist: f64::INFINITY,
            last_progress_tick: 0,
            repair_placed: 0,
            repair_started: 0,
            repair_placed_tick: 0,
            repair_placed_this_tick: 0,
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

/// Public: destination a citizen goal should walk to.
#[must_use]
pub fn post_for_role(mob: &dyn Mob, town_id: &str, role: VillagerRole) -> Option<PostTarget> {
    let dir = lock();
    let war = dir.towns.get(town_id)?;
    let post = match role {
        VillagerRole::Guard if war.alarm && war.occupy_hold > 0 => war.scene.flag(),
        VillagerRole::Guard if war.alarm && war.gate_breached => war.scene.flag(),
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
        VillagerRole::Civilian if war.alarm && war.occupy_hold > 0 => war.scene.flag(),
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
        war.scene.flag()
    } else {
        war.threatened.unwrap_or_else(|| war.scene.keep())
    };
    let world = mob.get_entity().world.load();
    Some(PostTarget::from_post(dest, standing_y(&world, dest)))
}

fn standing_y(world: &World, post: ScenePost) -> i32 {
    if post.y != 0 {
        return post.y;
    }
    let key = (post.x, post.z);
    {
        let cache = post_y_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(y) = cache.get(&key) {
            return *y;
        }
    }
    let seed = world.level.seed.0;
    let town = town_containing(seed, post.x, post.z);
    let plan_y = crate::plan::cached_plan(seed, town.cell_x, town.cell_z)
        .map(|p| p.heights.height(post.x, post.z) + 1);
    let y = resolve_stand_y(0, plan_y, world.sea_level);
    post_y_cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, y);
    y
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

fn kit_armor(rank: siege::Rank) -> [(EquipmentSlot, &'static Item); 4] {
    match rank {
        siege::Rank::Levy | siege::Rank::Freeman => [
            (EquipmentSlot::HEAD, &Item::LEATHER_HELMET),
            (EquipmentSlot::CHEST, &Item::LEATHER_CHESTPLATE),
            (EquipmentSlot::LEGS, &Item::LEATHER_LEGGINGS),
            (EquipmentSlot::FEET, &Item::LEATHER_BOOTS),
        ],
        siege::Rank::Sergeant => [
            (EquipmentSlot::HEAD, &Item::LEATHER_HELMET),
            (EquipmentSlot::CHEST, &Item::CHAINMAIL_CHESTPLATE),
            (EquipmentSlot::LEGS, &Item::LEATHER_LEGGINGS),
            (EquipmentSlot::FEET, &Item::LEATHER_BOOTS),
        ],
        siege::Rank::Knight | siege::Rank::Banneret => [
            (EquipmentSlot::HEAD, &Item::IRON_HELMET),
            (EquipmentSlot::CHEST, &Item::IRON_CHESTPLATE),
            (EquipmentSlot::LEGS, &Item::IRON_LEGGINGS),
            (EquipmentSlot::FEET, &Item::IRON_BOOTS),
        ],
    }
}

fn kit_weapon(rank: siege::Rank) -> &'static Item {
    match rank {
        siege::Rank::Levy => &Item::WOODEN_SWORD,
        siege::Rank::Freeman => &Item::STONE_SWORD,
        siege::Rank::Sergeant | siege::Rank::Knight | siege::Rank::Banneret => &Item::IRON_SWORD,
    }
}

fn kit_shield(kingdom: &KingdomId) -> Option<&'static Item> {
    match kingdom.as_str() {
        "alder" | "raven" => Some(&Item::SHIELD),
        _ => None,
    }
}

fn kingdom_banner(kingdom: &KingdomId) -> &'static Item {
    match kingdom.as_str() {
        "raven" => &Item::BLUE_BANNER,
        _ => &Item::GREEN_BANNER,
    }
}

fn kit_player(player: &Player, kingdom: &KingdomId, rank: siege::Rank, give_rewards: bool) {
    let rgb = kingdom_dye(kingdom);
    let pieces = kit_armor(rank);
    let mut changes = Vec::with_capacity(pieces.len() + 2);
    {
        let mut eq = player
            .living_entity
            .entity_equipment
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (slot, item) in pieces {
            let stack = if item.id == Item::LEATHER_HELMET.id
                || item.id == Item::LEATHER_CHESTPLATE.id
                || item.id == Item::LEATHER_LEGGINGS.id
                || item.id == Item::LEATHER_BOOTS.id
            {
                dyed_leather(item, rgb)
            } else {
                ItemStack::new(1, item)
            };
            eq.put(&slot, stack.clone());
            changes.push((slot, stack));
        }
        let weapon = ItemStack::new(1, kit_weapon(rank));
        eq.put(&EquipmentSlot::MAIN_HAND, weapon.clone());
        changes.push((EquipmentSlot::MAIN_HAND, weapon));
        if let Some(shield) = kit_shield(kingdom) {
            let stack = ItemStack::new(1, shield);
            eq.put(&EquipmentSlot::OFF_HAND, stack.clone());
            changes.push((EquipmentSlot::OFF_HAND, stack));
        }
    }
    // Entity equipment is what other players see. The local hotbar and
    // off-hand inventory are what CSetContainerContent actually carries.
    let weapon = ItemStack::new(1, kit_weapon(rank));
    player.inventory.set_held_item(weapon);
    if let Some(shield) = kit_shield(kingdom) {
        player
            .inventory
            .set_stack_in_hand(Hand::Left, ItemStack::new(1, shield));
    }
    player.living_entity.send_equipment_changes(&changes);
    if give_rewards {
        let mut stack = ItemStack::new(1, kit_weapon(rank));
        let _ = player.inventory.insert_stack_anywhere(&mut stack);
    }
    if rank >= siege::Rank::Freeman {
        let mut horn = ItemStack::new(1, &Item::GOAT_HORN);
        let _ = player.inventory.insert_stack_anywhere(&mut horn);
    }
    player.sync_inventory_to_client();
    tracing::info!(
        player = %player.gameprofile.name,
        weapon = kit_weapon(rank).registry_key,
        shield = kit_shield(kingdom).map_or("none", |s| s.registry_key),
        "kingdoms levy kit"
    );
}

fn maybe_give_banner(player: &Player, kingdom: &KingdomId, stats: Option<&mut siege::PlayerStats>) {
    let Some(stats) = stats else {
        return;
    };
    if stats.rank() != siege::Rank::Banneret || stats.banner_given {
        return;
    }
    let mut stack = ItemStack::new(1, kingdom_banner(kingdom));
    if player.inventory.insert_stack_anywhere(&mut stack) {
        stats.banner_given = true;
    }
}

/// Planner / post Y only. Live `sample_surface_height` / `get_block_state` on
/// join was a 7–21 s hitch that blocked other players from logging in.
fn resolve_stand_y(post_y: i32, plan_y: Option<i32>, sea_level: i32) -> i32 {
    if post_y != 0 {
        return post_y;
    }
    plan_y.unwrap_or(sea_level + 1)
}

struct KeepStandPlan {
    keep: siege::StandCandidate,
    skip: Vec<crate::geom::Rect>,
    town_id: String,
}

fn keep_stand_plan(kingdom: &KingdomId, towns: &HashMap<String, TownWar>) -> Option<KeepStandPlan> {
    let mut homes: Vec<&TownWar> = towns
        .values()
        .filter(|w| w.owner.as_str() == kingdom.as_str())
        .collect();
    if homes.is_empty() {
        homes = towns.values().filter(|w| w.kingdom == *kingdom).collect();
    }
    let home = homes.first()?;
    let keep = home.scene.keep();
    let castle = &home.scene.info().castle;
    let mut skip = vec![
        castle.keep,
        castle.hall,
        castle.chapel,
        castle.stables,
        castle.barracks,
        castle.kitchen,
        castle.gatehouse,
        castle.barbican,
    ];
    skip.extend(castle.towers.iter().copied());
    Some(KeepStandPlan {
        keep: siege::StandCandidate {
            x: keep.x,
            z: keep.z,
        },
        skip,
        town_id: home.info_id.clone(),
    })
}

fn pick_keep_stand(
    world: &World,
    kingdom: &KingdomId,
    towns: &HashMap<String, TownWar>,
) -> Option<(i32, i32, i32)> {
    let plan = keep_stand_plan(kingdom, towns)?;
    let occ = parse_town_id(&plan.town_id)
        .and_then(|(seed, cx, cz)| crate::plan::cached_plan(seed, cx, cz));
    if let Some(town_plan) = occ.as_ref() {
        let open = |x: i32, z: i32| {
            if plan.skip.iter().any(|r| r.contains(x, z)) {
                return false;
            }
            matches!(
                town_plan.occ.kind(x, z),
                crate::cells::CellKind::CastleCourt
                    | crate::cells::CellKind::Green
                    | crate::cells::CellKind::Plaza
            )
        };
        if let Some(stand) = siege::pick_open_court(
            plan.keep,
            open,
            |x, z| town_plan.heights.height(x, z) + 1,
            48,
        ) {
            tracing::debug!(
                x = stand.0,
                y = stand.1,
                z = stand.2,
                town = %plan.town_id,
                "kingdoms keep spawn via open court"
            );
            return Some(stand);
        }
        let y = town_plan.heights.height(plan.keep.x, plan.keep.z) + 1;
        return Some((plan.keep.x, y, plan.keep.z));
    }
    let scene_y = towns.get(&plan.town_id).map_or(0, |w| w.scene.keep().y);
    Some((
        plan.keep.x,
        resolve_stand_y(scene_y, None, world.sea_level),
        plan.keep.z,
    ))
}

fn set_keep_respawn(
    player: &Player,
    world: &World,
    kingdom: &KingdomId,
    towns: &HashMap<String, TownWar>,
) {
    let Some((x, y, z)) = pick_keep_stand(world, kingdom, towns) else {
        return;
    };
    let _ = player.set_respawn_point(
        world.dimension.clone(),
        BlockPos::new(x, y, z),
        0.0,
        0.0,
        true,
    );
}

/// Override vanilla spawn when a kingdom member would suffocate or land at world spawn.
pub fn fix_spawn_location(player: &Player, spawn: &mut Vector3<f64>) {
    let uuid = player.gameprofile.id.to_string();
    let world = player.get_entity().world.load();
    let stand = {
        let dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return;
        };
        pick_keep_stand(&world, &kingdom, &dir.towns)
    };
    let Some((x, y, z)) = stand else {
        return;
    };
    tracing::info!(
        "kingdoms keep spawn player={} x={} y={} z={}",
        player.gameprofile.name,
        x,
        y,
        z
    );
    *spawn = Vector3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5);
}

/// Re-kit a player after death so levy sword and shield come back.
pub fn respawn_kit(player: &Player) {
    let uuid = player.gameprofile.id.to_string();
    let (kingdom, rank) = {
        let dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return;
        };
        let rank = dir
            .player_stats
            .get(&uuid)
            .map_or(siege::Rank::Levy, siege::PlayerStats::rank);
        (kingdom, rank)
    };
    kit_player(player, &kingdom, rank, true);
}

/// Re-apply team colour, leather, and keep spawn after a reconnect.
pub fn restore_player(player: &Player) {
    let uuid = player.gameprofile.id.to_string();
    let (kingdom, rank) = {
        let dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return;
        };
        let rank = dir
            .player_stats
            .get(&uuid)
            .map_or(siege::Rank::Levy, siege::PlayerStats::rank);
        (kingdom, rank)
    };
    let entity = player.get_entity();
    let _ = entity.add_scoreboard_tag(&kingdom_tag(&kingdom));
    let world = entity.world.load();
    add_to_team(&world, &kingdom, &player.gameprofile.name);
    kit_player(player, &kingdom, rank, false);
    let dir = lock();
    set_keep_respawn(player, &world, &kingdom, &dir.towns);
}

/// Record a death and kit a player who ranked up from the kill.
pub fn note_entity_death(entity_id: i32) {
    let ranked = lock().note_death(entity_id);
    let Some((uuid, rank)) = ranked else {
        return;
    };
    let worlds = lock().attached_worlds();
    for world in worlds {
        for player in world.players.load().iter() {
            if player.gameprofile.id.to_string() != uuid {
                continue;
            }
            let kingdom = lock()
                .player_kingdom(&uuid)
                .cloned()
                .unwrap_or_else(|| KingdomId::new("alder"));
            announce_rank(player, rank);
            kit_player(player, &kingdom, rank, true);
            let mut dir = lock();
            maybe_give_banner(player, &kingdom, dir.player_stats.get_mut(&uuid));
            return;
        }
    }
}

fn announce_rank(player: &Player, rank: siege::Rank) {
    let msg = TextComponent::text(format!(
        "You are now a {} (renown {}).",
        rank.as_str(),
        lock()
            .player_stats
            .get(&player.gameprofile.id.to_string())
            .map_or(0, siege::PlayerStats::renown)
    ))
    .color_named(NamedColor::Gold);
    player.send_system_message(&msg);
    tracing::info!(
        player = %player.gameprofile.name,
        rank = rank.as_str(),
        "kingdoms rank up"
    );
}

/// One server tick: alarm, targets, LOD, raid waves.
pub fn tick_world(world: &Arc<World>) {
    if world.dimension.minecraft_name != Dimension::OVERWORLD.minecraft_name {
        return;
    }
    let t0 = Instant::now();
    let seed = world.level.seed.0;
    SPAWNS_THIS_TICK.store(0, Ordering::Relaxed);
    TICKETS_THIS_TICK.store(0, Ordering::Relaxed);
    HEAVY_THIS_TICK.store(0, Ordering::Relaxed);
    REPAIR_SCANS_THIS_TICK.store(0, Ordering::Relaxed);
    let mut dir = lock();
    dir.tick = dir.tick.wrapping_add(1);
    dir.path_budget.reset();
    let tick = dir.tick;

    // Spawn cell is pre-warmed in the plan cache. Do not register every
    // neighbour or the director ticks nine idle capitals each tick.
    let ensure_started = Instant::now();
    let spawn = world.level_info.load();
    let home = town_containing(seed, spawn.spawn_x, spawn.spawn_z);
    if let Some(info) = crate::info::town_info(seed, home.cell_x, home.cell_z) {
        dir.ensure_town_from_info(info);
    }
    let cached: Vec<TownInfo> = nearby_cached_towns(seed, world);
    for info in cached {
        dir.ensure_town_from_info(info);
    }

    let town_ids: Vec<String> = dir.towns.keys().cloned().collect();
    drop(dir);
    log_slow_phase(tick, "ensure", ensure_started);
    retry_keep_teleports(world);
    drain_one_capture();

    let towns_started = Instant::now();
    for town_id in town_ids {
        let town_started = Instant::now();
        tick_one_town(world, &town_id, tick);
        log_slow_phase(tick, &format!("town:{town_id}"), town_started);
    }
    let towns_us = towns_started.elapsed().as_micros();
    log_slow_phase(tick, "towns", towns_started);
    let clock_started = Instant::now();
    maybe_war_clock(world, tick);
    log_slow_phase(tick, "clock", clock_started);

    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let dir = lock();
    let raid_on = dir.towns.values().any(|w| w.raid.is_some());
    let persist_now = dir.persist_dirty || tick.is_multiple_of(200);
    drop(dir);
    if persist_now {
        let persist_started = Instant::now();
        let mut dir = lock();
        dir.persist();
        dir.persist_dirty = false;
        drop(dir);
        log_slow_phase(tick, "persist", persist_started);
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

fn log_slow_phase(tick: u32, phase: &str, started: Instant) {
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= PHASE_WARN_MS {
        tracing::warn!("kingdoms phase tick={tick} phase={phase} ms={ms:.2}");
    }
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
        }
    }
    out
}

fn tick_one_town(world: &Arc<World>, town_id: &str, tick: u32) {
    drain_pending_work(world, town_id);
    sync_front_lod(world, town_id, tick);
    let materialized = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .is_some_and(|w| w.raid.as_ref().is_some_and(|r| r.materialized))
    };
    let near = town_has_nearby_player(world, town_id);
    if materialized {
        tick_raid_warmup(world, town_id, tick);
        if tickets_pending(town_id) || raid_still_warming(town_id, tick) {
            prewarm_town_fields(town_id);
            return;
        }
    } else {
        tick_abstract_raid(world, town_id, tick);
        // Tickets only for a scheduled raid. Warming idle towns on approach
        // was the 200–470 ms first-contact hitch (209 tickets at join).
        tick_raid_warmup(world, town_id, tick);
        if tickets_pending(town_id) {
            prewarm_town_fields(town_id);
            return;
        }
        if !near {
            prewarm_town_fields(town_id);
            return;
        }
    }
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

    let live_age = {
        let dir = lock();
        dir.towns.get(town_id).map_or(0, |w| w.live_age)
    };
    let amortizing = live_age < JOIN_AMORTIZE_TICKS;
    let mut drop_ids: Vec<i32> = Vec::new();
    let mut entities = gather_town_entities(
        world,
        town_id,
        bounds,
        &camps,
        &gates,
        materialized,
        if amortizing { 16 } else { 48 },
    );
    entities = take_combat_entities(entities, &mut drop_ids);
    if !amortizing && tick.is_multiple_of(20) && drop_ids.len() < DROPS_PER_TICK {
        sweep_nearby_drops(world, bounds, &mut drop_ids);
    }
    if !drop_ids.is_empty() {
        remove_ephemeral(world, &drop_ids);
    }

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
    let mut credit_defense: Option<(ScenePost, KingdomId)> = None;
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
                war.alarm = false;
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

            // Living ids come from spawn + [`Director::note_death`]. A spatial
            // snapshot misses brand-new bodies (not yet in the entity index)
            // and was wiping the wave, ending the raid before it marched.
            let mut finished = false;
            if let Some(raid) = &mut war.raid {
                let tickets_done = war.pending_tickets.is_empty()
                    || war.ticket_cursor >= war.pending_tickets.len();
                let warmed = tick >= raid.warm_until && tickets_done;
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
                if soaking() {
                    // Hold the soak load: a gate blender would otherwise trip
                    // "raid defeated" and dump a live front in under a minute.
                    finished = false;
                    let present = raid.living.len() + raid.ghosts.len();
                    if raid.pending_members.is_empty()
                        && raid.pending_retake == 0
                        && present < usize::from(raid.spec.raiders) / 2
                    {
                        let need = raid.spec.raiders.saturating_sub(present as u16).max(40);
                        raid.pending_members = compose_wave(need, 0);
                    }
                } else if raid.materialized
                    && raid.wave >= raid.spec.waves
                    && raid.living.is_empty()
                    && raid.ghosts.is_empty()
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
            if !war.named_npcs {
                assign_named_npcs(&snapshots);
                war.named_npcs = true;
            }
            let keep = war.scene.keep();
            let mut closest = snapshots
                .iter()
                .filter(|(_, f)| matches!(f, Faction::Pillager))
                .map(|(e, _)| {
                    let p = e.get_entity().pos.load();
                    (p.x - f64::from(keep.x)).hypot(p.z - f64::from(keep.z))
                })
                .fold(f64::INFINITY, f64::min);
            if let Some(raid) = &war.raid {
                for ghost in &raid.ghosts {
                    if matches!(ghost.faction, Faction::Pillager) {
                        closest = closest
                            .min((ghost.x - f64::from(keep.x)).hypot(ghost.z - f64::from(keep.z)));
                    }
                }
            }
            war.raider_gate_dist = closest;
            if closest + 1.0 < war.last_progress_dist {
                war.last_progress_dist = closest;
                war.last_progress_tick = tick;
            }
            if finished {
                if let Some(raid) = war.raid.take() {
                    pending_despawn.extend(raid.living);
                    pending_despawn.extend(raid.temps);
                }
                if let Some(ram) = war.ram.take() {
                    pending_despawn.extend(ram.parts);
                }
                if let Some(tower) = war.siege_tower.take() {
                    pending_despawn.extend(tower.parts);
                }
                if let Some(shot) = war.treb_shot.take()
                    && shot.part >= 0
                {
                    pending_despawn.push(shot.part);
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
                credit_defense = Some((war.scene.keep(), war.kingdom.clone()));
            }
        }
        for event in pending_events {
            dir.push_event(event);
        }
    }
    if !pending_release.is_empty() || !pending_despawn.is_empty() {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.pending_unticket.append(&mut pending_release);
            war.pending_despawn.append(&mut pending_despawn);
        }
    }
    drain_pending_work(world, town_id);
    if dump_plots && !soaking() {
        dump_live_plots(Some(town_id));
    }
    if let Some((keep, kingdom)) = credit_defense {
        credit_nearby(keep, Some(kingdom.as_str()), 0, 1);
    }
    if should_close_gates {
        let gates = {
            let dir = lock();
            dir.towns
                .get(town_id)
                .map(|w| w.scene.gates().to_vec())
                .unwrap_or_default()
        };
        let cells = siege::portcullis_cells(world, &gates, |x, z| {
            standing_y(world, ScenePost::new(x, z))
        });
        {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.pending_bars = cells;
            }
        }
        if let Some(gate) = gates.first() {
            let mut dir = lock();
            let tick = dir.tick;
            dir.push_event(CombatEvent {
                tick,
                kind: CombatKind::Portcullis,
                actor: 0,
                other: Some(gates.len() as i32),
                x: f64::from(gate.x),
                z: f64::from(gate.z),
                note: format!("{town_id} portcullis dropped"),
            });
        }
    }
    if !amortizing {
        apply_pending_bars(world, town_id);
        apply_pending_breach(world, town_id);
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
        gatehouse_rect,
        breached,
        owner_pillager,
        owner_faction,
        camp_dest,
        live_ram,
        captain_down,
        work_posts,
        home_posts,
        wall_posts,
        ladders_placed,
        inner_gate_dest,
        well_dest,
        siege_gates,
        capturing,
    ) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let dest = war.threatened.unwrap_or_else(|| war.scene.keep());
        let y = standing_y(world, dest);
        let keep = war.scene.keep();
        let flag = war.scene.flag();
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
        let mut wall_posts: Vec<ScenePost> = war
            .scene
            .wall()
            .iter()
            .map(|p| ScenePost::new(p[0], p[1]))
            .collect();
        wall_posts.extend(war.scene.archer_posts().iter().copied());
        let inner = war
            .scene
            .gates()
            .iter()
            .min_by_key(|g| (g.x - keep.x).abs() + (g.z - keep.z).abs())
            .map_or(keep, |g| ScenePost::new(g.x, g.z));
        let well = war.scene.capture_point();
        (
            war.alarm,
            PostTarget::from_post(dest, y),
            PostTarget::from_post(flag, standing_y(world, flag)),
            PostTarget::from_post(ram_post, standing_y(world, ram_post)),
            PostTarget {
                x: ladder.x,
                y: standing_y(world, ladder) + WALL_WALK_LIFT,
                z: ladder.z,
            },
            war.scene.keep_rect(),
            war.scene.gatehouse_rect(),
            war.gate_breached,
            war.owner.is_pillager(),
            owner_faction(&war.owner, &war.kingdom),
            PostTarget::from_post(camp, standing_y(world, camp)),
            war.ram.clone(),
            war.captain_down,
            war.scene.workstations().to_vec(),
            war.scene.civilian_homes().to_vec(),
            wall_posts,
            war.ladders_placed,
            PostTarget::from_post(inner, standing_y(world, inner)),
            PostTarget::from_post(well, standing_y(world, well)),
            war.scene.gates().to_vec(),
            war.occupy_hold > 0,
        )
    };
    if !amortizing {
        maybe_refill_repair(world, town_id, tick, alarm);
        place_queued_repairs(world, town_id, tick);
    }
    let work_dests = standing_dests(&work_posts, keep_dest.y, 0);
    let home_dests = standing_dests(&home_posts, keep_dest.y, 0);
    let wall_dests = standing_dests(&wall_posts, keep_dest.y, WALL_WALK_LIFT);
    let player_xz = world_player_xz(world);
    arm_militia_tick(&snapshots, alarm);
    shout_named_npcs(world, town_id, tick, alarm, inner_gate_dest);
    let squads = collect_squads(world);

    let mut n_ticking = 0u32;
    let mut n_loaded = 0u32;
    let mut n_active = 0u32;
    let mut n_no_ai = 0u32;
    let mut n_sleep = 0u32;
    let mut n_slid = 0u32;
    let mut n_melee = 0u32;
    let mut n_ranged = 0u32;
    let mut n_arrows = 0u32;
    let mut n_repair = 0u32;
    let mut squad_n = 0u32;
    let mut ram_at_gate = false;
    let mut raiders_in_keep = 0u32;
    let mut defenders_in_keep = 0u32;
    let mut attackers_on_plate = 0u32;
    let mut defenders_on_plate = 0u32;
    let mut attackers_near = 0u32;
    let mut defenders_near = 0u32;
    let mut in_courtyard_n = 0u32;
    let mut cull: Vec<(i32, GhostSoldier)> = Vec::new();
    let flag_xz = (keep_dest.x, keep_dest.z);
    let mut curtain_cross: Option<(i32, f64, f64, f64)> = None;
    let mut archer_walk: Option<(i32, f64, f64, f64)> = None;
    let mut ram_sally: Option<(i32, f64, f64)> = None;
    let mut climbed = false;
    let count_chunks = tick.is_multiple_of(100);
    let active_chunks = count_chunks.then(|| {
        world
            .active_chunks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    for (snap_i, (entity, faction)) in snapshots.iter().enumerate() {
        let base = entity.get_entity();
        let chunk = base.chunk_pos.load();
        if count_chunks {
            if world.level.is_chunk_loaded(&chunk) {
                n_loaded += 1;
            }
            if active_chunks
                .as_ref()
                .is_some_and(|chunks| chunks.contains(&chunk))
            {
                n_active += 1;
            }
        }
        if base.tick_count.load(Ordering::Relaxed) > 0 {
            n_ticking += 1;
        }
        let Some(mob) = entity.get_mob() else {
            continue;
        };
        let pos = base.pos.load();
        let role = role_from_tags(base).and_then(|s| VillagerRole::parse(&s));
        let is_builder = role == Some(VillagerRole::Builder);
        let routed = has_tag(base, TAG_ROUTED);
        if !routed {
            if on_flag_plate(flag_xz, pos.x, pos.z) {
                if is_hostile(faction, &owner_faction) {
                    attackers_on_plate += 1;
                    raiders_in_keep += 1;
                } else if is_allied(faction, &owner_faction) {
                    defenders_on_plate += 1;
                }
            }
            if near_flag(flag_xz, pos.x, pos.z) {
                if is_hostile(faction, &owner_faction) {
                    attackers_near += 1;
                } else if is_allied(faction, &owner_faction) {
                    defenders_near += 1;
                    defenders_in_keep += 1;
                }
            }
            if matches!(faction, Faction::Pillager) && in_courtyard(gatehouse_rect, pos.x, pos.z) {
                in_courtyard_n += 1;
            }
        }
        if !amortizing
            && !is_builder
            && job_of(base) != MarchJob::Ram
            && cull.len() < DESPAWNS_PER_TICK
            && !player_near_list(&player_xz, pos.x, pos.z, PLAYER_CULL_RANGE)
        {
            cull.push((
                base.entity_id,
                GhostSoldier {
                    faction: faction.clone(),
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    bearer: has_tag(base, TAG_BEARER),
                    raider: has_tag(base, TAG_RAIDER),
                },
            ));
            continue;
        }
        if amortizing && !in_join_ai_window(snap_i, live_age, snapshots.len()) {
            continue;
        }
        let offscreen = !player_near_list(&player_xz, pos.x, pos.z, CIVILIAN_SLEEP_RANGE);
        let far_from_player = !player_near_list(&player_xz, pos.x, pos.z, PLAYER_SIM_RANGE);
        let idle_villager = matches!(role, Some(VillagerRole::Civilian | VillagerRole::Guard))
            || (matches!(faction, Faction::Kingdom(_)) && role.is_none());
        if (far_from_player || (offscreen && idle_villager)) && !is_builder {
            base.no_physics.store(true, Ordering::Relaxed);
        } else {
            base.no_physics.store(false, Ordering::Relaxed);
        }
        if offscreen && idle_villager && !is_builder {
            mob.get_mob_entity().set_no_ai(true);
            mob.get_mob_entity().set_director_driven(true);
            n_sleep += 1;
            n_no_ai += 1;
            continue;
        }
        if far_from_player && !is_builder {
            mob.get_mob_entity().set_no_ai(true);
            mob.get_mob_entity().set_director_driven(true);
            n_no_ai += 1;
            n_sleep += 1;
            let far_tier = if alarm || capturing || breached {
                LodTier::Alert
            } else {
                LodTier::Far
            };
            if far_tier.should_tick(base.entity_id, tick) {
                let dest = match faction {
                    Faction::Pillager => {
                        let dest = overlay_flag_dest(
                            march_dest,
                            faction,
                            has_tag(base, TAG_BEARER),
                            pos.x,
                            pos.z,
                            keep_rect,
                            gatehouse_rect,
                            &siege_gates,
                            breached,
                            capturing,
                            keep_dest,
                        );
                        if dest.x == keep_dest.x && dest.z == keep_dest.z {
                            plate_slot(base.entity_id, dest)
                        } else {
                            dest
                        }
                    }
                    _ => march_dest,
                };
                let dist = (pos.x - f64::from(dest.x)).hypot(pos.z - f64::from(dest.z));
                if amortizing || dist <= STUCK_STORM_RANGE {
                    slide_direct(base, dest);
                } else {
                    slide_along_field(base, town_id, dest);
                }
                n_slid += 1;
            }
            continue;
        }
        let hostile = buckets.nearest_hostile(pos.x, pos.z, faction, ACQUIRE_RANGE);
        let in_combat = hostile.is_some();
        let melee_hostile = hostile.filter(|h| h.within(pos.x, pos.z, LIVE_MELEE_RANGE));
        let ranged_hostile = hostile.filter(|h| h.within(pos.x, pos.z, LIVE_RANGED_RANGE));
        let tier = if in_combat {
            LodTier::Combat
        } else if alarm {
            LodTier::Alert
        } else if offscreen {
            LodTier::Far
        } else {
            LodTier::Idle
        };
        let id = base.entity_id;
        // Kingdoms-tagged mobs never run vanilla villager/pillager brains.
        let skip = if alarm || is_builder || in_combat {
            false
        } else {
            !tier.should_tick(id, tick)
        };
        mob.get_mob_entity().set_no_ai(true);
        mob.get_mob_entity().set_director_driven(true);
        n_no_ai += 1;
        if skip {
            continue;
        }
        if !alarm {
            if let Some(h) = hostile.filter(|h| h.within(pos.x, pos.z, SET_TARGET_RANGE))
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
                (Faction::Kingdom(_), Some(VillagerRole::Civilian)) => {
                    schedule_civilian_ready(id, world_time, &work_dests, &home_dests, keep_dest)
                }
                (Faction::Kingdom(_), Some(VillagerRole::Guard)) => {
                    patrol_ready(id, tick, &wall_dests, keep_dest)
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
        } else if live_ram.as_ref().is_some_and(|r| !r.destroyed)
            && !breached
            && (job == MarchJob::Ram
                || (role == Some(VillagerRole::Guard) && id % 3 == 0 && (alarm || in_combat)))
        {
            ram_dest
        } else {
            let needs_schedule = !alarm
                && matches!(faction, Faction::Kingdom(_))
                && matches!(role, Some(VillagerRole::Civilian | VillagerRole::Guard));
            let (work_d, home_d, patrol_d) = if needs_schedule {
                (
                    schedule_civilian_ready(id, world_time, &work_dests, &home_dests, keep_dest),
                    pick_ready_dest(id, &home_dests, keep_dest),
                    patrol_ready(id, tick, &wall_dests, keep_dest),
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
                capturing,
                repair_dest,
                if owner_pillager {
                    keep_dest
                } else {
                    march_dest
                },
                keep_dest,
                ladder_dest,
                pick_ready_dest(id, &wall_dests, ladder_dest),
                work_d,
                home_d,
                patrol_d,
                world_time,
            )
        };
        let dest = overlay_flag_dest(
            dest,
            faction,
            has_tag(base, TAG_BEARER),
            pos.x,
            pos.z,
            keep_rect,
            gatehouse_rect,
            &siege_gates,
            breached,
            capturing,
            keep_dest,
        );
        let dest = if matches!(faction, Faction::Pillager)
            && dest.x == keep_dest.x
            && dest.z == keep_dest.z
        {
            plate_slot(id, dest)
        } else {
            dest
        };
        let dest = overlay_civilian_posts(
            dest,
            role,
            alarm,
            capturing,
            inner_gate_dest,
            well_dest,
            keep_dest,
            civilian_duty(base, id),
        );
        let squad_dest = overlay_squad_order(dest, faction, pos.x, pos.z, &squads, march_dest);
        if squad_dest != dest {
            squad_n += 1;
        }
        let dest = squad_dest;
        if matches!(faction, Faction::Pillager) && ladders_placed {
            let near_ladders =
                (pos.x - f64::from(ladder_dest.x)).hypot(pos.z - f64::from(ladder_dest.z)) < 10.0;
            if !climbed && near_ladders && !in_courtyard(keep_rect, pos.x, pos.z) {
                let nx = f64::from(keep_rect.min_x + keep_rect.width() / 2) + 0.5;
                let ny = f64::from(ladder_dest.y.max(keep_dest.y));
                let nz = f64::from(keep_rect.min_z + keep_rect.depth() / 2) + 0.5;
                base.set_pos(Vector3::new(nx, ny, nz));
                climbed = true;
                curtain_cross.get_or_insert((id, nx, ny, nz));
            }
        }
        if matches!(faction, Faction::Pillager)
            && (in_courtyard(keep_rect, pos.x, pos.z)
                || (pos.x - f64::from(keep_dest.x)).hypot(pos.z - f64::from(keep_dest.z)) < 24.0)
        {
            curtain_cross.get_or_insert((id, pos.x, pos.y, pos.z));
        }
        if role == Some(VillagerRole::Archer) && dest.y >= keep_dest.y + WALL_WALK_LIFT - 1 {
            archer_walk.get_or_insert((id, pos.x, pos.y, pos.z));
        }
        if role == Some(VillagerRole::Guard)
            && live_ram.as_ref().is_some_and(|r| !r.destroyed)
            && (dest.x == ram_dest.x && dest.z == ram_dest.z || id % 3 == 0)
        {
            ram_sally.get_or_insert((id, pos.x, pos.z));
        }
        let dest_v = dest.vec();
        let dist = (pos.x - dest_v.x).hypot(pos.z - dest_v.z);
        let period = slide_period(dist, in_combat);
        let follower = !in_combat && !is_squad_leader(id);
        // Followers refresh every 24 ticks (~0.2 blk/s), too slow to cross a
        // two-ring capital before the soak ends. Raiders use the alarm period.
        let due = if is_builder && repair_dest.is_some() {
            true
        } else if matches!(faction, Faction::Pillager) && alarm {
            tick.is_multiple_of(period)
        } else if follower {
            should_refresh_path(id, tick as i32, dist < DIRECT_RANGE)
        } else if alarm {
            tick.is_multiple_of(period)
        } else {
            dist > DIRECT_RANGE || base.tick_count.load(Ordering::Relaxed) == 0
        };
        if due {
            if amortizing
                || (follower && dist < DIRECT_RANGE)
                || (matches!(faction, Faction::Pillager) && dist <= STUCK_STORM_RANGE)
                || (is_builder && repair_dest.is_some())
            {
                slide_direct(base, dest);
            } else {
                slide_along_field(base, town_id, dest);
            }
            n_slid += 1;
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
    tick_ghosts(
        world,
        town_id,
        tick,
        if amortizing {
            &[]
        } else {
            player_xz.as_slice()
        },
        march_dest,
        keep_dest,
        keep_rect,
        gatehouse_rect,
        &siege_gates,
        breached,
        capturing,
        &owner_faction,
        &mut attackers_on_plate,
        &mut defenders_on_plate,
        &mut attackers_near,
        &mut defenders_near,
        &mut raiders_in_keep,
        &mut defenders_in_keep,
        &mut in_courtyard_n,
    );
    let loop_us = loop_started.elapsed().as_micros();
    log_slow_phase(tick, "entities", loop_started);
    if curtain_cross.is_some() || archer_walk.is_some() || ram_sally.is_some() {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            if let Some((id, x, y, z)) = curtain_cross
                && !war.crossed_curtain
            {
                war.crossed_curtain = true;
                tracing::info!(
                    "kingdoms raider crossed curtain id={id} x={x:.1} y={y:.1} z={z:.1}"
                );
            }
            if let Some((id, x, y, z)) = archer_walk
                && !war.logged_archer
            {
                war.logged_archer = true;
                tracing::info!("kingdoms archer on wall walk id={id} x={x:.1} y={y:.1} z={z:.1}");
            }
            if let Some((id, x, z)) = ram_sally
                && !war.logged_sally
            {
                war.logged_sally = true;
                tracing::info!("kingdoms ram sally id={id} x={x:.1} z={z:.1}");
            }
        }
    }
    tick_moving_ram(world, town_id, tick, &snapshots, breached, &mut ram_at_gate);
    if amortizing {
        prewarm_siege_columns(world, town_id);
        continue_engine_spawns(world, town_id);
    }
    let run_heavy = !amortizing && take_heavy_town();
    if run_heavy {
        let engines_started = Instant::now();
        tick_siege_engines(world, town_id, tick, alarm);
        log_slow_phase(tick, "engines", engines_started);
    }
    let ghosts = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .and_then(|w| w.raid.as_ref())
            .map_or_else(Vec::new, |r| r.ghosts.clone())
    };
    let mut stormed = packed_storm_gate(
        &snapshots,
        &ghosts,
        &siege_gates,
        STORMING_RANGE,
        STORMING_COUNT,
    );
    let last_progress = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .map_or(tick, |w| w.last_progress_tick)
    };
    if stormed.is_none() && tick.saturating_sub(last_progress) >= STUCK_TICKS {
        stormed = packed_storm_gate(
            &snapshots,
            &ghosts,
            &siege_gates,
            STUCK_STORM_RANGE,
            STUCK_STORM_COUNT,
        )
        .or_else(|| nearest_gate_to_raiders(&snapshots, &ghosts, &siege_gates));
    }
    let storming = stormed.is_some();
    if ram_at_gate || storming {
        let ram_gate = {
            let dir = lock();
            dir.towns.get(town_id).and_then(|w| {
                w.threatened
                    .or_else(|| w.scene.gates().first().map(|g| ScenePost::new(g.x, g.z)))
                    .and_then(|post| gate_info_near(&siege_gates, post).map(|g| (post, g)))
            })
        };
        let gate = stormed.map(|g| (ScenePost::new(g.x, g.z), g)).or(ram_gate);
        let cells = if let Some((post, gate)) = gate {
            siege::gate_bar_cells(world, gate, standing_y(world, post))
        } else {
            Vec::new()
        };
        let n = cells.len();
        {
            let mut dir = lock();
            let fire = if let Some(war) = dir.towns.get_mut(town_id) {
                let already = war.gate_breached;
                war.gate_breached = true;
                let fresh = cells.iter().any(|c| !war.pending_breach.contains(c));
                if fresh {
                    war.pending_breach.extend(cells);
                }
                !already || fresh
            } else {
                false
            };
            if let Some((_, gate)) = gate
                && fire
                && (n > 0 || storming)
            {
                dir.push_event(CombatEvent {
                    tick,
                    kind: CombatKind::Breach,
                    actor: 0,
                    other: Some(n as i32),
                    x: f64::from(gate.x),
                    z: f64::from(gate.z),
                    note: if ram_at_gate && stormed.is_none() {
                        format!("{town_id} ram breached gate")
                    } else {
                        format!("{town_id} raiders stormed the gate")
                    },
                });
            }
        }
    }
    apply_cull(town_id, &cull);
    apply_morale(town_id, tick, &snapshots, captain_down, breached);
    {
        let dir = lock();
        for p in world.players.load().iter() {
            let pos = p.living_entity.entity.pos.load();
            let Some(k) = dir.player_kingdom(&p.gameprofile.id.to_string()) else {
                continue;
            };
            let fac = Faction::Kingdom(k.clone());
            if on_flag_plate(flag_xz, pos.x, pos.z) {
                if is_hostile(&fac, &owner_faction) {
                    attackers_on_plate += 1;
                    raiders_in_keep += 1;
                } else if is_allied(&fac, &owner_faction) {
                    defenders_on_plate += 1;
                }
            }
            if near_flag(flag_xz, pos.x, pos.z) {
                if is_hostile(&fac, &owner_faction) {
                    attackers_near += 1;
                } else if is_allied(&fac, &owner_faction) {
                    defenders_near += 1;
                    defenders_in_keep += 1;
                }
            }
        }
    }
    let presence = FlagPresence {
        attackers_on_plate,
        defenders_on_plate,
        attackers_near,
        defenders_near,
    };
    apply_capture(
        town_id,
        tick,
        presence,
        occupier_from_flag(&snapshots, flag_xz, &owner_faction),
        1,
    );
    update_capture_meter(
        world,
        town_id,
        keep_dest,
        presence.attackers_on_plate,
        presence.defenders_near,
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
    if squad_n > 0 && tick.is_multiple_of(20) {
        let order = squads.first().map_or("none", |s| s.order.as_str());
        tracing::info!(
            town = town_id,
            order,
            soldiers = squad_n,
            "kingdoms squad soldiers"
        );
    }
    if tick.is_multiple_of(100) && (alarm || n_repair > 0 || n_arrows > 0) {
        let n_entities = snapshots.len();
        let (ghost_n, flag_d, last_prog, placed, started) = {
            let dir = lock();
            dir.towns
                .get(town_id)
                .map_or((0, f64::INFINITY, 0, 0, 0), |w| {
                    (
                        w.raid.as_ref().map_or(0, |r| r.ghosts.len()),
                        w.raider_gate_dist,
                        w.last_progress_tick,
                        w.repair_placed,
                        w.repair_started,
                    )
                })
        };
        let reason = raid_outcome_reason(
            attackers_on_plate,
            in_courtyard_n,
            storming,
            snapshots
                .iter()
                .filter(|(_, f)| matches!(f, Faction::Pillager))
                .count() as u32
                + ghost_n as u32,
            tick,
            last_prog,
        );
        tracing::info!(
            "kingdoms raid entity tick town={town_id} ticking={n_ticking} loaded={n_loaded} active={n_active} no_ai={n_no_ai} sleep={n_sleep} slid={n_slid} melee={n_melee} ranged={n_ranged} arrows={n_arrows} repair={n_repair} loop_us={loop_us} keep_r={raiders_in_keep} keep_d={defenders_in_keep} entities={n_entities} ghosts={ghost_n}"
        );
        tracing::info!(
            "kingdoms raid outcome town={town_id} reason={reason} plate_r={attackers_on_plate} courtyard={in_courtyard_n} storming={storming} flag_d={flag_d:.1} ghosts={ghost_n}"
        );
        if placed > 0 {
            let elapsed = tick.saturating_sub(started).max(1);
            let per_min = f64::from(placed) * 1200.0 / f64::from(elapsed);
            tracing::info!(
                "kingdoms repair rate town={town_id} placed={placed} per_min={per_min:.1}"
            );
        }
    }

    if spawn_soldiers {
        spawn_soldiers_this_tick(world, town_id, tick);
    }
    maybe_soldier_chatter(world, town_id, tick);

    if !amortizing && tick.is_multiple_of(RESPAWN_PERIOD) {
        crate::populate::respawn_missing(world, town_id);
    }
    {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.live_age = war.live_age.saturating_add(1);
        }
    }
}

fn take_spawn_slots(want: u32) -> u32 {
    if want == 0 {
        return 0;
    }
    loop {
        let used = SPAWNS_THIS_TICK.load(Ordering::Relaxed);
        if used >= SPAWNS_PER_WORLD_TICK {
            return 0;
        }
        let take = want.min(SPAWNS_PER_WORLD_TICK - used);
        if SPAWNS_THIS_TICK
            .compare_exchange(used, used + take, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return take;
        }
    }
}

fn spawn_soldiers_this_tick(world: &Arc<World>, town_id: &str, tick: u32) {
    // Far wave members become ghosts even when the live body budget is full.
    spawn_wave_batch(world, town_id, tick);
    if lock().live_body_count() >= body_budget() {
        return;
    }
    // Raiders first so extras cannot spend the whole world spawn budget and
    // leave the wave empty (which used to trip "raid defeated").
    let extra = lock()
        .towns
        .get(town_id)
        .and_then(|w| w.raid.as_ref())
        .map_or(0, |r| r.extra_left);
    if extra > 0 {
        let n = take_spawn_slots(SPAWNS_PER_TICK as u32);
        if n > 0 {
            spawn_extra_defenders(world, town_id, n as u16);
        }
    }
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
    let spawn_tower = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .and_then(|w| w.raid.as_ref())
            .is_some_and(|r| wave == 0 && !r.tower_spawned)
    };
    if spawn_tower {
        if take_spawn_slots(1) == 0 {
            return;
        }
        let origin = ladders.first().copied().unwrap_or(camp);
        let y = f64::from(standing_y(world, origin));
        let mut tower = engines::empty_tower(Vector3::new(
            f64::from(origin.x) + 0.5,
            y,
            f64::from(origin.z) + 0.5,
        ));
        engines::append_tower_parts(world, &mut tower, engines::ENGINE_PARTS_PER_TICK);
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.siege_tower = Some(tower);
            if let Some(raid) = &mut war.raid {
                raid.tower_spawned = true;
            }
        }
        return;
    }
    let spawn_ram = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .and_then(|w| w.raid.as_ref())
            .is_some_and(|r| wave == 0 && !r.ram_spawned)
            && !rams.is_empty()
    };
    if spawn_ram {
        if take_spawn_slots(1) == 0 {
            return;
        }
        let origin = rams[0];
        let y = f64::from(standing_y(world, origin));
        let mut ram = crate::ram::empty_ram(Vector3::new(
            f64::from(origin.x) + 0.5,
            y,
            f64::from(origin.z) + 0.5,
        ));
        crate::ram::append_ram_parts(world, &mut ram, crate::ram::RAM_PARTS_PER_TICK);
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ram = Some(ram);
            if let Some(raid) = &mut war.raid {
                raid.ram_spawned = true;
            }
        }
        return;
    }
    if lock().live_body_count() >= body_budget() {
        return;
    }
    let players = world_player_xz(world);
    let pull = SPAWNS_PER_TICK;
    let batch: Vec<(usize, WaveMember)> = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = &mut war.raid else {
            return;
        };
        let n = pull.min(raid.pending_members.len());
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
    let camp_y = standing_y(world, camp);
    let mut remaining = body_budget().saturating_sub(lock().live_body_count());
    let mut spawned_ids = Vec::new();
    let mut new_ghosts = Vec::new();
    let mut leftover = Vec::new();
    let mut live_jobs = Vec::new();
    for (i, member) in batch {
        let vindicator = member.entity_type == &EntityType::VINDICATOR;
        let job = MarchJob::assign(i, vindicator);
        let post = match job {
            MarchJob::Ram if !rams.is_empty() => rams[i % rams.len()],
            MarchJob::Ladder if !ladders.is_empty() => ladders[i % ladders.len()],
            _ => tents[i % tents.len()],
        };
        let y = if job == MarchJob::Ram || job == MarchJob::Ladder {
            standing_y(world, post)
        } else {
            camp_y
        };
        let ring = (i / tents.len().max(1)) as u16;
        let pos = spawn_offset(post, ring, y);
        if job != MarchJob::Ram && should_virtualize(&players, pos.x, pos.z) {
            if remaining == 0 {
                leftover.push(member);
            } else {
                new_ghosts.push(GhostSoldier {
                    faction: Faction::Pillager,
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                    bearer: member.captain,
                    raider: true,
                });
                remaining -= 1;
            }
        } else {
            live_jobs.push((member, pos, job, post));
        }
    }
    let slots =
        take_spawn_slots(live_jobs.len().min(SPAWNS_PER_TICK).min(remaining) as u32) as usize;
    for (n, (member, pos, job, post)) in live_jobs.into_iter().enumerate() {
        if n >= slots {
            leftover.push(member);
            continue;
        }
        let vindicator = member.entity_type == &EntityType::VINDICATOR;
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
        } else {
            leftover.push(member);
        }
    }
    let mut dir = lock();
    let mut wave_done = None;
    if let Some(war) = dir.towns.get_mut(town_id)
        && let Some(raid) = &mut war.raid
        && raid.id == raid_id
    {
        leftover.append(&mut raid.pending_members);
        raid.pending_members = leftover;
        let added = spawned_ids.len() + new_ghosts.len();
        raid.spawn_cursor = raid.spawn_cursor.saturating_add(added as u16);
        raid.living.extend(spawned_ids.iter().copied());
        raid.ghosts.extend(new_ghosts);
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
    let spawned = world.spawn_entity(base);
    if spawned && role == VillagerRole::Builder {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id)
            && !war.builders.contains(&id)
        {
            war.builders.push(id);
        }
    }
    spawned.then_some(id)
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
    queue_raid_warmup(town_id);
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

fn queue_raid_warmup(town_id: &str) {
    let Some((chunks, camp, keep)) = raid_chunk_list(town_id) else {
        return;
    };
    let n = chunks.len();
    {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            if !war.pending_tickets.is_empty() || !war.ticketed.is_empty() {
                return;
            }
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
}

fn tickets_pending(town_id: &str) -> bool {
    let dir = lock();
    dir.towns
        .get(town_id)
        .is_some_and(|w| w.ticket_cursor < w.pending_tickets.len())
}

fn raid_still_warming(town_id: &str, tick: u32) -> bool {
    let dir = lock();
    dir.towns
        .get(town_id)
        .is_some_and(|w| w.raid.as_ref().is_some_and(|r| tick < r.warm_until))
}

fn take_ticket_slots(want: u32) -> u32 {
    if want == 0 {
        return 0;
    }
    loop {
        let used = TICKETS_THIS_TICK.load(Ordering::Relaxed);
        if used >= TICKETS_PER_WORLD_TICK {
            return 0;
        }
        let take = want.min(TICKETS_PER_WORLD_TICK - used);
        if TICKETS_THIS_TICK
            .compare_exchange(used, used + take, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return take;
        }
    }
}

fn take_heavy_town() -> bool {
    loop {
        let used = HEAVY_THIS_TICK.load(Ordering::Relaxed);
        if used >= HEAVY_TOWNS_PER_TICK {
            return false;
        }
        if HEAVY_THIS_TICK
            .compare_exchange(used, used + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

fn is_ephemeral_drop(ty: &EntityType) -> bool {
    ty == &EntityType::ITEM || ty == &EntityType::EXPERIENCE_ORB
}

fn take_combat_entities(
    entities: Vec<Arc<dyn EntityBase>>,
    drop_ids: &mut Vec<i32>,
) -> Vec<Arc<dyn EntityBase>> {
    let mut out = Vec::with_capacity(entities.len());
    for entity in entities {
        if is_ephemeral_drop(entity.get_entity().entity_type) {
            if drop_ids.len() < DROPS_PER_TICK {
                drop_ids.push(entity.get_entity().entity_id);
            }
            continue;
        }
        out.push(entity);
    }
    out
}

fn raid_snapshot_entities(world: &World, town_id: &str) -> Vec<Arc<dyn EntityBase>> {
    let ids = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        if let Some(raid) = &war.raid {
            ids.extend(raid.living.iter().copied());
            ids.extend(raid.temps.iter().copied());
        }
        ids.extend(war.wired.keys().copied());
        ids.extend(war.builders.iter().copied());
        ids
    };
    entities_from_ids(world, ids)
}

fn gather_town_entities(
    world: &World,
    town_id: &str,
    bounds: crate::geom::Rect,
    camps: &[ScenePost],
    gates: &[ScenePost],
    materialized: bool,
    pad: i32,
) -> Vec<Arc<dyn EntityBase>> {
    if materialized {
        return raid_snapshot_entities(world, town_id);
    }
    let aabb = town_search_aabb(bounds, camps, gates, pad);
    let cursor = take_snapshot_cursor(town_id);
    // One X-strip per tick so the capital's first live query is a slice, not
    // the whole town AABB. Accumulated ids cover the town after one cycle.
    let strip = aabb_x_strip(&aabb, cursor, JOIN_AMORTIZE_TICKS);
    let batch = world.get_all_at_box(&strip);
    let ids = merge_snapshot_ids(town_id, &batch);
    entities_from_ids(world, ids)
}

fn prewarm_town_fields(town_id: &str) {
    let mut dir = lock();
    let Some(war) = dir.towns.get_mut(town_id) else {
        return;
    };
    if !war.fields.is_empty() {
        return;
    }
    let dest = war.scene.flag();
    let _ = war.field_toward(dest);
}

fn take_snapshot_cursor(town_id: &str) -> u32 {
    let mut dir = lock();
    let Some(war) = dir.towns.get_mut(town_id) else {
        return 0;
    };
    let cursor = war.snapshot_cursor;
    war.snapshot_cursor = war.snapshot_cursor.wrapping_add(1);
    cursor
}

fn town_search_aabb(
    bounds: crate::geom::Rect,
    camps: &[ScenePost],
    gates: &[ScenePost],
    pad: i32,
) -> BoundingBox {
    let mut min_x = bounds.min_x - pad;
    let mut min_z = bounds.min_z - pad;
    let mut max_x = bounds.max_x + pad;
    let mut max_z = bounds.max_z + pad;
    for p in camps.iter().chain(gates.iter()) {
        min_x = min_x.min(p.x - pad);
        min_z = min_z.min(p.z - pad);
        max_x = max_x.max(p.x + pad);
        max_z = max_z.max(p.z + pad);
    }
    BoundingBox::new(
        Vector3::new(f64::from(min_x), -64.0, f64::from(min_z)),
        Vector3::new(f64::from(max_x), 320.0, f64::from(max_z)),
    )
}

fn aabb_x_strip(aabb: &BoundingBox, age: u32, n: u32) -> BoundingBox {
    let n = n.max(1);
    let width = (aabb.max.x - aabb.min.x).max(1.0);
    let i = f64::from(age % n);
    let strips = f64::from(n);
    let x0 = aabb.min.x + width * i / strips;
    let x1 = aabb.min.x + width * (i + 1.0) / strips;
    BoundingBox::new(
        Vector3::new(x0, aabb.min.y, aabb.min.z),
        Vector3::new(x1, aabb.max.y, aabb.max.z),
    )
}

fn merge_snapshot_ids(town_id: &str, batch: &[Arc<dyn EntityBase>]) -> Vec<i32> {
    let mut dir = lock();
    let Some(war) = dir.towns.get_mut(town_id) else {
        return Vec::new();
    };
    for entity in batch {
        war.snapshot_ids.insert(entity.get_entity().entity_id);
    }
    war.snapshot_ids.iter().copied().collect()
}

fn entities_from_ids(
    world: &World,
    ids: impl IntoIterator<Item = i32>,
) -> Vec<Arc<dyn EntityBase>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if !seen.insert(id) {
            continue;
        }
        if let Some(entity) = world.get_entity_by_id(id) {
            out.push(entity);
        }
    }
    out
}

fn sweep_nearby_drops(world: &World, bounds: crate::geom::Rect, drop_ids: &mut Vec<i32>) {
    if drop_ids.len() >= DROPS_PER_TICK {
        return;
    }
    let (cx, cz) = bounds.center();
    let aabb = BoundingBox::new(
        Vector3::new(f64::from(cx - 24), -64.0, f64::from(cz - 24)),
        Vector3::new(f64::from(cx + 24), 320.0, f64::from(cz + 24)),
    );
    for entity in world.get_all_at_box(&aabb) {
        if drop_ids.len() >= DROPS_PER_TICK {
            break;
        }
        if is_ephemeral_drop(entity.get_entity().entity_type) {
            drop_ids.push(entity.get_entity().entity_id);
        }
    }
}

fn remove_ephemeral(world: &World, ids: &[i32]) {
    if ids.is_empty() {
        return;
    }
    let mut n = 0u32;
    let mut seen = HashSet::new();
    for id in ids {
        if !seen.insert(*id) {
            continue;
        }
        if let Some(entity) = world.get_entity_by_id(*id) {
            entity.get_entity().remove();
            n += 1;
        }
    }
    if n > 0 {
        tracing::debug!(despawned = n, "kingdoms drops cleared");
    }
}

fn tick_raid_warmup(world: &Arc<World>, town_id: &str, tick: u32) {
    let has_work = {
        let dir = lock();
        dir.towns
            .get(town_id)
            .is_some_and(|w| w.ticket_cursor < w.pending_tickets.len())
    };
    if !has_work {
        return;
    }
    let slots = take_ticket_slots(TICKETS_PER_TICK as u32) as usize;
    if slots == 0 {
        return;
    }
    let (batch, remaining, camp, keep) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if war.ticket_cursor >= war.pending_tickets.len() {
            return;
        }
        let start = war.ticket_cursor;
        let end = (start + slots).min(war.pending_tickets.len());
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
    flush_tickets_off_tick(world, batch.clone(), true);
    let n = batch.len();
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
}

fn apply_force_tickets(world: &World, chunks: &[Vector2<i32>], add: bool) {
    if chunks.is_empty() {
        return;
    }
    {
        let mut forced = world
            .forced_chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in chunks {
            if add {
                forced.insert(*pos);
            } else {
                forced.remove(pos);
            }
        }
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
}

/// Apply tickets, `send_change`, and fetch chunks off the director tick.
/// Locking `chunk_loading` on the tick was the 200–470 ms first-contact hitch.
fn flush_tickets_off_tick(world: &Arc<World>, chunks: Vec<Vector2<i32>>, add: bool) {
    if chunks.is_empty() {
        return;
    }
    let loaded_world = Arc::clone(world);
    world.level.spawn_task(async move {
        apply_force_tickets(&loaded_world, &chunks, add);
        send_ticket_change(&loaded_world.level);
        if add {
            for pos in &chunks {
                loaded_world.level.get_or_fetch_chunk(*pos, |_| ()).await;
            }
        }
        loaded_world.update_active_chunks();
    });
}

fn send_ticket_change(level: &pumpkin_world::level::Level) {
    let mut loading = level
        .chunk_loading
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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

fn slide_direct(entity: &Entity, dest: PostTarget) {
    let pos = entity.pos.load();
    let dx = f64::from(dest.x) + 0.5 - pos.x;
    let dz = f64::from(dest.z) + 0.5 - pos.z;
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

struct RepairScanOut {
    town_id: String,
    jobs: Vec<RepairJob>,
    next_cursor: usize,
}

fn repair_scan_slot() -> &'static Mutex<Option<RepairScanOut>> {
    static SLOT: OnceLock<Mutex<Option<RepairScanOut>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn apply_finished_repair_scan() {
    let out = repair_scan_slot()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(out) = out else {
        return;
    };
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(&out.town_id) {
        war.repair_scan_cursor = out.next_cursor;
        if !out.jobs.is_empty() {
            war.repair_jobs = out.jobs.into_iter().collect();
            tracing::info!(
                town = out.town_id.as_str(),
                queued = war.repair_jobs.len(),
                "kingdoms builders queued repair jobs"
            );
        }
    }
}

fn remember_builders(world: &World, town_id: &str) {
    let posts = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        if war.builders_scanned {
            return;
        }
        let mut posts = vec![war.scene.keep()];
        posts.extend(war.scene.civilian_homes().iter().copied().take(8));
        posts
    };
    let mut ids = Vec::new();
    for post in posts {
        let aabb = BoundingBox::new(
            Vector3::new(f64::from(post.x) - 16.0, -64.0, f64::from(post.z) - 16.0),
            Vector3::new(f64::from(post.x) + 16.0, 320.0, f64::from(post.z) + 16.0),
        );
        for entity in world.get_all_at_box(&aabb) {
            let base = entity.get_entity();
            if role_from_tags(base).and_then(|s| VillagerRole::parse(&s))
                == Some(VillagerRole::Builder)
            {
                ids.push(base.entity_id);
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        war.builders_scanned = true;
        war.builders = ids;
        tracing::info!(
            town = town_id,
            n = war.builders.len(),
            "kingdoms builders remembered"
        );
    }
}

fn place_queued_repairs(world: &Arc<World>, town_id: &str, tick: u32) {
    let in_window = {
        let dir = lock();
        dir.towns.get(town_id).is_some_and(|w| {
            tick >= w.hold_lockout && tick < w.repair_until.max(w.hold_lockout.saturating_add(1))
        })
    };
    if !in_window {
        return;
    }
    for _ in 0..REPAIR_PLACE_PER_TICK {
        let job = {
            let mut dir = lock();
            let Some(war) = dir.towns.get_mut(town_id) else {
                return;
            };
            if war.repair_placed_tick != tick {
                war.repair_placed_tick = tick;
                war.repair_placed_this_tick = 0;
            }
            if war.repair_placed_this_tick >= REPAIR_PLACE_PER_TICK {
                return;
            }
            war.repair_jobs.pop_front()
        };
        let Some(job) = job else {
            break;
        };
        if !repair::place_one(world, job) {
            continue;
        }
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            if war.repair_started == 0 {
                war.repair_started = tick;
            }
            war.repair_placed = war.repair_placed.saturating_add(1);
            war.repair_placed_this_tick = war.repair_placed_this_tick.saturating_add(1);
            war.last_repair_tick = tick;
        }
    }
}

fn maybe_refill_repair(world: &Arc<World>, town_id: &str, tick: u32, _alarm: bool) {
    apply_finished_repair_scan();
    let in_window = {
        let dir = lock();
        dir.towns.get(town_id).is_some_and(|w| {
            tick >= w.hold_lockout && tick < w.repair_until.max(w.hold_lockout.saturating_add(1))
        })
    };
    // Only the post-flip window. Idle towns used to consume the one scan
    // slot per tick and starve the town that actually needed repair.
    if !in_window || !tick.is_multiple_of(REPAIR_PERIOD) {
        return;
    }
    remember_builders(world, town_id);
    let (scene, threatened, queued, cursor, busy) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        (
            war.scene.clone(),
            war.threatened.map(|p| (p.x, p.z)),
            war.repair_jobs.len() + war.repair_assigned.len(),
            war.repair_scan_cursor,
            // Cull despawns every tick while a soak raid is held open; that
            // must not starve the post-flip repair window.
            !in_window && (!war.pending_despawn.is_empty() || !war.pending_unticket.is_empty()),
        )
    };
    if busy || queued >= REPAIR_QUEUE_CAP {
        return;
    }
    if REPAIR_SCANS_THIS_TICK.fetch_add(1, Ordering::Relaxed) >= 1 {
        return;
    }
    if REPAIR_SCAN_BUSY.load(Ordering::Relaxed) {
        return;
    }
    let Some((_, cx, cz)) = parse_town_id(town_id) else {
        return;
    };
    let seed = world.level.seed.0;
    if crate::plan::cached_plan(seed, cx, cz).is_none() {
        crate::plan::request_plan(seed, cx, cz);
        return;
    }
    let cols = repair::repair_columns(&scene, threatened);
    if cols.is_empty() {
        return;
    }
    let start = cursor % cols.len();
    let slice: Vec<_> = cols
        .iter()
        .cycle()
        .skip(start)
        .take(REPAIR_SCAN_COLUMNS)
        .copied()
        .collect();
    let next_cursor = start.saturating_add(REPAIR_SCAN_COLUMNS);
    let town = town_id.to_owned();
    let world = Arc::clone(world);
    REPAIR_SCAN_BUSY.store(true, Ordering::Relaxed);
    let scan = move || {
        let jobs = crate::plan::cached_plan(seed, cx, cz).map_or_else(Vec::new, |plan| {
            repair::collect_jobs(&world, &plan, &slice, REPAIR_QUEUE_CAP)
        });
        *repair_scan_slot()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(RepairScanOut {
            town_id: town,
            jobs,
            next_cursor,
        });
        REPAIR_SCAN_BUSY.store(false, Ordering::Relaxed);
    };
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::spawn_blocking(scan);
    } else {
        scan();
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
        if war.repair_placed_tick != tick {
            war.repair_placed_tick = tick;
            war.repair_placed_this_tick = 0;
        }
        if war.repair_placed_this_tick >= REPAIR_PLACE_PER_TICK {
            return false;
        }
        let Some(job) = war.repair_assigned.remove(&entity_id) else {
            return false;
        };
        war.repair_placed_this_tick += 1;
        war.last_repair_tick = tick;
        job
    };
    if repair::place_one(world, job) {
        {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                if war.repair_started == 0 {
                    war.repair_started = tick;
                }
                war.repair_placed = war.repair_placed.saturating_add(1);
            }
        }
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
    let batch = n.min(take_spawn_slots(SPAWNS_PER_TICK as u32) as u16);
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

fn drain_pending_work(world: &Arc<World>, town_id: &str) {
    let (despawn, tickets) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let n = DESPAWNS_PER_TICK.min(war.pending_despawn.len());
        let despawn: Vec<i32> = war.pending_despawn.drain(..n).collect();
        let t = TICKETS_PER_TICK.min(war.pending_unticket.len());
        let tickets: Vec<Vector2<i32>> = war.pending_unticket.drain(..t).collect();
        (despawn, tickets)
    };
    despawn_entities(world, &despawn);
    flush_tickets_off_tick(world, tickets, false);
}

fn credit_nearby(keep: ScenePost, kingdom: Option<&str>, captures: u32, defenses: u32) {
    let worlds = lock().attached_worlds();
    let mut ranked_up: Vec<(Arc<Player>, KingdomId, siege::Rank)> = Vec::new();
    {
        let mut dir = lock();
        for world in &worlds {
            for player in world.players.load().iter() {
                let pos = player.living_entity.entity.pos.load();
                if (pos.x - f64::from(keep.x)).hypot(pos.z - f64::from(keep.z)) > CREDIT_RANGE {
                    continue;
                }
                let uuid = player.gameprofile.id.to_string();
                let Some(pk) = dir.player_kingdoms.get(&uuid).cloned() else {
                    continue;
                };
                if let Some(want) = kingdom
                    && pk.as_str() != want
                {
                    continue;
                }
                let (before, after) = {
                    let stats = dir.player_stats.entry(uuid).or_default();
                    let before = stats.rank();
                    stats.captures = stats.captures.saturating_add(captures);
                    stats.defenses = stats.defenses.saturating_add(defenses);
                    (before, stats.rank())
                };
                dir.persist_dirty = true;
                if after != before {
                    ranked_up.push((Arc::clone(player), pk, after));
                }
            }
        }
    }
    for (player, kingdom, rank) in ranked_up {
        announce_rank(&player, rank);
        kit_player(&player, &kingdom, rank, true);
        let uuid = player.gameprofile.id.to_string();
        let mut dir = lock();
        maybe_give_banner(&player, &kingdom, dir.player_stats.get_mut(&uuid));
    }
}

fn maybe_soldier_chatter(world: &World, town_id: &str, tick: u32) {
    let (alarm, gate_breached, occupy, captain_down, keep, kingdom) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        if tick.saturating_sub(dir.last_any_shout) < GLOBAL_CHAT_GAP {
            return;
        }
        (
            war.alarm,
            war.gate_breached,
            war.occupy_hold,
            war.captain_down,
            war.scene.keep(),
            war.kingdom.clone(),
        )
    };
    if !alarm {
        return;
    }
    let hash = town_id
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    if tick % 80 != hash % 80 {
        return;
    }
    let kind = if captain_down {
        ShoutKind::Fallen
    } else if occupy > 0 {
        ShoutKind::Charge
    } else if gate_breached {
        ShoutKind::Hold
    } else {
        ShoutKind::Rally
    };
    let mut dir = lock();
    let hero = dir.heroes.get(kingdom.as_str()).cloned();
    let speaker = hero
        .as_ref()
        .map(|h| h.name.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("soldier")
        .to_owned();
    let spoken = hero.as_ref().and_then(|h| {
        let line = h.line.trim();
        if line.is_empty() {
            None
        } else {
            Some(line.to_owned())
        }
    });
    if chat::try_shout_as(
        world,
        tick,
        town_id,
        kind,
        f64::from(keep.x),
        f64::from(keep.z),
        &mut dir.last_shout,
        &speaker,
        spoken.as_deref(),
    ) {
        dir.last_any_shout = tick;
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

fn continue_engine_spawns(world: &Arc<World>, town_id: &str) -> bool {
    let mut tower = {
        let mut dir = lock();
        dir.towns
            .get_mut(town_id)
            .and_then(|w| w.siege_tower.take())
    };
    if let Some(t) = tower.as_mut()
        && !engines::tower_complete(t)
    {
        let n = engines::append_tower_parts(world, t, engines::ENGINE_PARTS_PER_TICK);
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.siege_tower = tower;
        }
        return n > 0;
    }
    if tower.is_some() {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.siege_tower = tower;
        }
    }
    let mut ram = {
        let mut dir = lock();
        dir.towns.get_mut(town_id).and_then(|w| w.ram.take())
    };
    if let Some(r) = ram.as_mut()
        && !r.destroyed
        && !crate::ram::ram_complete(r)
    {
        let n = crate::ram::append_ram_parts(world, r, crate::ram::RAM_PARTS_PER_TICK);
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ram = ram;
        }
        return n > 0;
    }
    if ram.is_some() {
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.ram = ram;
        }
    }
    false
}

fn prewarm_siege_columns(world: &World, town_id: &str) {
    let posts = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let camp = war
            .raid
            .as_ref()
            .map_or_else(|| war.scene.keep(), |r| r.camp);
        let wall = treb_wall_target(&war.scene, war.threatened, camp);
        let mut posts = vec![camp, wall];
        posts.extend(war.scene.trebuchet_posts().iter().copied());
        posts.extend(war.scene.ram_posts().iter().copied());
        posts.extend(war.scene.ladder_posts().iter().copied());
        posts
    };
    for post in posts {
        let y = standing_y(world, post);
        let pos = BlockPos::new(post.x, y, post.z);
        if world.is_loaded(&pos) {
            let _ = world.get_block_and_state(&pos);
        }
    }
}

fn tick_siege_engines(world: &Arc<World>, town_id: &str, tick: u32, alarm: bool) {
    if continue_engine_spawns(world, town_id) {
        return;
    }
    let spawn_missing_tower = {
        let dir = lock();
        dir.towns.get(town_id).is_some_and(|w| {
            alarm && w.raid.as_ref().is_some_and(|r| !r.tower_spawned) && w.siege_tower.is_none()
        })
    };
    if spawn_missing_tower {
        let origin = {
            let dir = lock();
            dir.towns.get(town_id).map_or_else(
                || ScenePost::new(0, 0),
                |w| {
                    w.scene
                        .ladder_posts()
                        .first()
                        .copied()
                        .or_else(|| w.scene.raid_camps().first().copied())
                        .unwrap_or_else(|| w.scene.keep())
                },
            )
        };
        let y = f64::from(standing_y(world, origin));
        let mut tower = engines::empty_tower(Vector3::new(
            f64::from(origin.x) + 0.5,
            y,
            f64::from(origin.z) + 0.5,
        ));
        engines::append_tower_parts(world, &mut tower, engines::ENGINE_PARTS_PER_TICK);
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.siege_tower = Some(tower);
            if let Some(raid) = &mut war.raid {
                raid.tower_spawned = true;
            }
        }
        return;
    }
    if apply_punch_break(world, town_id) {
        apply_punch_repair(world, town_id, tick);
        return;
    }

    let (camp, wall, origin, should_fire, mut shot, tower, ladders_done, keep) = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let camp = war
            .raid
            .as_ref()
            .map_or_else(|| war.scene.keep(), |r| r.camp);
        let wall = treb_wall_target(&war.scene, war.threatened, camp);
        let origin = war
            .scene
            .trebuchet_posts()
            .first()
            .copied()
            .or_else(|| war.scene.ram_posts().first().copied())
            .or_else(|| war.scene.raid_camps().first().copied())
            .unwrap_or(camp);
        let should_fire = alarm
            && war.treb_shot.is_none()
            && tick.saturating_sub(war.last_treb_tick) >= engines::TREB_PERIOD;
        (
            camp,
            wall,
            origin,
            should_fire,
            war.treb_shot.take(),
            war.siege_tower.take(),
            war.ladders_placed,
            war.scene.keep_rect(),
        )
    };
    let wall_top = standing_y(world, wall);
    let ladder_floor = wall_top.saturating_sub(WALL_WALK_LIFT);
    let mut wrote = should_fire;

    if should_fire {
        let oy = f64::from(standing_y(world, origin) + 8);
        let ty = f64::from(wall_top);
        let fired = engines::fire_trebuchet(
            world,
            Vector3::new(f64::from(origin.x) + 0.5, oy, f64::from(origin.z) + 0.5),
            Vector3::new(f64::from(wall.x) + 0.5, ty, f64::from(wall.z) + 0.5),
        );
        let mut dir = lock();
        if let Some(war) = dir.towns.get_mut(town_id) {
            war.last_treb_tick = tick;
            war.treb_shot = Some(fired);
        }
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Trebuchet,
            actor: 0,
            other: None,
            x: f64::from(origin.x),
            z: f64::from(origin.z),
            note: format!("{town_id} trebuchet loosed"),
        });
        tracing::info!(
            town = town_id,
            x = origin.x,
            z = origin.z,
            wall_x = wall.x,
            wall_z = wall.z,
            "kingdoms trebuchet loosed"
        );
        shot = None;
    }

    if let Some(mut flying) = shot {
        if let Some(hits) = engines::tick_shot(world, &mut flying, keep) {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.treb_shot = None;
            }
            drop(dir);
            enqueue_punch_break(town_id, hits, tick);
        } else {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.treb_shot = Some(flying);
            }
        }
    }

    if let Some(mut moving) = tower {
        let arrived = engines::tick_tower(world, &mut moving, wall);
        if arrived && !ladders_done && !wrote {
            let n = raise_ladders(world, wall, camp, ladder_floor);
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.ladders_placed = true;
                war.siege_tower = Some(moving);
            }
            note_ladders_raised(&mut dir, town_id, tick, wall, n);
            wrote = true;
        } else {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id) {
                war.siege_tower = Some(moving);
            }
        }
    }

    if alarm && !ladders_done && !wrote {
        let crew_at_wall = {
            let dir = lock();
            dir.towns.get(town_id).is_some_and(|w| {
                w.wired.len() > 2 && (w.raider_gate_dist < 16.0 || tick.is_multiple_of(40))
            })
        };
        if crew_at_wall || tick.is_multiple_of(40) {
            let n = raise_ladders(world, wall, camp, ladder_floor);
            if n > 0 {
                let mut dir = lock();
                if let Some(war) = dir.towns.get_mut(town_id) {
                    war.ladders_placed = true;
                }
                note_ladders_raised(&mut dir, town_id, tick, wall, n);
                wrote = true;
            }
        }
    }
    if !wrote {
        apply_punch_repair(world, town_id, tick);
    }
}

fn treb_wall_target(
    scene: &TownInfoScene,
    threatened: Option<ScenePost>,
    camp: ScenePost,
) -> ScenePost {
    let keep = scene.keep_rect();
    let allow = |p: ScenePost| engines::punch_column_allowed(p.x, p.z, keep);
    let threat = threatened.unwrap_or(camp);
    if let Some(p) = scene
        .wall()
        .iter()
        .map(|w| ScenePost::new(w[0], w[1]))
        .filter(|p| allow(*p))
        .min_by_key(|p| (p.x - threat.x).abs() + (p.z - threat.z).abs())
    {
        return p;
    }
    let k = scene.keep();
    let fx = (camp.x - k.x).signum() * 16;
    let fz = (camp.z - k.z).signum() * 16;
    ScenePost::new(k.x + fx, k.z + fz)
}

fn raise_ladders(world: &Arc<World>, wall: ScenePost, camp: ScenePost, floor: i32) -> u32 {
    let mut n = engines::place_assault_ladders(world, wall, camp, floor);
    if n > 0 {
        return n;
    }
    for (dx, dz) in [(4, 0), (-4, 0), (0, 4), (0, -4), (8, 0), (0, 8)] {
        n = engines::place_assault_ladders(
            world,
            ScenePost::new(wall.x + dx, wall.z + dz),
            camp,
            floor,
        );
        if n > 0 {
            break;
        }
    }
    n
}

fn note_ladders_raised(dir: &mut Director, town_id: &str, tick: u32, wall: ScenePost, n: u32) {
    tracing::info!(
        town = town_id,
        x = wall.x,
        z = wall.z,
        placed = n,
        "kingdoms assault ladders raised"
    );
    if n == 0 {
        return;
    }
    dir.push_event(CombatEvent {
        tick,
        kind: CombatKind::LaddersRaised,
        actor: n as i32,
        other: None,
        x: f64::from(wall.x),
        z: f64::from(wall.z),
        note: format!("{town_id} assault ladders raised"),
    });
}

fn enqueue_punch_break(town_id: &str, hits: Vec<engines::PunchHit>, tick: u32) {
    if hits.is_empty() {
        return;
    }
    enqueue_punch_repair(town_id, &hits, tick);
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        war.punch_break.extend(hits);
    }
}

fn apply_punch_break(world: &Arc<World>, town_id: &str) -> bool {
    let hits = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return false;
        };
        if war.punch_break.is_empty() {
            return false;
        }
        let n = war.punch_break.len().min(PUNCH_BREAK_PER_TICK);
        war.punch_break.drain(..n).collect::<Vec<_>>()
    };
    let mut placed = 0u32;
    for hit in hits {
        if engines::apply_punch_hit(world, hit) {
            placed += 1;
        }
    }
    if placed > 0 {
        tracing::info!("kingdoms trebuchet punched wall holes={placed} town={town_id}");
    }
    placed > 0
}

fn enqueue_punch_repair(town_id: &str, hits: &[engines::PunchHit], tick: u32) {
    if hits.is_empty() {
        return;
    }
    let Some((seed, cx, cz)) = parse_town_id(town_id) else {
        return;
    };
    let air = pumpkin_data::Block::AIR.default_state.id.as_u16();
    let mut jobs = Vec::new();
    if let Some(plan) = crate::plan::cached_plan(seed, cx, cz) {
        for hit in hits {
            let col = crate::stamp::planned_column(&plan, hit.x, hit.z);
            if let Some(planned) = col.into_iter().find(|b| b.y == hit.y) {
                if let Some(job) = repair::job_if_mismatch(air, planned) {
                    jobs.push(job);
                }
            } else {
                jobs.push(repair::RepairJob {
                    x: hit.x,
                    y: hit.y,
                    z: hit.z,
                    state: hit.was,
                });
            }
        }
    }
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        war.punch_repair = jobs;
        war.punch_repair_at = tick.saturating_add(40);
    }
}

fn apply_punch_repair(world: &Arc<World>, town_id: &str, tick: u32) {
    let jobs = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if tick < war.punch_repair_at || war.punch_repair.is_empty() {
            return;
        }
        let n = war.punch_repair.len().min(PUNCH_REPAIR_PER_TICK);
        war.punch_repair.drain(..n).collect::<Vec<_>>()
    };
    let mut placed = 0u32;
    for job in jobs {
        if repair::place_one(world, job) {
            placed += 1;
        }
    }
    if placed > 0 {
        tracing::info!(
            "kingdoms repair-by-diff refilled trebuchet hole placed={placed} town={town_id}"
        );
    }
}

fn apply_pending_breach(world: &Arc<World>, town_id: &str) {
    let cells = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if war.pending_breach.is_empty() {
            return;
        }
        let n = war.pending_breach.len().min(PUNCH_BREAK_PER_TICK);
        war.pending_breach.drain(..n).collect::<Vec<_>>()
    };
    let mut cleared = 0u32;
    for pos in cells {
        if siege::clear_gate_bar(world, pos) {
            cleared += 1;
        }
    }
    if cleared > 0 {
        tracing::info!(town = town_id, cleared, "kingdoms ram cleared gate bars");
    }
}

fn apply_pending_bars(world: &Arc<World>, town_id: &str) {
    let cells = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if war.pending_bars.is_empty() {
            return;
        }
        let n = war.pending_bars.len().min(PORTCULLIS_BARS_PER_TICK);
        war.pending_bars.drain(..n).collect::<Vec<_>>()
    };
    let mut placed = 0u32;
    for pos in cells {
        if siege::place_portcullis_bar(world, pos) {
            placed += 1;
        }
    }
    if placed > 0 {
        tracing::info!(town = town_id, placed, "kingdoms portcullis bars placed");
    }
}

/// Player looses a shot at the threatened wall of an active raid.
pub fn try_fire_trebuchet(player: &Player) -> Result<String, String> {
    let world = Arc::clone(&player.get_entity().world.load());
    let pos = player.get_entity().pos.load();
    let uuid = player.gameprofile.id.to_string();
    let (town_id, origin, wall) = {
        let dir = lock();
        if dir.player_kingdom(&uuid).is_none() {
            return Err("join a kingdom first".to_owned());
        }
        let mut near = None;
        let mut fallback = None;
        for (id, war) in &dir.towns {
            if war.raid.is_none() {
                continue;
            }
            if war.treb_shot.is_some() {
                continue;
            }
            let origin = war
                .scene
                .trebuchet_posts()
                .first()
                .copied()
                .or_else(|| war.scene.ram_posts().first().copied())
                .or_else(|| war.scene.raid_camps().first().copied())
                .or_else(|| war.raid.as_ref().map(|r| r.camp));
            let Some(origin) = origin else {
                continue;
            };
            let wall = treb_wall_target(&war.scene, war.threatened, origin);
            let cand = (id.clone(), origin, wall);
            if engines::near_controls(pos, war.scene.trebuchet_posts(), 8.0) {
                near = Some(cand);
                break;
            }
            fallback = Some(cand);
        }
        near.or(fallback)
            .ok_or_else(|| "no raid to fire on (join, then wait for the war clock)".to_owned())?
    };
    let oy = f64::from(standing_y(&world, origin) + 8);
    let ty = f64::from(standing_y(&world, wall) + WALL_WALK_LIFT);
    let fired = engines::fire_trebuchet(
        &world,
        Vector3::new(f64::from(origin.x) + 0.5, oy, f64::from(origin.z) + 0.5),
        Vector3::new(f64::from(wall.x) + 0.5, ty, f64::from(wall.z) + 0.5),
    );
    let mut dir = lock();
    let tick = dir.tick;
    if let Some(war) = dir.towns.get_mut(&town_id) {
        war.last_treb_tick = tick;
        war.treb_shot = Some(fired);
    }
    dir.push_event(CombatEvent {
        tick,
        kind: CombatKind::Trebuchet,
        actor: 0,
        other: None,
        x: pos.x,
        z: pos.z,
        note: format!("{town_id} player loosed the trebuchet"),
    });
    tracing::info!(
        town = %town_id,
        x = origin.x,
        z = origin.z,
        "kingdoms trebuchet loosed"
    );
    Ok(format!("loosed the trebuchet at {town_id}"))
}

/// Freeman or above: nearby kingdom soldiers follow, hold, or charge.
pub fn try_rally(player: &Player, order: SquadOrder) -> Result<String, String> {
    let uuid = player.gameprofile.id.to_string();
    let world = Arc::clone(&player.get_entity().world.load());
    let pos = player.get_entity().pos.load();
    let mut dir = lock();
    let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
        return Err("join a kingdom first".to_owned());
    };
    let rank = dir
        .player_stats
        .get(&uuid)
        .map_or(siege::Rank::Levy, siege::PlayerStats::rank);
    if rank < siege::Rank::Freeman {
        return Err("freeman rank or higher to command a squad".to_owned());
    }
    if let Some(prev) = dir.squad_orders.get(&uuid)
        && order != prev.order
        && tick_since(dir.tick, prev.at_tick) < HORN_COOLDOWN_TICKS
    {
        return Err("squad order is on cooldown".to_owned());
    }
    let hold = PostTarget {
        x: pos.x.floor() as i32,
        y: pos.y.floor() as i32,
        z: pos.z.floor() as i32,
    };
    let tick = dir.tick;
    dir.squad_orders.insert(
        uuid,
        SquadCommand {
            order,
            kingdom: kingdom.clone(),
            hold,
            x: pos.x,
            y: pos.y,
            z: pos.z,
            at_tick: tick,
        },
    );
    let town_id = nearest_town_id(&world, pos.x.floor() as i32, pos.z.floor() as i32);
    let _ = chat::try_shout_as(
        &world,
        tick,
        &town_id,
        order.shout(),
        pos.x,
        pos.z,
        &mut dir.last_shout,
        &player.gameprofile.name,
        Some(order.line()),
    );
    tracing::info!(
        player = %player.gameprofile.name,
        order = order.as_str(),
        kingdom = kingdom.as_str(),
        "kingdoms squad order"
    );
    Ok(format!(
        "squad {}: nearby {} soldiers {}",
        order.as_str(),
        kingdom.as_str(),
        order.line()
    ))
}

/// Op/test path: grant a rank without farming kills.
pub fn set_player_rank(player: &Player, rank: siege::Rank) -> Result<String, String> {
    let uuid = player.gameprofile.id.to_string();
    let kingdom = {
        let mut dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return Err("join a kingdom first".to_owned());
        };
        let stats = dir.player_stats.entry(uuid).or_default();
        stats.kills = stats.kills.max(rank.min_kills());
        dir.persist_dirty = true;
        kingdom
    };
    kit_player(player, &kingdom, rank, false);
    announce_rank(player, rank);
    tracing::info!(
        player = %player.gameprofile.name,
        rank = rank.as_str(),
        "kingdoms rank set"
    );
    Ok(format!(
        "{} is now a {}",
        player.gameprofile.name,
        rank.as_str()
    ))
}

/// Goat horn: cycle follow → hold → charge for a ranked player.
pub fn try_horn(player: &Player) -> Result<String, String> {
    let uuid = player.gameprofile.id.to_string();
    let next = {
        let dir = lock();
        dir.squad_orders
            .get(&uuid)
            .map_or(SquadOrder::Follow, |c| c.order.next())
    };
    tracing::info!(
        player = %player.gameprofile.name,
        order = next.as_str(),
        "kingdoms horn"
    );
    try_rally(player, next)
}

fn tick_since(now: u32, then: u32) -> u32 {
    now.saturating_sub(then)
}

fn nearest_town_id(world: &World, x: i32, z: i32) -> String {
    let seed = world.level.seed.0;
    let town = crate::town::nearest_town(seed, x, z);
    crate::info::town_id(seed, town.cell_x, town.cell_z)
}

const fn is_militia_id(id: i32) -> bool {
    id.unsigned_abs().is_multiple_of(2)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CivilianDuty {
    Townsfolk,
    Militia,
    Priest,
    Blacksmith,
}

fn civilian_duty(entity: &Entity, id: i32) -> CivilianDuty {
    if has_tag(entity, TAG_NPC_PRIEST) {
        CivilianDuty::Priest
    } else if has_tag(entity, TAG_NPC_BLACKSMITH) {
        CivilianDuty::Blacksmith
    } else if has_tag(entity, TAG_MILITIA) || is_militia_id(id) {
        CivilianDuty::Militia
    } else {
        CivilianDuty::Townsfolk
    }
}

fn overlay_civilian_posts(
    dest: PostTarget,
    role: Option<VillagerRole>,
    alarm: bool,
    capturing: bool,
    inner_gate: PostTarget,
    well: PostTarget,
    keep: PostTarget,
    duty: CivilianDuty,
) -> PostTarget {
    if capturing && role == Some(VillagerRole::Civilian) {
        return dest;
    }
    match duty {
        CivilianDuty::Priest => keep,
        CivilianDuty::Blacksmith => dest,
        CivilianDuty::Militia if role == Some(VillagerRole::Civilian) && alarm => inner_gate,
        CivilianDuty::Militia if role == Some(VillagerRole::Civilian) => well,
        CivilianDuty::Townsfolk if role == Some(VillagerRole::Civilian) && alarm => well,
        _ => dest,
    }
}

fn overlay_squad_order(
    dest: PostTarget,
    faction: &Faction,
    x: f64,
    z: f64,
    squads: &[SquadCommand],
    charge_dest: PostTarget,
) -> PostTarget {
    let Faction::Kingdom(kingdom) = faction else {
        return dest;
    };
    let Some(cmd) = squads
        .iter()
        .filter(|s| s.kingdom == *kingdom)
        .filter(|s| (s.x - x).hypot(s.z - z) <= SQUAD_RANGE)
        .min_by(|a, b| {
            let da = (a.x - x).hypot(a.z - z);
            let db = (b.x - x).hypot(b.z - z);
            da.total_cmp(&db)
        })
    else {
        return dest;
    };
    match cmd.order {
        SquadOrder::Follow => PostTarget {
            x: cmd.x.floor() as i32,
            y: cmd.y.floor() as i32,
            z: cmd.z.floor() as i32,
        },
        SquadOrder::Hold => cmd.hold,
        SquadOrder::Charge => charge_dest,
    }
}

fn collect_squads(world: &World) -> Vec<SquadCommand> {
    let players = world.players.load();
    let mut dir = lock();
    let mut out = Vec::new();
    for player in players.iter() {
        let uuid = player.gameprofile.id.to_string();
        let Some(cmd) = dir.squad_orders.get_mut(&uuid) else {
            continue;
        };
        let pos = player.living_entity.entity.pos.load();
        cmd.x = pos.x;
        cmd.y = pos.y;
        cmd.z = pos.z;
        if cmd.order == SquadOrder::Follow {
            cmd.hold = PostTarget {
                x: pos.x.floor() as i32,
                y: pos.y.floor() as i32,
                z: pos.z.floor() as i32,
            };
        }
        out.push(cmd.clone());
    }
    out
}

fn assign_named_npcs(snapshots: &[(Arc<dyn EntityBase>, Faction)]) {
    let mut smith = snapshots
        .iter()
        .any(|(e, _)| has_tag(e.get_entity(), TAG_NPC_BLACKSMITH));
    let mut priest = snapshots
        .iter()
        .any(|(e, _)| has_tag(e.get_entity(), TAG_NPC_PRIEST));
    let mut captain = snapshots
        .iter()
        .any(|(e, _)| has_tag(e.get_entity(), TAG_CAPTAIN));
    for (entity, _) in snapshots {
        let base = entity.get_entity();
        let role = role_from_tags(base).and_then(|s| VillagerRole::parse(&s));
        if !smith && role == Some(VillagerRole::Builder) {
            let _ = base.add_scoreboard_tag(TAG_NPC_BLACKSMITH);
            smith = true;
        } else if !priest && role == Some(VillagerRole::Civilian) {
            let _ = base.add_scoreboard_tag(TAG_NPC_PRIEST);
            priest = true;
        } else if !captain && role == Some(VillagerRole::Guard) {
            let _ = base.add_scoreboard_tag(TAG_CAPTAIN);
            captain = true;
        }
    }
}

fn arm_militia_tick(snapshots: &[(Arc<dyn EntityBase>, Faction)], alarm: bool) {
    if !alarm {
        return;
    }
    let mut n = 0usize;
    for (entity, faction) in snapshots {
        if n >= ARM_PER_TICK {
            break;
        }
        if !matches!(faction, Faction::Kingdom(_)) {
            continue;
        }
        let Some(mob) = entity.get_mob() else {
            continue;
        };
        let base = entity.get_entity();
        let role = role_from_tags(base).and_then(|s| VillagerRole::parse(&s));
        if role != Some(VillagerRole::Civilian) || has_tag(base, TAG_ARMED) {
            continue;
        }
        let _ = base.add_scoreboard_tag(TAG_ARMED);
        if is_militia_id(base.entity_id) {
            let _ = base.add_scoreboard_tag(TAG_MILITIA);
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::WOODEN_SWORD);
        } else {
            equip(mob, EquipmentSlot::MAIN_HAND, &Item::BUCKET);
        }
        n += 1;
    }
}

fn shout_named_npcs(world: &World, town_id: &str, tick: u32, alarm: bool, dest: PostTarget) {
    if !alarm {
        return;
    }
    let mut dir = lock();
    let x = f64::from(dest.x);
    let z = f64::from(dest.z);
    let _ = chat::try_shout_as(
        world,
        tick,
        town_id,
        ShoutKind::Rally,
        x,
        z,
        &mut dir.last_shout,
        "captain",
        Some("Militia to the inner gate. Form a line."),
    );
    let _ = chat::try_shout_as(
        world,
        tick,
        town_id,
        ShoutKind::Hold,
        x,
        z,
        &mut dir.last_shout,
        "blacksmith",
        Some("Hammers out. Take a weapon from the armoury."),
    );
    let _ = chat::try_shout_as(
        world,
        tick,
        town_id,
        ShoutKind::Fallen,
        x,
        z,
        &mut dir.last_shout,
        "priest",
        Some("The bell is rung. God keep the gate."),
    );
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
            if rallied_n < ROUT_TAGS_PER_TICK as u32
                && has_tag(base, TAG_ROUTED)
                && base.remove_scoreboard_tag(TAG_ROUTED)
            {
                rallied_n += 1;
            }
            continue;
        }
        if captain_down && !captain && !has_tag(base, TAG_ROUTED) {
            if routed_n < ROUT_TAGS_PER_TICK as u32 {
                let _ = base.add_scoreboard_tag(TAG_ROUTED);
                routed_n += 1;
            }
            continue;
        }
        if crate::morale::should_rout(friends, foes) && !captain {
            if routed_n < ROUT_TAGS_PER_TICK as u32 && base.add_scoreboard_tag(TAG_ROUTED) {
                routed_n += 1;
            }
            continue;
        }
        if captain && !has_tag(base, TAG_ROUTED) {
            for (other, other_f) in snapshots {
                if rallied_n >= ROUT_TAGS_PER_TICK as u32 {
                    break;
                }
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
    if captain_down && routed_n < ROUT_TAGS_PER_TICK as u32 {
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
    let ticks = hold_ticks();
    let progress = (hold as f32 / ticks as f32).clamp(0.0, 1.0);
    let title = TextComponent::text(format!(
        "Flag {raiders_in_keep}/{need}  hold {hold}/{ticks}  def {defenders_in_keep}"
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
    let period = war_clock_period();
    let fronts = war_clock_fronts();
    {
        let dir = lock();
        let active = dir.towns.values().filter(|w| w.raid.is_some()).count();
        if active >= fronts {
            return;
        }
        if !dir.clock_filling && tick.saturating_sub(dir.last_clock_tick) < period {
            return;
        }
    }
    heal_repaired_towns(tick);
    ensure_border_towns(world);
    let Some((town_id, side)) = pick_clock_front(tick, fronts) else {
        let mut dir = lock();
        dir.clock_filling = false;
        dir.last_clock_tick = tick;
        dir.mark_dirty();
        return;
    };
    let spec = RaidSpec::new(war_clock_size())
        .with_extra_defenders(war_clock_defenders())
        .with_side(side);
    match start_raid(world, &town_id, spec) {
        Ok(id) => {
            let mut dir = lock();
            dir.clock_filling = true;
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
            let active = dir.towns.values().filter(|w| w.raid.is_some()).count();
            if active >= fronts {
                dir.clock_filling = false;
                dir.last_clock_tick = tick;
            }
            dir.mark_dirty();
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
            let mut dir = lock();
            dir.clock_filling = false;
            dir.last_clock_tick = tick;
            dir.mark_dirty();
        }
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
    if ids.is_empty() {
        return;
    }
    let mut still = Vec::new();
    for id in ids {
        if crate::damage::step_repair(&id).is_damaged() {
            still.push(id);
        }
    }
    let mut dir = lock();
    let next = tick.saturating_add(REPAIR_WINDOW_TICKS);
    for id in &still {
        if let Some(war) = dir.towns.get_mut(id) {
            war.repair_until = next;
        }
    }
    dir.mark_dirty();
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

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "mirrors the director tick flags (alarm/combat/breach/capture)"
)]
fn dest_for(
    faction: &Faction,
    job: MarchJob,
    role: Option<VillagerRole>,
    alarm: bool,
    in_combat: bool,
    breached: bool,
    capturing: bool,
    repair_dest: Option<PostTarget>,
    march_dest: PostTarget,
    keep_dest: PostTarget,
    ladder_dest: PostTarget,
    archer_dest: PostTarget,
    work_dest: PostTarget,
    home_dest: PostTarget,
    patrol_dest: PostTarget,
    world_time: i64,
) -> PostTarget {
    if role == Some(VillagerRole::Builder) {
        return repair_dest.unwrap_or(keep_dest);
    }
    if role == Some(VillagerRole::Civilian) {
        if capturing && alarm {
            return work_dest;
        }
        return match schedule::civilian_post(alarm, world_time) {
            CivilianPost::Keep => keep_dest,
            CivilianPost::Work => work_dest,
            CivilianPost::Home => home_dest,
        };
    }
    match faction {
        Faction::Neutral => keep_dest,
        Faction::Kingdom(_) if role == Some(VillagerRole::Archer) => archer_dest,
        Faction::Kingdom(_) if role == Some(VillagerRole::Guard) && !alarm && !in_combat => {
            patrol_dest
        }
        Faction::Kingdom(_) if alarm || in_combat => march_dest,
        Faction::Kingdom(_) => patrol_dest,
        Faction::Pillager => {
            if job == MarchJob::Ladder && !breached {
                ladder_dest
            } else if capturing || breached {
                keep_dest
            } else {
                march_dest
            }
        }
    }
}

fn plate_slot(entity_id: i32, flag: PostTarget) -> PostTarget {
    let i = entity_id.unsigned_abs() % 9;
    PostTarget {
        x: flag.x + (i % 3) as i32 - 1,
        y: flag.y,
        z: flag.z + (i / 3) as i32 - 1,
    }
}

fn overlay_flag_dest(
    dest: PostTarget,
    faction: &Faction,
    bearer: bool,
    x: f64,
    z: f64,
    inner: Rect,
    gatehouse: Rect,
    gates: &[GateInfo],
    breached: bool,
    capturing: bool,
    flag_dest: PostTarget,
) -> PostTarget {
    let raider = bearer || matches!(faction, Faction::Pillager);
    if !raider {
        return dest;
    }
    if in_capture_zone(inner, gatehouse, x, z) {
        return flag_dest;
    }
    if in_courtyard(gatehouse, x, z) && (breached || capturing) {
        return flag_dest;
    }
    if !gates.is_empty() && !breached && !capturing {
        return outermost_gate_dest(gates, flag_dest);
    }
    if breached || capturing {
        let dist = (x - f64::from(flag_dest.x)).hypot(z - f64::from(flag_dest.z));
        if dist <= STUCK_STORM_RANGE {
            return flag_dest;
        }
        return inward_siege_dest(x, z, gates, gatehouse, inner, flag_dest);
    }
    dest
}

fn outermost_gate_dest(gates: &[GateInfo], flag: PostTarget) -> PostTarget {
    let fx = f64::from(flag.x);
    let fz = f64::from(flag.z);
    gates
        .iter()
        .max_by(|a, b| {
            let da = (f64::from(a.x) - fx).hypot(f64::from(a.z) - fz);
            let db = (f64::from(b.x) - fx).hypot(f64::from(b.z) - fz);
            da.total_cmp(&db)
        })
        .map_or(flag, |g| PostTarget {
            x: g.x,
            y: flag.y,
            z: g.z,
        })
}

fn gate_info_near(gates: &[GateInfo], post: ScenePost) -> Option<GateInfo> {
    gates
        .iter()
        .copied()
        .min_by_key(|g| (g.x - post.x).abs() + (g.z - post.z).abs())
}

fn packed_storm_gate(
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    ghosts: &[GhostSoldier],
    gates: &[GateInfo],
    range: f64,
    need: usize,
) -> Option<GateInfo> {
    let mut best: Option<(usize, GateInfo)> = None;
    for &gate in gates {
        let gx = f64::from(gate.x) + 0.5;
        let gz = f64::from(gate.z) + 0.5;
        let live = snapshots
            .iter()
            .filter(|(entity, faction)| {
                matches!(faction, Faction::Pillager) && {
                    let p = entity.get_entity().pos.load();
                    (p.x - gx).hypot(p.z - gz) <= range
                }
            })
            .count();
        let ghost_n = ghosts
            .iter()
            .filter(|g| {
                matches!(g.faction, Faction::Pillager) && (g.x - gx).hypot(g.z - gz) <= range
            })
            .count();
        let n = live + ghost_n;
        if n >= need && best.is_none_or(|(bn, _)| n >= bn) {
            best = Some((n, gate));
        }
    }
    best.map(|(_, g)| g)
}

fn nearest_gate_to_raiders(
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    ghosts: &[GhostSoldier],
    gates: &[GateInfo],
) -> Option<GateInfo> {
    let mut px = 0.0;
    let mut pz = 0.0;
    let mut n = 0u32;
    for (entity, faction) in snapshots {
        if !matches!(faction, Faction::Pillager) {
            continue;
        }
        let p = entity.get_entity().pos.load();
        px += p.x;
        pz += p.z;
        n += 1;
    }
    for ghost in ghosts {
        if !matches!(ghost.faction, Faction::Pillager) {
            continue;
        }
        px += ghost.x;
        pz += ghost.z;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let mx = px / f64::from(n);
    let mz = pz / f64::from(n);
    gates.iter().copied().min_by(|a, b| {
        let da = (f64::from(a.x) - mx).hypot(f64::from(a.z) - mz);
        let db = (f64::from(b.x) - mx).hypot(f64::from(b.z) - mz);
        da.total_cmp(&db)
    })
}

fn raid_outcome_reason(
    attackers_on_plate: u32,
    in_courtyard: u32,
    storming: bool,
    raiders: u32,
    tick: u32,
    last_progress_tick: u32,
) -> &'static str {
    if attackers_on_plate > 0 {
        "on_plate"
    } else if in_courtyard > 0 {
        "in_courtyard"
    } else if storming {
        "storming"
    } else if raiders == 0 {
        "repelled"
    } else if tick.saturating_sub(last_progress_tick) >= STUCK_TICKS {
        "stuck"
    } else {
        "marching"
    }
}

fn apply_cull(town_id: &str, cull: &[(i32, GhostSoldier)]) {
    if cull.is_empty() {
        return;
    }
    let ids: Vec<i32> = cull.iter().map(|(id, _)| *id).collect();
    let mut dir = lock();
    if let Some(war) = dir.towns.get_mut(town_id) {
        if let Some(raid) = &mut war.raid {
            raid.living.retain(|id| !ids.contains(id));
            raid.temps.retain(|id| !ids.contains(id));
            raid.ghosts.extend(cull.iter().map(|(_, g)| g.clone()));
        }
        war.pending_despawn.extend(ids);
    }
}

fn count_ghost_presence(
    ghost: &GhostSoldier,
    flag_xz: (i32, i32),
    gatehouse: Rect,
    owner_faction: &Faction,
    attackers_on_plate: &mut u32,
    defenders_on_plate: &mut u32,
    attackers_near: &mut u32,
    defenders_near: &mut u32,
    raiders_in_keep: &mut u32,
    defenders_in_keep: &mut u32,
    in_courtyard_n: &mut u32,
) {
    if on_flag_plate(flag_xz, ghost.x, ghost.z) {
        if is_hostile(&ghost.faction, owner_faction) {
            *attackers_on_plate += 1;
            *raiders_in_keep += 1;
        } else if is_allied(&ghost.faction, owner_faction) {
            *defenders_on_plate += 1;
        }
    }
    if near_flag(flag_xz, ghost.x, ghost.z) {
        if is_hostile(&ghost.faction, owner_faction) {
            *attackers_near += 1;
        } else if is_allied(&ghost.faction, owner_faction) {
            *defenders_near += 1;
            *defenders_in_keep += 1;
        }
    }
    if matches!(ghost.faction, Faction::Pillager) && in_courtyard(gatehouse, ghost.x, ghost.z) {
        *in_courtyard_n += 1;
    }
}

fn slide_ghost(ghost: &mut GhostSoldier, dest: PostTarget) {
    let dx = f64::from(dest.x) + 0.5 - ghost.x;
    let dz = f64::from(dest.z) + 0.5 - ghost.z;
    let len = dx.hypot(dz);
    let climb = (f64::from(dest.y) - ghost.y).clamp(-0.45, 0.45);
    if len < 0.05 {
        ghost.y += climb;
        return;
    }
    let step = SLIDE_STEP.min(len);
    ghost.x += dx / len * step;
    ghost.z += dz / len * step;
    ghost.y += climb;
}

fn spawn_lod_body(world: &Arc<World>, town_id: &str, ghost: &GhostSoldier) -> Option<i32> {
    if take_spawn_slots(1) == 0 {
        return None;
    }
    let pos = Vector3::new(ghost.x, ghost.y, ghost.z);
    match &ghost.faction {
        Faction::Pillager => {
            let entity = Entity::new(world.clone(), pos, &EntityType::PILLAGER);
            let base: Arc<dyn EntityBase> =
                pumpkin_core::entity::mob::pillager::PillagerEntity::new(entity)
                    as Arc<dyn EntityBase>;
            if let Some(mob) = base.get_mob() {
                bind_raider(
                    mob,
                    town_id,
                    ghost.bearer,
                    MarchJob::Assault,
                    world,
                    ScenePost::new(ghost.x.floor() as i32, ghost.z.floor() as i32),
                );
                add_pillager_team(world, &base.get_scoreboard_name());
            }
            let id = base.get_entity().entity_id;
            world.spawn_entity(base).then_some(id)
        }
        Faction::Kingdom(kingdom) => {
            spawn_citizen(world, town_id, kingdom, VillagerRole::Guard, false, pos)
        }
        Faction::Neutral => None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "ghosts share the live plate counters"
)]
fn tick_ghosts(
    world: &Arc<World>,
    town_id: &str,
    tick: u32,
    player_xz: &[(f64, f64)],
    march_dest: PostTarget,
    keep_dest: PostTarget,
    keep_rect: Rect,
    gatehouse_rect: Rect,
    siege_gates: &[GateInfo],
    breached: bool,
    capturing: bool,
    owner_faction: &Faction,
    attackers_on_plate: &mut u32,
    defenders_on_plate: &mut u32,
    attackers_near: &mut u32,
    defenders_near: &mut u32,
    raiders_in_keep: &mut u32,
    defenders_in_keep: &mut u32,
    in_courtyard_n: &mut u32,
) {
    let flag_xz = (keep_dest.x, keep_dest.z);
    let respawn = {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = war.raid.as_mut() else {
            return;
        };
        let far_tier = LodTier::Alert;
        let mut keep = Vec::with_capacity(raid.ghosts.len());
        let mut respawn = Vec::new();
        for (i, mut ghost) in raid.ghosts.drain(..).enumerate() {
            if player_near_list(player_xz, ghost.x, ghost.z, PLAYER_SIM_RANGE) {
                respawn.push(ghost);
                continue;
            }
            if far_tier.should_tick(i as i32, tick) {
                let dest = overlay_flag_dest(
                    march_dest,
                    &ghost.faction,
                    ghost.bearer,
                    ghost.x,
                    ghost.z,
                    keep_rect,
                    gatehouse_rect,
                    siege_gates,
                    breached,
                    capturing,
                    keep_dest,
                );
                let dest = if dest.x == keep_dest.x && dest.z == keep_dest.z {
                    plate_slot(i as i32, dest)
                } else {
                    dest
                };
                slide_ghost(&mut ghost, dest);
            }
            count_ghost_presence(
                &ghost,
                flag_xz,
                gatehouse_rect,
                owner_faction,
                attackers_on_plate,
                defenders_on_plate,
                attackers_near,
                defenders_near,
                raiders_in_keep,
                defenders_in_keep,
                in_courtyard_n,
            );
            keep.push(ghost);
        }
        raid.ghosts = keep;
        respawn
    };
    let mut spawned: Vec<(i32, bool)> = Vec::new();
    let mut leftover = Vec::new();
    for ghost in respawn {
        if spawned.len() >= GHOST_SPAWNS_PER_TICK {
            leftover.push(ghost);
            continue;
        }
        if let Some(id) = spawn_lod_body(world, town_id, &ghost) {
            spawned.push((
                id,
                ghost.raider || matches!(ghost.faction, Faction::Pillager),
            ));
        } else {
            leftover.push(ghost);
        }
    }
    if spawned.is_empty() && leftover.is_empty() {
        return;
    }
    let mut dir = lock();
    if let Some(raid) = dir.towns.get_mut(town_id).and_then(|w| w.raid.as_mut()) {
        for (id, raider) in spawned {
            if raider {
                raid.living.push(id);
            } else {
                raid.temps.push(id);
            }
        }
        raid.ghosts.extend(leftover);
    }
}

/// Next ring toward the keep-court plate: outer town, civic, castle, then
/// the inner-bailey side of the gatehouse.
fn inward_siege_dest(
    x: f64,
    z: f64,
    gates: &[GateInfo],
    gatehouse: Rect,
    inner: Rect,
    flag: PostTarget,
) -> PostTarget {
    if in_capture_zone(inner, gatehouse, x, z) || in_courtyard(gatehouse, x, z) {
        return flag;
    }
    let fx = f64::from(flag.x);
    let fz = f64::from(flag.z);
    let me = (x - fx).hypot(z - fz);
    let mut ordered: Vec<&GateInfo> = gates.iter().collect();
    ordered.sort_by(|a, b| {
        let da = (f64::from(a.x) - fx).hypot(f64::from(a.z) - fz);
        let db = (f64::from(b.x) - fx).hypot(f64::from(b.z) - fz);
        db.total_cmp(&da)
    });
    for gate in ordered {
        let gd = (f64::from(gate.x) - fx).hypot(f64::from(gate.z) - fz);
        if me > gd + 10.0 {
            return PostTarget {
                x: gate.x,
                y: flag.y,
                z: gate.z,
            };
        }
    }
    gatehouse_inner_dest(gatehouse, inner, flag)
}

fn in_join_ai_window(index: usize, live_age: u32, n: usize) -> bool {
    if n == 0 {
        return false;
    }
    let budget = JOIN_ENTITY_BUDGET.min(n);
    let start = (live_age as usize).wrapping_mul(budget) % n;
    let pos = if index >= start {
        index - start
    } else {
        index + n - start
    };
    pos < budget
}

/// First waypoint past the portcullis, on the inner-bailey side of the gatehouse.
fn gatehouse_inner_dest(gatehouse: Rect, inner: Rect, flag: PostTarget) -> PostTarget {
    let (gx, gz) = gatehouse.center();
    let dx = (flag.x - gx).signum();
    let dz = (flag.z - gz).signum();
    let mut x = gx;
    let mut z = gz;
    for _ in 0..24 {
        x += dx;
        z += dz;
        if inner.contains(x, z) && !gatehouse.contains(x, z) {
            return PostTarget { x, y: flag.y, z };
        }
    }
    flag
}

fn swap_flag_banner(town_id: &str, owner: &TownOwner) {
    let (world, fx, fy, fz, kx, kz) = {
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let flag = war.scene.flag();
        let Some(world) = dir.attached_worlds().into_iter().next() else {
            return;
        };
        let keep = war.scene.keep();
        (world, flag.x, flag.height_or(64), flag.z, keep.x, keep.z)
    };
    let banner_y = fy + FLAG_MAST_FENCES + 1;
    world.set_block_state(
        &BlockPos::new(fx, banner_y, fz),
        owner_banner(owner).default_state.id,
        siege::SIEGE_PLACE_FLAGS,
    );
    let wool = owner_wool(owner).default_state.id;
    let (dx, dz, px, pz) = flag_panel_axes((kx, kz), (fx, fz));
    let base_y = fy + FLAG_MAST_FENCES;
    let half = FLAG_PANEL_WIDTH / 2;
    for row in 0..FLAG_PANEL_HEIGHT {
        for col in -half..=half {
            world.set_block_state(
                &BlockPos::new(fx + dx + col * px, base_y + row, fz + dz + col * pz),
                wool,
                siege::SIEGE_PLACE_FLAGS,
            );
        }
    }
}

fn flag_panel_axes(keep: (i32, i32), flag: (i32, i32)) -> (i32, i32, i32, i32) {
    let dx = (flag.0 - keep.0).signum();
    let dz = (flag.1 - keep.1).signum();
    if dx.abs() >= dz.abs() {
        (if dx == 0 { 1 } else { dx }, 0, 0, 1)
    } else {
        (0, dz, 1, 0)
    }
}

/// True when `(x, z)` is the town flag plinth (pedestal, mast, or plates).
#[must_use]
pub fn is_protected_flag_column(world: &World, x: i32, z: i32) -> bool {
    if lock().towns.values().any(|w| {
        let f = w.scene.flag();
        siege::is_flag_column((f.x, f.z), x, z)
    }) {
        return true;
    }
    let seed = world.level.seed.0;
    let origin = town_containing(seed, x, z);
    crate::plan::cached_plan(seed, origin.cell_x, origin.cell_z)
        .is_some_and(|p| siege::is_flag_column(p.castle.flag, x, z))
}

fn standing_dests(posts: &[ScenePost], fallback_y: i32, lift: i32) -> Vec<PostTarget> {
    posts
        .iter()
        .map(|post| PostTarget {
            x: post.x,
            y: post.height_or(fallback_y) + lift,
            z: post.z,
        })
        .collect()
}

fn pick_ready_dest(entity_id: i32, dests: &[PostTarget], fallback: PostTarget) -> PostTarget {
    if dests.is_empty() {
        return fallback;
    }
    dests[entity_id.unsigned_abs() as usize % dests.len()]
}

fn schedule_civilian_ready(
    entity_id: i32,
    world_time: i64,
    work_dests: &[PostTarget],
    home_dests: &[PostTarget],
    keep_dest: PostTarget,
) -> PostTarget {
    match schedule::civilian_post(false, world_time) {
        CivilianPost::Keep => keep_dest,
        CivilianPost::Work => pick_ready_dest(entity_id, work_dests, keep_dest),
        CivilianPost::Home => pick_ready_dest(entity_id, home_dests, keep_dest),
    }
}

fn patrol_ready(
    entity_id: i32,
    tick: u32,
    wall: &[PostTarget],
    fallback: PostTarget,
) -> PostTarget {
    if wall.is_empty() {
        return fallback;
    }
    wall[schedule::patrol_index(entity_id, tick, wall.len())]
}

fn player_near_list(players: &[(f64, f64)], x: f64, z: f64, range: f64) -> bool {
    players
        .iter()
        .any(|(px, pz)| (*px - x).hypot(*pz - z) <= range)
}

fn world_player_xz(world: &World) -> Vec<(f64, f64)> {
    world
        .players
        .load()
        .iter()
        .map(|p| {
            let pos = p.living_entity.entity.pos.load();
            (pos.x, pos.z)
        })
        .collect()
}

fn should_virtualize(players: &[(f64, f64)], x: f64, z: f64) -> bool {
    !player_near_list(players, x, z, PLAYER_CULL_RANGE)
}

fn apply_capture(
    town_id: &str,
    tick: u32,
    presence: FlagPresence,
    occupier: TownOwner,
    delta: u32,
) {
    let mut dir = lock();
    let (captured, retaken, kingdom, keep, need, event) = {
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        if tick < war.hold_lockout {
            return;
        }
        tick_town_flag(war, town_id, tick, presence, delta)
    };
    if let Some(event) = event {
        dir.push_event(event);
    }
    drop(dir);
    if captured {
        enqueue_capture(PendingFlip {
            town_id: town_id.to_owned(),
            captured: true,
            occupier,
            kingdom,
            keep,
            raiders_in_keep: presence.attackers_on_plate,
            defenders_in_keep: presence.defenders_near,
            need,
        });
    } else if retaken {
        enqueue_capture(PendingFlip {
            town_id: town_id.to_owned(),
            captured: false,
            occupier: TownOwner::kingdom(&kingdom),
            kingdom,
            keep,
            raiders_in_keep: presence.attackers_on_plate,
            defenders_in_keep: presence.defenders_on_plate,
            need,
        });
    }
}

fn tick_town_flag(
    war: &mut TownWar,
    town_id: &str,
    tick: u32,
    presence: FlagPresence,
    delta: u32,
) -> (bool, bool, KingdomId, ScenePost, u32, Option<CombatEvent>) {
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
    let occupy_need = capture_need(surviving.max(presence.attackers_on_plate));
    let allied = war
        .wired
        .values()
        .filter(|w| is_allied(&w.faction, &owner_fac))
        .count() as u32;
    let retake_need = capture_need(if live_raid { allied } else { virt_defenders });
    let was_occupy = war.occupy_hold;
    let was_paused = war.flag_paused;
    let update = tick_flag_hold(
        &war.owner,
        presence,
        war.occupy_hold,
        war.retake_hold,
        occupy_need,
        retake_need,
        delta,
    );
    war.occupy_hold = update.occupy_hold;
    war.retake_hold = update.retake_hold;
    war.flag_paused = update.paused;
    war.flag_attackers = presence.attackers_on_plate;
    war.flag_defenders = presence.defenders_near;
    if war.occupy_hold > 0 && tick.is_multiple_of(20) {
        tracing::info!(
            "kingdoms capture hold town={town_id} plate_r={} plate_d={} near_d={} need={occupy_need} hold={}/{HOLD_TICKS} paused={}",
            presence.attackers_on_plate,
            presence.defenders_on_plate,
            presence.defenders_near,
            war.occupy_hold,
            update.paused
        );
        if parse_town_id(town_id).is_some_and(|(_, cx, cz)| cx == 0 && cz == 0) {
            tracing::info!(
                "kingdoms capital hold town={town_id} plate_r={} plate_d={} hold={}/{}",
                presence.attackers_on_plate,
                presence.defenders_on_plate,
                war.occupy_hold,
                hold_ticks()
            );
        }
    } else if presence.attackers_on_plate > 0 && tick.is_multiple_of(20) {
        tracing::info!(
            "kingdoms capture blocked town={town_id} plate_r={} plate_d={} near_r={} need={occupy_need} paused={}",
            presence.attackers_on_plate,
            presence.defenders_on_plate,
            presence.attackers_near,
            update.paused
        );
    }
    let flag = war.scene.flag();
    let fx = f64::from(flag.x);
    let fz = f64::from(flag.z);
    let event = if update.captured || update.retaken {
        None
    } else if update.paused && !was_paused && (was_occupy > 0 || war.occupy_hold > 0) {
        Some(CombatEvent {
            tick,
            kind: CombatKind::CapturePaused,
            actor: presence.defenders_near as i32,
            other: Some(presence.attackers_on_plate as i32),
            x: fx,
            z: fz,
            note: format!("{town_id} flag capture paused"),
        })
    } else if war.occupy_hold > 0 && was_occupy == 0 {
        Some(CombatEvent {
            tick,
            kind: CombatKind::CaptureStarted,
            actor: presence.attackers_on_plate as i32,
            other: Some(occupy_need as i32),
            x: fx,
            z: fz,
            note: format!("{town_id} flag capture started"),
        })
    } else if (war.occupy_hold > 0 || war.retake_hold > 0)
        && tick.saturating_sub(war.last_flag_event) >= 40
    {
        war.last_flag_event = tick;
        Some(CombatEvent {
            tick,
            kind: CombatKind::CaptureProgress,
            actor: war.occupy_hold.max(war.retake_hold) as i32,
            other: Some(occupy_need as i32),
            x: fx,
            z: fz,
            note: format!(
                "{town_id} flag capture {}/{}",
                war.occupy_hold.max(war.retake_hold),
                hold_ticks()
            ),
        })
    } else {
        None
    };
    if update.captured || update.retaken {
        let mut ids = Vec::new();
        if let Some(raid) = war.raid.take() {
            ids.extend(raid.living);
            ids.extend(raid.temps);
        }
        if let Some(ram) = war.ram.take() {
            ids.extend(ram.parts);
        }
        if let Some(tower) = war.siege_tower.take() {
            ids.extend(tower.parts);
        }
        if let Some(shot) = war.treb_shot.take()
            && shot.part >= 0
        {
            ids.push(shot.part);
        }
        war.pending_despawn.extend(ids);
        war.pending_unticket.append(&mut war.ticketed);
        war.alarm = false;
        war.hold_lockout = tick.saturating_add(HOLD_LOCKOUT_TICKS);
        war.repair_until = tick.saturating_add(HOLD_LOCKOUT_TICKS + REPAIR_WINDOW_TICKS);
    }
    (
        update.captured,
        update.retaken,
        war.kingdom.clone(),
        war.scene.keep(),
        occupy_need,
        event,
    )
}

fn enqueue_capture(flip: PendingFlip) {
    lock().pending_flips.push_back(flip);
}

fn drain_one_capture() {
    for _ in 0..CAPTURES_PER_TICK {
        let flip = {
            let mut dir = lock();
            dir.pending_flips.pop_front()
        };
        let Some(flip) = flip else {
            return;
        };
        commit_capture(flip);
    }
}

fn commit_capture(flip: PendingFlip) {
    if flip.captured {
        let owner_name = flip.occupier.as_str().to_owned();
        let mut dir = lock();
        let tick = dir.tick;
        dir.add_score(&owner_name, 1);
        dir.persist_owner(&flip.town_id, flip.occupier.clone());
        crate::damage::note_capture(&flip.town_id);
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Capture,
            actor: flip.raiders_in_keep as i32,
            other: Some(flip.need as i32),
            x: f64::from(flip.keep.x),
            z: f64::from(flip.keep.z),
            note: format!(
                "{} captured by {owner_name} raiders_in_keep={} need={} hold={HOLD_TICKS}",
                flip.town_id, flip.raiders_in_keep, flip.need
            ),
        });
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::FlagChanged,
            actor: flip.raiders_in_keep as i32,
            other: Some(flip.need as i32),
            x: f64::from(flip.keep.x),
            z: f64::from(flip.keep.z),
            note: format!("{} flag raised for {owner_name}", flip.town_id),
        });
        drop(dir);
        credit_nearby(flip.keep, Some(&owner_name), 1, 0);
        swap_flag_banner(&flip.town_id, &flip.occupier);
        tracing::info!(
            "kingdoms town captured town={} raiders_in_keep={} need={} hold={HOLD_TICKS} score={owner_name}",
            flip.town_id,
            flip.raiders_in_keep,
            flip.need
        );
        if parse_town_id(&flip.town_id).is_some_and(|(_, cx, cz)| cx == 0 && cz == 0) {
            tracing::info!(
                "kingdoms capital captured town={} raiders_in_keep={} need={} hold={HOLD_TICKS}",
                flip.town_id,
                flip.raiders_in_keep,
                flip.need
            );
        }
    } else {
        let owner = TownOwner::kingdom(&flip.kingdom);
        let mut dir = lock();
        let tick = dir.tick;
        dir.add_score(flip.kingdom.as_str(), 1);
        dir.persist_owner(&flip.town_id, owner.clone());
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::Retake,
            actor: flip.defenders_in_keep as i32,
            other: Some(flip.raiders_in_keep as i32),
            x: f64::from(flip.keep.x),
            z: f64::from(flip.keep.z),
            note: format!("{} retaken by {}", flip.town_id, flip.kingdom.as_str()),
        });
        dir.push_event(CombatEvent {
            tick,
            kind: CombatKind::FlagChanged,
            actor: flip.defenders_in_keep as i32,
            other: Some(flip.raiders_in_keep as i32),
            x: f64::from(flip.keep.x),
            z: f64::from(flip.keep.z),
            note: format!(
                "{} flag restored for {}",
                flip.town_id,
                flip.kingdom.as_str()
            ),
        });
        drop(dir);
        credit_nearby(flip.keep, Some(flip.kingdom.as_str()), 1, 0);
        swap_flag_banner(&flip.town_id, &owner);
        tracing::info!(
            "kingdoms town retaken town={} defenders_in_keep={} owner={}",
            flip.town_id,
            flip.defenders_in_keep,
            flip.kingdom.as_str()
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
    town_has_player_within(world, town_id, PLAYER_LIVE_RANGE)
}

fn town_has_player_within(world: &World, town_id: &str, range: f64) -> bool {
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
    player_near_posts(world, &posts, range)
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
    let want = (near || soaking()) && (was_live || under_budget);
    if want && !was_live {
        materialize_raid(world, town_id, tick);
        return;
    }
    if tickets_pending(town_id) || raid_still_warming(town_id, tick) {
        // Keep the front live while tickets flush so a first-contact hitch
        // cannot fold the raid back to abstract and drop the body count.
        return;
    }
    if !want && was_live {
        dematerialize_raid(world, town_id);
    }
}

fn materialize_raid(_world: &Arc<World>, town_id: &str, tick: u32) {
    {
        let mut dir = lock();
        let Some(war) = dir.towns.get_mut(town_id) else {
            return;
        };
        let Some(raid) = war.raid.as_mut() else {
            return;
        };
        raid.materialized = true;
        if raid.warm_until <= tick {
            raid.warm_until = tick.saturating_add(WARM_TICKS);
        }
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
    queue_raid_warmup(town_id);
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
        if let Some(tower) = war.siege_tower.take() {
            despawn.extend(tower.parts);
        }
        if let Some(shot) = war.treb_shot.take()
            && shot.part >= 0
        {
            despawn.push(shot.part);
        }
        let tickets = std::mem::take(&mut war.ticketed);
        war.pending_tickets.clear();
        war.ticket_cursor = 0;
        war.wired.clear();
        (despawn, tickets)
    };
    despawn_entities(world, &despawn);
    flush_tickets_off_tick(world, tickets, false);
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
        FlagPresence::stacked(raiders_in_keep, defenders_in_keep),
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
    flush_tickets_off_tick(world, tickets, false);
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

fn occupier_from_flag(
    snapshots: &[(Arc<dyn EntityBase>, Faction)],
    flag: (i32, i32),
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
        if !on_flag_plate(flag, pos.x, pos.z) {
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
        let dir = lock();
        let Some(war) = dir.towns.get(town_id) else {
            return;
        };
        let Some(raid) = war.raid.as_ref() else {
            return;
        };
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
        let remaining = {
            let mut dir = lock();
            if let Some(war) = dir.towns.get_mut(town_id)
                && let Some(raid) = &mut war.raid
            {
                raid.temps.extend(extras.iter().copied());
                raid.extra_left = raid.extra_left.saturating_sub(spawned as u16);
                raid.extra_left
            } else {
                0
            }
        };
        tracing::info!(
            town = town_id,
            spawned,
            remaining,
            "spawned extra defenders for raid"
        );
    }
}

#[expect(
    dead_code,
    reason = "warmup only applies tickets; extras spawn after warm_until"
)]
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
        flush_tickets_off_tick(&world, tickets.clone(), false);
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

/// Active battles for `/kingdom fronts`.
#[must_use]
pub fn format_fronts() -> String {
    let front = war_front();
    let mut lines: Vec<String> = front
        .towns
        .iter()
        .filter(|t| t.raid_active)
        .map(|t| {
            format!(
                "{} side={} owner={} home={} live={} raiders={} defenders={} hold={}/{} flag={}",
                t.id,
                t.raid_side.as_deref().unwrap_or("-"),
                t.owner,
                t.kingdom,
                t.live,
                t.raiders,
                t.defenders,
                t.occupy_hold,
                t.occupy_need,
                t.flag_status
            )
        })
        .collect();
    if lines.is_empty() {
        "no active fronts".to_owned()
    } else {
        lines.sort();
        lines.join(" | ")
    }
}

/// Persisted rank line for `/war stats` / `/kingdom stats`.
#[must_use]
pub fn format_player_stats(uuid: &str, name: &str) -> String {
    let dir = lock();
    let kingdom = dir
        .player_kingdom(uuid)
        .map_or_else(|| "none".to_owned(), |k| k.as_str().to_owned());
    let stats = dir.player_stats.get(uuid).cloned().unwrap_or_default();
    format!(
        "{name} kingdom={kingdom} rank={} renown={} kills={} captures={} defenses={}",
        stats.rank().as_str(),
        stats.renown(),
        stats.kills,
        stats.captures,
        stats.defenses
    )
}

/// Teleport a player to a friendly rally and materialise the front if it is abstract.
pub fn deploy_player(player: &Player, town_arg: Option<&str>) -> Result<String, String> {
    let uuid = player.gameprofile.id.to_string();
    let world = Arc::clone(&player.get_entity().world.load());
    let pos = player.get_entity().pos.load();
    let (kingdom, town_id, post, live) = {
        let dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return Err("join a kingdom first (/kingdom join alder|raven)".to_owned());
        };
        let town_id = if let Some(arg) = town_arg {
            resolve_town_id(&world, pos, arg).ok_or_else(|| format!("unknown town {arg}"))?
        } else {
            pick_deploy_front(&dir, &kingdom).ok_or_else(|| "no active fronts".to_owned())?
        };
        let Some(war) = dir.towns.get(&town_id) else {
            return Err(format!("town {town_id} is not cached yet"));
        };
        if war.raid.is_none() {
            return Err(format!("no battle at {town_id}"));
        }
        let keep = war.scene.keep();
        let friendly = war.owner.as_str() == kingdom.as_str() || war.kingdom == kingdom;
        // Keep posts store the planner roof/floor Y. Deploy to the threatened
        // gate (y=0, live surface) so the player lands in the fight, not the attic.
        let post = if friendly {
            war.threatened
                .unwrap_or_else(|| ScenePost::new(keep.x, keep.z))
        } else {
            war.raid
                .as_ref()
                .map(|r| r.camp)
                .or_else(|| war.scene.raid_camps().first().copied())
                .or_else(|| war.scene.road_approach())
                .unwrap_or(keep)
        };
        let live = war.raid.as_ref().is_some_and(|r| r.materialized);
        (kingdom, town_id, post, live)
    };
    queue_raid_warmup(&town_id);
    if !live {
        let tick = lock().tick;
        materialize_raid(&world, &town_id, tick);
    }
    let y = standing_y(&world, post);
    let dest = Vector3::new(
        f64::from(post.x) + 0.5,
        f64::from(y),
        f64::from(post.z) + 0.5,
    );
    player.teleport(dest, None, None, world);
    Ok(format!(
        "deployed to {town_id} as {} ({})",
        kingdom.as_str(),
        if live { "live" } else { "materialising" }
    ))
}

fn pick_deploy_front(dir: &Director, kingdom: &KingdomId) -> Option<String> {
    let mut best: Option<(u8, String)> = None;
    for (id, war) in &dir.towns {
        if war.raid.is_none() {
            continue;
        }
        let live = war.raid.as_ref().is_some_and(|r| r.materialized);
        let allied = war.kingdom == *kingdom || war.owner.as_str() == kingdom.as_str();
        let rank = match (live, allied) {
            (true, true) => 0u8,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, id.clone()));
        }
    }
    best.map(|(_, id)| id)
}

/// Upsert a named hero/commander for a kingdom.
pub fn set_hero(kingdom: &str, name: &str, title: &str, line: &str) -> Hero {
    let hero = Hero {
        kingdom: KingdomId::new(kingdom).as_str().to_owned(),
        name: name.trim().to_owned(),
        title: title.trim().to_owned(),
        line: line.trim().to_owned(),
    };
    let mut dir = lock();
    dir.heroes.insert(hero.kingdom.clone(), hero.clone());
    hero
}

#[must_use]
pub fn all_heroes() -> Vec<Hero> {
    let mut heroes: Vec<Hero> = lock().heroes.values().cloned().collect();
    heroes.sort_by(|a, b| a.kingdom.cmp(&b.kingdom));
    heroes
}

/// Broadcast a line as the kingdom's named hero.
pub fn shout_hero(kingdom: &str, text: &str) -> Result<Hero, String> {
    let key = KingdomId::new(kingdom).as_str().to_owned();
    let mut dir = lock();
    let Some(hero) = dir.heroes.get_mut(&key) else {
        return Err(format!("no hero for {kingdom}"));
    };
    let spoken = text.trim();
    if !spoken.is_empty() {
        spoken.clone_into(&mut hero.line);
    }
    let hero = hero.clone();
    let worlds = dir.attached_worlds();
    drop(dir);
    let line = if spoken.is_empty() {
        hero.line.as_str()
    } else {
        spoken
    };
    let title = if hero.title.is_empty() {
        String::new()
    } else {
        format!(" {}", hero.title)
    };
    let msg =
        TextComponent::text(format!("<{}{title}> {line}", hero.name)).color_named(NamedColor::Gold);
    for world in worlds {
        world.broadcast_system_message(&msg, false);
    }
    Ok(hero)
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
            "{} owner={} home={} alarm={} raid={} live={} side={} hold={}/{} flag={}@{}/{} repair_until={}",
            t.id,
            t.owner,
            t.kingdom,
            t.alarm,
            t.raid_active,
            t.live,
            t.raid_side.as_deref().unwrap_or("-"),
            t.occupy_hold,
            t.occupy_need,
            t.flag_status,
            t.flag.0,
            t.flag.1,
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

fn retry_keep_teleports(world: &World) {
    let uuids: Vec<String> = {
        let dir = lock();
        if dir.pending_keep_tp.is_empty() {
            return;
        }
        dir.pending_keep_tp.iter().cloned().collect()
    };
    let players = world.players.load();
    for uuid in uuids {
        let Some(player) = players
            .iter()
            .find(|p| p.gameprofile.id.to_string() == uuid)
        else {
            continue;
        };
        teleport_to_keep(player);
    }
}

pub fn join_kingdom(player: &Player, name: &str) -> KingdomId {
    let id = lock().join_kingdom(player, KingdomId::new(name));
    if !player_already_in_home_town(player) {
        teleport_to_keep(player);
    }
    id
}

fn player_already_in_home_town(player: &Player) -> bool {
    let pos = player.get_entity().pos.load();
    let dir = lock();
    let uuid = player.gameprofile.id.to_string();
    let Some(kingdom) = dir.player_kingdom(&uuid) else {
        return false;
    };
    let Some(plan) = keep_stand_plan(kingdom, &dir.towns) else {
        return false;
    };
    let Some(war) = dir.towns.get(&plan.town_id) else {
        return false;
    };
    let keep = war.scene.keep();
    let mut posts = vec![keep];
    posts.extend(war.scene.gates().iter().map(|g| ScenePost::new(g.x, g.z)));
    posts.iter().any(|post| {
        (pos.x - f64::from(post.x)).hypot(pos.z - f64::from(post.z)) <= PLAYER_APPROACH_RANGE
    })
}

fn teleport_to_keep(player: &Player) {
    let uuid = player.gameprofile.id.to_string();
    let world = Arc::clone(&player.get_entity().world.load());
    let stand = {
        let dir = lock();
        let Some(kingdom) = dir.player_kingdom(&uuid).cloned() else {
            return;
        };
        if keep_stand_plan(&kingdom, &dir.towns).is_none() {
            None
        } else {
            pick_keep_stand(&world, &kingdom, &dir.towns)
        }
    };
    let Some((x, y, z)) = stand else {
        let mut dir = lock();
        dir.pending_keep_tp
            .insert(player.gameprofile.id.to_string());
        return;
    };
    {
        let mut dir = lock();
        dir.pending_keep_tp
            .remove(&player.gameprofile.id.to_string())
    };
    let _ = player.set_respawn_point(
        world.dimension.clone(),
        BlockPos::new(x, y, z),
        0.0,
        0.0,
        true,
    );
    tracing::info!(
        "kingdoms keep spawn player={} x={} y={} z={}",
        player.gameprofile.name,
        x,
        y,
        z
    );
    player.teleport(
        Vector3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5),
        None,
        None,
        world,
    );
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
    fn capture_scars_then_ruins_and_builders_step_back() {
        let id = "director-capture-damage:0:0";
        crate::damage::set_damage_state(id, crate::damage::DamageState::Intact);
        crate::damage::note_capture(id);
        assert_eq!(
            crate::damage::damage_state(id),
            crate::damage::DamageState::Scarred
        );
        crate::damage::note_capture(id);
        assert_eq!(
            crate::damage::damage_state(id),
            crate::damage::DamageState::Ruined
        );
        assert_eq!(
            crate::damage::step_repair(id),
            crate::damage::DamageState::Scarred
        );
        assert_eq!(
            crate::damage::step_repair(id),
            crate::damage::DamageState::Intact
        );
        crate::damage::set_damage_state(id, crate::damage::DamageState::Intact);
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
        assert_eq!(REPAIR_WINDOW_TICKS, 2400);
        assert_eq!(HOLD_LOCKOUT_TICKS + REPAIR_WINDOW_TICKS, 2800);
    }

    #[test]
    fn spawn_and_rout_caps_are_spread() {
        assert_eq!(SPAWNS_PER_TICK, 3);
        assert_eq!(SPAWNS_PER_WORLD_TICK, 12);
        assert_eq!(ROUT_TAGS_PER_TICK, 8);
        assert_eq!(DESPAWNS_PER_TICK, 12);
        assert_eq!(GHOST_SPAWNS_PER_TICK, 8);
        assert_eq!(REPAIR_SCAN_COLUMNS, 4);
        assert_eq!(REPAIR_PLACE_PER_TICK, 2);
        assert_eq!(TICKETS_PER_TICK, 1);
        assert_eq!(TICKETS_PER_WORLD_TICK, 3);
        assert_eq!(CAPTURES_PER_TICK, 1);
        assert_eq!(PORTCULLIS_BARS_PER_TICK, 2);
        assert_eq!(JOIN_AMORTIZE_TICKS, 40);
        assert_eq!(JOIN_ENTITY_BUDGET, 64);
        assert!(in_join_ai_window(0, 0, 480));
        assert!(!in_join_ai_window(64, 0, 480));
        assert!(in_join_ai_window(64, 1, 480));
        let aabb = BoundingBox::new(
            Vector3::new(0.0, -64.0, 0.0),
            Vector3::new(400.0, 320.0, 400.0),
        );
        let mut min_x = f64::MAX;
        let mut max_x = f64::MIN;
        for i in 0..JOIN_AMORTIZE_TICKS {
            let strip = aabb_x_strip(&aabb, i, JOIN_AMORTIZE_TICKS);
            assert_eq!(strip.min.z, aabb.min.z);
            assert_eq!(strip.max.z, aabb.max.z);
            min_x = min_x.min(strip.min.x);
            max_x = max_x.max(strip.max.x);
            assert!(strip.max.x > strip.min.x);
            assert!(strip.max.x - strip.min.x < (aabb.max.x - aabb.min.x) * 0.5);
        }
        assert!((min_x - aabb.min.x).abs() < 1.0e-9);
        assert!((max_x - aabb.max.x).abs() < 1.0e-9);
        assert_eq!(DROPS_PER_TICK, 8);
        assert_eq!(HEAVY_TOWNS_PER_TICK, 1);
        assert_eq!(STORMING_COUNT, 8);
        assert!((STORMING_RANGE - 16.0).abs() < f64::EPSILON);
        assert_eq!(STUCK_STORM_COUNT, 3);
        assert!((STUCK_STORM_RANGE - 24.0).abs() < f64::EPSILON);
        assert!((PLAYER_SIM_RANGE - 64.0).abs() < f64::EPSILON);
        assert!((PLAYER_CULL_RANGE - 96.0).abs() < f64::EPSILON);
        assert_eq!(resolve_stand_y(80, Some(70), 63), 80);
        assert_eq!(resolve_stand_y(0, Some(70), 63), 70);
        assert_eq!(resolve_stand_y(0, None, 63), 64);
    }

    #[test]
    fn kit_tiers_follow_renown() {
        assert_eq!(kit_weapon(siege::Rank::Levy), &Item::WOODEN_SWORD);
        assert_eq!(kit_shield(&KingdomId::new("alder")), Some(&Item::SHIELD));
        assert_eq!(kit_shield(&KingdomId::new("raven")), Some(&Item::SHIELD));
        assert_eq!(kit_weapon(siege::Rank::Freeman), &Item::STONE_SWORD);
        assert_eq!(kit_weapon(siege::Rank::Knight), &Item::IRON_SWORD);
        let levy = kit_armor(siege::Rank::Levy);
        assert_eq!(levy[1].1.id, Item::LEATHER_CHESTPLATE.id);
        let sergeant = kit_armor(siege::Rank::Sergeant);
        assert_eq!(sergeant[1].1.id, Item::CHAINMAIL_CHESTPLATE.id);
        let knight = kit_armor(siege::Rank::Banneret);
        assert_eq!(knight[1].1.id, Item::IRON_CHESTPLATE.id);
        assert_eq!(
            kingdom_banner(&KingdomId::new("raven")).id,
            Item::BLUE_BANNER.id
        );
    }

    #[test]
    fn war_front_marks_unmaterialised_raids_as_not_live() {
        let mut dir = Director::new();
        dir.tick = 10;
        let front = dir.war_front();
        assert!(front.towns.iter().all(|t| !t.live));
        assert_eq!(crate::abstract_combat::DEFAULT_BODY_BUDGET, 240);
        assert_eq!(crate::abstract_combat::PLAYER_LIVE_RANGE, 128.0);
        assert_eq!(crate::abstract_combat::PLAYER_APPROACH_RANGE, 256.0);
        assert_eq!(crate::lod::CIVILIAN_SLEEP_RANGE, 64.0);
        assert_eq!(crate::lod::PLAYER_CULL_RANGE, 96.0);
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
            false,
            None,
            march,
            keep,
            march,
            patrol,
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
            false,
            None,
            march,
            keep,
            march,
            patrol,
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
            false,
            None,
            march,
            keep,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (patrol.x, patrol.z));
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Guard),
            true,
            true,
            false,
            false,
            None,
            march,
            keep,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (march.x, march.z));
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Archer),
            true,
            false,
            false,
            false,
            None,
            march,
            keep,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!((dest.x, dest.z), (patrol.x, patrol.z));
        let flag = PostTarget {
            x: 11,
            y: 64,
            z: 11,
        };
        let dest = dest_for(
            &Faction::Pillager,
            MarchJob::Assault,
            None,
            true,
            false,
            true,
            false,
            None,
            march,
            flag,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        let inner = Rect::new(0, 0, 40, 40);
        let gatehouse = Rect::new(16, 0, 24, 8);
        let dest = overlay_flag_dest(
            dest,
            &Faction::Pillager,
            true,
            20.5,
            22.5,
            inner,
            gatehouse,
            &[],
            true,
            false,
            flag,
        );
        assert_eq!((dest.x, dest.z), (flag.x, flag.z));
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            true,
            20.5,
            -3.5,
            inner,
            gatehouse,
            &[],
            false,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (march.x, march.z),
            "bearer waits at the gate until it falls"
        );
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            false,
            20.5,
            22.5,
            inner,
            gatehouse,
            &[],
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (flag.x, flag.z),
            "courtyard raid paths to the plate"
        );
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            true,
            20.5,
            -3.5,
            inner,
            gatehouse,
            &[],
            true,
            false,
            flag,
        );
        assert!(
            inner.contains(dest.x, dest.z) && !gatehouse.contains(dest.x, dest.z),
            "after the breach the column walks through the gatehouse, dest={dest:?}"
        );
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            false,
            f64::from(flag.x) + 10.5,
            f64::from(flag.z) + 0.5,
            inner,
            gatehouse,
            &[],
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (flag.x, flag.z),
            "raiders already inside {STUCK_STORM_RANGE} of the mast walk the plate"
        );
    }

    #[test]
    fn one_hundred_twenty_raiders_path_the_gatehouse_to_the_plate() {
        let flag = PostTarget {
            x: 11,
            y: 64,
            z: 22,
        };
        let march = PostTarget { x: 20, y: 64, z: 4 };
        let inner = Rect::new(0, 8, 40, 40);
        let gatehouse = Rect::new(16, 0, 24, 8);
        let mut through_gate = 0u32;
        let mut on_plate = 0u32;
        for i in 0..120 {
            let x = 16.5 + f64::from(i % 12);
            let z = -2.5 - f64::from(i / 12) * 0.4;
            let dest = overlay_flag_dest(
                march,
                &Faction::Pillager,
                i == 0,
                x,
                z,
                inner,
                gatehouse,
                &[],
                true,
                false,
                flag,
            );
            assert!(
                inner.contains(dest.x, dest.z) && !gatehouse.contains(dest.x, dest.z),
                "raider {i} at gate must walk through the gatehouse dest={dest:?}"
            );
            through_gate += 1;
            let dest = overlay_flag_dest(
                dest,
                &Faction::Pillager,
                i == 0,
                f64::from(dest.x) + 0.5,
                f64::from(dest.z) + 0.5,
                inner,
                gatehouse,
                &[],
                true,
                false,
                flag,
            );
            assert_eq!(
                (dest.x, dest.z),
                (flag.x, flag.z),
                "once inside, raider {i} (bearer={}) marches to the plate",
                i == 0
            );
            on_plate += 1;
        }
        assert_eq!(through_gate, 120);
        assert_eq!(on_plate, 120);
        let need = capture_need(120);
        assert!(need >= 3);
        let owner = TownOwner::Kingdom("alder".to_owned());
        let mut hold = 0;
        let mut update = siege::HoldUpdate::default();
        for _ in 0..HOLD_TICKS {
            update = tick_flag_hold(
                &owner,
                FlagPresence::stacked(120, 0),
                hold,
                0,
                need,
                need,
                1,
            );
            hold = update.occupy_hold;
        }
        assert!(
            update.captured,
            "120 raiders on an empty plate flip the town"
        );
        let paused = tick_flag_hold(
            &owner,
            FlagPresence::stacked(120, 120),
            40,
            0,
            need,
            need,
            1,
        );
        assert!(
            paused.paused,
            "an equal defender stack on the plate holds the 120-raider push"
        );
        assert!(!paused.captured);
        let stacked = tick_flag_hold(&owner, FlagPresence::stacked(120, 1), 40, 0, need, need, 1);
        assert!(
            !stacked.paused && stacked.occupy_hold > 40,
            "one keep guard on the plate does not freeze a 120-raider column"
        );
        assert_eq!(STORMING_COUNT, 8);
        assert!((STORMING_RANGE - 16.0).abs() < f64::EPSILON);
        assert_eq!(STUCK_STORM_COUNT, 3);
        assert!((STUCK_STORM_RANGE - 24.0).abs() < f64::EPSILON);
    }

    #[test]
    fn standard_bearer_completes_a_flag_capture_in_a_live_raid() {
        assert_eq!(crate::faction::TAG_BEARER, "kingdoms:bearer");
        let flag = PostTarget {
            x: 40,
            y: 64,
            z: 40,
        };
        let march = PostTarget { x: 1, y: 64, z: 1 };
        let inner = Rect::new(30, 30, 50, 50);
        let gatehouse = Rect::new(36, 28, 44, 32);
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            true,
            40.5,
            40.5,
            inner,
            gatehouse,
            &[],
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (flag.x, flag.z),
            "raid captain standard-bearer paths to the plate"
        );
        let owner = TownOwner::Kingdom("alder".to_owned());
        let mut hold = 0;
        let mut update = siege::HoldUpdate::default();
        for _ in 0..HOLD_TICKS {
            update = tick_flag_hold(&owner, FlagPresence::stacked(3, 0), hold, 0, 3, 3, 1);
            hold = update.occupy_hold;
        }
        assert!(update.captured);
        assert_eq!(
            owner_banner(&TownOwner::Pillager),
            owner_banner(&TownOwner::parse("pillager"))
        );
    }

    #[test]
    fn capital_raid_walks_outer_then_civic_then_castle_to_the_plate() {
        use crate::geom::Dir;
        use crate::info::GateKind;
        let flag = PostTarget {
            x: 11,
            y: 64,
            z: 22,
        };
        let march = PostTarget {
            x: 20,
            y: 64,
            z: -80,
        };
        let inner = Rect::new(0, 8, 40, 40);
        let gatehouse = Rect::new(16, 0, 24, 8);
        let gates = [
            GateInfo {
                kind: GateKind::Town,
                x: 20,
                z: -80,
                facing: Dir::North,
            },
            GateInfo {
                kind: GateKind::Town,
                x: 20,
                z: -20,
                facing: Dir::North,
            },
            GateInfo {
                kind: GateKind::Castle,
                x: 20,
                z: 4,
                facing: Dir::North,
            },
        ];
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            false,
            20.5,
            -96.0,
            inner,
            gatehouse,
            &gates,
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (gates[0].x, gates[0].z),
            "outside the outer wall, dest is the town gate"
        );
        let dest = overlay_flag_dest(
            dest,
            &Faction::Pillager,
            false,
            20.5,
            -80.0,
            inner,
            gatehouse,
            &gates,
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (gates[1].x, gates[1].z),
            "at the town gate, dest is the civic ring"
        );
        let dest = overlay_flag_dest(
            dest,
            &Faction::Pillager,
            false,
            20.5,
            -20.0,
            inner,
            gatehouse,
            &gates,
            true,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (gates[2].x, gates[2].z),
            "at the civic gate, dest is the castle gate"
        );
        let dest = overlay_flag_dest(
            dest,
            &Faction::Pillager,
            false,
            20.5,
            4.0,
            inner,
            gatehouse,
            &gates,
            true,
            false,
            flag,
        );
        assert!(
            inner.contains(dest.x, dest.z) && !gatehouse.contains(dest.x, dest.z),
            "past the castle gate the column walks the gatehouse dest={dest:?}"
        );
        let dest = overlay_flag_dest(
            march,
            &Faction::Pillager,
            false,
            20.5,
            -96.0,
            inner,
            gatehouse,
            &gates,
            false,
            false,
            flag,
        );
        assert_eq!(
            (dest.x, dest.z),
            (gates[0].x, gates[0].z),
            "before any breach the column walks the outer town gate, not the castle"
        );
    }

    #[test]
    fn packed_storm_picks_the_gate_with_bodies_not_the_first() {
        use crate::geom::Dir;
        use crate::info::GateKind;
        let outer = GateInfo {
            kind: GateKind::Town,
            x: 20,
            z: -80,
            facing: Dir::North,
        };
        let castle = GateInfo {
            kind: GateKind::Castle,
            x: 20,
            z: 4,
            facing: Dir::North,
        };
        let ghosts: Vec<GhostSoldier> = (0..8)
            .map(|i| GhostSoldier {
                faction: Faction::Pillager,
                x: 20.5,
                y: 64.0,
                z: -80.0 + f64::from(i) * 0.1,
                bearer: false,
                raider: true,
            })
            .collect();
        let packed = packed_storm_gate(
            &[],
            &ghosts,
            &[outer, castle],
            STORMING_RANGE,
            STORMING_COUNT,
        );
        assert_eq!(packed.map(|g| (g.x, g.z)), Some((outer.x, outer.z)));
        assert_eq!(
            nearest_gate_to_raiders(&[], &ghosts, &[outer, castle]).map(|g| (g.x, g.z)),
            Some((outer.x, outer.z))
        );
        assert!(should_virtualize(&[], 0.0, 0.0));
        assert!(!should_virtualize(&[(0.0, 0.0)], 10.0, 10.0));
        assert!(should_virtualize(&[(0.0, 0.0)], 200.0, 0.0));
        assert_eq!(raid_outcome_reason(3, 0, false, 8, 100, 90), "on_plate");
        assert_eq!(raid_outcome_reason(0, 4, false, 8, 100, 90), "in_courtyard");
        assert_eq!(raid_outcome_reason(0, 0, true, 8, 100, 90), "storming");
        assert_eq!(raid_outcome_reason(0, 0, false, 0, 100, 90), "repelled");
        assert_eq!(raid_outcome_reason(0, 0, false, 8, 400, 100), "stuck");
        assert_eq!(raid_outcome_reason(0, 0, false, 8, 150, 100), "marching");
    }

    #[test]
    fn capturing_defenders_hold_the_gate_not_the_mast() {
        let keep = PostTarget {
            x: 11,
            y: 64,
            z: 22,
        };
        let march = PostTarget { x: 20, y: 64, z: 4 };
        let work = PostTarget { x: 9, y: 64, z: 9 };
        let home = PostTarget { x: 3, y: 64, z: 3 };
        let patrol = PostTarget { x: 5, y: 70, z: 5 };
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Guard),
            true,
            true,
            true,
            true,
            None,
            march,
            keep,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!(
            (dest.x, dest.z),
            (march.x, march.z),
            "capturing guards hold the gate instead of piling the mast"
        );
        let dest = dest_for(
            &Faction::Kingdom(KingdomId::new("alder")),
            MarchJob::Assault,
            Some(VillagerRole::Civilian),
            true,
            false,
            true,
            true,
            None,
            march,
            keep,
            march,
            patrol,
            work,
            home,
            patrol,
            6_000,
        );
        assert_eq!(
            (dest.x, dest.z),
            (work.x, work.z),
            "capturing civilians stay at work, not the mast"
        );
        let inner = Rect::new(0, 8, 40, 40);
        let gatehouse = Rect::new(16, 0, 24, 8);
        let dest = overlay_flag_dest(
            march,
            &Faction::Kingdom(KingdomId::new("alder")),
            false,
            20.5,
            -3.5,
            inner,
            gatehouse,
            &[],
            true,
            true,
            keep,
        );
        assert_eq!(
            (dest.x, dest.z),
            (march.x, march.z),
            "defenders keep their post instead of rushing the plate dest={dest:?}"
        );
        assert_eq!(chat::line(ShoutKind::Charge), "To the flag!");
        let militia = overlay_civilian_posts(
            keep,
            Some(VillagerRole::Civilian),
            true,
            true,
            march,
            work,
            keep,
            CivilianDuty::Militia,
        );
        assert_eq!(
            (militia.x, militia.z),
            (keep.x, keep.z),
            "capturing overlay leaves militia dest unchanged"
        );
        let dest = overlay_flag_dest(
            march,
            &Faction::Kingdom(KingdomId::new("alder")),
            false,
            f64::from(keep.x) + 0.5,
            f64::from(keep.z) + 60.5,
            inner,
            gatehouse,
            &[],
            true,
            true,
            keep,
        );
        assert_eq!(
            (dest.x, dest.z),
            (march.x, march.z),
            "defenders beyond 48 blocks of the plate keep their post"
        );
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
            occupy_need: 3,
            ram_hp: 12.0,
            flag: (10, 20),
            flag_status: "capturing".to_owned(),
            flag_paused: false,
            flag_attackers: 4,
            flag_defenders: 0,
        };
        assert_eq!(state.owner, "pillager");
        assert!(state.gate_breached);
        assert_eq!(state.occupy_need, 3);
        assert_eq!(state.flag_status, "capturing");
    }

    #[test]
    fn militia_lines_the_inner_gate_and_carries_water() {
        let keep = PostTarget { x: 1, y: 64, z: 1 };
        let inner = PostTarget {
            x: 10,
            y: 64,
            z: 10,
        };
        let well = PostTarget { x: 4, y: 64, z: 4 };
        let home = PostTarget { x: 8, y: 64, z: 8 };
        let alarm_line = overlay_civilian_posts(
            home,
            Some(VillagerRole::Civilian),
            true,
            false,
            inner,
            well,
            keep,
            CivilianDuty::Militia,
        );
        assert_eq!((alarm_line.x, alarm_line.z), (inner.x, inner.z));
        let water = overlay_civilian_posts(
            home,
            Some(VillagerRole::Civilian),
            false,
            false,
            inner,
            well,
            keep,
            CivilianDuty::Militia,
        );
        assert_eq!((water.x, water.z), (well.x, well.z));
        let priest = overlay_civilian_posts(
            home,
            Some(VillagerRole::Civilian),
            true,
            false,
            inner,
            well,
            keep,
            CivilianDuty::Priest,
        );
        assert_eq!((priest.x, priest.z), (keep.x, keep.z));
    }

    #[test]
    fn squad_orders_follow_hold_and_charge() {
        let dest = PostTarget { x: 0, y: 64, z: 0 };
        let charge = PostTarget {
            x: 20,
            y: 64,
            z: 20,
        };
        let alder = KingdomId::new("alder");
        let cmd = SquadCommand {
            order: SquadOrder::Charge,
            kingdom: alder.clone(),
            hold: PostTarget { x: 5, y: 64, z: 5 },
            x: 1.0,
            y: 64.0,
            z: 1.0,
            at_tick: 1,
        };
        let out = overlay_squad_order(
            dest,
            &Faction::Kingdom(alder.clone()),
            1.0,
            1.0,
            std::slice::from_ref(&cmd),
            charge,
        );
        assert_eq!((out.x, out.z), (charge.x, charge.z));
        let mut hold = cmd;
        hold.order = SquadOrder::Hold;
        let out = overlay_squad_order(dest, &Faction::Kingdom(alder), 1.0, 1.0, &[hold], charge);
        assert_eq!((out.x, out.z), (5, 5));
        assert_eq!(SquadOrder::parse("rally"), Some(SquadOrder::Follow));
        assert_eq!(SquadOrder::Follow.next(), SquadOrder::Hold);
        assert_eq!(PUNCH_REPAIR_PER_TICK, 2);
        assert_eq!(PUNCH_BREAK_PER_TICK, 2);
        assert_eq!(PORTCULLIS_BARS_PER_TICK, 2);
        assert_eq!(engines::ENGINE_PARTS_PER_TICK, 2);
        assert_eq!(crate::ram::RAM_PARTS_PER_TICK, 2);
        assert!((PHASE_WARN_MS - 20.0).abs() < f64::EPSILON);
        assert_eq!(resolve_stand_y(117, None, 63), 117);
    }
}
