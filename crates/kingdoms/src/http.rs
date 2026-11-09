//! Optional authenticated HTTP director for an external game director.
//!
//! Off by default. Enable with `KINGDOMS_HTTP=1` and a bearer token in
//! `KINGDOMS_HTTP_TOKEN`. Binds `127.0.0.1:8742` unless `KINGDOMS_HTTP_BIND`
//! says otherwise. Outbound webhooks POST JSON with `X-Kingdoms-Signature`.

use std::sync::OnceLock;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hmac::{Hmac, KeyInit, Mac};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::net::TcpListener;

use crate::director::{self, CombatEvent, CombatKind, RaidId};
use crate::raid::{RaidSide, RaidSpec};

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_BIND: &str = "127.0.0.1:8742";
const SIG_HEADER: &str = "x-kingdoms-signature";

static CONFIG: OnceLock<HttpConfig> = OnceLock::new();

#[derive(Clone, Debug)]
struct HttpConfig {
    token: String,
    webhook: Option<String>,
    webhook_secret: String,
}

#[derive(Clone)]
struct AppState {
    token: String,
}

/// True when `KINGDOMS_HTTP` / `KINGDOMS_DIRECTOR_HTTP` is set and a token exists.
#[must_use]
pub fn enabled() -> bool {
    env_flag("KINGDOMS_HTTP") || env_flag("KINGDOMS_DIRECTOR_HTTP")
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .is_ok_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn load_config() -> Option<HttpConfig> {
    if !enabled() {
        return None;
    }
    let token =
        env_nonempty("KINGDOMS_HTTP_TOKEN").or_else(|| env_nonempty("KINGDOMS_DIRECTOR_TOKEN"))?;
    let webhook = env_nonempty("KINGDOMS_HTTP_WEBHOOK");
    let webhook_secret = env_nonempty("KINGDOMS_HTTP_WEBHOOK_SECRET")
        .or_else(|| env_nonempty("KINGDOMS_HTTP_TOKEN"))
        .unwrap_or_else(|| token.clone());
    Some(HttpConfig {
        token,
        webhook,
        webhook_secret,
    })
}

/// Bind the director HTTP server on the current Tokio runtime, if enabled.
pub fn spawn_if_enabled() {
    let Some(cfg) = load_config() else {
        return;
    };
    let bind = env_nonempty("KINGDOMS_HTTP_BIND").unwrap_or_else(|| DEFAULT_BIND.to_owned());
    let state = AppState {
        token: cfg.token.clone(),
    };
    let _ = CONFIG.set(cfg);
    tracing::info!(bind, "kingdoms director HTTP listening");
    tokio::spawn(async move {
        let listener = match TcpListener::bind(&bind).await {
            Ok(l) => l,
            Err(err) => {
                tracing::error!(%err, bind, "kingdoms director HTTP bind failed");
                return;
            }
        };
        let app = router(state);
        if let Err(err) = axum::serve(listener, app).await {
            tracing::error!(%err, "kingdoms director HTTP stopped");
        }
    });
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/state", get(get_state))
        .route("/towns/{id}", get(get_town))
        .route("/raids", post(post_raid))
        .route("/raids/{id}/stop", post(stop_raid))
        .route("/chat", post(post_chat))
        .with_state(state)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
}

fn json_ok(value: Value) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        value.to_string(),
    )
        .into_response()
}

fn check_auth(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|got| got == token)
}

async fn get_state(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !check_auth(&headers, &state.token) {
        return unauthorized();
    }
    json_ok(director_state())
}

async fn get_town(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !check_auth(&headers, &state.token) {
        return unauthorized();
    }
    director::battle_state(&id).map_or_else(
        || (StatusCode::NOT_FOUND, format!("unknown town {id}")).into_response(),
        |b| json_ok(battle_json(&b)),
    )
}

#[derive(Deserialize)]
struct RaidBody {
    town: String,
    #[serde(default = "default_size")]
    size: u16,
    #[serde(default)]
    defenders: u16,
    #[serde(default)]
    side: RaidSide,
    /// Source camp id; currently unused (raids spawn from the town's hinterland camp).
    #[serde(default)]
    #[expect(dead_code, reason = "reserved for a later camp picker")]
    camp: Option<String>,
}

const fn default_size() -> u16 {
    24
}

async fn post_raid(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !check_auth(&headers, &state.token) {
        return unauthorized();
    }
    let body: RaidBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, format!("invalid json: {err}")).into_response();
        }
    };
    let worlds = director::lock().attached_worlds();
    let Some(world) = worlds.into_iter().next() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "no world").into_response();
    };
    let pos = Vector3::new(0.0, 64.0, 0.0);
    let Some(town_id) = director::resolve_town_id(&world, pos, &body.town) else {
        return (
            StatusCode::BAD_REQUEST,
            format!("unknown town {}", body.town),
        )
            .into_response();
    };
    let spec = RaidSpec::new(body.size)
        .with_extra_defenders(body.defenders)
        .with_side(body.side);
    match director::start_raid(&world, &town_id, spec) {
        Ok(id) => json_ok(json!({
            "id": id.0,
            "town": town_id,
            "size": spec.raiders,
            "side": spec.side,
        })),
        Err(err) => (StatusCode::CONFLICT, err).into_response(),
    }
}

async fn stop_raid(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    headers: HeaderMap,
) -> Response {
    if !check_auth(&headers, &state.token) {
        return unauthorized();
    }
    let n = director::stop_raid_by_id(RaidId(id));
    json_ok(json!({ "stopped": n, "id": id }))
}

