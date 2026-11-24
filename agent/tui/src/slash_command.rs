use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

/// The plugin catalog (`/plugins`) stays off in TUI until Forall ships its own plugin surface.
const APPS_AND_PLUGINS_SLASH_COMMANDS_ENABLED: bool = false;

pub(crate) fn apps_and_plugins_slash_commands_enabled() -> bool {
    APPS_AND_PLUGINS_SLASH_COMMANDS_ENABLED
}

/// Commands that can be invoked by starting a message with a leading slash.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    // DO NOT ALPHA-SORT! Enum order is presentation order in the popup, so
    // more frequently used commands should be listed first.
    Model,
    Provider,
    Ide,
    Permissions,
    Keymap,
    Vim,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    #[strum(to_string = "approve")]
    AutoReview,
    Memories,
    Skills,
    Import,
    Hooks,
    Review,
    Rename,
    New,
    Archive,
    Delete,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Goal,
    Agent,
    Side,
    Btw,
    Copy,
    Raw,
    Diff,
    Mention,
    Status,
    Usage,
    DebugConfig,
    Title,
    Statusline,
    Theme,
    Mcp,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    #[strum(to_string = "stop", serialize = "clean")]
    Stop,
    Clear,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
    // Debugging commands.
    #[strum(serialize = "debug-m-drop")]
    MemoryDrop,
    #[strum(serialize = "debug-m-update")]
    MemoryUpdate,
}

impl SlashCommand {
    /// User-visible description shown in the popup.
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "show where to report an issue",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create an AGENTS.md file with instructions for Forall",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Rename => "rename the current thread",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Archive => "archive this session and exit",
            SlashCommand::Delete => "permanently delete this session and exit",
            SlashCommand::Clear => "clear the terminal and start a new chat",
            SlashCommand::Fork => "fork the current chat",
            SlashCommand::Quit | SlashCommand::Exit => "exit Forall",
            SlashCommand::Copy => "copy last response as markdown",
            SlashCommand::Raw => "toggle raw scrollback mode for copy-friendly terminal selection",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Skills => "use skills to improve how Forall performs specific tasks",
            SlashCommand::Import => "import setup, this project, and recent chats from Claude Code",
            SlashCommand::Hooks => "view and manage lifecycle hooks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::Usage => "view account usage or use a usage limit reset",
            SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
            SlashCommand::Title => "configure which items appear in the terminal title",
            SlashCommand::Statusline => "configure which items appear in the status line",
            SlashCommand::Theme => "choose a syntax highlighting theme",
            SlashCommand::Ps => "list background terminals",
            SlashCommand::Stop => "stop all background terminals",
            SlashCommand::MemoryDrop => "DO NOT USE",
            SlashCommand::MemoryUpdate => "DO NOT USE",
            SlashCommand::Provider => "choose a model provider and manage its API key",
            SlashCommand::Model => {
                "choose what model and reasoning effort to use (or `/model <slug>`)"
            }
            SlashCommand::Ide => {
                "include current selection, open files, and other context from your IDE"
            }
            SlashCommand::Plan => "switch to Plan mode",
            SlashCommand::Goal => "set or view the goal for a long-running task",
            SlashCommand::Agent | SlashCommand::MultiAgents => "switch the active agent thread",
            SlashCommand::Side | SlashCommand::Btw => {
                "start a side conversation in an ephemeral fork"
            }
            SlashCommand::Permissions => "choose what Forall is allowed to do",
            SlashCommand::Keymap => "remap TUI shortcuts",
            SlashCommand::Vim => "toggle Vim mode for the composer",
            SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
            SlashCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            SlashCommand::Experimental => "toggle experimental features",
            SlashCommand::AutoReview => "approve one retry of a recent auto-review denial",
            SlashCommand::Memories => "configure memory use and generation",
            SlashCommand::Mcp => "list configured MCP tools; use /mcp verbose for details",
            SlashCommand::Plugins => "browse plugins",
            SlashCommand::Logout => "log out of Forall",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
        }
    }

    /// Command string without the leading '/'. Provided for compatibility with
    /// existing code that expects a method named `command()`.
    pub fn command(self) -> &'static str {
        self.into()
    }

    /// Whether this command supports inline args (for example `/review ...`).
    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Review
                | SlashCommand::Rename
                | SlashCommand::Plan
                | SlashCommand::Goal
                | SlashCommand::Ide
                | SlashCommand::Keymap
                | SlashCommand::Mcp
                | SlashCommand::Model
                | SlashCommand::Raw
                | SlashCommand::Usage
                | SlashCommand::Side
                | SlashCommand::Btw
                | SlashCommand::Resume
                | SlashCommand::SandboxReadRoot
        )
    }

    /// Whether this command remains available inside an active side conversation.
    pub fn available_in_side_conversation(self) -> bool {
        matches!(
            self,
            SlashCommand::Copy
                | SlashCommand::Raw
                | SlashCommand::Diff
                | SlashCommand::Mention
                | SlashCommand::Status
                | SlashCommand::Usage
                | SlashCommand::Ide
        )
    }

    /// Whether this command can be run while a task is in progress.
    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Archive
            | SlashCommand::Delete
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Keymap
            | SlashCommand::Vim
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::Memories
            | SlashCommand::Import
            | SlashCommand::Review
            | SlashCommand::Plan
            | SlashCommand::Clear
            | SlashCommand::Logout
            | SlashCommand::MemoryDrop
            | SlashCommand::Provider
            | SlashCommand::MemoryUpdate => false,
            SlashCommand::Diff
            | SlashCommand::Resume
            | SlashCommand::Model
            | SlashCommand::Permissions
            | SlashCommand::Copy
            | SlashCommand::Raw
            | SlashCommand::Rename
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Hooks
            | SlashCommand::Status
            | SlashCommand::Usage
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Goal
            | SlashCommand::Mcp
            | SlashCommand::Plugins
            | SlashCommand::Title
            | SlashCommand::Statusline
            | SlashCommand::AutoReview
            | SlashCommand::Feedback
            | SlashCommand::Ide
            | SlashCommand::Quit
            | SlashCommand::Exit
            | SlashCommand::Side
            | SlashCommand::Btw => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
            SlashCommand::Agent | SlashCommand::MultiAgents => true,
            SlashCommand::Theme => false,
        }
    }

    fn is_visible(self) -> bool {
        match self {
            SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            SlashCommand::Copy => !cfg!(target_os = "android"),
            SlashCommand::Plugins => apps_and_plugins_slash_commands_enabled(),
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }
}

/// Return all built-in commands in a Vec paired with their command string.
pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    use super::SlashCommand;

    #[test]
    fn stop_command_is_canonical_name() {
        assert_eq!(SlashCommand::Stop.command(), "stop");
    }

    #[test]
    fn clean_alias_parses_to_stop_command() {
        assert_eq!(SlashCommand::from_str("clean"), Ok(SlashCommand::Stop));
    }

    #[test]
    fn certain_commands_are_available_during_task() {
        assert!(SlashCommand::Goal.available_during_task());
        assert!(SlashCommand::Ide.available_during_task());
        assert!(SlashCommand::Title.available_during_task());
        assert!(SlashCommand::Statusline.available_during_task());
        assert!(SlashCommand::Raw.available_during_task());
        assert!(SlashCommand::Raw.available_in_side_conversation());
        assert!(SlashCommand::Raw.supports_inline_args());
    }

    #[test]
    fn auto_review_command_is_approve() {
        assert_eq!(SlashCommand::AutoReview.command(), "approve");
        assert_eq!(
            SlashCommand::from_str("approve"),
            Ok(SlashCommand::AutoReview)
        );
    }
}
