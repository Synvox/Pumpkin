//! Headless Java client that logs in and stands still so a town ticks.
//!
//! Pumpkin only simulates entities in chunks a player (or a lasting FULL
//! ticket) is watching. This bot is the player: join, walk to `--x --y --z`,
//! answer keep-alives, and stay until Ctrl-C or `--seconds`.
//!
//! With `--fight` it joins a kingdom, runs `/kingdom fronts` and
//! `/kingdom deploy`, then melee-attacks nearby pillagers until a rank-up.
//!
//! The server must have `online_mode = false`, `encryption = false`. Compression
//! can stay on; the bot enables it if the login handshake asks.
//!
//!   kingdoms-raid-bot --host 127.0.0.1:25565 --x 1349 --y 80 --z 693 --seconds 180
//!   kingdoms-raid-bot --fight --name alder1 --x 1349 --y 80 --z 693 --seconds 600

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines
)]

use std::collections::HashMap;
use std::env;
use std::io::Cursor;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use bytes::Bytes;
use pumpkin_data::entity::EntityType;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_protocol::codec::item_stack_seralizer::ItemStackSerializer;
use pumpkin_protocol::java::client::config::{CFinishConfig, CKnownPacks};
use pumpkin_protocol::java::client::login::{CLoginSuccess, CSetCompression};
use pumpkin_protocol::java::client::play::{
    CDamageEvent, CEntityPositionSync, CKeepAlive, CLogin, CPlayerPosition, CRemoveEntities,
    CSetContainerContent, CSetContainerSlot, CSetEquipment, CSetHealth, CSetPlayerInventory,
    CSpawnEntity, CSystemChatMessage,
};
use pumpkin_protocol::java::packet_decoder::TCPNetworkDecoder;
use pumpkin_protocol::java::packet_encoder::TCPNetworkEncoder;
use pumpkin_protocol::java::server::config::{
    SAcknowledgeFinishConfig, SClientInformationConfig, SKnownPacks,
};
use pumpkin_protocol::java::server::handshake::SHandShake;
use pumpkin_protocol::java::server::login::{SLoginAcknowledged, SLoginStart};
use pumpkin_protocol::java::server::play::{
    SAttack, SChatCommand, SClientCommand, SConfirmTeleport, SKeepAlive, SPlayerLoaded,
    SPlayerPosition, SPlayerPositionRotation, SSwingArm,
};
use pumpkin_protocol::packet::MultiVersionJavaPacket;
use pumpkin_protocol::ser::{NetworkReadExt, NetworkWriteExt};
use pumpkin_protocol::{ClientPacket, ConnectionState, RawPacket, ServerPacket, VarInt};
use pumpkin_util::math::vector3::Vector3;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

const MELEE_RANGE: f64 = 3.5;
const WALK_STEP: f64 = 0.28;
const DISCOVER_RANGE: f64 = 96.0;

fn entity_id(ty: &EntityType) -> i32 {
    i32::from(ty.id)
}

fn is_hostile_kind(kind: i32) -> bool {
    kind == entity_id(&EntityType::PILLAGER)
        || kind == entity_id(&EntityType::VINDICATOR)
        || kind == entity_id(&EntityType::EVOKER)
        || kind == entity_id(&EntityType::RAVAGER)
        || kind == entity_id(&EntityType::WITCH)
        || kind == entity_id(&EntityType::ILLUSIONER)
        || kind == entity_id(&EntityType::VEX)
}

fn is_soldier_kind(kind: i32) -> bool {
    kind == entity_id(&EntityType::VILLAGER) || kind == entity_id(&EntityType::IRON_GOLEM)
}

struct Args {
    host: SocketAddr,
    name: String,
    x: f64,
    y: f64,
    z: f64,
    seconds: u64,
    commands: Vec<String>,
    fight: bool,
    prove: bool,
    kingdom: String,
}

