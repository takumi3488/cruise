import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  CleanupResult,
  Session,
  WorkflowEvent,
} from "../types";

/** List all sessions, sorted oldest-first. */
export function listSessions(): Promise<Session[]> {
  return invoke<Session[]>("list_sessions");
}

/** Get a single session by ID. */
export function getSession(sessionId: string): Promise<Session> {
  return invoke<Session>("get_session", { sessionId });
}

/** Return the plan markdown for a session. */
export function getSessionPlan(sessionId: string): Promise<string> {
  return invoke<string>("get_session_plan", { sessionId });
}

/**
 * Run a session's workflow, streaming events via the returned Channel.
 *
 * @example
 * const channel = new Channel<WorkflowEvent>();
 * channel.onmessage = (event) => { ... };
 * await runSession(sessionId, channel);
 */
export function runSession(
  sessionId: string,
  channel: Channel<WorkflowEvent>
): Promise<void> {
  return invoke<void>("run_session", { sessionId, channel });
}

/** Cancel the currently running workflow. */
export function cancelSession(): Promise<void> {
  return invoke<void>("cancel_session");
}

/**
 * Respond to a pending option-step request.
 *
 * @param result.nextStep  The next step to jump to (selector choice).
 * @param result.textInput Free-text input (text-input choice).
 */
export function respondToOption(result: {
  nextStep?: string;
  textInput?: string;
}): Promise<void> {
  return invoke<void>("respond_to_option", { result });
}

/** Remove Completed sessions whose PR is closed or merged. */
export function cleanSessions(): Promise<CleanupResult> {
  return invoke<CleanupResult>("clean_sessions");
}
