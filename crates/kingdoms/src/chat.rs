//! Captain shouts to nearby players, plus a director hook for supplied lines.
//!
//! Captains post short in-character lines at gate breach, rally, retreat, and
//! victory. Lines are rate-limited per town and event. `POST /lines` lets the
//! Sails director override the default text without pausing the war clock.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use pumpkin_core::entity::player::Player;
use pumpkin_core::world::World;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;
use serde::{Deserialize, Serialize};

/// How close a player must be (blocks) to hear a captain.
pub const CHAT_RANGE: f64 = 48.0;
/// Minimum ticks between the same shout kind in one town (~10 s).
pub const CHAT_COOLDOWN_TICKS: u32 = 200;
/// Per-player gap for squad IC replies (~1 s). Town captain lines do not share this.
pub const SQUAD_REPLY_COOLDOWN_TICKS: u32 = 20;

/// Combat beats that get a spoken line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShoutKind {
    GateBreached,
    Rally,
    Retreat,
    Victory,
    Charge,
    Hold,
    Fallen,
}

impl ShoutKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GateBreached => "gate_breached",
            Self::Rally => "rally",
            Self::Retreat => "retreat",
            Self::Victory => "victory",
            Self::Charge => "charge",
            Self::Hold => "hold",
            Self::Fallen => "fallen",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gate_breached" | "breach" => Some(Self::GateBreached),
            "rally" | "raid_started" | "war_clock" => Some(Self::Rally),
            "retreat" | "rout" => Some(Self::Retreat),
            "victory" | "town_captured" | "capture" | "retake" | "flag_changed" => {
                Some(Self::Victory)
            }
            "capture_started" => Some(Self::Charge),
            "capture_paused" => Some(Self::Hold),
            "charge" | "advance" => Some(Self::Charge),
            "hold" => Some(Self::Hold),
            "fallen" | "captain_down" => Some(Self::Fallen),
            _ => None,
        }
    }

    const fn default_line(self) -> &'static str {
        match self {
            Self::GateBreached => "The gate is down. Through the breach!",
            Self::Rally => "To the walls. Hold the line.",
            Self::Retreat => "Fall back to the keep. Do not break.",
            Self::Victory => "The town is ours. Raise the colours.",
            Self::Charge => "To the flag!",
            Self::Hold => "Shields! Hold this ground.",
            Self::Fallen => "Captain is down. Stand fast.",
        }
    }
}

static LINES: OnceLock<Mutex<HashMap<ShoutKind, String>>> = OnceLock::new();

fn line_store() -> &'static Mutex<HashMap<ShoutKind, String>> {
    LINES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Override or clear the spoken line for `kind`. Empty text restores the default.
pub fn set_line(kind: ShoutKind, text: &str) {
    let mut map = line_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        map.remove(&kind);
    } else {
        map.insert(kind, trimmed.to_owned());
    }
}

/// Current line, supplied override or the in-character default.
#[must_use]
pub fn line(kind: ShoutKind) -> String {
    let map = line_store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get(&kind)
        .cloned()
        .unwrap_or_else(|| kind.default_line().to_owned())
}

/// Snapshot of every kind for `GET /lines`.
#[must_use]
pub fn all_lines() -> HashMap<&'static str, String> {
    let mut out = HashMap::new();
    for kind in [
        ShoutKind::GateBreached,
        ShoutKind::Rally,
        ShoutKind::Retreat,
        ShoutKind::Victory,
        ShoutKind::Charge,
        ShoutKind::Hold,
        ShoutKind::Fallen,
    ] {
        out.insert(kind.as_str(), line(kind));
    }
    out
}

/// Rate-limit table keyed by town and shout kind.
pub type ShoutLog = HashMap<(String, ShoutKind), u32>;
/// Per-player last-tick for squad IC replies.
pub type SquadReplyLog = HashMap<String, u32>;

/// Captain bounty line to nearby enlisted players. Does not share town shout cooldowns.
#[allow(clippy::implicit_hasher)]
pub fn try_bounty_shout(
    world: &World,
    town_id: &str,
    x: f64,
    z: f64,
    speaker: &str,
    text: &str,
    enlisted: &std::collections::HashSet<String>,
) -> u32 {
    let text = text.trim();
    if text.is_empty() {
        return 0;
    }
    let speaker = speaker.trim();
    let speaker = if speaker.is_empty() {
        "captain"
    } else {
        speaker
    };
    let msg = TextComponent::text(format!("<{speaker}> {text}")).color_named(NamedColor::Gold);
    let mut n = 0u32;
    for player in world.players.load().iter() {
        if !enlisted.contains(&player.gameprofile.id.to_string()) {
            continue;
        }
        let pos = player.living_entity.entity.pos.load();
        if (pos.x - x).hypot(pos.z - z) <= CHAT_RANGE {
            player.send_system_message(&msg);
            n += 1;
        }
    }
    tracing::info!(
        town = town_id,
        speaker,
        heard = n,
        kind = "bounty",
        "kingdoms captain bounty"
    );
    n
}

/// Speak to players within [`CHAT_RANGE`] of `(x, z)` if the cooldown has elapsed.
pub fn try_shout(
    world: &World,
    tick: u32,
    town_id: &str,
    kind: ShoutKind,
    x: f64,
    z: f64,
    last: &mut ShoutLog,
) -> bool {
    try_shout_as(world, tick, town_id, kind, x, z, last, "captain", None)
}

