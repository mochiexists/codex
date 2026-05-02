use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::ClientDetachReason;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::common;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::ClientDetachCommandInput;

#[derive(Debug, Clone)]
pub struct ClientDetachRequest {
    pub session_id: ThreadId,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub reason: ClientDetachReason,
}

#[derive(Debug)]
pub struct ClientDetachOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ClientDetachHandlerData;

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    _request: &ClientDetachRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::ClientDetach,
        /*matcher_input*/ None,
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: ClientDetachRequest,
) -> ClientDetachOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::ClientDetach,
        /*matcher_input*/ None,
    );
    if matched.is_empty() {
        return ClientDetachOutcome {
            hook_events: Vec::new(),
        };
    }

    let input_json = match serde_json::to_string(&ClientDetachCommandInput::new(
        request.session_id,
        request.transcript_path.clone(),
        request.cwd.display().to_string(),
        request.model.clone(),
        request.permission_mode.clone(),
        client_detach_reason_label(request.reason),
    )) {
        Ok(input_json) => input_json,
        Err(error) => {
            return ClientDetachOutcome {
                hook_events: common::serialization_failure_hook_events(
                    matched,
                    /*turn_id*/ None,
                    format!("failed to serialize client detach hook input: {error}"),
                ),
            };
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

    ClientDetachOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<ClientDetachHandlerData> {
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
                if trimmed_stdout.is_empty() {
                } else if let Some(parsed) = output_parser::parse_client_detach(&run_result.stdout)
                {
                    if let Some(system_message) = parsed.universal.system_message {
                        entries.push(HookOutputEntry {
                            kind: HookOutputEntryKind::Warning,
                            text: system_message,
                        });
                    }
                    let _ = parsed.universal.suppress_output;
                    if !parsed.universal.continue_processing {
                        status = HookRunStatus::Stopped;
                        if let Some(stop_reason_text) = parsed.universal.stop_reason {
                            entries.push(HookOutputEntry {
                                kind: HookOutputEntryKind::Stop,
                                text: stop_reason_text,
                            });
                        }
                    }
                } else {
                    status = HookRunStatus::Failed;
                    entries.push(HookOutputEntry {
                        kind: HookOutputEntryKind::Error,
                        text: "hook returned invalid client detach hook JSON output".to_string(),
                    });
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

    let completed = HookCompletedEvent {
        turn_id,
        run: dispatcher::completed_summary(handler, &run_result, status, entries),
    };

    dispatcher::ParsedHandler {
        completed,
        data: ClientDetachHandlerData,
    }
}

fn client_detach_reason_label(reason: ClientDetachReason) -> &'static str {
    match reason {
        ClientDetachReason::UserRequested => "user_requested",
        ClientDetachReason::ThreadSwitch => "thread_switch",
        ClientDetachReason::Programmatic => "programmatic",
    }
}
