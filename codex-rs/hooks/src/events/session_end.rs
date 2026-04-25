use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_protocol::protocol::SessionEndReason;

use super::common;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::schema::SessionEndCommandInput;
use crate::schema::SessionEndCommandOutputWire;

#[derive(Debug, Clone)]
pub struct SessionEndRequest {
    pub session_id: ThreadId,
    pub cwd: PathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub reason: SessionEndReason,
}

#[derive(Debug)]
pub struct SessionEndOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
}

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    _request: &SessionEndRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::SessionEnd,
        /*matcher_input*/ None,
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: SessionEndRequest,
) -> SessionEndOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::SessionEnd,
        /*matcher_input*/ None,
    );
    if matched.is_empty() {
        return SessionEndOutcome {
            hook_events: Vec::new(),
        };
    }

    let input_json = match serde_json::to_string(&SessionEndCommandInput::new(
        request.session_id.to_string(),
        request.transcript_path.clone(),
        request.cwd.display().to_string(),
        request.model.clone(),
        request.permission_mode.clone(),
        request.reason,
    )) {
        Ok(input_json) => input_json,
        Err(error) => {
            return serialization_failure_outcome(common::serialization_failure_hook_events(
                matched,
                /*turn_id*/ None,
                format!("failed to serialize session end hook input: {error}"),
            ));
        }
    };

    let results = dispatcher::execute_handlers(
        shell,
        matched,
        input_json,
        request.cwd.as_path(),
        /*turn_id*/ None,
        parse_completed,
    )
    .await;

    SessionEndOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<()> {
    let mut entries = Vec::new();
    let mut status = HookRunStatus::Completed;

    match run_result.error.as_deref() {
        Some(error) => {
            status = HookRunStatus::Failed;
            entries.push(HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: error.to_string(),
            });
        }
        None => match run_result.exit_code {
            Some(0) => {
                let trimmed_stdout = run_result.stdout.trim();
                if !trimmed_stdout.is_empty() {
                    match serde_json::from_str::<SessionEndCommandOutputWire>(&run_result.stdout) {
                        Ok(parsed) => {
                            if let Some(system_message) = parsed.universal.system_message {
                                entries.push(HookOutputEntry {
                                    kind: HookOutputEntryKind::Warning,
                                    text: system_message,
                                });
                            }
                            if !parsed.universal.r#continue {
                                status = HookRunStatus::Stopped;
                                if let Some(stop_reason) = parsed.universal.stop_reason {
                                    entries.push(HookOutputEntry {
                                        kind: HookOutputEntryKind::Stop,
                                        text: stop_reason,
                                    });
                                }
                            }
                        }
                        Err(_) => {
                            status = HookRunStatus::Failed;
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Error,
                                text: "hook returned invalid session end JSON output".to_string(),
                            });
                        }
                    }
                }
            }
            Some(exit_code) => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: format!("hook exited with code {exit_code}"),
                });
            }
            None => {
                status = HookRunStatus::Failed;
                entries.push(HookOutputEntry {
                    kind: HookOutputEntryKind::Error,
                    text: "hook exited without a status code".to_string(),
                });
            }
        },
    }

    dispatcher::ParsedHandler {
        completed: HookCompletedEvent {
            turn_id,
            run: dispatcher::completed_summary(handler, &run_result, status, entries),
        },
        data: (),
    }
}

fn serialization_failure_outcome(hook_events: Vec<HookCompletedEvent>) -> SessionEndOutcome {
    SessionEndOutcome { hook_events }
}