fn parse_args() -> Result<Args, String> {
    let mut host = "127.0.0.1:25565".to_owned();
    let mut name = "kingdomsbot".to_owned();
    let mut x = 0.0;
    let mut y = 80.0;
    let mut z = 0.0;
    let mut seconds = 180u64;
    let mut commands = Vec::new();
    let mut fight = false;
    let mut prove = false;
    let mut kingdom = "alder".to_owned();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut next = |flag: &str| args.next().ok_or_else(|| format!("{flag} needs a value"));
        match arg.as_str() {
            "--host" => host = next("--host")?,
            "--name" => name = next("--name")?,
            "--x" => x = next("--x")?.parse().map_err(|e| format!("--x: {e}"))?,
            "--y" => y = next("--y")?.parse().map_err(|e| format!("--y: {e}"))?,
            "--z" => z = next("--z")?.parse().map_err(|e| format!("--z: {e}"))?,
            "--seconds" => {
                seconds = next("--seconds")?
                    .parse()
                    .map_err(|e| format!("--seconds: {e}"))?;
            }
            "--command" => commands.push(next("--command")?),
            "--fight" => fight = true,
            "--prove" => prove = true,
            "--kingdom" => kingdom = next("--kingdom")?,
            "-h" | "--help" => {
                println!(
                    "kingdoms-raid-bot --host 127.0.0.1:25565 --x X --y Y --z Z --seconds 180 [--command CMD]... [--fight|--prove --kingdom alder]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }
    let host: SocketAddr = host.parse().map_err(|e| format!("--host {host}: {e}"))?;
    Ok(Args {
        host,
        name,
        x,
        y,
        z,
        seconds,
        commands,
        fight,
        prove,
        kingdom,
    })
}

struct SeenMob {
    kind: i32,
    pos: Vector3<f64>,
}

struct Bot {
    name: String,
    reader: TCPNetworkDecoder<BufReader<OwnedReadHalf>>,
    writer: TCPNetworkEncoder<BufWriter<OwnedWriteHalf>>,
    position: Vector3<f64>,
    yaw: f32,
    pitch: f32,
    hostiles: HashMap<i32, SeenMob>,
    soldiers: HashMap<i32, SeenMob>,
    unknown: HashMap<i32, SeenMob>,
    chat: Vec<String>,
    last_rank: String,
    last_stats: String,
    dead: bool,
    needs_deploy: bool,
    last_tp: Instant,
    last_respawn_cmd: Instant,
    self_id: i32,
    spawn_log: u32,
    pkt_log: u32,
    pkt_ids: HashMap<i32, u32>,
}

impl Bot {
    async fn send_packet<P: ClientPacket + MultiVersionJavaPacket>(
        &mut self,
        packet: &P,
    ) -> Result<(), String> {
        let packet_id = P::to_id(CURRENT_MC_VERSION);
        let mut buf = Vec::new();
        buf.write_var_int(&VarInt(packet_id))
            .map_err(|err| format!("write packet id: {err:?}"))?;
        packet
            .write_packet_data(&mut buf, &CURRENT_MC_VERSION)
            .map_err(|err| format!("serialize packet: {err:?}"))?;
        self.writer
            .write_packet(Bytes::from(buf))
            .await
            .map_err(|err| format!("write socket: {err}"))?;
        self.writer
            .flush()
            .await
            .map_err(|err| format!("flush socket: {err}"))
    }

    async fn next_packet(&mut self) -> Result<RawPacket, String> {
        match timeout(Duration::from_secs(120), self.reader.get_raw_packet()).await {
            Ok(Ok(packet)) => Ok(packet),
            Ok(Err(err)) => Err(format!("decode: {err}")),
            Err(_) => Err(format!("timed out waiting for a packet ({})", self.name)),
        }
    }

    async fn connect(addr: SocketAddr, name: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|err| format!("connect {addr}: {err}"))?;
        let _ = stream.set_nodelay(true);
        let (read_half, write_half) = stream.into_split();
        let mut bot = Self {
            name: name.to_owned(),
            reader: TCPNetworkDecoder::new(BufReader::new(read_half)),
            writer: TCPNetworkEncoder::new(BufWriter::new(write_half)),
            position: Vector3::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            hostiles: HashMap::new(),
            soldiers: HashMap::new(),
            unknown: HashMap::new(),
            chat: Vec::new(),
            last_rank: "levy".to_owned(),
            last_stats: String::new(),
            dead: false,
            needs_deploy: false,
            last_tp: Instant::now() - Duration::from_secs(10),
            last_respawn_cmd: Instant::now() - Duration::from_secs(10),
            self_id: -1,
            spawn_log: 0,
            pkt_log: 0,
            pkt_ids: HashMap::new(),
        };

        bot.send_packet(&SHandShake {
            protocol_version: VarInt(CURRENT_MC_VERSION.protocol_version()),
            server_address: "127.0.0.1".into(),
            server_port: addr.port(),
            next_state: ConnectionState::Login,
        })
        .await?;
        bot.send_packet(&SLoginStart {
            name: name.into(),
            uuid: Uuid::new_v4(),
        })
        .await?;

        let login_success_id = CLoginSuccess::to_id(CURRENT_MC_VERSION);
        let set_compression_id = CSetCompression::to_id(CURRENT_MC_VERSION);
        loop {
            let pkt = bot.next_packet().await?;
            if pkt.id == set_compression_id {
                let mut slice = &pkt.payload[..];
                let comp = CSetCompression::read(&mut slice, &CURRENT_MC_VERSION)
                    .map_err(|err| format!("CSetCompression: {err}"))?;
                let threshold = comp.threshold.0.max(0) as usize;
                bot.reader.set_compression(threshold);
                bot.writer.set_compression((threshold, 4));
            } else if pkt.id == login_success_id {
                break;
            }
        }
        bot.send_packet(&SLoginAcknowledged).await?;

        bot.send_packet(&SClientInformationConfig {
            locale: "en_us",
            view_distance: 12,
            chat_mode: VarInt(0),
            chat_colors: true,
            skin_parts: 0x7F,
            main_hand: VarInt(1),
            text_filtering: false,
            server_listing: true,
        })
        .await?;

        let known_packs_id = CKnownPacks::to_id(CURRENT_MC_VERSION);
        let finish_config_id = CFinishConfig::to_id(CURRENT_MC_VERSION);
        loop {
            let pkt = bot.next_packet().await?;
            if pkt.id == known_packs_id {
                bot.send_packet(&SKnownPacks {
                    known_packs: Vec::new(),
                })
                .await?;
            } else if pkt.id == finish_config_id {
                bot.send_packet(&SAcknowledgeFinishConfig).await?;
                break;
            }
        }

        let play_login_id = CLogin::to_id(CURRENT_MC_VERSION);
        let player_pos_id = CPlayerPosition::to_id(CURRENT_MC_VERSION);
        let mut saw_play_login = false;
        let mut saw_initial_pos = false;
        while !(saw_play_login && saw_initial_pos) {
            let pkt = bot.next_packet().await?;
            if pkt.id == play_login_id {
                saw_play_login = true;
                if pkt.payload.len() >= 4 {
                    bot.self_id = i32::from_be_bytes([
                        pkt.payload[0],
                        pkt.payload[1],
                        pkt.payload[2],
                        pkt.payload[3],
                    ]);
                    println!("SELF id={}", bot.self_id);
                }
            }
            if saw_play_login {
                bot.handle_play_packet(&pkt).await?;
                if pkt.id == player_pos_id {
                    let _ = bot.send_packet(&SPlayerLoaded).await;
                    saw_initial_pos = true;
                }
            }
        }
        Ok(bot)
    }

    async fn move_to(&mut self, position: Vector3<f64>) -> Result<(), String> {
        self.position = position;
        self.send_packet(&SPlayerPosition {
            position,
            collision: 1,
        })
        .await
    }

    async fn look_and_move(
        &mut self,
        position: Vector3<f64>,
        yaw: f32,
        pitch: f32,
    ) -> Result<(), String> {
        self.position = position;
        self.yaw = yaw;
        self.pitch = pitch;
        self.send_packet(&SPlayerPositionRotation {
            position,
            yaw,
            pitch,
            collision: 1,
        })
        .await
    }

    async fn chat_command(&mut self, command: &str) -> Result<(), String> {
        println!("CMD /{command}");
        self.send_packet(&SChatCommand { command }).await
    }

    async fn attack(&mut self, entity_id: i32) -> Result<(), String> {
        self.send_packet(&SSwingArm { hand: VarInt(0) }).await?;
        self.send_packet(&SAttack {
            entity_id: VarInt(entity_id),
        })
        .await
    }

    fn note_chat(&mut self, raw: &str) {
        let text = raw.trim();
        if text.is_empty() {
            return;
        }
        println!("CHAT {text}");
        self.chat.push(text.to_owned());
        if text.contains("death.attack") || text.contains("died") {
            self.dead = true;
        }
        if text.contains("rank=") {
            text.clone_into(&mut self.last_stats);
            if let Some(rank) = text.split("rank=").nth(1).and_then(|s| s.split(' ').next())
                && rank != self.last_rank
            {
                println!("RANK-UP {rank} stats={text}");
                rank.clone_into(&mut self.last_rank);
            }
        }
        if text.contains("You are now a ") {
            println!("RANK-UP-EVENT {text}");
        }
    }

    fn note_spawn(&mut self, id: i32, kind: i32, pos: Vector3<f64>) {
        let mob = SeenMob { kind, pos };
        if is_hostile_kind(kind) {
            if self.spawn_log < 80 {
                println!(
                    "SPAWN-HOSTILE id={id} kind={kind} pos={:.1},{:.1},{:.1}",
                    pos.x, pos.y, pos.z
                );
                self.spawn_log += 1;
            }
            self.hostiles.insert(id, mob);
            self.soldiers.remove(&id);
            self.unknown.remove(&id);
        } else if is_soldier_kind(kind) {
            if self.spawn_log < 80 {
                println!(
                    "SPAWN-SOLDIER id={id} kind={kind} pos={:.1},{:.1},{:.1}",
                    pos.x, pos.y, pos.z
                );
                self.spawn_log += 1;
            }
            self.soldiers.insert(id, mob);
            self.hostiles.remove(&id);
            self.unknown.remove(&id);
        }
    }

    fn note_move(&mut self, id: i32, pos: Vector3<f64>) {
        if let Some(mob) = self.hostiles.get_mut(&id) {
            mob.pos = pos;
            return;
        }
        if let Some(mob) = self.soldiers.get_mut(&id) {
            mob.pos = pos;
            return;
        }
        if let Some(mob) = self.unknown.get_mut(&id) {
            mob.pos = pos;
            return;
        }
        let dist = (pos.x - self.position.x).hypot(pos.z - self.position.z);
        if dist <= DISCOVER_RANGE {
            if self.spawn_log < 40 {
                println!("SYNC-NEW id={id} dist={dist:.1}");
                self.spawn_log += 1;
            }
            self.unknown.insert(id, SeenMob { kind: -1, pos });
        }
    }

    fn note_remove(&mut self, ids: &[i32]) {
        for id in ids {
            self.hostiles.remove(id);
            self.soldiers.remove(id);
            self.unknown.remove(id);
        }
    }

    fn nearest_target(
        &self,
        include_soldiers: bool,
        include_unknown: bool,
    ) -> Option<(i32, Vector3<f64>, f64)> {
        let mut best = None;
        let pick = |best: &mut Option<(i32, Vector3<f64>, f64)>, map: &HashMap<i32, SeenMob>| {
            for (id, mob) in map {
                let dx = mob.pos.x - self.position.x;
                let dz = mob.pos.z - self.position.z;
                let dist = dx.hypot(dz);
                if dist > DISCOVER_RANGE {
                    continue;
                }
                if best.as_ref().is_none_or(|(_, _, d)| dist < *d) {
                    *best = Some((*id, mob.pos, dist));
                }
            }
        };
        pick(&mut best, &self.hostiles);
        if include_soldiers {
            pick(&mut best, &self.soldiers);
        }
        if include_unknown && best.is_none() {
            pick(&mut best, &self.unknown);
        }
        best
    }

    async fn handle_play_packet(&mut self, pkt: &RawPacket) -> Result<(), String> {
        if self.pkt_log < 250 {
            *self.pkt_ids.entry(pkt.id).or_insert(0) += 1;
            self.pkt_log += 1;
            if self.pkt_log == 250 {
                let spawn_id = CSpawnEntity::to_id(CURRENT_MC_VERSION);
                let sync_id = CEntityPositionSync::to_id(CURRENT_MC_VERSION);
                println!(
                    "PKTS spawn_id={spawn_id} sync_id={sync_id} counts={:?}",
                    self.pkt_ids
                );
            }
        }
        let keep_id = CKeepAlive::to_id(CURRENT_MC_VERSION);
        let pos_id = CPlayerPosition::to_id(CURRENT_MC_VERSION);
        let spawn_id = CSpawnEntity::to_id(CURRENT_MC_VERSION);
        let chat_id = CSystemChatMessage::to_id(CURRENT_MC_VERSION);
        let remove_id = CRemoveEntities::to_id(CURRENT_MC_VERSION);
        let sync_id = CEntityPositionSync::to_id(CURRENT_MC_VERSION);
        let health_id = CSetHealth::to_id(CURRENT_MC_VERSION);
        let inv_id = CSetPlayerInventory::to_id(CURRENT_MC_VERSION);
        let eq_id = CSetEquipment::to_id(CURRENT_MC_VERSION);
        let content_id = CSetContainerContent::to_id(CURRENT_MC_VERSION);
        let slot_id = CSetContainerSlot::to_id(CURRENT_MC_VERSION);
        if pkt.id == keep_id {
            let mut slice = &pkt.payload[..];
            let keep = CKeepAlive::read(&mut slice, &CURRENT_MC_VERSION)
                .map_err(|err| format!("CKeepAlive: {err}"))?;
            self.send_packet(&SKeepAlive {
                keep_alive_id: keep.keep_alive_id,
            })
            .await?;
        } else if pkt.id == pos_id {
            let mut slice = &pkt.payload[..];
            let pos_pkt = CPlayerPosition::read(&mut slice, &CURRENT_MC_VERSION)
                .map_err(|err| format!("CPlayerPosition: {err}"))?;
            self.position = pos_pkt.position;
            self.yaw = pos_pkt.yaw;
            self.pitch = pos_pkt.pitch;
            self.send_packet(&SConfirmTeleport {
                teleport_id: pos_pkt.teleport_id,
                position: pos_pkt.position,
                yaw: pos_pkt.yaw,
                pitch: pos_pkt.pitch,
            })
            .await?;
            self.last_tp = Instant::now();
            println!(
                "TELEPORT {}, {}, {}",
                self.position.x, self.position.y, self.position.z
            );
        } else if pkt.id == spawn_id {
            match CSpawnEntity::read_packet_data(Cursor::new(&pkt.payload[..]), &CURRENT_MC_VERSION)
            {
                Ok(spawn) => self.note_spawn(spawn.entity_id.0, spawn.r#type.0, spawn.position),
                Err(_) => {
                    if let Some((id, kind, pos)) = read_spawn_head(&pkt.payload) {
                        self.note_spawn(id, kind, pos);
                    }
                }
            }
        } else if pkt.id == chat_id {
            self.note_chat(&extract_text(&pkt.payload));
        } else if pkt.id == remove_id {
            self.note_remove(&read_var_int_list(&pkt.payload));
        } else if pkt.id == sync_id
            && let Some((id, pos)) = read_entity_sync(&pkt.payload)
        {
            self.note_move(id, pos);
        } else if pkt.id == CDamageEvent::to_id(CURRENT_MC_VERSION) {
            let mut slice = &pkt.payload[..];
            if let (Ok(victim), Ok(_ty), Ok(cause), Ok(_direct)) = (
                slice.get_var_int(),
                slice.get_var_int(),
                slice.get_var_int(),
                slice.get_var_int(),
            ) && victim.0 == self.self_id
                && cause.0 > 0
            {
                let attacker = cause.0 - 1;
                if attacker != self.self_id && attacker > 0 && self.spawn_log < 80 {
                    println!("ATTACKER id={attacker}");
                    self.spawn_log += 1;
                }
            }
        } else if pkt.id == health_id {
            let mut slice = &pkt.payload[..];
            match CSetHealth::read(&mut slice, &CURRENT_MC_VERSION) {
                Ok(health) => {
                    if health.health <= 0.0 && !self.dead {
                        self.dead = true;
                        self.hostiles.clear();
                        self.soldiers.clear();
                        self.unknown.clear();
                        println!("DIED health={}", health.health);
                        self.send_packet(&SClientCommand {
                            action_id: VarInt(SClientCommand::PERFORM_RESPAWN),
                        })
                        .await?;
                    } else if health.health > 0.0 {
                        if self.dead {
                            println!("RESPAWNED health={}", health.health);
                            self.needs_deploy = true;
                        }
                        self.dead = false;
                    }
                }
                Err(err) => eprintln!("health decode: {err}"),
            }
        } else if pkt.id == eq_id {
            if self.self_id > 0 {
                println!("EQ-RAW self={} bytes={}", self.self_id, pkt.payload.len());
            }
            let mut slice = &pkt.payload[..];
            match CSetEquipment::read(&mut slice, &CURRENT_MC_VERSION) {
                Ok(eq) => {
                    if eq.entity_id.0 == self.self_id || self.self_id == 0 {
                        println!(
                            "EQ entity={} self={} n={}",
                            eq.entity_id.0,
                            self.self_id,
                            eq.equipment.len()
                        );
                        for (slot, stack) in eq.equipment {
                            dump_kit("eq", i32::from(slot), &stack);
                        }
                    }
                }
                Err(err) => eprintln!("eq decode: {err}"),
            }
        } else if pkt.id == inv_id {
            println!("INV-RAW bytes={}", pkt.payload.len());
            let mut slice = &pkt.payload[..];
            if let Ok(slot) = slice.get_var_int()
                && let Ok(stack) =
                    ItemStackSerializer::read_with_version(&mut slice, &CURRENT_MC_VERSION)
            {
                dump_kit("inv", slot.0, &stack);
            }
        } else if pkt.id == content_id {
            println!("BAG-RAW bytes={}", pkt.payload.len());
            dump_bag(&pkt.payload);
        } else if pkt.id == slot_id {
            dump_container_slot(&pkt.payload);
        }
        Ok(())
    }

    async fn pump(&mut self, wait: Duration) -> Result<(), String> {
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            match timeout(
                left.min(Duration::from_millis(50)),
                self.reader.get_raw_packet(),
            )
            .await
            {
                Ok(Ok(pkt)) => self.handle_play_packet(&pkt).await?,
                Ok(Err(err)) => return Err(format!("disconnected: {err}")),
                Err(_) => {}
            }
        }
    }
}

fn dump_kit(tag: &str, slot: i32, stack: &ItemStackSerializer<'_>) {
    let key = stack.0.item.registry_key;
    if stack.0.item_count == 0 || key.is_empty() || key.ends_with(":air") {
        return;
    }
    println!("KIT {tag} slot={slot} {key} x{}", stack.0.item_count);
    if key.contains("wooden_sword") || key.ends_with(":shield") || key.contains("stone_sword") {
        println!("KIT-PROVE {tag} slot={slot} {key}");
    }
}

fn dump_bag(payload: &[u8]) {
    let mut slice = payload;
    let Ok(window) = slice.get_container_id(&CURRENT_MC_VERSION) else {
        eprintln!("bag decode: window");
        return;
    };
    if slice.get_var_int().is_err() {
        eprintln!("bag decode: state");
        return;
    }
    let Ok(count) = slice.get_var_int() else {
        eprintln!("bag decode: count");
        return;
    };
    println!("BAG window={} n={}", window.0, count.0);
    for slot in 0..count.0.min(128) {
        let Ok(stack) = ItemStackSerializer::read_with_version(&mut slice, &CURRENT_MC_VERSION)
        else {
            eprintln!("bag decode: slot {slot}");
            break;
        };
        dump_kit("bag", slot, &stack);
    }
}

fn dump_container_slot(payload: &[u8]) {
    let mut slice = payload;
    if slice.get_container_id(&CURRENT_MC_VERSION).is_err() {
        return;
    }
    let _ = slice.get_var_int();
    let Ok(slot) = slice.get_i16_be() else {
        return;
    };
    let Ok(stack) = ItemStackSerializer::read_with_version(&mut slice, &CURRENT_MC_VERSION) else {
        return;
    };
    dump_kit("slot", i32::from(slot), &stack);
}

fn extract_text(payload: &[u8]) -> String {
    let mut out = String::new();
    let mut run = Vec::new();
    for &byte in payload {
        if (32..127).contains(&byte) {
            run.push(byte);
        } else if !run.is_empty() {
            if run.len() >= 4
                && let Ok(piece) = std::str::from_utf8(&run)
            {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(piece);
            }
            run.clear();
        }
    }
    if run.len() >= 4
        && let Ok(piece) = std::str::from_utf8(&run)
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(piece);
    }
    out
}

fn read_var_int_list(payload: &[u8]) -> Vec<i32> {
    let mut slice = payload;
    let Ok(count) = slice.get_var_int() else {
        return Vec::new();
    };
    let n = count.0.max(0) as usize;
    let mut ids = Vec::with_capacity(n.min(256));
    for _ in 0..n.min(256) {
        let Ok(id) = slice.get_var_int() else {
            break;
        };
        ids.push(id.0);
    }
    ids
}

fn read_entity_sync(payload: &[u8]) -> Option<(i32, Vector3<f64>)> {
    let mut slice = payload;
    let id = slice.get_var_int().ok()?.0;
    let _path = slice.get_var_int().ok()?;
    let x = slice.get_f64_be().ok()?;
    let y = slice.get_f64_be().ok()?;
    let z = slice.get_f64_be().ok()?;
    Some((id, Vector3::new(x, y, z)))
}

fn read_spawn_head(payload: &[u8]) -> Option<(i32, i32, Vector3<f64>)> {
    let mut slice = payload;
    let id = slice.get_var_int().ok()?.0;
    let _uuid = slice.get_uuid().ok()?;
    let kind = slice.get_var_int().ok()?.0;
    let x = slice.get_f64_be().ok()?;
    let y = slice.get_f64_be().ok()?;
    let z = slice.get_f64_be().ok()?;
    Some((id, kind, Vector3::new(x, y, z)))
}

fn yaw_toward(from: Vector3<f64>, to: Vector3<f64>) -> f32 {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    dx.atan2(dz).to_degrees() as f32
}

async fn recover_stand(bot: &mut Bot, stand: Vector3<f64>) -> Result<(), String> {
    let dist = (bot.position.x - stand.x).hypot(bot.position.z - stand.z);
    if bot.position.y < stand.y - 1.0 && dist < 12.0 {
        println!("UNSTICK y={:.1} -> {}", bot.position.y, stand.y);
        bot.move_to(Vector3::new(bot.position.x, stand.y, bot.position.z))
            .await?;
        bot.pump(Duration::from_millis(100)).await?;
        return Ok(());
    }
    let dy = (bot.position.y - stand.y).abs();
    if dy > 8.0 || dist > 160.0 {
        println!(
            "RECOVER from {:.1},{:.1},{:.1} to {}, {}, {}",
            bot.position.x, bot.position.y, bot.position.z, stand.x, stand.y, stand.z
        );
        bot.move_to(stand).await?;
        bot.pump(Duration::from_millis(250)).await?;
    }
    Ok(())
}

async fn fight_loop(
    bot: &mut Bot,
    kingdom: &str,
    seconds: u64,
    stand: Vector3<f64>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    recover_stand(bot, stand).await?;
    bot.chat_command(&format!("kingdom join {kingdom}")).await?;
    bot.pump(Duration::from_secs(1)).await?;
    recover_stand(bot, stand).await?;
    bot.chat_command("kingdom stats").await?;
    bot.pump(Duration::from_secs(1)).await?;

    let mut fronts_ok = false;
    for attempt in 0..24 {
        bot.chat_command("kingdom fronts").await?;
        bot.pump(Duration::from_secs(3)).await?;
        if bot.chat.iter().rev().take(8).any(|line| {
            line.contains("raid_active") || line.contains("live=") || line.contains("side=")
        }) {
            fronts_ok = true;
            println!("FRONTS-OK attempt={attempt}");
            break;
        }
        println!("FRONTS-WAIT attempt={attempt}");
        bot.pump(Duration::from_secs(5)).await?;
        if Instant::now() >= deadline {
            break;
        }
    }
    if !fronts_ok {
        println!("FRONTS-MISSING continuing to deploy anyway");
    }

    bot.chat_command("kingdom deploy").await?;
    bot.pump(Duration::from_secs(1)).await?;
    bot.chat_command("war stats").await?;
    bot.pump(Duration::from_millis(400)).await?;
    bot.chat_command("kingdom fire").await?;
    bot.pump(Duration::from_millis(400)).await?;

    let mut last_attack = Instant::now() - Duration::from_secs(1);
    let mut last_stats = Instant::now();
    let mut last_deploy = Instant::now();
    let mut saw_hostile = Instant::now();
    let patrol = Vector3::new(stand.x + 24.0, stand.y, stand.z - 40.0);
    while Instant::now() < deadline {
        bot.pump(Duration::from_millis(50)).await?;
        if bot.dead {
            if bot.last_respawn_cmd.elapsed() >= Duration::from_secs(2) {
                let _ = bot
                    .send_packet(&SClientCommand {
                        action_id: VarInt(SClientCommand::PERFORM_RESPAWN),
                    })
                    .await;
                bot.last_respawn_cmd = Instant::now();
            }
            bot.pump(Duration::from_millis(200)).await?;
            continue;
        }
        if bot.last_tp.elapsed() < Duration::from_millis(500) {
            bot.pump(Duration::from_millis(50)).await?;
            continue;
        }
        if bot.needs_deploy && last_deploy.elapsed() >= Duration::from_secs(2) {
            println!("REDEPLOY after death");
            bot.needs_deploy = false;
            bot.chat_command("kingdom deploy").await?;
            bot.pump(Duration::from_secs(1)).await?;
            last_deploy = Instant::now();
            continue;
        }
        if let Some((id, pos, dist)) = bot.nearest_target(false, false) {
            saw_hostile = Instant::now();
            let yaw = yaw_toward(bot.position, pos);
            if dist > MELEE_RANGE {
                let dx = pos.x - bot.position.x;
                let dz = pos.z - bot.position.z;
                let len = dx.hypot(dz).max(0.001);
                let step = WALK_STEP.min(dist - 1.5).max(0.0);
                let next = Vector3::new(
                    bot.position.x + dx / len * step,
                    bot.position.y,
                    bot.position.z + dz / len * step,
                );
                bot.look_and_move(next, yaw, 0.0).await?;
            } else {
                bot.look_and_move(bot.position, yaw, 20.0).await?;
                let kind = bot
                    .hostiles
                    .get(&id)
                    .or_else(|| bot.soldiers.get(&id))
                    .or_else(|| bot.unknown.get(&id))
                    .map_or(-1, |m| m.kind);
                if last_attack.elapsed() >= Duration::from_millis(550)
                    && id != bot.self_id
                    && dist >= 0.2
                    && kind >= 0
                {
                    bot.attack(id).await?;
                    last_attack = Instant::now();
                    println!(
                        "ATTACK id={id} kind={kind} dist={dist:.1} hostiles={} soldiers={} unknown={}",
                        bot.hostiles.len(),
                        bot.soldiers.len(),
                        bot.unknown.len()
                    );
                }
            }
        } else if Instant::now().saturating_duration_since(saw_hostile) > Duration::from_secs(4) {
            let dest = if (bot.position.x - patrol.x).hypot(bot.position.z - patrol.z) < 4.0 {
                stand
            } else {
                patrol
            };
            let dx = dest.x - bot.position.x;
            let dz = dest.z - bot.position.z;
            let len = dx.hypot(dz).max(0.001);
            let step = WALK_STEP * 2.0;
            let next = Vector3::new(
                bot.position.x + dx / len * step,
                bot.position.y,
                bot.position.z + dz / len * step,
            );
            bot.move_to(next).await?;
        } else {
            bot.move_to(bot.position).await?;
        }
        if last_stats.elapsed() >= Duration::from_secs(15) {
            bot.chat_command("war stats").await?;
            last_stats = Instant::now();
            println!(
                "SEEN hostiles={} soldiers={} unknown={} pos={:.1},{:.1},{:.1}",
                bot.hostiles.len(),
                bot.soldiers.len(),
                bot.unknown.len(),
                bot.position.x,
                bot.position.y,
                bot.position.z
            );
            if bot.last_rank != "levy" {
                println!("DONE rank={} stats={}", bot.last_rank, bot.last_stats);
                bot.chat_command("kingdom stats").await?;
                bot.pump(Duration::from_secs(1)).await?;
                return Ok(());
            }
        }
    }
    bot.chat_command("war stats").await?;
    bot.pump(Duration::from_secs(1)).await?;
    println!(
        "TIMEOUT rank={} stats={} hostiles={} soldiers={} unknown={}",
        bot.last_rank,
        bot.last_stats,
        bot.hostiles.len(),
        bot.soldiers.len(),
        bot.unknown.len()
    );
    Ok(())
}

async fn prove_loop(bot: &mut Bot, kingdom: &str, seconds: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    println!(
        "PROVE-JOIN pos={:.1},{:.1},{:.1}",
        bot.position.x, bot.position.y, bot.position.z
    );
    bot.chat_command(&format!("kingdom join {kingdom}")).await?;
    bot.pump(Duration::from_secs(3)).await?;
    println!(
        "PROVE-AFTER-JOIN pos={:.1},{:.1},{:.1}",
        bot.position.x, bot.position.y, bot.position.z
    );
    for attempt in 0..90 {
        bot.pump(Duration::from_secs(1)).await?;
        let dist = bot.position.x.hypot(bot.position.z);
        println!(
            "PROVE-KEEP-WAIT attempt={attempt} dist={dist:.0} pos={:.1},{:.1},{:.1}",
            bot.position.x, bot.position.y, bot.position.z
        );
        if dist > 400.0 {
            println!(
                "PROVE-KEEP-TP pos={:.1},{:.1},{:.1}",
                bot.position.x, bot.position.y, bot.position.z
            );
            break;
        }
    }
    println!(
        "PROVE-PRE-KILL pos={:.1},{:.1},{:.1}",
        bot.position.x, bot.position.y, bot.position.z
    );
    bot.chat_command("kill").await?;
    bot.pump(Duration::from_millis(400)).await?;
    let mut saw_death = false;
    let kill_deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < kill_deadline {
        bot.pump(Duration::from_millis(200)).await?;
        if bot.dead {
            if !saw_death {
                println!(
                    "PROVE-DIED pos={:.1},{:.1},{:.1}",
                    bot.position.x, bot.position.y, bot.position.z
                );
            }
            saw_death = true;
            if bot.last_respawn_cmd.elapsed() >= Duration::from_secs(2) {
                let _ = bot
                    .send_packet(&SClientCommand {
                        action_id: VarInt(SClientCommand::PERFORM_RESPAWN),
                    })
                    .await;
                bot.last_respawn_cmd = Instant::now();
            }
        } else if saw_death {
            println!(
                "PROVE-RESPAWN pos={:.1},{:.1},{:.1}",
                bot.position.x, bot.position.y, bot.position.z
            );
            break;
        }
    }
    if !saw_death {
        println!("PROVE-NO-KILL continuing");
    }
    for attempt in 0..16 {
        bot.chat_command("kingdom fronts").await?;
        bot.pump(Duration::from_secs(2)).await?;
        if bot.chat.iter().rev().take(8).any(|line| {
            line.contains("raid_active") || line.contains("live=") || line.contains("side=")
        }) {
            println!("PROVE-FRONTS attempt={attempt}");
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    bot.chat_command("kingdom deploy").await?;
    bot.pump(Duration::from_secs(2)).await?;
    println!(
        "PROVE-DEPLOY pos={:.1},{:.1},{:.1}",
        bot.position.x, bot.position.y, bot.position.z
    );
    bot.chat_command("kingdom fire").await?;
    bot.pump(Duration::from_millis(400)).await?;
    let mut last_fire = Instant::now();
    let mut last_stats = Instant::now();
    while Instant::now() < deadline {
        bot.pump(Duration::from_millis(50)).await?;
        if bot.dead {
            if bot.last_respawn_cmd.elapsed() >= Duration::from_secs(2) {
                let _ = bot
                    .send_packet(&SClientCommand {
                        action_id: VarInt(SClientCommand::PERFORM_RESPAWN),
                    })
                    .await;
                bot.last_respawn_cmd = Instant::now();
            }
            continue;
        }
        if last_fire.elapsed() >= Duration::from_secs(90) {
            bot.chat_command("kingdom fire").await?;
            last_fire = Instant::now();
        }
        if last_stats.elapsed() >= Duration::from_secs(10) {
            last_stats = Instant::now();
            println!(
                "PROVE-SEEN hostiles={} soldiers={} unknown={} pos={:.1},{:.1},{:.1}",
                bot.hostiles.len(),
                bot.soldiers.len(),
                bot.unknown.len(),
                bot.position.x,
                bot.position.y,
                bot.position.z
            );
            if let Some((_, pos, dist)) = bot.nearest_target(true, true) {
                println!(
                    "PROVE-NEAR id-dist={dist:.1} at {:.1},{:.1},{:.1}",
                    pos.x, pos.y, pos.z
                );
            }
            bot.chat_command("war stats").await?;
        }
        bot.move_to(bot.position).await?;
    }
    bot.chat_command("war stats").await?;
    bot.pump(Duration::from_secs(1)).await?;
    println!(
        "PROVE-DONE pos={:.1},{:.1},{:.1} hostiles={} soldiers={}",
        bot.position.x,
        bot.position.y,
        bot.position.z,
        bot.hostiles.len(),
        bot.soldiers.len()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    };
    println!(
        "joining {} as {} then standing at {}, {}, {}",
        args.host, args.name, args.x, args.y, args.z
    );
    println!(
        "PKT-IDS eq={} inv={} bag={} slot={}",
        CSetEquipment::to_id(CURRENT_MC_VERSION),
        CSetPlayerInventory::to_id(CURRENT_MC_VERSION),
        CSetContainerContent::to_id(CURRENT_MC_VERSION),
        CSetContainerSlot::to_id(CURRENT_MC_VERSION)
    );
    let mut bot = match Bot::connect(args.host, &args.name).await {
        Ok(bot) => bot,
        Err(err) => {
            eprintln!("join failed: {err}");
            return ExitCode::from(1);
        }
    };
    println!(
        "joined at {}, {}, {}",
        bot.position.x, bot.position.y, bot.position.z
    );
    let stand = Vector3::new(args.x, args.y, args.z);
    if args.prove {
        if let Err(err) = prove_loop(&mut bot, &args.kingdom, args.seconds).await {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
        println!("done");
        return ExitCode::SUCCESS;
    }
    if let Err(err) = bot.move_to(stand).await {
        eprintln!("move failed: {err}");
        return ExitCode::from(1);
    }
    println!("standing; holding for {}s", args.seconds);
    for cmd in &args.commands {
        if let Err(err) = bot.chat_command(cmd).await {
            eprintln!("command failed: {err}");
            return ExitCode::from(1);
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    if args.fight {
        if let Err(err) = fight_loop(&mut bot, &args.kingdom, args.seconds, stand).await {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
        println!("done");
        return ExitCode::SUCCESS;
    }

    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    loop {
        if Instant::now() >= deadline {
            println!("done");
            return ExitCode::SUCCESS;
        }
        match timeout(Duration::from_millis(500), bot.reader.get_raw_packet()).await {
            Ok(Ok(pkt)) => {
                if let Err(err) = bot.handle_play_packet(&pkt).await {
                    eprintln!("{err}");
                    return ExitCode::from(1);
                }
            }
            Ok(Err(err)) => {
                eprintln!("disconnected: {err}");
                return ExitCode::from(1);
            }
            Err(_) => {
                if let Err(err) = bot.move_to(stand).await {
                    eprintln!("move failed: {err}");
                    return ExitCode::from(1);
                }
            }
        }
    }
}