#[derive(Deserialize)]
struct ChatBody {
    name: String,
    message: String,
}

async fn post_chat(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !check_auth(&headers, &state.token) {
        return unauthorized();
    }
    let body: ChatBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, format!("invalid json: {err}")).into_response();
        }
    };
    let text = TextComponent::text(format!("[{}] {}", body.name, body.message));
    let worlds = director::lock().attached_worlds();
    for world in worlds {
        world.broadcast_system_message(&text, false);
    }
    tracing::info!(npc = %body.name, "kingdoms director chat");
    json_ok(json!({ "ok": true }))
}

fn battle_json(b: &director::BattleState) -> Value {
    json!({
        "id": b.town_id,
        "kingdom": b.kingdom,
        "owner": b.owner,
        "alarm": b.alarm,
        "raid_active": b.raid_active,
        "wave": b.wave,
        "waves": b.waves,
        "defenders_alive": b.defenders_alive,
        "raiders_alive": b.raiders_alive,
        "defenders_killed": b.defenders_killed,
        "raiders_killed": b.raiders_killed,
        "threatened_gate": b.threatened_gate,
        "gate_breached": b.gate_breached,
    })
}

fn director_state() -> Value {
    let dir = director::lock();
    let tick = dir.tick;
    let towns: Vec<Value> = dir.all_battles().iter().map(battle_json).collect();
    let mut players = Vec::new();
    for world in dir.attached_worlds() {
        for p in world.players.load().iter() {
            let pos = p.living_entity.entity.pos.load();
            let kingdom = dir
                .player_kingdom(&p.gameprofile.id.to_string())
                .map(|k| k.as_str().to_owned());
            players.push(json!({
                "name": p.gameprofile.name,
                "uuid": p.gameprofile.id.to_string(),
                "kingdom": kingdom,
                "x": pos.x,
                "y": pos.y,
                "z": pos.z,
            }));
        }
    }
    let defenders: u32 = towns
        .iter()
        .filter_map(|t| t.get("defenders_alive").and_then(Value::as_u64))
        .sum::<u64>() as u32;
    let raiders: u32 = towns
        .iter()
        .filter_map(|t| t.get("raiders_alive").and_then(Value::as_u64))
        .sum::<u64>() as u32;
    json!({
        "tick": tick,
        "towns": towns,
        "players": players,
        "troops": { "defenders": defenders, "raiders": raiders },
    })
}

/// HMAC-SHA256 hex of `body` with `secret`.
#[must_use]
pub fn sign_body(secret: &str, body: &[u8]) -> String {
    // HMAC-SHA256 accepts any key length, including empty.
    #[expect(clippy::unwrap_used, reason = "HmacSha256::new_from_slice cannot fail")]
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

fn webhook_name(kind: CombatKind) -> Option<&'static str> {
    match kind {
        CombatKind::RaidStart => Some("raid_started"),
        CombatKind::Breach => Some("gate_breached"),
        CombatKind::Capture => Some("town_captured"),
        CombatKind::Retake => Some("town_captured"),
        _ => None,
    }
}

/// Fire an outbound webhook for a combat event, if configured.
pub fn notify_combat(event: &CombatEvent) {
    let Some(name) = webhook_name(event.kind) else {
        return;
    };
    let payload = json!({
        "event": name,
        "tick": event.tick,
        "actor": event.actor,
        "other": event.other,
        "x": event.x,
        "z": event.z,
        "note": event.note,
    });
    emit(name, payload);
}

/// Fire `player_joined_a_kingdom`.
pub fn notify_join(player: &str, kingdom: &str) {
    emit(
        "player_joined_a_kingdom",
        json!({
            "event": "player_joined_a_kingdom",
            "player": player,
            "kingdom": kingdom,
        }),
    );
}

fn emit(name: &str, payload: Value) {
    let Some(cfg) = CONFIG.get() else {
        return;
    };
    let Some(url) = cfg.webhook.clone() else {
        return;
    };
    let body = match serde_json::to_vec(&payload) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(%err, "kingdoms webhook encode failed");
            return;
        }
    };
    let sig = sign_body(&cfg.webhook_secret, &body);
    let name = name.to_owned();
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let req = client
            .post(&url)
            .header("content-type", "application/json")
            .header(SIG_HEADER, format!("sha256={sig}"))
            .body(body);
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::warn!(
                event = name,
                status = resp.status().as_u16(),
                "kingdoms webhook rejected"
            ),
            Err(err) => tracing::warn!(event = name, %err, "kingdoms webhook failed"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_is_stable_for_known_bytes() {
        let sig = sign_body("secret", b"{\"event\":\"raid_started\"}");
        assert_eq!(sig.len(), 64);
        assert_eq!(sig, sign_body("secret", b"{\"event\":\"raid_started\"}"));
        assert_ne!(sig, sign_body("other", b"{\"event\":\"raid_started\"}"));
    }

    #[test]
    fn webhook_names_cover_director_events() {
        assert_eq!(webhook_name(CombatKind::RaidStart), Some("raid_started"));
        assert_eq!(webhook_name(CombatKind::Breach), Some("gate_breached"));
        assert_eq!(webhook_name(CombatKind::Capture), Some("town_captured"));
        assert!(webhook_name(CombatKind::Kill).is_none());
    }
}