/// Same as [`try_shout`] with a named speaker and optional supplied text.
pub fn try_shout_as(
    world: &World,
    tick: u32,
    town_id: &str,
    kind: ShoutKind,
    x: f64,
    z: f64,
    last: &mut ShoutLog,
    speaker: &str,
    spoken: Option<&str>,
) -> bool {
    let key = (town_id.to_owned(), kind);
    if last
        .get(&key)
        .is_some_and(|t| tick.saturating_sub(*t) < CHAT_COOLDOWN_TICKS)
    {
        return false;
    }
    last.insert(key, tick);
    let text = spoken
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map_or_else(|| line(kind), ToOwned::to_owned);
    let speaker = speaker.trim();
    let speaker = if speaker.is_empty() {
        "soldier"
    } else {
        speaker
    };
    let msg = TextComponent::text(format!("<{speaker}> {text}")).color_named(NamedColor::Gold);
    let mut n = 0u32;
    for player in world.players.load().iter() {
        let pos = player.living_entity.entity.pos.load();
        if (pos.x - x).hypot(pos.z - z) <= CHAT_RANGE {
            player.send_system_message(&msg);
            n += 1;
        }
    }
    tracing::info!(
        town = town_id,
        kind = kind.as_str(),
        speaker,
        heard = n,
        "kingdoms captain shout"
    );
    true
}

/// True when this player can hear another squad reply.
#[must_use]
pub fn squad_reply_ready(last: Option<u32>, tick: u32) -> bool {
    last.is_none_or(|t| tick.saturating_sub(t) >= SQUAD_REPLY_COOLDOWN_TICKS)
}

/// Player-directed IC reply. Does not touch the town captain/blacksmith shout log.
#[allow(clippy::implicit_hasher)]
pub fn try_squad_reply(
    world: &World,
    tick: u32,
    commander: &Player,
    speaker: &str,
    text: &str,
    last: &mut SquadReplyLog,
) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let uuid = commander.gameprofile.id.to_string();
    if !squad_reply_ready(last.get(&uuid).copied(), tick) {
        return false;
    }
    last.insert(uuid, tick);
    let speaker = speaker.trim();
    let speaker = if speaker.is_empty() {
        "soldier"
    } else {
        speaker
    };
    let msg = TextComponent::text(format!("<{speaker}> {text}")).color_named(NamedColor::Gold);
    commander.send_system_message(&msg);
    let origin = commander.living_entity.entity.pos.load();
    let commander_id = commander.gameprofile.id;
    let mut heard = 1u32;
    for player in world.players.load().iter() {
        if player.gameprofile.id == commander_id {
            continue;
        }
        let pos = player.living_entity.entity.pos.load();
        if (pos.x - origin.x).hypot(pos.z - origin.z) <= CHAT_RANGE {
            player.send_system_message(&msg);
            heard += 1;
        }
    }
    tracing::info!(
        player = %commander.gameprofile.name,
        speaker,
        text,
        heard,
        "kingdoms squad reply"
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_lines_are_short_and_named() {
        assert!(line(ShoutKind::GateBreached).contains("gate"));
        assert!(line(ShoutKind::Rally).contains("walls"));
        assert!(line(ShoutKind::Retreat).contains("keep"));
        assert!(line(ShoutKind::Victory).contains("ours"));
        assert_eq!(
            ShoutKind::parse("gate_breached"),
            Some(ShoutKind::GateBreached)
        );
        assert_eq!(ShoutKind::parse("rout"), Some(ShoutKind::Retreat));
        assert_eq!(ShoutKind::parse("charge"), Some(ShoutKind::Charge));
        assert_eq!(ShoutKind::parse("hold"), Some(ShoutKind::Hold));
        assert_eq!(ShoutKind::parse("fallen"), Some(ShoutKind::Fallen));
        assert!(line(ShoutKind::Charge).contains("flag"));
        assert!(ShoutKind::parse("kill").is_none());
    }

    #[test]
    fn supplied_line_overrides_then_clears() {
        set_line(ShoutKind::Rally, "  Banners high.  ");
        assert_eq!(line(ShoutKind::Rally), "Banners high.");
        set_line(ShoutKind::Rally, "");
        assert_eq!(line(ShoutKind::Rally), ShoutKind::Rally.default_line());
    }

    #[test]
    fn squad_reply_has_its_own_short_cooldown() {
        assert!(squad_reply_ready(None, 0));
        assert!(!squad_reply_ready(Some(10), 29));
        assert!(squad_reply_ready(Some(10), 30));
        const { assert!(SQUAD_REPLY_COOLDOWN_TICKS < CHAT_COOLDOWN_TICKS) };
        // Follow then hold 4 s apart (soak bot) always prints.
        assert!(squad_reply_ready(Some(100), 180));
    }

    #[test]
    fn cooldown_blocks_the_same_town_kind() {
        let mut last = ShoutLog::new();
        last.insert(("t".to_owned(), ShoutKind::Victory), 10);
        assert!(
            last.get(&("t".to_owned(), ShoutKind::Victory))
                .is_some_and(|t| 50u32.saturating_sub(*t) < CHAT_COOLDOWN_TICKS)
        );
        assert!(
            last.get(&("t".to_owned(), ShoutKind::Victory))
                .is_some_and(|t| 300u32.saturating_sub(*t) >= CHAT_COOLDOWN_TICKS)
        );
    }
}
