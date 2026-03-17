import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import type { ChoiceDto, Session, SessionPhase, WorkflowEvent } from "./types";
import {
  cancelSession,
  cleanSessions,
  getSessionLog,
  listSessions,
  respondToOption,
  runSession,
} from "./lib/commands";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function formatLocalTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString(undefined, {
      dateStyle: "short",
      timeStyle: "short",
    });
  } catch {
    return iso;
  }
}

// ─── Phase badge ──────────────────────────────────────────────────────────────

const PHASE_COLORS: Partial<Record<SessionPhase, string>> = {
  "Awaiting Approval": "bg-yellow-900/50 text-yellow-300",
  Planned: "bg-blue-900/50 text-blue-300",
  Running: "bg-green-900/50 text-green-300",
  Completed: "bg-gray-700/50 text-gray-300",
  Failed: "bg-red-900/50 text-red-300",
  Suspended: "bg-orange-900/50 text-orange-300",
};

function PhaseBadge({ phase }: { phase: SessionPhase }) {
  const cls = PHASE_COLORS[phase] ?? "bg-gray-700/50 text-gray-300";
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {phase}
    </span>
  );
}

// ─── SessionSidebar ───────────────────────────────────────────────────────────

interface SessionSidebarProps {
  selectedId: string | null;
  onSelect: (session: Session) => void;
  onNewSession: () => void;
}

