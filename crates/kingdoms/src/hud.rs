//! Capture boss bar, kingdom sidebar, and tab-list colours.
//!
//! Vanilla packets only: boss bar, dummy sidebar objective, and coloured
//! `PlayerInfo` names. Core is untouched.

use std::collections::HashMap;

use pumpkin_core::entity::player::Player;
use pumpkin_core::world::World;
use pumpkin_core::world::bossbar::BossbarColor;
use pumpkin_core::world::scoreboard::ScoreboardObjective;
use pumpkin_data::scoreboard::ScoreboardDisplaySlot;
use pumpkin_protocol::java::client::play::RenderType;
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

/// World dummy objective shown on the sidebar.
pub const SIDEBAR_OBJECTIVE: &str = "kingdoms";
const SIDEBAR_TITLE: &str = "Kingdoms";

/// Snapshot the director gathers once per HUD pass.
pub struct HudSnapshot {
    pub alder_towns: i32,
    pub raven_towns: i32,
    pub alder_morale: i32,
    pub raven_morale: i32,
    pub players: HashMap<String, HudPlayer>,
    pub paint_tab: bool,
}

/// Per-player sidebar and tab-list fields.
pub struct HudPlayer {
    pub kingdom: String,
    pub rank: i32,
    pub renown: i32,
}

/// Short town name for the capture bar.
#[must_use]
pub fn town_label(town_id: &str) -> String {
    let mut parts = town_id.rsplit(':');
    let z = parts.next();
    let x = parts.next();
    match (x, z) {
        (Some("0"), Some("0")) => "capital".to_owned(),
        (Some(x), Some(z)) => format!("{x},{z}"),
        _ => town_id.to_owned(),
    }
}

/// Side currently taking the plate: occupier while capturing, owner while reclaiming.
#[must_use]
pub fn capturing_side<'a>(occupy: bool, occupier: &'a str, kingdom: &'a str) -> &'a str {
    if occupy { occupier } else { kingdom }
}

/// `{town}  {side}  {remain}s`
#[must_use]
pub fn capture_title(town: &str, side: &str, hold: u32, ticks: u32) -> String {
    let remain = ticks.saturating_sub(hold);
    let secs = remain.div_ceil(20);
    format!("{town}  {side}  {secs}s")
}

#[must_use]
pub fn capture_progress(hold: u32, ticks: u32) -> f32 {
    if ticks == 0 {
        0.0
    } else {
        (hold as f32 / ticks as f32).clamp(0.0, 1.0)
    }
}

#[must_use]
pub fn capture_color(side: &str) -> BossbarColor {
    match side {
        "alder" => BossbarColor::Green,
        "raven" => BossbarColor::Blue,
        _ => BossbarColor::Red,
    }
}

#[must_use]
pub const fn color_name(color: BossbarColor) -> &'static str {
    match color {
        BossbarColor::Green => "green",
        BossbarColor::Blue => "blue",
        BossbarColor::Red => "red",
        _ => "red",
    }
}

#[must_use]
pub fn tab_color(kingdom: &str) -> NamedColor {
    match kingdom {
        "alder" => NamedColor::Green,
        "raven" => NamedColor::Blue,
        _ => NamedColor::Red,
    }
}

/// Levy 1, sergeant 2, captain 3.
#[must_use]
pub fn rank_score(enlist: &str) -> i32 {
    match enlist {
        "sergeant" => 2,
        "captain" => 3,
        _ => 1,
    }
}

/// Colour this player's tab-list name by kingdom.
pub fn apply_tab_name(player: &Player, kingdom: &str) {
    player.set_tab_list_name(Some(
        TextComponent::text(player.gameprofile.name.clone()).color_named(tab_color(kingdom)),
    ));
}

/// Send the sidebar and (optionally) recolour tab-list names.
pub fn tick(world: &World, snap: &HudSnapshot) {
    ensure_sidebar(world);
    let mut sb = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sb.set_score_value(world, "Alder towns", SIDEBAR_OBJECTIVE, snap.alder_towns);
    sb.set_score_value(world, "Raven towns", SIDEBAR_OBJECTIVE, snap.raven_towns);
    sb.set_score_value(world, "Alder morale", SIDEBAR_OBJECTIVE, snap.alder_morale);
    sb.set_score_value(world, "Raven morale", SIDEBAR_OBJECTIVE, snap.raven_morale);
    for player in world.players.load().iter() {
        let uuid = player.gameprofile.id.to_string();
        let Some(row) = snap.players.get(&uuid) else {
            continue;
        };
        sb.set_score_value(player.as_ref(), "Your rank", SIDEBAR_OBJECTIVE, row.rank);
        sb.set_score_value(
            player.as_ref(),
            "Your renown",
            SIDEBAR_OBJECTIVE,
            row.renown,
        );
        if snap.paint_tab {
            apply_tab_name(player, &row.kingdom);
        }
    }
}

fn ensure_sidebar(world: &World) {
    let mut sb = world
        .scoreboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if sb.get_objective(SIDEBAR_OBJECTIVE).is_some() {
        return;
    }
    sb.add_objective(
        world,
        ScoreboardObjective::new(
            SIDEBAR_OBJECTIVE,
            TextComponent::text(SIDEBAR_TITLE),
            RenderType::Integer,
            None,
            "dummy",
        ),
    );
    sb.set_display_objective(
        world,
        ScoreboardDisplaySlot::Sidebar,
        Some(SIDEBAR_OBJECTIVE),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capital_bar_names_the_side_and_hold_timer() {
        assert_eq!(town_label("20261025:0:0"), "capital");
        assert_eq!(town_label("20261025:0:-1"), "0,-1");
        assert_eq!(
            capture_title("capital", "alder", 100, 200),
            "capital  alder  5s"
        );
        assert_eq!(capturing_side(true, "raven", "alder"), "raven");
        assert_eq!(capturing_side(false, "pillager", "alder"), "alder");
        assert_eq!(color_name(capture_color("alder")), "green");
        assert_eq!(color_name(capture_color("raven")), "blue");
        assert_eq!(color_name(capture_color("pillager")), "red");
        assert_eq!(color_name(BossbarColor::Green), "green");
        assert!((capture_progress(50, 100) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn sidebar_rank_and_tab_colours() {
        assert_eq!(rank_score("levy"), 1);
        assert_eq!(rank_score("sergeant"), 2);
        assert_eq!(rank_score("captain"), 3);
        assert_eq!(tab_color("alder"), NamedColor::Green);
        assert_eq!(tab_color("raven"), NamedColor::Blue);
        assert_eq!(tab_color("pillager"), NamedColor::Red);
    }
}
