use std::cell::Cell;
use std::sync::{Arc, Mutex};

use cruise::cancellation::CancellationToken;
use cruise::session::{SessionManager, SessionPhase, current_iso8601, get_cruise_home};
use cruise::step::option::OptionResult;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::events::WorkflowEvent;
use crate::gui_option_handler::GuiOptionHandler;
use crate::state::AppState;

// ─── DTOs ─────────────────────────────────────────────────────────────────────

/// Serializable representation of a session, sent to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub phase: String,
    /// Error message when `phase == "Failed"`.
    pub phase_error: Option<String>,
    pub config_source: String,
    pub input: String,
    pub current_step: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub worktree_branch: Option<String>,
    pub pr_url: Option<String>,
}

impl From<cruise::session::SessionState> for SessionDto {
    fn from(s: cruise::session::SessionState) -> Self {
        let (phase_label, phase_error) = match &s.phase {
            SessionPhase::Failed(e) => ("Failed".to_string(), Some(e.clone())),
            other => (other.label().to_string(), None),
        };
        Self {
            id: s.id,
            phase: phase_label,
            phase_error,
            config_source: s.config_source,
            input: s.input,
            current_step: s.current_step,
            created_at: s.created_at,
            completed_at: s.completed_at,
            worktree_branch: s.worktree_branch,
            pr_url: s.pr_url,
        }
    }
}

/// Result of a cleanup operation, returned to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResultDto {
    pub deleted: usize,
    pub skipped: usize,
}

/// Option result sent by the frontend when responding to an [`WorkflowEvent::OptionRequired`].
///
/// Mirrors [`OptionResult`] but derives [`Deserialize`] for IPC deserialization.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionResultDto {
    pub next_step: Option<String>,
    pub text_input: Option<String>,
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

fn new_session_manager() -> std::result::Result<SessionManager, String> {
    let cruise_home = get_cruise_home().map_err(|e| e.to_string())?;
    Ok(SessionManager::new(cruise_home))
}

// ─── Read commands ─────────────────────────────────────────────────────────────

/// List all sessions, sorted oldest-first.
#[tauri::command]
pub fn list_sessions() -> std::result::Result<Vec<SessionDto>, String> {
    let manager = new_session_manager()?;
    manager
        .list()
        .map(|sessions| sessions.into_iter().map(SessionDto::from).collect())
        .map_err(|e| e.to_string())
}

/// Get a single session by ID.
#[tauri::command]
pub fn get_session(session_id: String) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    manager
        .load(&session_id)
        .map(SessionDto::from)
        .map_err(|e| e.to_string())
}

/// Return the plan markdown for a session.
#[tauri::command]
pub fn get_session_plan(session_id: String) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    let session = manager.load(&session_id).map_err(|e| e.to_string())?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    std::fs::read_to_string(&plan_path)
        .map_err(|e| format!("failed to read plan {}: {}", plan_path.display(), e))
}

// ─── Write commands ────────────────────────────────────────────────────────────

/// Cancel the currently running workflow session.
#[tauri::command]
pub fn cancel_session(state: tauri::State<'_, AppState>) -> std::result::Result<(), String> {
    do_cancel_session(&state.cancel_token)
}

/// Deliver the frontend's option-step response to the engine.
#[tauri::command]
pub fn respond_to_option(
    result: OptionResultDto,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let option_result = OptionResult {
        next_step: result.next_step,
        text_input: result.text_input,
    };
    do_respond_to_option(&state.option_responder, option_result)
}

/// Remove Completed sessions whose PR is closed or merged.
#[tauri::command]
pub fn clean_sessions() -> std::result::Result<CleanupResultDto, String> {
    let manager = new_session_manager()?;
    manager
        .cleanup_by_pr_status()
        .map(|r| CleanupResultDto {
            deleted: r.deleted,
            skipped: r.skipped,
        })
        .map_err(|e| e.to_string())
}

// ─── run_session ───────────────────────────────────────────────────────────────

