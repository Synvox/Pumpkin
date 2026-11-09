//! Optional authenticated HTTP director for an external game director.
//!
//! Off by default. Enable with `KINGDOMS_HTTP=1` and a bearer token of at
//! least [`MIN_TOKEN_LEN`] characters in `KINGDOMS_HTTP_TOKEN`. Binds
//! `127.0.0.1:8742` unless `KINGDOMS_HTTP_BIND` says otherwise. Outbound
//! webhooks POST JSON with `X-Kingdoms-Signature`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use hmac::{Hmac, KeyInit, Mac};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::text::TextComponent;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;

use crate::chat::{self, ShoutKind};
use crate::director::{self, CombatEvent, CombatKind, RaidId};
use crate::raid::{RaidSide, RaidSpec};

type HmacSha256 = Hmac<Sha256>;

const DEFAULT_BIND: &str = "127.0.0.1:8742";
const SIG_HEADER: &str = "x-kingdoms-signature";
/// Bearer tokens shorter than this refuse to bind.
pub const MIN_TOKEN_LEN: usize = 32;
/// Reject request bodies larger than this (also set as axum's limit).
pub const MAX_BODY: usize = 16 * 1024;
/// Simple sliding-window cap per process.
pub const RATE_PER_SEC: u32 = 20;
/// An HTTP director that has talked to us this recently overrides the war clock.
const DIRECTOR_SEEN_SECS: u64 = 30;

static CONFIG: std::sync::OnceLock<HttpConfig> = std::sync::OnceLock::new();
static LAST_SEEN_SECS: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct HttpConfig {
    token: String,
    webhook: Option<String>,
    webhook_secret: String,
}

#[derive(Clone)]
struct AppState {
    token: String,
    limiter: Arc<RateLimit>,
}

/// Sliding-window rate limiter. One bucket for the whole listener.
pub struct RateLimit {
    hits: Mutex<VecDeque<Instant>>,
    per_sec: u32,
}

impl RateLimit {
    #[must_use]
    pub fn new(per_sec: u32) -> Self {
        Self {
            hits: Mutex::new(VecDeque::new()),
            per_sec,
        }
    }

    /// True if this call is under the cap.
    pub fn allow(&self) -> bool {
        let now = Instant::now();
        let mut q = self
            .hits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t).as_secs() >= 1)
        {
            q.pop_front();
        }
        if q.len() as u32 >= self.per_sec {
            return false;
        }
        q.push_back(now);
        true
    }
}

/// True when `KINGDOMS_HTTP` / `KINGDOMS_DIRECTOR_HTTP` is set.
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

