use std::path::PathBuf;

use codex_protocol::ThreadId;
use codex_protocol::protocol::HookCompletedEvent;
use codex_protocol::protocol::HookEventName;
use codex_protocol::protocol::HookOutputEntry;
use codex_protocol::protocol::HookOutputEntryKind;
use codex_protocol::protocol::HookRunStatus;
use codex_protocol::protocol::HookRunSummary;
use codex_protocol::protocol::ThreadUnsubscribeReason;
use codex_utils_absolute_path::AbsolutePathBuf;

use super::common;
use crate::engine::CommandShell;
use crate::engine::ConfiguredHandler;
use crate::engine::command_runner::CommandRunResult;
use crate::engine::dispatcher;
use crate::engine::output_parser;
use crate::schema::ThreadUnsubscribeCommandInput;

#[derive(Debug, Clone)]
pub struct ThreadUnsubscribeRequest {
    pub session_id: ThreadId,
    pub cwd: AbsolutePathBuf,
    pub transcript_path: Option<PathBuf>,
    pub model: String,
    pub permission_mode: String,
    pub thread_id: ThreadId,
    pub reason: ThreadUnsubscribeReason,
}

#[derive(Debug)]
pub struct ThreadUnsubscribeOutcome {
    pub hook_events: Vec<HookCompletedEvent>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ThreadUnsubscribeHandlerData;

pub(crate) fn preview(
    handlers: &[ConfiguredHandler],
    _request: &ThreadUnsubscribeRequest,
) -> Vec<HookRunSummary> {
    dispatcher::select_handlers(
        handlers,
        HookEventName::ThreadUnsubscribe,
        /*matcher_input*/ None,
    )
    .into_iter()
    .map(|handler| dispatcher::running_summary(&handler))
    .collect()
}

pub(crate) async fn run(
    handlers: &[ConfiguredHandler],
    shell: &CommandShell,
    request: ThreadUnsubscribeRequest,
) -> ThreadUnsubscribeOutcome {
    let matched = dispatcher::select_handlers(
        handlers,
        HookEventName::ThreadUnsubscribe,
        /*matcher_input*/ None,
    );
    if matched.is_empty() {
        return ThreadUnsubscribeOutcome {
            hook_events: Vec::new(),
        };
    }

    let input_json = match serde_json::to_string(&ThreadUnsubscribeCommandInput::new(
        request.session_id,
        request.transcript_path.clone(),
        request.cwd.display().to_string(),
        request.model.clone(),
        request.permission_mode.clone(),
        request.thread_id,
        thread_unsubscribe_reason_label(request.reason),
    )) {
        Ok(input_json) => input_json,
        Err(error) => {
            return ThreadUnsubscribeOutcome {
                hook_events: common::serialization_failure_hook_events(
                    matched,
                    /*turn_id*/ None,
                    format!("failed to serialize thread unsubscribe hook input: {error}"),
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

    ThreadUnsubscribeOutcome {
        hook_events: results.into_iter().map(|result| result.completed).collect(),
    }
}

fn parse_completed(
    handler: &ConfiguredHandler,
    run_result: CommandRunResult,
    turn_id: Option<String>,
) -> dispatcher::ParsedHandler<ThreadUnsubscribeHandlerData> {
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
                } else if let Some(parsed) =
                    output_parser::parse_thread_unsubscribe(&run_result.stdout)
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
                        text: "hook returned invalid thread unsubscribe hook JSON output"
                            .to_string(),
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
        data: ThreadUnsubscribeHandlerData,
        completion_order: 0,
    }
}

fn thread_unsubscribe_reason_label(reason: ThreadUnsubscribeReason) -> &'static str {
    match reason {
        ThreadUnsubscribeReason::UserRequested => "user_requested",
        ThreadUnsubscribeReason::ThreadSwitch => "thread_switch",
        ThreadUnsubscribeReason::Programmatic => "programmatic",
    }
}

#[cfg(test)]
mod tests {
    use codex_protocol::protocol::HookEventName;
    use codex_protocol::protocol::HookOutputEntry;
    use codex_protocol::protocol::HookOutputEntryKind;
    use codex_protocol::protocol::HookRunStatus;
    use codex_protocol::protocol::ThreadUnsubscribeReason;
    use codex_utils_absolute_path::test_support::PathBufExt;
    use codex_utils_absolute_path::test_support::test_path_buf;
    use pretty_assertions::assert_eq;

    use super::ThreadUnsubscribeHandlerData;
    use super::parse_completed;
    use super::thread_unsubscribe_reason_label;
    use crate::engine::ConfiguredHandler;
    use crate::engine::command_runner::CommandRunResult;

    #[test]
    fn parse_completed_records_system_message() {
        let parsed = parse_completed(
            &handler(),
            run_result(
                Some(0),
                r#"{"systemMessage":"thread unsubscribe observed"}"#,
                "",
            ),
            /*turn_id*/ None,
        );

        assert_eq!(parsed.data, ThreadUnsubscribeHandlerData);
        assert_eq!(parsed.completed.run.status, HookRunStatus::Completed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Warning,
                text: "thread unsubscribe observed".to_string(),
            }]
        );
    }

    #[test]
    fn parse_completed_rejects_invalid_json_like_stdout() {
        let parsed = parse_completed(
            &handler(),
            run_result(Some(0), r#"{"systemMessage": "#, ""),
            /*turn_id*/ None,
        );

        assert_eq!(parsed.data, ThreadUnsubscribeHandlerData);
        assert_eq!(parsed.completed.run.status, HookRunStatus::Failed);
        assert_eq!(
            parsed.completed.run.entries,
            vec![HookOutputEntry {
                kind: HookOutputEntryKind::Error,
                text: "hook returned invalid thread unsubscribe hook JSON output".to_string(),
            }]
        );
    }

    #[test]
    fn reason_labels_match_hook_schema_values() {
        assert_eq!(
            thread_unsubscribe_reason_label(ThreadUnsubscribeReason::UserRequested),
            "user_requested"
        );
        assert_eq!(
            thread_unsubscribe_reason_label(ThreadUnsubscribeReason::ThreadSwitch),
            "thread_switch"
        );
        assert_eq!(
            thread_unsubscribe_reason_label(ThreadUnsubscribeReason::Programmatic),
            "programmatic"
        );
    }

    fn handler() -> ConfiguredHandler {
        ConfiguredHandler {
            event_name: HookEventName::ThreadUnsubscribe,
            matcher: None,
            command: "echo hook".to_string(),
            timeout_sec: 600,
            status_message: None,
            source_path: test_path_buf("/tmp/hooks.json").abs(),
            source: codex_protocol::protocol::HookSource::User,
            display_order: 0,
            env: std::collections::HashMap::new(),
        }
    }

    fn run_result(exit_code: Option<i32>, stdout: &str, stderr: &str) -> CommandRunResult {
        CommandRunResult {
            started_at: 1,
            completed_at: 2,
            duration_ms: 1,
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
            error: None,
        }
    }
}