/// Execute a session's workflow, streaming [`WorkflowEvent`]s over `channel`.
///
/// The engine runs on a dedicated blocking thread (`spawn_blocking`) so that
/// [`GuiOptionHandler::select_option`]'s `blocking_recv()` does not starve the
/// async runtime. `execute_steps` is driven via `Handle::current().block_on()`
/// inside that thread.
///
/// # Phase-2 simplifications
/// - No worktree creation (uses `worktree_path` if already set, else `base_dir`)
/// - No conflict resolution on session saves
/// - No config hot-reloading
/// - No automatic PR creation
#[tauri::command]
#[expect(clippy::too_many_lines)]
pub async fn run_session(
    session_id: String,
    channel: tauri::ipc::Channel<WorkflowEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;

    if !session.phase.is_runnable() {
        return Err(format!(
            "Session {} is in '{}' phase and cannot be run",
            session_id,
            session.phase.label()
        ));
    }

    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let compiled = cruise::workflow::compile(config).map_err(|e| e.to_string())?;

    let start_step = session.current_step.clone().map_or_else(
        || {
            compiled
                .steps
                .keys()
                .next()
                .ok_or_else(|| "config has no steps".to_string())
                .map(Clone::clone)
        },
        Ok,
    )?;

    session.phase = SessionPhase::Running;
    manager.save(&session).map_err(|e| e.to_string())?;

    let cancel_token = CancellationToken::new();
    {
        let mut guard = state
            .cancel_token
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = Some(cancel_token.clone());
    }

    let option_responder = Arc::clone(&state.option_responder);
    let sessions_dir = manager.sessions_dir();
    let plan_path = session.plan_path(&sessions_dir);
    let input = session.input.clone();
    let exec_root = session
        .worktree_path
        .clone()
        .unwrap_or_else(|| session.base_dir.clone());
    let token_for_task = cancel_token.clone();
    let channel_for_step = channel.clone();
    let channel_for_handler = Arc::new(channel.clone());
    let sid_for_spawn = session_id.clone();

    let exec_result = tokio::task::spawn_blocking(
        move || -> cruise::error::Result<cruise::engine::ExecutionResult> {
            use cruise::engine::{ExecutionContext, execute_steps};
            use cruise::file_tracker::FileTracker;
            use cruise::variable::VariableStore;

            // Temporarily change the working directory for command steps.
            let original_dir = std::env::current_dir().ok();
            std::env::set_current_dir(&exec_root).map_err(|e| {
                cruise::error::CruiseError::Other(format!("failed to set working dir: {e}"))
            })?;

            let total_steps = compiled.steps.len();
            let step_counter = Cell::new(0usize);
            let on_step_start = |step: &str| -> cruise::error::Result<()> {
                let index = step_counter.get();
                step_counter.set(index + 1);
                let _ = channel_for_step.send(WorkflowEvent::StepStarted {
                    step: step.to_string(),
                    index,
                    total: total_steps,
                });
                Ok(())
            };

            let handler =
                GuiOptionHandler::new(channel_for_handler, sid_for_spawn, option_responder);

            let mut vars = VariableStore::new(input);
            vars.set_named_file("plan", plan_path);
            let mut tracker = FileTracker::with_root(exec_root);

            let ctx = ExecutionContext {
                compiled: &compiled,
                max_retries: 10,
                rate_limit_retries: 5,
                on_step_start: &on_step_start,
                cancel_token: Some(&token_for_task),
                option_handler: &handler,
                config_reloader: None,
            };

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(execute_steps(&ctx, &mut vars, &mut tracker, &start_step));

            if let Some(dir) = original_dir {
                let _ = std::env::set_current_dir(dir);
            }

            result
        },
    )
    .await
    .map_err(|e| format!("execution task panicked: {e}"))?;

    // Clear the cancel token slot.
    {
        let mut guard = state
            .cancel_token
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = None;
    }

    // Reload session to pick up any intermediate saves, then apply the final phase.
    let mut final_session = manager.load(&session_id).unwrap_or(session);

    match exec_result {
        Ok(exec) => {
            final_session.phase = SessionPhase::Completed;
            final_session.completed_at = Some(current_iso8601());
            let _ = channel.send(WorkflowEvent::WorkflowCompleted {
                run: exec.run,
                skipped: exec.skipped,
                failed: exec.failed,
            });
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(cruise::error::CruiseError::Interrupted) => {
            final_session.phase = SessionPhase::Suspended;
            let _ = channel.send(WorkflowEvent::WorkflowCancelled);
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            final_session.phase = SessionPhase::Failed(msg.clone());
            final_session.completed_at = Some(current_iso8601());
            let _ = channel.send(WorkflowEvent::WorkflowFailed { error: msg.clone() });
            manager.save(&final_session).map_err(|e2| e2.to_string())?;
            Err(msg)
        }
    }
}

/// Inner logic for the `cancel_session` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Calls `cancel()` on the active token if one is present; succeeds silently if not.
/// The token is removed from the slot after cancellation to free the underlying `Arc`.
///
/// # Errors
///
/// Returns an error string if the mutex is poisoned.
pub fn do_cancel_session(
    cancel_token: &Mutex<Option<CancellationToken>>,
) -> std::result::Result<(), String> {
    let mut guard = cancel_token
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    if let Some(token) = guard.take() {
        token.cancel();
    }
    Ok(())
}

/// Inner logic for the `respond_to_option` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Takes the pending `oneshot::Sender` from `option_responder` and delivers the user's choice.
///
/// # Errors
///
/// Returns an error string if the mutex is poisoned or no option request is currently pending.
pub fn do_respond_to_option(
    option_responder: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    result: OptionResult,
) -> std::result::Result<(), String> {
    let mut guard = option_responder
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    let sender = guard
        .take()
        .ok_or_else(|| "no pending option request".to_string())?;
    sender
        .send(result)
        .map_err(|_| "option receiver dropped: response not delivered".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::cancellation::CancellationToken;

    /// Polls `pending` until a sender is available.
    fn wait_for_pending(pending: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>) {
        loop {
            let guard = pending.lock().unwrap_or_else(|e| panic!("{e}"));
            if guard.is_some() {
                return;
            }
            drop(guard);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // ─── cancel_session ──────────────────────────────────────────────────────

    #[test]
    fn test_cancel_session_with_no_active_token_succeeds() {
        // Given: no active cancellation token in state
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(None);
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token);
        // Then: succeeds without error
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel_session_with_active_token_cancels_it() {
        // Given: an active token stored in state
        let token = CancellationToken::new();
        let token_for_assert = token.clone();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token);
        // Then: succeeds and the token reports cancelled
        assert!(result.is_ok());
        assert!(token_for_assert.is_cancelled());
    }

    #[test]
    fn test_cancel_session_clears_token_from_slot_after_cancelling() {
        // Given: an active token
        let token = CancellationToken::new();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let _ = do_cancel_session(&cancel_token);
        // Then: the token slot is cleared (frees the Arc)
        assert!(
            cancel_token
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    // ─── respond_to_option ───────────────────────────────────────────────────

    #[test]
    fn test_respond_to_option_with_no_pending_request_returns_error() {
        // Given: no pending option request
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(None));
        // When: respond_to_option is called
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error mentioning no pending request
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_lowercase()
                .contains("no pending option request"),
            "error message should mention 'no pending option request'"
        );
    }

    #[test]
    fn test_respond_to_option_sends_next_step_to_handler() {
        // Given: a pending option request (sender in state)
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with a next_step choice
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: Some("next_step".to_string()),
                text_input: None,
            },
        );
        // Then: succeeds and the handler receives the next_step
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.next_step, Some("next_step".to_string()));
        assert_eq!(received.text_input, None);
    }

    #[test]
    fn test_respond_to_option_sends_text_input_to_handler() {
        // Given: a pending option request
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with text input
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: Some("my text input".to_string()),
            },
        );
        // Then: the text is delivered to the handler
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.text_input, Some("my text input".to_string()));
    }

    #[test]
    fn test_respond_to_option_clears_sender_from_state_after_use() {
        // Given: a pending option request
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: the sender slot is cleared (idempotency guard)
        assert!(
            option_responder
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn test_respond_to_option_second_call_returns_error() {
        // Given: a request that was already handled
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // When: respond_to_option is called again
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error (no pending request remains)
        assert!(result.is_err());
    }

    // ─── Integration: full option-selection round-trip ────────────────────────
    //
    // Data flow:
    //   GuiOptionHandler::select_option (engine thread)
    //     → stores sender in shared pending_response slot
    //     → emits WorkflowEvent::OptionRequired
    //   do_respond_to_option (IPC command handler / test thread)
    //     → extracts sender from slot
    //     → sends OptionResult
    //   GuiOptionHandler::select_option (engine thread)
    //     → blocking_recv returns OptionResult
    //
    // Modules covered: events, gui_option_handler, state, commands
    //
    #[test]
    fn test_option_flow_integration_select_and_respond_round_trip() {
        use crate::events::WorkflowEvent;
        use crate::gui_option_handler::{EventEmitter, GuiOptionHandler};
        use cruise::option_handler::OptionHandler;
        use cruise::step::OptionChoice;

        /// Minimal emitter that records the last emitted event.
        struct CapturingEmitter {
            last: Mutex<Option<WorkflowEvent>>,
        }
        impl CapturingEmitter {
            fn new() -> Self {
                Self {
                    last: Mutex::new(None),
                }
            }
        }
        impl EventEmitter for CapturingEmitter {
            fn emit(&self, event: WorkflowEvent) {
                *self.last.lock().unwrap_or_else(|e| panic!("{e}")) = Some(event);
            }
        }

        // Given: a GuiOptionHandler wired to a shared pending_response slot
        let emitter = Arc::new(CapturingEmitter::new());
        let pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> = Arc::new(Mutex::new(None));
        let handler = GuiOptionHandler::new(
            Arc::clone(&emitter),
            "integration-req".to_string(),
            Arc::clone(&pending),
        );
        let choices = vec![OptionChoice::Selector {
            label: "Proceed".to_string(),
            next: Some("finalize".to_string()),
        }];

        // When: the engine thread calls select_option (blocks until response)
        let pending_for_cmd = Arc::clone(&pending);
        let engine_thread =
            std::thread::spawn(move || handler.select_option(&choices, Some("plan text")));

        // And: the IPC command thread responds once the sender is populated
        wait_for_pending(&pending_for_cmd);
        do_respond_to_option(
            &pending_for_cmd,
            OptionResult {
                next_step: Some("finalize".to_string()),
                text_input: None,
            },
        )
        .unwrap_or_else(|e| panic!("respond_to_option failed: {e}"));

        // Then: the engine thread receives the OptionResult
        let result = engine_thread
            .join()
            .unwrap_or_else(|e| panic!("engine thread panicked: {e:?}"))
            .unwrap_or_else(|e| panic!("select_option failed: {e}"));
        assert_eq!(result.next_step, Some("finalize".to_string()));
        assert_eq!(result.text_input, None);

        // And: the OptionRequired event was emitted with the correct data
        let emitted = emitter.last.lock().unwrap_or_else(|e| panic!("{e}")).take();
        match emitted {
            Some(WorkflowEvent::OptionRequired {
                request_id,
                plan,
                choices,
            }) => {
                assert_eq!(request_id, "integration-req");
                assert_eq!(plan.as_deref(), Some("plan text"));
                assert_eq!(choices.len(), 1);
                assert_eq!(choices[0].label, "Proceed");
            }
            other => panic!("expected OptionRequired event, got: {other:?}"),
        }
    }
}