/// Constant-time compare. Different lengths are rejected without leaking the token.
#[must_use]
pub fn token_matches(got: &str, expected: &str) -> bool {
    if got.is_empty() || got.len() != expected.len() {
        return false;
    }
    got.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// True when a token is long enough to bind the HTTP server.
#[must_use]
pub const fn token_is_strong(token: &str) -> bool {
    token.len() >= MIN_TOKEN_LEN
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Record that an authenticated HTTP director just talked to us.
pub fn mark_director_seen() {
    LAST_SEEN_SECS.store(unix_secs(), Ordering::Relaxed);
}

/// True when an external HTTP director has used the API recently.
#[must_use]
pub fn external_director_connected() -> bool {
    let last = LAST_SEEN_SECS.load(Ordering::Relaxed);
    if last == 0 {
        return false;
    }
    unix_secs().saturating_sub(last) <= DIRECTOR_SEEN_SECS
}

fn load_config() -> Option<HttpConfig> {
    if !enabled() {
        return None;
    }
    let token =
        env_nonempty("KINGDOMS_HTTP_TOKEN").or_else(|| env_nonempty("KINGDOMS_DIRECTOR_TOKEN"));
    let Some(token) = token else {
        tracing::error!(
            "kingdoms HTTP refused: set KINGDOMS_HTTP_TOKEN to at least {MIN_TOKEN_LEN} characters"
        );
        return None;
    };
    if !token_is_strong(&token) {
        tracing::error!(
            len = token.len(),
            min = MIN_TOKEN_LEN,
            "kingdoms HTTP refused: token shorter than {MIN_TOKEN_LEN} characters"
        );
        return None;
    }
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
        limiter: Arc::new(RateLimit::new(RATE_PER_SEC)),
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
        .route("/war", get(get_war))
        .route("/lines", get(get_lines).post(post_lines))
        .route("/heroes", get(get_heroes).post(post_heroes))
        .route("/heroes/{kingdom}/shout", post(post_hero_shout))
        .route("/towns/{id}", get(get_town))
        .route("/raids", post(post_raid))
        .route("/raids/{id}/stop", post(stop_raid))
        .route("/chat", post(post_chat))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response()
}

fn too_large() -> Response {
    (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response()
}

fn too_many() -> Response {
    (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response()
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
        .is_some_and(|got| token_matches(got, token))
}

/// Shared gate: rate limit, body size, then bearer token.
fn gate(headers: &HeaderMap, state: &AppState, body_len: usize) -> Option<Response> {
    if !state.limiter.allow() {
        return Some(too_many());
    }
    if body_len > MAX_BODY {
        return Some(too_large());
    }
    if !check_auth(headers, &state.token) {
        return Some(unauthorized());
    }
    mark_director_seen();
    None
}

async fn get_state(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(err) = gate(&headers, &state, 0) {
        return err;
    }
    json_ok(director_state())
}

/// Observational: auth + rate limit, but does not pause the war clock.
fn gate_observe(headers: &HeaderMap, state: &AppState) -> Option<Response> {
    if !state.limiter.allow() {
        return Some(too_many());
    }
    if !check_auth(headers, &state.token) {
        return Some(unauthorized());
    }
    None
}

async fn get_war(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(err) = gate_observe(&headers, &state) {
        return err;
    }
    json_ok(war_json())
}

async fn get_lines(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(err) = gate_observe(&headers, &state) {
        return err;
    }
    json_ok(json!({ "lines": chat::all_lines() }))
}

#[derive(Deserialize)]
struct LineBody {
    event: String,
    #[serde(default)]
    text: String,
}

async fn post_lines(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(err) = gate_observe(&headers, &state) {
        return err;
    }
    if body.len() > MAX_BODY {
        return too_large();
    }
    let body: LineBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, format!("invalid json: {err}")).into_response();
        }
    };
    let Some(kind) = ShoutKind::parse(&body.event) else {
        return (
            StatusCode::BAD_REQUEST,
            "event must be gate_breached, rally, retreat, victory, charge, hold, or fallen",
        )
            .into_response();
    };
    chat::set_line(kind, &body.text);
    json_ok(json!({
        "ok": true,
        "event": kind.as_str(),
        "text": chat::line(kind),
    }))
}

fn hero_json(hero: &director::Hero) -> Value {
    json!({
        "kingdom": hero.kingdom,
        "name": hero.name,
        "title": hero.title,
        "line": hero.line,
    })
}

async fn get_heroes(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(err) = gate_observe(&headers, &state) {
        return err;
    }
    json_ok(json!({
        "heroes": director::all_heroes().iter().map(hero_json).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize)]
struct HeroBody {
    kingdom: String,
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    line: String,
    #[serde(default)]
    text: String,
}

async fn post_heroes(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Some(err) = gate_observe(&headers, &state) {
        return err;
    }
    if body.len() > MAX_BODY {
        return too_large();
    }
    let body: HeroBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, format!("invalid json: {err}")).into_response();
        }
    };
    if body.kingdom.trim().is_empty() || body.name.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "kingdom and name are required").into_response();
    }
    let line = if body.line.trim().is_empty() {
        body.text.as_str()
    } else {
        body.line.as_str()
    };
    let hero = director::set_hero(&body.kingdom, &body.name, &body.title, line);
    json_ok(hero_json(&hero))
}

#[derive(Deserialize)]
struct HeroShoutBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    message: String,
}

async fn post_hero_shout(
    State(state): State<AppState>,
    Path(kingdom): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(err) = gate(&headers, &state, body.len()) {
        return err;
    }
    let body: HeroShoutBody = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, format!("invalid json: {err}")).into_response();
        }
    };
    let text = if body.text.trim().is_empty() {
        body.message.as_str()
    } else {
        body.text.as_str()
    };
    match director::shout_hero(&kingdom, text) {
        Ok(hero) => json_ok(json!({ "ok": true, "hero": hero_json(&hero) })),
        Err(err) => (StatusCode::NOT_FOUND, err).into_response(),
    }
}

