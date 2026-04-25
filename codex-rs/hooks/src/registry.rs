use codex_config::ConfigLayerStack;
use codex_protocol::protocol::HookEventName;
use tokio::process::Command;

use crate::engine::ClaudeHooksEngine;
use crate::engine::CommandShell;
use crate::events::post_tool_use::PostToolUseOutcome;
use crate::events::post_tool_use::PostToolUseRequest;
use crate::events::pre_tool_use::PreToolUseOutcome;
use crate::events::pre_tool_use::PreToolUseRequest;
use crate::events::session_end::SessionEndOutcome;
use crate::events::session_end::SessionEndRequest;
use crate::events::session_start::SessionStartOutcome;
use crate::events::session_start::SessionStartRequest;
use crate::events::stop::StopOutcome;
use crate::events::stop::StopRequest;
use crate::events::user_prompt_submit::UserPromptSubmitOutcome;
use crate::events::user_prompt_submit::UserPromptSubmitRequest;
use crate::types::Hook;
use crate::types::HookEvent;
use crate::types::HookPayload;
use crate::types::HookResponse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchHook {
    pub event_name: HookEventName,
    pub command: String,
}

impl std::str::FromStr for LaunchHook {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (event_name, command) = value
            .split_once('=')
            .ok_or_else(|| "expected EVENT=COMMAND".to_string())?;
        let command = command.to_string();
        if command.trim().is_empty() {
            return Err("hook command cannot be empty".to_string());
        }

        let event_name = parse_hook_event_name(event_name)?;
        Ok(Self {
            event_name,
            command,
        })
    }
}

fn parse_hook_event_name(value: &str) -> Result<HookEventName, String> {
    let normalized = value
        .trim()
        .chars()
        .filter(|ch| *ch != '-' && *ch != '_')
        .flat_map(char::to_lowercase)
        .collect::<String>();
    match normalized.as_str() {
        "sessionstart" => Ok(HookEventName::SessionStart),
        "sessionend" => Ok(HookEventName::SessionEnd),
        "userpromptsubmit" => Ok(HookEventName::UserPromptSubmit),
        "stop" => Ok(HookEventName::Stop),
        _ => Err(format!(
            "unsupported hook event `{value}`; expected session_start, session_end, user_prompt_submit, or stop"
        )),
    }
}

#[derive(Default, Clone)]
pub struct HooksConfig {
    pub legacy_notify_argv: Option<Vec<String>>,
    pub feature_enabled: bool,
    pub config_layer_stack: Option<ConfigLayerStack>,
    pub launch_hooks: Vec<LaunchHook>,
    pub shell_program: Option<String>,
    pub shell_args: Vec<String>,
}

#[derive(Clone)]
pub struct Hooks {
    after_agent: Vec<Hook>,
    after_tool_use: Vec<Hook>,
    engine: ClaudeHooksEngine,
}

impl Default for Hooks {
    fn default() -> Self {
        Self::new(HooksConfig::default())
    }
}

impl Hooks {
    pub fn new(config: HooksConfig) -> Self {
        let after_agent = config
            .legacy_notify_argv
            .filter(|argv| !argv.is_empty() && !argv[0].is_empty())
            .map(crate::notify_hook)
            .into_iter()
            .collect();
        let engine = ClaudeHooksEngine::new(
            config.feature_enabled,
            config.config_layer_stack.as_ref(),
            &config.launch_hooks,
            CommandShell {
                program: config.shell_program.unwrap_or_default(),
                args: config.shell_args,
            },
        );
        Self {
            after_agent,
            after_tool_use: Vec::new(),
            engine,
        }
    }

    pub fn startup_warnings(&self) -> &[String] {
        self.engine.warnings()
    }

    fn hooks_for_event(&self, hook_event: &HookEvent) -> &[Hook] {
        match hook_event {
            HookEvent::AfterAgent { .. } => &self.after_agent,
            HookEvent::AfterToolUse { .. } => &self.after_tool_use,
        }
    }

    pub async fn dispatch(&self, hook_payload: HookPayload) -> Vec<HookResponse> {
        let hooks = self.hooks_for_event(&hook_payload.hook_event);
        let mut outcomes = Vec::with_capacity(hooks.len());
        for hook in hooks {
            let outcome = hook.execute(&hook_payload).await;
            let should_abort_operation = outcome.result.should_abort_operation();
            outcomes.push(outcome);
            if should_abort_operation {
                break;
            }
        }

        outcomes
    }

    pub fn preview_session_start(
        &self,
        request: &SessionStartRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_session_start(request)
    }

    pub fn preview_pre_tool_use(
        &self,
        request: &PreToolUseRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_pre_tool_use(request)
    }

    pub fn preview_post_tool_use(
        &self,
        request: &PostToolUseRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_post_tool_use(request)
    }

    pub async fn run_session_start(
        &self,
        request: SessionStartRequest,
        turn_id: Option<String>,
    ) -> SessionStartOutcome {
        self.engine.run_session_start(request, turn_id).await
    }

    pub fn preview_session_end(
        &self,
        request: &SessionEndRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_session_end(request)
    }

    pub async fn run_session_end(&self, request: SessionEndRequest) -> SessionEndOutcome {
        self.engine.run_session_end(request).await
    }

    pub async fn run_pre_tool_use(&self, request: PreToolUseRequest) -> PreToolUseOutcome {
        self.engine.run_pre_tool_use(request).await
    }

    pub async fn run_post_tool_use(&self, request: PostToolUseRequest) -> PostToolUseOutcome {
        self.engine.run_post_tool_use(request).await
    }

    pub fn preview_user_prompt_submit(
        &self,
        request: &UserPromptSubmitRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_user_prompt_submit(request)
    }

    pub async fn run_user_prompt_submit(
        &self,
        request: UserPromptSubmitRequest,
    ) -> UserPromptSubmitOutcome {
        self.engine.run_user_prompt_submit(request).await
    }

    pub fn preview_stop(
        &self,
        request: &StopRequest,
    ) -> Vec<codex_protocol::protocol::HookRunSummary> {
        self.engine.preview_stop(request)
    }

    pub async fn run_stop(&self, request: StopRequest) -> StopOutcome {
        self.engine.run_stop(request).await
    }
}

pub fn command_from_argv(argv: &[String]) -> Option<Command> {
    let (program, args) = argv.split_first()?;
    if program.is_empty() {
        return None;
    }
    let mut command = Command::new(program);
    command.args(args);
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::LaunchHook;
    use codex_protocol::protocol::HookEventName;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_launch_hook_aliases() {
        let hook = "session-start=echo started"
            .parse::<LaunchHook>()
            .expect("launch hook should parse");
        assert_eq!(
            hook,
            LaunchHook {
                event_name: HookEventName::SessionStart,
                command: "echo started".to_string(),
            }
        );
    }

    #[test]
    fn parses_user_prompt_submit_launch_hook_aliases() {
        let hook = "user-prompt-submit=echo running"
            .parse::<LaunchHook>()
            .expect("launch hook should parse");
        assert_eq!(
            hook,
            LaunchHook {
                event_name: HookEventName::UserPromptSubmit,
                command: "echo running".to_string(),
            }
        );
    }

    #[test]
    fn rejects_unknown_launch_hook_event() {
        let error = "pause=echo nope"
            .parse::<LaunchHook>()
            .expect_err("unknown event should be rejected");
        assert_eq!(
            error,
            "unsupported hook event `pause`; expected session_start, session_end, user_prompt_submit, or stop"
        );
    }
}
