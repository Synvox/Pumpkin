use pumpkin_core::command::argument_builder::{ArgumentBuilder, command};
use pumpkin_core::command::context::command_context::CommandContext;
use pumpkin_core::command::node::dispatcher::CommandDispatcher;
use pumpkin_core::command::node::{CommandExecutor, CommandExecutorResult};
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionManager};
use pumpkin_util::text::TextComponent;
use pumpkin_util::text::color::NamedColor;

use crate::town::nearest_town;

const NAME: &str = "locatetown";
const DESCRIPTION: &str = "Reports the nearest planned kingdoms town.";
const PERMISSION: &str = "kingdoms:command.locatetown";

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

        context.source.send_feedback(
            TextComponent::text(format!(
                "Nearest kingdom town: {} {} {} (cell {}, {})",
                town.x, y, town.z, town.cell_x, town.cell_z
            ))
            .color_named(NamedColor::Gold),
            false,
        );
        Ok(1)
    }
}

/// Registers `/locatetown` on the server dispatcher.
pub fn register(dispatcher: &mut CommandDispatcher, permissions: &PermissionManager) {
    permissions.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));
    dispatcher.register(
        command(NAME, DESCRIPTION)
            .requires(PERMISSION)
            .executes(LocateTownExecutor),
    );
}
