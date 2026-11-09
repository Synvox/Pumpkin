use pumpkin_core::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use pumpkin_core::command::argument_types::core::integer::IntegerArgumentType;
use pumpkin_core::command::argument_types::core::string::StringArgumentType;
use pumpkin_core::command::context::command_context::CommandContext;
use pumpkin_core::command::node::dispatcher::CommandDispatcher;
use pumpkin_core::command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionManager};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

use crate::director;
use crate::plan::cached_plan;
use crate::raid::{RaidSide, RaidSpec};
use crate::town::nearest_town;

const LOCATE_NAME: &str = "locatetown";
const LOCATE_DESC: &str = "Reports the nearest planned kingdoms town.";
const LOCATE_PERM: &str = "kingdoms:command.locatetown";

const KINGDOM_NAME: &str = "kingdom";
const KINGDOM_DESC: &str = "Kingdom info, join, fronts, deploy, and stats.";
const KINGDOM_PERM: &str = "kingdoms:command.kingdom";

const RAID_NAME: &str = "raid";
const RAID_DESC: &str = "Start or stop a pillager raid on a town.";
const RAID_PERM: &str = "kingdoms:command.raid";

const WAR_NAME: &str = "war";
const WAR_DESC: &str = "Front-line scores, clock, and kingdom captures.";
const WAR_PERM: &str = "kingdoms:command.war";

struct LocateTownExecutor;

impl CommandExecutor for LocateTownExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world();
        let seed = world.level.seed.0;
        let pos = context.position;
        let town = nearest_town(seed, pos.x.floor() as i32, pos.z.floor() as i32);
        let y = world
            .sample_surface_height(town.x, town.z)
            .unwrap_or(world.sea_level);

        let extra = cached_plan(seed, town.cell_x, town.cell_z).map_or_else(
            || {
                crate::plan::request_plan(seed, town.cell_x, town.cell_z);
                String::new()
            },
            |plan| {
                let (cx, cz) = plan.castle.bounds.center();
                let (w, d) = plan.castle_footprint();
                format!(
                    " castle {cx} {cz} ({w}x{d}) well {} {}",
                    plan.well.0, plan.well.1
                )
            },
        );

        context.source.send_feedback(
            TextComponent::text(format!(
                "Nearest kingdom town: {} {} {} (cell {}, {}){extra}",
                town.x, y, town.z, town.cell_x, town.cell_z
            ))
            .color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

struct KingdomInfoExecutor;

impl CommandExecutor for KingdomInfoExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world().clone();
        let pos = context.position;
        let seed = world.level.seed.0;
        let town = nearest_town(seed, pos.x.floor() as i32, pos.z.floor() as i32);
        let town_id = crate::info::town_id(seed, town.cell_x, town.cell_z);
        let kingdom = crate::faction::KingdomId::for_cell(town.cell_x, town.cell_z);
        let battle = director::battle_state(&town_id);
        let plan = cached_plan(seed, town.cell_x, town.cell_z);
        if plan.is_none() {
            crate::plan::request_plan(seed, town.cell_x, town.cell_z);
        }
        let mut lines = vec![format!(
            "Town {town_id}  kingdom {}  origin {} {}",
            kingdom.as_str(),
            town.x,
            town.z
        )];
        if let Some(plan) = plan {
            lines.push(format!(
                "keep {} {}  gates {}  lots {}",
                plan.castle.keep.center().0,
                plan.castle.keep.center().1,
                plan.gates.len(),
                plan.lots.len()
            ));
        }
        if let Some(b) = battle {
            lines.push(format!(
                "owner={} alarm={} raid={} wave {}/{} defenders {} raiders {} killed d/r {}/{} breached={}",
                b.owner,
                b.alarm,
                b.raid_active,
                b.wave,
                b.waves,
                b.defenders_alive,
                b.raiders_alive,
                b.defenders_killed,
                b.raiders_killed,
                b.gate_breached
            ));
        } else {
            lines.push("no active battle".to_owned());
        }
        if let Some(player) = context.as_player() {
            let joined = director::lock()
                .player_kingdom(&player.gameprofile.id.to_string())
                .map(|k| k.as_str().to_owned());
            lines.push(format!(
                "you: {}",
                joined.unwrap_or_else(|| "not in a kingdom".to_owned())
            ));
        }
        context.source.send_feedback(
            TextComponent::text(lines.join(" | ")).color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

struct KingdomJoinExecutor;

impl CommandExecutor for KingdomJoinExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let Some(player) = context.as_player() else {
            context.source.send_feedback(
                TextComponent::text("Only players can join a kingdom").color_named(NamedColor::Red),
                false,
            );
            return Ok(0);
        };
        let name = StringArgumentType::get(context, "kingdom")?;
        let id = director::join_kingdom(&player, name);
        context.source.send_feedback(
            TextComponent::text(format!("Joined kingdom {}", id.as_str()))
                .color_named(NamedColor::Green),
            false,
        );
        Ok(1)
    }
}