function SessionSidebar({ selectedId, onSelect, onNewSession }: SessionSidebarProps) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const sessions = await listSessions();
      const sorted = [...sessions].sort((a, b) => {
        const aInput = a.awaitingInput || a.phase === "Awaiting Approval";
        const bInput = b.awaitingInput || b.phase === "Awaiting Approval";
        if (aInput !== bInput) return aInput ? -1 : 1;
        const aTime = a.updatedAt ?? a.createdAt;
        const bTime = b.updatedAt ?? b.createdAt;
        return bTime.localeCompare(aTime);
      });
      setSessions(sorted);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function handleClean() {
    setCleaning(true);
    try {
      const result = await cleanSessions();
      alert(`Removed ${result.deleted} session(s). Skipped: ${result.skipped}`);
      void load();
    } catch (e) {
      alert(`Clean failed: ${e}`);
    } finally {
      setCleaning(false);
    }
  }

  return (
    <div className="h-full flex flex-col">
      {/* Sidebar header */}
      <div className="px-3 py-3 border-b border-gray-800 flex items-center justify-between gap-2">
        <h1 className="text-sm font-semibold text-gray-200">Sessions</h1>
        <div className="flex items-center gap-1">
          <button
            onClick={() => void load()}
            className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded"
            title="Refresh"
          >
            ↻
          </button>
          <button
            onClick={() => void handleClean()}
            disabled={cleaning}
            className="px-2 py-1 text-xs text-gray-400 hover:text-gray-200 hover:bg-gray-800 rounded disabled:opacity-50"
            title="Clean completed sessions"
          >
            {cleaning ? "…" : "Clean"}
          </button>
          <button
            onClick={onNewSession}
            className="px-2 py-1 text-xs bg-blue-600 text-white hover:bg-blue-700 rounded"
          >
            + New
          </button>
        </div>
      </div>

      {/* Session list */}
      <div className="flex-1 overflow-y-auto">
        {loading && (
          <p className="p-3 text-xs text-gray-500">Loading…</p>
        )}
        {error && (
          <p className="p-3 text-xs text-red-400">Error: {error}</p>
        )}
        {!loading && !error && sessions.length === 0 && (
          <p className="p-3 text-xs text-gray-500">No sessions found.</p>
        )}
        {sessions.map((s) => (
          <button
            key={s.id}
            onClick={() => onSelect(s)}
            className={`w-full text-left px-3 py-2.5 border-b border-gray-800/50 hover:bg-gray-800 transition-colors ${
              selectedId === s.id ? "bg-gray-800" : ""
            }`}
          >
            <div className="flex items-center justify-between gap-2 mb-0.5">
              <span className="text-xs text-gray-500 font-mono truncate">{s.id}</span>
              <PhaseBadge phase={s.phase} />
            </div>
            <p className="text-sm text-gray-300 truncate">{s.input}</p>
            <div className="flex items-center gap-1.5 mt-0.5">
              <span className="text-xs text-blue-400/70 font-mono truncate">
                {s.baseDir.replace(/\\/g, "/").split("/").filter(Boolean).at(-1) ?? s.baseDir}
              </span>
              <span className="text-xs text-gray-600">{formatLocalTime(s.updatedAt ?? s.createdAt)}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

// ─── OptionDialog ─────────────────────────────────────────────────────────────

interface OptionDialogProps {
  choices: ChoiceDto[];
  plan?: string;
  onRespond: (result: { nextStep?: string; textInput?: string }) => void;
}

function OptionDialog({ choices, plan, onRespond }: OptionDialogProps) {
  const [textValue, setTextValue] = useState("");

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div className="bg-gray-900 rounded-lg shadow-xl border border-gray-700 p-6 max-w-lg w-full space-y-4">
        <h2 className="text-lg font-semibold text-gray-100">Choose an option</h2>
        {plan && (
          <pre className="text-xs bg-gray-800 border border-gray-700 rounded p-3 max-h-48 overflow-auto text-gray-300">
            {plan}
          </pre>
        )}
        <div className="space-y-2">
          {choices.map((choice) =>
            choice.kind === "selector" ? (
              <button
                key={choice.label}
                onClick={() => onRespond({ nextStep: choice.next ?? undefined })}
                className="w-full text-left px-4 py-2 border border-gray-700 rounded hover:bg-gray-800 text-sm text-gray-200 transition-colors"
              >
                {choice.label}
              </button>
            ) : (
              <div key={choice.label} className="space-y-1">
                <label className="text-sm text-gray-400">{choice.label}</label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={textValue}
                    onChange={(e) => setTextValue(e.target.value)}
                    className="flex-1 border border-gray-700 bg-gray-800 rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-blue-500"
                    placeholder="Type here…"
                    onKeyDown={(e) => {
                      if (e.key === "Enter")
                        onRespond({
                          nextStep: choice.next ?? undefined,
                          textInput: textValue,
                        });
                    }}
                  />
                  <button
                    onClick={() =>
                      onRespond({
                        nextStep: choice.next ?? undefined,
                        textInput: textValue,
                      })
                    }
                    className="px-3 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
                  >
                    Submit
                  </button>
                </div>
              </div>
            )
          )}
        </div>
      </div>
    </div>
  );
}

// ─── WorkflowRunner ───────────────────────────────────────────────────────────

interface WorkflowRunnerProps {
  session: Session;
}

interface StepEntry {
  name: string;
  index: number;
  total: number;
}

interface PendingOption {
  requestId: string;
  choices: ChoiceDto[];
  plan?: string;
}

type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";

function WorkflowRunner({ session }: WorkflowRunnerProps) {
  const [status, setStatus] = useState<RunStatus>("idle");
  const [currentStep, setCurrentStep] = useState<StepEntry | null>(null);
  const [liveLog, setLiveLog] = useState<string[]>([]);
  const [savedLog, setSavedLog] = useState<string>("");
  const [logLoading, setLogLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<"info" | "log">("info");
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(null);
  const _channelRef = useRef<Channel<WorkflowEvent> | null>(null);
  const logEndRef = useRef<HTMLSpanElement | null>(null);

  // Load saved log from file when tab is opened or after run finishes
  const loadSavedLog = useCallback(async () => {
    setLogLoading(true);
    try {
      const content = await getSessionLog(session.id);
      setSavedLog(content);
    } catch (e) {
      setSavedLog(`(failed to load log: ${e})`);
    } finally {
      setLogLoading(false);
    }
  }, [session.id]);

  // Reset all state when the selected session changes
  useEffect(() => {
    setStatus("idle");
    setCurrentStep(null);
    setLiveLog([]);
    setSavedLog("");
    setPendingOption(null);
    setActiveTab("info");
    setLogLoading(false);
    _channelRef.current = null;
  }, [session.id]);

  // Scroll live log to bottom when new entries arrive
  useEffect(() => {
    if (status === "running") {
      logEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [liveLog, status]);

  async function startRun() {
    setStatus("running");
    setCurrentStep(null);
    setLiveLog([]);
    setActiveTab("log");

    const channel = new Channel<WorkflowEvent>();
    _channelRef.current = channel;

    channel.onmessage = (event) => {
      if (event.event === "stepStarted") {
        setCurrentStep({
          name: event.data.step,
          index: event.data.index,
          total: event.data.total,
        });
        setLiveLog((prev) => [
          ...prev,
          `[${event.data.index + 1}/${event.data.total}] ${event.data.step}`,
        ]);
      } else if (event.event === "optionRequired") {
        setPendingOption({
          requestId: event.data.requestId,
          choices: event.data.choices,
          plan: event.data.plan,
        });
      } else if (event.event === "workflowCompleted") {
        setStatus("completed");
        setLiveLog((prev) => [
          ...prev,
          `✓ Completed — run: ${event.data.run}, skipped: ${event.data.skipped}, failed: ${event.data.failed}`,
        ]);
      } else if (event.event === "workflowFailed") {
        setStatus("failed");
        setLiveLog((prev) => [...prev, `✗ Failed: ${event.data.error}`]);
      } else if (event.event === "workflowCancelled") {
        setStatus("cancelled");
        setLiveLog((prev) => [...prev, "⏸ Cancelled"]);
      }
    };

    try {
      await runSession(session.id, channel);
    } catch (e) {
      setStatus("failed");
      setLiveLog((prev) => [...prev, `Error: ${e}`]);
    }

    // Reload saved log after run finishes
    void loadSavedLog();
  }

  async function handleCancel() {
    try {
      await cancelSession();
    } catch (e) {
      setLiveLog((prev) => [...prev, `Cancel error: ${e}`]);
    }
  }

  async function handleOptionRespond(result: {
    nextStep?: string;
    textInput?: string;
  }) {
    setPendingOption(null);
    try {
      await respondToOption(result);
    } catch (e) {
      setLiveLog((prev) => [...prev, `Option response error: ${e}`]);
    }
  }

  // Load saved log when switching to log tab (and not running)
  function handleTabChange(tab: "info" | "log") {
    setActiveTab(tab);
    if (tab === "log" && status !== "running") {
      void loadSavedLog();
    }
  }

  const isRunnable =
    session.phase === "Planned" ||
    session.phase === "Running" ||
    session.phase === "Failed" ||
    session.phase === "Suspended";

  // Decide which log content to show
  const showLive = status === "running" || (status !== "idle" && liveLog.length > 0);
  const logContent = showLive ? liveLog.join("\n") : savedLog;

  return (
    <div className="h-full flex flex-col">
      {/* Header */}
      <div className="px-6 pt-6 pb-4 border-b border-gray-800 space-y-3">
        <div className="flex items-center gap-3">
          <h1 className="text-lg font-semibold font-mono text-gray-100">{session.id}</h1>
          <PhaseBadge phase={session.phase} />
        </div>

        {session.prUrl && (
          <a
            href={session.prUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1.5 text-sm text-blue-400 hover:text-blue-300 hover:underline"
          >
            PR: {session.prUrl.split("/").slice(-2).join(" #")}
            <span className="text-xs">↗</span>
          </a>
        )}

        <div className="text-sm text-gray-400 italic">{session.input}</div>

        {/* Controls */}
        <div className="flex gap-2">
          {isRunnable && status !== "running" && (
            <button
              onClick={() => void startRun()}
              className="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700"
            >
              {status === "idle" ? "Run" : "Re-run"}
            </button>
          )}
          {status === "running" && (
            <button
              onClick={() => void handleCancel()}
              className="px-4 py-2 bg-red-600 text-white rounded text-sm hover:bg-red-700"
            >
              Cancel
            </button>
          )}
        </div>

        {/* Progress indicator */}
        {status === "running" && currentStep && (
          <div className="text-sm text-gray-400">
            Step {currentStep.index + 1}/{currentStep.total}:{" "}
            <span className="font-medium text-gray-200">{currentStep.name}</span>
          </div>
        )}
      </div>

      {/* Tabs */}
      <div className="flex border-b border-gray-800">
        <button
          onClick={() => handleTabChange("info")}
          className={`px-4 py-2 text-xs font-medium transition-colors ${
            activeTab === "info"
              ? "text-blue-400 border-b-2 border-blue-500"
              : "text-gray-500 hover:text-gray-300"
          }`}
        >
          Info
        </button>
        <button
          onClick={() => handleTabChange("log")}
          className={`px-4 py-2 text-xs font-medium transition-colors ${
            activeTab === "log"
              ? "text-blue-400 border-b-2 border-blue-500"
              : "text-gray-500 hover:text-gray-300"
          }`}
        >
          Log
          {status === "running" && (
            <span className="ml-1.5 inline-block w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
          )}
        </button>
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-auto">
        {activeTab === "info" && (
          <div className="p-6 space-y-3 text-sm text-gray-400">
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Config</span>
              <p className="font-mono text-gray-300 mt-0.5">{session.configSource}</p>
            </div>
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Base dir</span>
              <p className="font-mono text-gray-300 mt-0.5">{session.baseDir}</p>
            </div>
            {session.worktreeBranch && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Branch</span>
                <p className="font-mono text-gray-300 mt-0.5">{session.worktreeBranch}</p>
              </div>
            )}
            <div>
              <span className="text-gray-600 text-xs uppercase tracking-wide">Created</span>
              <p className="text-gray-300 mt-0.5">{formatLocalTime(session.createdAt)}</p>
            </div>
            {session.completedAt && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Completed</span>
                <p className="text-gray-300 mt-0.5">{formatLocalTime(session.completedAt)}</p>
              </div>
            )}
            {session.phaseError && (
              <div>
                <span className="text-gray-600 text-xs uppercase tracking-wide">Error</span>
                <p className="text-red-400 mt-0.5 font-mono text-xs">{session.phaseError}</p>
              </div>
            )}
          </div>
        )}

        {activeTab === "log" && (
          <div className="h-full flex flex-col">
            {logLoading && status !== "running" ? (
              <p className="p-4 text-xs text-gray-500">Loading log…</p>
            ) : logContent ? (
              <pre
                className="flex-1 text-xs font-mono bg-gray-950 text-gray-300 p-4 overflow-auto whitespace-pre-wrap leading-relaxed"
              >
                {logContent}
                <span ref={logEndRef} />
              </pre>
            ) : (
              <p className="p-4 text-xs text-gray-600">
                {status === "idle" ? "Run the session to see logs here." : "No log entries yet."}
              </p>
            )}
          </div>
        )}
      </div>

      {/* Option dialog */}
      {pendingOption && (
        <OptionDialog
          choices={pendingOption.choices}
          plan={pendingOption.plan}
          onRespond={(result) => void handleOptionRespond(result)}
        />
      )}
    </div>
  );
}

// ─── EmptyState / NewSessionPlaceholder ───────────────────────────────────────

function EmptyState() {
  return (
    <div className="h-full flex items-center justify-center">
      <p className="text-gray-600 text-sm">サイドバーからセッションを選択してください</p>
    </div>
  );
}

function NewSessionPlaceholder() {
  return (
    <div className="h-full flex items-center justify-center">
      <div className="text-center space-y-2">
        <p className="text-gray-400 text-sm font-medium">新規セッションを作成</p>
        <p className="text-gray-600 text-xs">
          CLIから <code className="bg-gray-800 px-1 py-0.5 rounded text-gray-300">cruise plan</code> を実行してください
        </p>
      </div>
    </div>
  );
}

// ─── App ──────────────────────────────────────────────────────────────────────

export default function App() {
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [view, setView] = useState<"session" | "new">("session");

  return (
    <div className="h-screen flex bg-gray-950 text-gray-100 font-sans">
      {/* サイドバー */}
      <aside className="w-72 flex-shrink-0 border-r border-gray-800 flex flex-col">
        <SessionSidebar
          selectedId={selectedSession?.id ?? null}
          onSelect={(s) => { setSelectedSession(s); setView("session"); }}
          onNewSession={() => { setSelectedSession(null); setView("new"); }}
        />
      </aside>
      {/* メインコンテンツ */}
      <main className="flex-1 overflow-auto">
        {view === "new" ? (
          <NewSessionPlaceholder />
        ) : selectedSession ? (
          <WorkflowRunner session={selectedSession} />
        ) : (
          <EmptyState />
        )}
      </main>
    </div>
  );
}
