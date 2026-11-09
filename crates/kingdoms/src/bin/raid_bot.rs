//! Headless Java client that logs in and stands still so a town ticks.
//!
//! Pumpkin only simulates entities in chunks a player (or a lasting FULL
//! ticket) is watching. This bot is the player: join, walk to `--x --y --z`,
//! answer keep-alives, and stay until Ctrl-C or `--seconds`.
//!
//! The server must have `online_mode = false`, `encryption = false`. Compression
//! can stay on; the bot enables it if the login handshake asks.
//!
//!   kingdoms-raid-bot --host 127.0.0.1:25565 --x 1349 --y 80 --z 693 --seconds 180

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::env;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use bytes::Bytes;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_protocol::java::client::config::{CFinishConfig, CKnownPacks};
use pumpkin_protocol::java::client::login::{CLoginSuccess, CSetCompression};
use pumpkin_protocol::java::client::play::{CKeepAlive, CLogin, CPlayerPosition};
use pumpkin_protocol::java::packet_decoder::TCPNetworkDecoder;
use pumpkin_protocol::java::packet_encoder::TCPNetworkEncoder;
use pumpkin_protocol::java::server::config::{
    SAcknowledgeFinishConfig, SClientInformationConfig, SKnownPacks,
};
use pumpkin_protocol::java::server::handshake::SHandShake;
use pumpkin_protocol::java::server::login::{SLoginAcknowledged, SLoginStart};
use pumpkin_protocol::java::server::play::{
    SConfirmTeleport, SKeepAlive, SPlayerLoaded, SPlayerPosition,
};
use pumpkin_protocol::packet::MultiVersionJavaPacket;
use pumpkin_protocol::ser::NetworkWriteExt;
use pumpkin_protocol::{ClientPacket, ConnectionState, RawPacket, ServerPacket, VarInt};
use pumpkin_util::math::vector3::Vector3;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::time::{Instant, timeout};
use uuid::Uuid;

struct Args {
    host: SocketAddr,
    name: String,
    x: f64,
    y: f64,
    z: f64,
    seconds: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut host = "127.0.0.1:25565".to_owned();
    let mut name = "kingdomsbot".to_owned();
    let mut x = 0.0;
    let mut y = 80.0;
    let mut z = 0.0;
    let mut seconds = 180u64;
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
            "-h" | "--help" => {
                println!(
                    "kingdoms-raid-bot --host 127.0.0.1:25565 --x X --y Y --z Z --seconds 180"
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
    })
}

struct Bot {
    name: String,
    reader: TCPNetworkDecoder<BufReader<OwnedReadHalf>>,
    writer: TCPNetworkEncoder<BufWriter<OwnedWriteHalf>>,
    position: Vector3<f64>,
    yaw: f32,
    pitch: f32,
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
        match timeout(Duration::from_secs(30), self.reader.get_raw_packet()).await {
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
            } else if pkt.id == player_pos_id {
                let mut slice = &pkt.payload[..];
                let pos_pkt = CPlayerPosition::read(&mut slice, &CURRENT_MC_VERSION)
                    .map_err(|err| format!("CPlayerPosition: {err}"))?;
                bot.position = pos_pkt.position;
                bot.yaw = pos_pkt.yaw;
                bot.pitch = pos_pkt.pitch;
                bot.send_packet(&SConfirmTeleport {
                    teleport_id: pos_pkt.teleport_id,
                    position: pos_pkt.position,
                    yaw: pos_pkt.yaw,
                    pitch: pos_pkt.pitch,
                })
                .await?;
                bot.send_packet(&SPlayerLoaded).await?;
                saw_initial_pos = true;
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

    async fn handle_play_packet(&mut self, pkt: &RawPacket) -> Result<(), String> {
        let keep_id = CKeepAlive::to_id(CURRENT_MC_VERSION);
        let pos_id = CPlayerPosition::to_id(CURRENT_MC_VERSION);
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
        }
        Ok(())
    }
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
    if let Err(err) = bot.move_to(stand).await {
        eprintln!("move failed: {err}");
        return ExitCode::from(1);
    }
    println!("standing; holding for {}s", args.seconds);

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