struct RaidStartExecutor {
    has_size: bool,
    has_defenders: bool,
}

impl CommandExecutor for RaidStartExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world().clone();
        let pos = context.position;
        let town_arg = StringArgumentType::get(context, "town")?;
        let Some(town_id) = director::resolve_town_id(&world, pos, town_arg) else {
            context.source.send_feedback(
                TextComponent::text(format!("Unknown town {town_arg}"))
                    .color_named(NamedColor::Red),
                false,
            );
            return Ok(0);
        };
        let size = if self.has_size {
            IntegerArgumentType::get(context, "size")?
        } else {
            24
        };
        let mut spec = RaidSpec::new(size as u16);
        if self.has_defenders {
            spec =
                spec.with_extra_defenders(IntegerArgumentType::get(context, "defenders")? as u16);
        }
        match director::start_raid(&world, &town_id, spec) {
            Ok(id) => {
                context.source.send_feedback(
                    TextComponent::text(format!(
                        "Raid {} started on {town_id} ({} raiders, {} waves, {} extra defenders)",
                        id.0, spec.raiders, spec.waves, spec.extra_defenders
                    ))
                    .color_named(NamedColor::Gold),
                    true,
                );
                Ok(1)
            }
            Err(err) => {
                context
                    .source
                    .send_feedback(TextComponent::text(err).color_named(NamedColor::Red), false);
                Ok(0)
            }
        }
    }
}

struct RaidRetakeExecutor {
    has_size: bool,
}

impl CommandExecutor for RaidRetakeExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context.world().clone();
        let pos = context.position;
        let town_arg = StringArgumentType::get(context, "town")?;
        let Some(town_id) = director::resolve_town_id(&world, pos, town_arg) else {
            context.source.send_feedback(
                TextComponent::text(format!("Unknown town {town_arg}"))
                    .color_named(NamedColor::Red),
                false,
            );
            return Ok(0);
        };
        let size = if self.has_size {
            IntegerArgumentType::get(context, "size")? as u16
        } else {
            40
        };
        let spec = RaidSpec::new(size).with_side(RaidSide::Kingdom);
        match director::start_raid(&world, &town_id, spec) {
            Ok(id) => {
                context.source.send_feedback(
                    TextComponent::text(format!(
                        "Retake {} started on {town_id} ({} soldiers)",
                        id.0, spec.raiders
                    ))
                    .color_named(NamedColor::Gold),
                    true,
                );
                Ok(1)
            }
            Err(err) => {
                context
                    .source
                    .send_feedback(TextComponent::text(err).color_named(NamedColor::Red), false);
                Ok(0)
            }
        }
    }
}

struct WarExecutor;

