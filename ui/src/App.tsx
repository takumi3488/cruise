import { useCallback, useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import type { ChoiceDto, Session, WorkflowEvent } from "./types";
import {
  cancelSession,
  cleanSessions,
  listSessions,
  respondToOption,
  runSession,
} from "./lib/commands";

// ─── Phase badge ──────────────────────────────────────────────────────────────

const PHASE_COLORS: Record<string, string> = {
  AwaitingApproval: "bg-yellow-100 text-yellow-800",
  Planned: "bg-blue-100 text-blue-800",
  Running: "bg-green-100 text-green-800",
  Completed: "bg-gray-100 text-gray-700",
  Failed: "bg-red-100 text-red-800",
  Suspended: "bg-orange-100 text-orange-800",
};

function PhaseBadge({ phase }: { phase: string }) {
  const cls = PHASE_COLORS[phase] ?? "bg-gray-100 text-gray-600";
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {phase}
    </span>
  );
}

// ─── SessionList ──────────────────────────────────────────────────────────────

interface SessionListProps {
  onSelect: (session: Session) => void;
}

function SessionList({ onSelect }: SessionListProps) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [cleaning, setCleaning] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setSessions(await listSessions());
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

  if (loading) return <div className="p-4 text-gray-500">Loading…</div>;
  if (error) return <div className="p-4 text-red-600">Error: {error}</div>;

  return (
    <div className="p-4 space-y-3">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Sessions</h1>
        <div className="flex gap-2">
          <button
            onClick={() => void load()}
            className="px-3 py-1 text-sm border rounded hover:bg-gray-50"
          >
            Refresh
          </button>
          <button
            onClick={() => void handleClean()}
            disabled={cleaning}
            className="px-3 py-1 text-sm border rounded hover:bg-gray-50 disabled:opacity-50"
          >
            {cleaning ? "Cleaning…" : "Clean"}
          </button>
        </div>
      </div>

      {sessions.length === 0 && (
        <p className="text-gray-400 text-sm">No sessions found.</p>
      )}

      <table className="w-full text-sm border-collapse">
        <thead>
          <tr className="border-b bg-gray-50">
            <th className="text-left py-2 px-2 font-medium">ID</th>
            <th className="text-left py-2 px-2 font-medium">Phase</th>
            <th className="text-left py-2 px-2 font-medium">Input</th>
            <th className="text-left py-2 px-2 font-medium">Created</th>
          </tr>
        </thead>
        <tbody>
          {sessions.map((s) => (
            <tr
              key={s.id}
              onClick={() => onSelect(s)}
              className="border-b hover:bg-blue-50 cursor-pointer"
            >
              <td className="py-2 px-2 font-mono text-xs">{s.id}</td>
              <td className="py-2 px-2">
                <PhaseBadge phase={s.phase} />
              </td>
              <td className="py-2 px-2 max-w-xs truncate text-gray-700">
                {s.input}
              </td>
              <td className="py-2 px-2 text-gray-500">{s.createdAt}</td>
            </tr>
          ))}
        </tbody>
      </table>
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
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg shadow-xl p-6 max-w-lg w-full space-y-4">
        <h2 className="text-lg font-semibold">Choose an option</h2>
        {plan && (
          <pre className="text-xs bg-gray-50 border rounded p-3 max-h-48 overflow-auto">
            {plan}
          </pre>
        )}
        <div className="space-y-2">
          {choices.map((choice) =>
            choice.kind === "selector" ? (
              <button
                key={choice.label}
                onClick={() => onRespond({ nextStep: choice.next ?? undefined })}
                className="w-full text-left px-4 py-2 border rounded hover:bg-blue-50 text-sm"
              >
                {choice.label}
              </button>
            ) : (
              <div key={choice.label} className="space-y-1">
                <label className="text-sm text-gray-600">{choice.label}</label>
                <div className="flex gap-2">
                  <input
                    type="text"
                    value={textValue}
                    onChange={(e) => setTextValue(e.target.value)}
                    className="flex-1 border rounded px-3 py-1.5 text-sm"
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
  onBack: () => void;
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

function WorkflowRunner({ session, onBack }: WorkflowRunnerProps) {
  const [status, setStatus] = useState<RunStatus>("idle");
  const [steps, setSteps] = useState<StepEntry[]>([]);
  const [log, setLog] = useState<string[]>([]);
  const [pendingOption, setPendingOption] = useState<PendingOption | null>(
    null
  );
  const _channelRef = useRef<Channel<WorkflowEvent> | null>(null);

  async function startRun() {
    setStatus("running");
    setSteps([]);
    setLog([]);

    const channel = new Channel<WorkflowEvent>();
    _channelRef.current = channel;

    channel.onmessage = (event) => {
      if (event.event === "stepStarted") {
        setSteps((prev) => [
          ...prev,
          {
            name: event.data.step,
            index: event.data.index,
            total: event.data.total,
          },
        ]);
        setLog((prev) => [
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
        setLog((prev) => [
          ...prev,
          `✓ Completed — run: ${event.data.run}, skipped: ${event.data.skipped}, failed: ${event.data.failed}`,
        ]);
      } else if (event.event === "workflowFailed") {
        setStatus("failed");
        setLog((prev) => [...prev, `✗ Failed: ${event.data.error}`]);
      } else if (event.event === "workflowCancelled") {
        setStatus("cancelled");
        setLog((prev) => [...prev, "⏸ Cancelled"]);
      }
    };

    try {
      await runSession(session.id, channel);
    } catch (e) {
      setStatus("failed");
      setLog((prev) => [...prev, `Error: ${e}`]);
    }
  }

  async function handleCancel() {
    try {
      await cancelSession();
    } catch (e) {
      setLog((prev) => [...prev, `Cancel error: ${e}`]);
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
      setLog((prev) => [...prev, `Option response error: ${e}`]);
    }
  }

  const currentStep = steps.at(-1);
  const isRunnable =
    session.phase === "Planned" ||
    session.phase === "Running" ||
    session.phase === "Failed" ||
    session.phase === "Suspended";

  return (
    <div className="p-4 space-y-4">
      {/* Header */}
      <div className="flex items-center gap-3">
        <button
          onClick={onBack}
          className="text-sm text-blue-600 hover:underline"
        >
          ← Back
        </button>
        <h1 className="text-xl font-semibold font-mono">{session.id}</h1>
        <PhaseBadge phase={session.phase} />
      </div>

      <div className="text-sm text-gray-600 italic">{session.input}</div>

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

      {/* Progress */}
      {status === "running" && currentStep && (
        <div className="text-sm text-gray-700">
          Step {currentStep.index + 1}/{currentStep.total}:{" "}
          <span className="font-medium">{currentStep.name}</span>
        </div>
      )}

      {/* Log */}
      {log.length > 0 && (
        <pre className="text-xs bg-gray-900 text-gray-100 rounded p-3 max-h-64 overflow-auto">
          {log.join("\n")}
        </pre>
      )}

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

// ─── App ──────────────────────────────────────────────────────────────────────

export default function App() {
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);

  return (
    <div className="min-h-screen bg-white text-gray-900 font-sans">
      {selectedSession ? (
        <WorkflowRunner
          session={selectedSession}
          onBack={() => setSelectedSession(null)}
        />
      ) : (
        <SessionList onSelect={setSelectedSession} />
      )}
    </div>
  );
}
