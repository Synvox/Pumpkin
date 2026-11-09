//! Captain shouts to nearby players, plus a director hook for supplied lines.
//!
//! Captains post short in-character lines at gate breach, rally, retreat, and
//! victory. Lines are rate-limited per town and event. `POST /lines` lets the
//! Sails director override the default text without pausing the war clock.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use pumpkin_core::world::World;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;
use serde::{Deserialize, Serialize};

/// How close a player must be (blocks) to hear a captain.
pub const CHAT_RANGE: f64 = 48.0;
/// Minimum ticks between the same shout kind in one town (~10 s).
pub const CHAT_COOLDOWN_TICKS: u32 = 200;

/// Combat beats that get a spoken line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShoutKind {
    GateBreached,
    Rally,
    Retreat,
    Victory,
}

impl ShoutKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GateBreached => "gate_breached",
            Self::Rally => "rally",
            Self::Retreat => "retreat",
            Self::Victory => "victory",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "gate_breached" | "breach" => Some(Self::GateBreached),
            "rally" | "raid_started" | "war_clock" => Some(Self::Rally),
            "retreat" | "rout" => Some(Self::Retreat),
            "victory" | "town_captured" | "capture" | "retake" => Some(Self::Victory),
            _ => None,
        }
    }

    const fn default_line(self) -> &'static str {
        match self {
            Self::GateBreached => "The gate is down. Through the breach!",
            Self::Rally => "To the walls. Hold the line.",
            Self::Retreat => "Fall back to the keep. Do not break.",
            Self::Victory => "The town is ours. Raise the colours.",
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
    ] {
        out.insert(kind.as_str(), line(kind));
    }
    out
}

/// Rate-limit table keyed by town and shout kind.
pub type ShoutLog = HashMap<(String, ShoutKind), u32>;

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
    let key = (town_id.to_owned(), kind);
    if last
        .get(&key)
        .is_some_and(|t| tick.saturating_sub(*t) < CHAT_COOLDOWN_TICKS)
    {
        return false;
    }
    last.insert(key, tick);
    let text = line(kind);
    let msg = TextComponent::text(format!("<captain> {text}")).color_named(NamedColor::Gold);
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
        heard = n,
        "kingdoms captain shout"
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