async fn get_town(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(err) = gate(&headers, &state, 0) {
        return err;
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
    if let Some(err) = gate(&headers, &state, body.len()) {
        return err;
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
    if let Some(err) = gate(&headers, &state, 0) {
        return err;
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
    if let Some(err) = gate(&headers, &state, body.len()) {
        return err;
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
        "occupy_hold": b.occupy_hold,
        "occupy_need": b.occupy_need,
        "ram_hp": b.ram_hp,
    })
}

fn war_json() -> Value {
    let front = director::war_front();
    json!({
        "tick": front.tick,
        "scores": front.scores,
        "clock": {
            "enabled": front.clock_enabled,
            "period": front.clock_period,
            "overridden": front.clock_overridden,
            "last_tick": front.last_clock_tick,
        },
        "next_raid_id": front.next_raid_id,
        "towns": front.towns.iter().map(|t| json!({
            "id": t.id,
            "kingdom": t.kingdom,
            "owner": t.owner,
            "alarm": t.alarm,
            "raid_active": t.raid_active,
            "gate_breached": t.gate_breached,
            "keep": { "x": t.keep.0, "z": t.keep.1 },
            "gate": t.gate.map(|(x, z)| json!({ "x": x, "z": z })),
            "occupy_hold": t.occupy_hold,
            "occupy_need": t.occupy_need,
            "defenders": t.defenders,
            "raiders": t.raiders,
            "raid_side": t.raid_side,
            "repair_until": t.repair_until,
            "live": t.live,
        })).collect::<Vec<_>>(),
        "fronts": front.fronts,
        "heroes": director::all_heroes().iter().map(hero_json).collect::<Vec<_>>(),
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
        "war_clock": !external_director_connected(),
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
        CombatKind::RamDestroyed => Some("ram_destroyed"),
        CombatKind::Rout => Some("rout"),
        CombatKind::WarClock => Some("war_clock"),
        CombatKind::Alarm => Some("town_alarm"),
        CombatKind::Trebuchet => Some("trebuchet_fired"),
        CombatKind::LaddersRaised => Some("ladders_raised"),
        CombatKind::Portcullis => Some("portcullis_dropped"),
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
    use axum::http::{HeaderMap, HeaderValue};

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
        assert_eq!(
            webhook_name(CombatKind::RamDestroyed),
            Some("ram_destroyed")
        );
        assert_eq!(webhook_name(CombatKind::WarClock), Some("war_clock"));
        assert_eq!(webhook_name(CombatKind::Alarm), Some("town_alarm"));
        assert_eq!(webhook_name(CombatKind::Trebuchet), Some("trebuchet_fired"));
        assert_eq!(
            webhook_name(CombatKind::LaddersRaised),
            Some("ladders_raised")
        );
        assert_eq!(
            webhook_name(CombatKind::Portcullis),
            Some("portcullis_dropped")
        );
        assert!(webhook_name(CombatKind::Kill).is_none());
    }

    #[test]
    fn token_compare_is_length_checked_and_equal() {
        let token = "a".repeat(MIN_TOKEN_LEN);
        assert!(token_matches(&token, &token));
        assert!(!token_matches("short", &token));
        assert!(!token_matches(&"b".repeat(MIN_TOKEN_LEN), &token));
        assert!(!token_matches("", &token));
        assert!(!token_is_strong("dev-token"));
        assert!(token_is_strong(&token));
    }

    #[test]
    fn rate_limit_trips_after_cap() {
        let lim = RateLimit::new(3);
        assert!(lim.allow());
        assert!(lim.allow());
        assert!(lim.allow());
        assert!(!lim.allow());
    }

    #[test]
    fn body_over_max_is_rejected_by_gate() {
        let token = "a".repeat(MIN_TOKEN_LEN);
        let state = AppState {
            token: token.clone(),
            limiter: Arc::new(RateLimit::new(100)),
        };
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        let err = gate(&headers, &state, MAX_BODY + 1).unwrap();
        assert_eq!(err.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(gate(&headers, &state, 16).is_none());
    }

    #[test]
    fn war_json_has_front_line_fields() {
        let v = war_json();
        assert!(v.get("tick").is_some());
        assert!(v.get("scores").is_some());
        assert!(v.get("clock").is_some());
        assert!(v.get("towns").is_some());
        assert!(v.get("heroes").is_some());
    }

    #[test]
    fn hero_json_lists_name_and_kingdom() {
        let hero = director::Hero {
            kingdom: "alder".to_owned(),
            name: "Sir Aldric".to_owned(),
            title: "marshal".to_owned(),
            line: "Hold the gate.".to_owned(),
        };
        let v = hero_json(&hero);
        assert_eq!(v["kingdom"], "alder");
        assert_eq!(v["name"], "Sir Aldric");
        assert_eq!(v["title"], "marshal");
    }

    #[test]
    fn missing_bearer_is_unauthorized() {
        let token = "a".repeat(MIN_TOKEN_LEN);
        let state = AppState {
            token,
            limiter: Arc::new(RateLimit::new(100)),
        };
        let err = gate(&HeaderMap::new(), &state, 0).unwrap();
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);
    }
}