impl CommandExecutor for WarExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        context.source.send_feedback(
            TextComponent::text(director::format_war_status()).color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

struct WarStatsExecutor;

impl CommandExecutor for WarStatsExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let Some(player) = context.as_player() else {
            context.source.send_feedback(
                TextComponent::text("Only players have war stats").color_named(NamedColor::Red),
                false,
            );
            return Ok(0);
        };
        let line = director::format_player_stats(
            &player.gameprofile.id.to_string(),
            &player.gameprofile.name,
        );
        context.source.send_feedback(
            TextComponent::text(line).color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

struct KingdomFrontsExecutor;

impl CommandExecutor for KingdomFrontsExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        context.source.send_feedback(
            TextComponent::text(director::format_fronts()).color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

struct KingdomDeployExecutor {
    has_town: bool,
}

impl CommandExecutor for KingdomDeployExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let Some(player) = context.as_player() else {
            context.source.send_feedback(
                TextComponent::text("Only players can deploy").color_named(NamedColor::Red),
                false,
            );
            return Ok(0);
        };
        let town = if self.has_town {
            Some(StringArgumentType::get(context, "town")?)
        } else {
            None
        };
        match director::deploy_player(&player, town) {
            Ok(msg) => {
                context.source.send_feedback(
                    TextComponent::text(msg).color_named(NamedColor::Green),
                    true,
                );
                Ok(1)
            }
            Err(err) => {
                context
                    .source
                    .send_feedback(TextComponent::text(err).color_named(NamedColor::Red), false);
                Ok(0)
            }
        }
    }
}

struct RaidStopExecutor;

impl CommandExecutor for RaidStopExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let n = director::stop_raid(None);
        context.source.send_feedback(
            TextComponent::text(format!("Stopped {n} raid(s)")).color_named(NamedColor::Gold),
            true,
        );
        Ok(1)
    }
}

/// Registers `/locatetown`, `/kingdom`, and `/raid`.
pub fn register(dispatcher: &mut CommandDispatcher, permissions: &PermissionManager) {
    permissions.register_permission_or_panic(Permission::new(
        LOCATE_PERM,
        LOCATE_DESC,
        PermissionDefault::Op(PermissionLvl::Two),
    ));
    permissions.register_permission_or_panic(Permission::new(
        KINGDOM_PERM,
        KINGDOM_DESC,
        PermissionDefault::Allow,
    ));
    permissions.register_permission_or_panic(Permission::new(
        RAID_PERM,
        RAID_DESC,
        PermissionDefault::Op(PermissionLvl::Two),
    ));
    permissions.register_permission_or_panic(Permission::new(
        WAR_PERM,
        WAR_DESC,
        PermissionDefault::Allow,
    ));

    dispatcher.register(
        command(LOCATE_NAME, LOCATE_DESC)
            .requires(LOCATE_PERM)
            .executes(LocateTownExecutor),
    );

    dispatcher.register(
        command(KINGDOM_NAME, KINGDOM_DESC)
            .requires(KINGDOM_PERM)
            .then(literal("info").executes(KingdomInfoExecutor))
            .then(literal("join").then(
                argument("kingdom", StringArgumentType::SingleWord).executes(KingdomJoinExecutor),
            ))
            .then(literal("fronts").executes(KingdomFrontsExecutor))
            .then(
                literal("deploy")
                    .executes(KingdomDeployExecutor { has_town: false })
                    .then(
                        argument("town", StringArgumentType::QuotablePhrase)
                            .executes(KingdomDeployExecutor { has_town: true }),
                    ),
            )
            .then(literal("stats").executes(WarStatsExecutor)),
    );

    dispatcher.register(
        command(RAID_NAME, RAID_DESC)
            .requires(RAID_PERM)
            .then(
                literal("start").then(
                    argument("town", StringArgumentType::QuotablePhrase)
                        .executes(RaidStartExecutor {
                            has_size: false,
                            has_defenders: false,
                        })
                        .then(
                            argument("size", IntegerArgumentType::new(1, 400))
                                .executes(RaidStartExecutor {
                                    has_size: true,
                                    has_defenders: false,
                                })
                                .then(
                                    argument("defenders", IntegerArgumentType::new(0, 400))
                                        .executes(RaidStartExecutor {
                                            has_size: true,
                                            has_defenders: true,
                                        }),
                                ),
                        ),
                ),
            )
            .then(
                literal("retake").then(
                    argument("town", StringArgumentType::QuotablePhrase)
                        .executes(RaidRetakeExecutor { has_size: false })
                        .then(
                            argument("size", IntegerArgumentType::new(1, 400))
                                .executes(RaidRetakeExecutor { has_size: true }),
                        ),
                ),
            )
            .then(literal("stop").executes(RaidStopExecutor)),
    );

    dispatcher.register(
        command(WAR_NAME, WAR_DESC)
            .requires(WAR_PERM)
            .executes(WarExecutor)
            .then(literal("stats").executes(WarStatsExecutor)),
    );
}
