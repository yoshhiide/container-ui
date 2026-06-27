import {
  Activity,
  Boxes,
  ChevronRight,
  CircleAlert,
  CircleCheck,
  Clock3,
  Cpu,
  Database,
  HardDrive,
  Image,
  Loader2,
  Network,
  Pause,
  Play,
  RefreshCw,
  Server,
  SquareTerminal,
} from "lucide-react";
import type { ReactNode } from "react";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  getActivity,
  getContainerLogs,
  getSnapshot,
  inspectContainer,
  isTauriRuntime,
  requestStopApproval,
  startContainer,
  stopContainer,
} from "./lib/api";
import {
  containerId,
  containerImage,
  containerState,
  formatBytes,
  formatDateTime,
  imageName,
  imagePlatforms,
  isProtectedContainer,
  isRunning,
  memoryPercent,
  panelData,
  publishedPorts,
  redactSensitive,
  statusTone,
  summarizeError,
} from "./lib/format";
import type {
  ActivityRecord,
  CommandFailure,
  ContainerRecord,
  ImageRecord,
  Snapshot,
  StatsRecord,
  StopApprovalChallenge,
  StopApprovalInput,
} from "./lib/types";
import "./App.css";

type View = "dashboard" | "containers" | "images" | "activity";

const navItems: Array<{ view: View; label: string; icon: typeof Boxes }> = [
  { view: "dashboard", label: "Dashboard", icon: Activity },
  { view: "containers", label: "Containers", icon: Boxes },
  { view: "images", label: "Images", icon: Image },
  { view: "activity", label: "Activity", icon: SquareTerminal },
];

function App() {
  const [view, setView] = useState<View>("containers");
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [activity, setActivity] = useState<ActivityRecord[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [inspectJson, setInspectJson] = useState<unknown>(null);
  const [logs, setLogs] = useState("");
  const [bootLogs, setBootLogs] = useState(false);
  const [logLines, setLogLines] = useState(200);
  const [loading, setLoading] = useState(true);
  const [detailLoading, setDetailLoading] = useState(false);
  const [operatingId, setOperatingId] = useState<string | null>(null);
  const [pendingStop, setPendingStop] = useState<ContainerRecord | null>(null);
  const [stopChallenge, setStopChallenge] = useState<StopApprovalChallenge | null>(null);
  const [stopAcknowledgement, setStopAcknowledgement] = useState("");
  const [stopReason, setStopReason] = useState("");
  const [error, setError] = useState<string | null>(null);

  const loadActivity = useCallback(async () => {
    const records = await getActivity();
    setActivity(records);
  }, []);

  const loadSnapshot = useCallback(async () => {
    setError(null);
    try {
      const next = await getSnapshot();
      setSnapshot(next);
      const containers = panelData(next.containers, []);
      setSelectedId((current) => {
        if (current && containers.some((container) => containerId(container) === current)) {
          return current;
        }
        return containers[0] ? containerId(containers[0]) : null;
      });
      await loadActivity();
    } catch (err) {
      setError(redactSensitive(err instanceof Error ? err.message : String(err)));
    } finally {
      setLoading(false);
    }
  }, [loadActivity]);

  const loadDetail = useCallback(
    async (id: string, useBootLogs = bootLogs, lines = logLines) => {
      setDetailLoading(true);
      try {
        const [inspectResult, logResult] = await Promise.all([
          inspectContainer(id),
          getContainerLogs(id, useBootLogs, lines),
        ]);
        setInspectJson(inspectResult);
        setLogs(redactSensitive(logResult.text));
      } catch (err) {
        const failure = err as Partial<CommandFailure>;
        setError(redactSensitive(failure.stderr || failure.message || String(err)));
      } finally {
        setDetailLoading(false);
      }
    },
    [bootLogs, logLines],
  );

  useEffect(() => {
    void loadSnapshot();
    const interval = window.setInterval(() => {
      void loadSnapshot();
    }, 7_500);
    return () => window.clearInterval(interval);
  }, [loadSnapshot]);

  useEffect(() => {
    if (selectedId) {
      void loadDetail(selectedId);
    }
  }, [loadDetail, selectedId]);

  const containers = useMemo(() => panelData(snapshot?.containers, []), [snapshot]);
  const images = useMemo(() => panelData(snapshot?.images, []), [snapshot]);
  const volumes = useMemo(() => panelData(snapshot?.volumes, []), [snapshot]);
  const networks = useMemo(() => panelData(snapshot?.networks, []), [snapshot]);
  const stats = useMemo(() => panelData(snapshot?.stats, []), [snapshot]);
  const statsById = useMemo(
    () => new Map(stats.map((record) => [record.id ?? "", record])),
    [stats],
  );
  const selectedContainer = useMemo(
    () => containers.find((container) => containerId(container) === selectedId) ?? containers[0] ?? null,
    [containers, selectedId],
  );

  useEffect(() => {
    if (selectedContainer && selectedId !== containerId(selectedContainer)) {
      setSelectedId(containerId(selectedContainer));
    }
  }, [selectedContainer, selectedId]);

  const runningCount = containers.filter(isRunning).length;
  const systemStatus = snapshot?.systemStatus.data?.status ?? "unknown";
  const pendingStopId = pendingStop ? containerId(pendingStop) : "";
  const requiredStopPhrase = stopChallenge?.requiredPhrase ?? (pendingStopId ? `STOP ${pendingStopId}` : "");
  const stopReady = Boolean(
    stopChallenge &&
      stopAcknowledgement.trim() === stopChallenge.requiredPhrase &&
      stopReason.trim().length >= 4,
  );

  function resetStopDialog() {
    setPendingStop(null);
    setStopChallenge(null);
    setStopAcknowledgement("");
    setStopReason("");
  }

  async function runContainerAction(
    container: ContainerRecord,
    action: "start" | "stop",
    approval?: StopApprovalInput,
  ) {
    if (action === "stop" && !approval) {
      setError("Stop requires a fresh approval token.");
      return;
    }
    const id = containerId(container);
    setOperatingId(id);
    setError(null);
    try {
      if (action === "start") {
        await startContainer(id);
      } else if (approval) {
        await stopContainer(id, approval);
      }
      await loadSnapshot();
      await loadActivity();
    } catch (err) {
      const failure = err as Partial<CommandFailure>;
      setError(redactSensitive(failure.stderr || failure.message || String(err)));
    } finally {
      setOperatingId(null);
      if (action === "stop") {
        resetStopDialog();
      }
    }
  }

  async function requestContainerAction(container: ContainerRecord, action: "start" | "stop") {
    if (action === "stop") {
      const id = containerId(container);
      setOperatingId(id);
      setError(null);
      try {
        const challenge = await requestStopApproval(id);
        setPendingStop(container);
        setStopChallenge(challenge);
        setStopAcknowledgement("");
        setStopReason("");
      } catch (err) {
        const failure = err as Partial<CommandFailure>;
        setError(redactSensitive(failure.stderr || failure.message || String(err)));
      } finally {
        setOperatingId(null);
      }
      return;
    }
    void runContainerAction(container, action);
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <Boxes size={22} aria-hidden="true" />
          </div>
          <div>
            <strong>Container UI</strong>
            <span>{isTauriRuntime() ? "Desktop" : "Browser preview"}</span>
          </div>
        </div>

        <nav className="nav-list" aria-label="Primary">
          {navItems.map((item) => {
            const Icon = item.icon;
            return (
              <button
                key={item.view}
                className={view === item.view ? "nav-item active" : "nav-item"}
                type="button"
                onClick={() => setView(item.view)}
              >
                <Icon size={18} aria-hidden="true" />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>
      </aside>

      <main className="workspace">
        <header className="topbar">
          <div>
            <p className="eyebrow">apple/container console</p>
            <h1>{navItems.find((item) => item.view === view)?.label}</h1>
          </div>
          <div className="topbar-actions">
            <StatusPill label={systemStatus} tone={systemStatus === "running" ? "good" : "bad"} />
            <button className="icon-button" type="button" onClick={() => void loadSnapshot()} title="Refresh">
              {loading ? <Loader2 className="spin" size={18} /> : <RefreshCw size={18} />}
            </button>
          </div>
        </header>

        {error && (
          <div className="banner bad">
            <CircleAlert size={18} aria-hidden="true" />
            <span>{error}</span>
          </div>
        )}

        {snapshot && (
          <PanelErrors
            errors={[
              snapshot.cliVersion.error,
              snapshot.systemStatus.error,
              snapshot.containers.error,
              snapshot.images.error,
              snapshot.stats.error,
            ]}
          />
        )}

        {loading && !snapshot ? (
          <div className="empty-state">
            <Loader2 className="spin" size={28} />
            <span>Loading container state</span>
          </div>
        ) : (
          <>
            {view === "dashboard" && (
              <Dashboard
                snapshot={snapshot}
                containers={containers}
                images={images}
                volumes={volumes}
                networks={networks}
                runningCount={runningCount}
              />
            )}
            {view === "containers" && (
              <ContainersView
                containers={containers}
                statsById={statsById}
                selectedId={selectedContainer ? containerId(selectedContainer) : null}
                onSelect={setSelectedId}
                onAction={requestContainerAction}
                operatingId={operatingId}
                detail={
                  selectedContainer ? (
                    <ContainerDetail
                      container={selectedContainer}
                      stats={statsById.get(containerId(selectedContainer))}
                      inspectJson={inspectJson}
                      logs={logs}
                      bootLogs={bootLogs}
                      logLines={logLines}
                      loading={detailLoading}
                      onBootLogsChange={(next) => {
                        setBootLogs(next);
                        void loadDetail(containerId(selectedContainer), next, logLines);
                      }}
                      onLogLinesChange={(next) => {
                        setLogLines(next);
                        void loadDetail(containerId(selectedContainer), bootLogs, next);
                      }}
                      onReload={() => void loadDetail(containerId(selectedContainer))}
                    />
                  ) : (
                    <div className="empty-state">No containers found</div>
                  )
                }
              />
            )}
            {view === "images" && <ImagesView images={images} volumes={volumes} networks={networks} />}
            {view === "activity" && <ActivityView activity={activity} />}
          </>
        )}
      </main>

      {pendingStop && (
        <div className="modal-backdrop" role="presentation">
          <div className="confirm-dialog" role="dialog" aria-modal="true" aria-labelledby="stop-title">
            <CircleAlert size={22} aria-hidden="true" />
            <div>
              <h2 id="stop-title">Stop container</h2>
              <p>
                {isProtectedContainer(pendingStop)
                  ? `${containerId(pendingStop)} is managed by apple/container.`
                  : `${containerId(pendingStop)} will receive a stop request.`}
              </p>
              <div className="approval-fields">
                <label>
                  Required phrase
                  <code>{requiredStopPhrase}</code>
                </label>
                <label>
                  Confirmation
                  <input
                    autoFocus
                    value={stopAcknowledgement}
                    onChange={(event) => setStopAcknowledgement(event.currentTarget.value)}
                    placeholder={requiredStopPhrase}
                  />
                </label>
                <label>
                  Reason
                  <input
                    maxLength={180}
                    value={stopReason}
                    onChange={(event) => setStopReason(event.currentTarget.value)}
                    placeholder="Why this container should be stopped"
                  />
                </label>
                {stopChallenge && <small>Approval expires at {formatDateTime(stopChallenge.expiresAtMs)}.</small>}
              </div>
              <div className="dialog-actions">
                <button type="button" className="secondary-button" onClick={resetStopDialog}>
                  Cancel
                </button>
                <button
                  type="button"
                  className="danger-button"
                  disabled={!stopReady || operatingId === pendingStopId}
                  onClick={() => {
                    if (!stopChallenge) return;
                    void runContainerAction(pendingStop, "stop", {
                      approvalId: stopChallenge.approvalId,
                      acknowledgement: stopAcknowledgement,
                      reason: stopReason,
                    });
                  }}
                >
                  {operatingId === pendingStopId ? <Loader2 className="spin" size={14} /> : <Pause size={14} />}
                  Stop
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function Dashboard({
  snapshot,
  containers,
  images,
  volumes,
  networks,
  runningCount,
}: {
  snapshot: Snapshot | null;
  containers: ContainerRecord[];
  images: ImageRecord[];
  volumes: unknown[];
  networks: unknown[];
  runningCount: number;
}) {
  const status = snapshot?.systemStatus.data;
  return (
    <div className="dashboard-grid">
      <Metric icon={Server} label="System" value={status?.status ?? "unknown"} detail={status?.apiServerVersion ?? "-"} />
      <Metric icon={Boxes} label="Containers" value={`${runningCount}/${containers.length}`} detail="running / total" />
      <Metric icon={Image} label="Images" value={String(images.length)} detail="local OCI images" />
      <Metric icon={Database} label="Volumes" value={String(volumes.length)} detail={`${networks.length} networks`} />

      <section className="panel wide">
        <div className="panel-header">
          <div>
            <h2>Runtime</h2>
            <p>Current installation and data roots.</p>
          </div>
        </div>
        <dl className="definition-grid">
          <div>
            <dt>CLI</dt>
            <dd>{snapshot?.cliVersion.data?.text ?? "-"}</dd>
          </div>
          <div>
            <dt>API server</dt>
            <dd>{status?.apiServerAppName ?? "-"}</dd>
          </div>
          <div>
            <dt>Install root</dt>
            <dd>{status?.installRoot ?? "-"}</dd>
          </div>
          <div>
            <dt>App root</dt>
            <dd>{status?.appRoot ?? "-"}</dd>
          </div>
        </dl>
      </section>
    </div>
  );
}

function ContainersView({
  containers,
  statsById,
  selectedId,
  onSelect,
  onAction,
  operatingId,
  detail,
}: {
  containers: ContainerRecord[];
  statsById: Map<string, StatsRecord>;
  selectedId: string | null;
  onSelect: (id: string) => void;
  onAction: (container: ContainerRecord, action: "start" | "stop") => void;
  operatingId: string | null;
  detail: ReactNode;
}) {
  return (
    <div className="split-layout">
      <section className="panel table-panel">
        <div className="panel-header">
          <div>
            <h2>Containers</h2>
            <p>{containers.length} discovered from container list --all.</p>
          </div>
        </div>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>State</th>
                <th>Image</th>
                <th>Ports</th>
                <th>Memory</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {containers.map((container) => {
                const id = containerId(container);
                const state = containerState(container);
                const stats = statsById.get(id);
                const active = selectedId === id;
                return (
                  <tr key={id} className={active ? "selected" : ""} onClick={() => onSelect(id)}>
                    <td>
                      <div className="name-cell">
                        <span>{id}</span>
                        {isProtectedContainer(container) && <small>managed</small>}
                      </div>
                    </td>
                    <td>
                      <StatusPill label={state} tone={statusTone(state)} />
                    </td>
                    <td className="truncate">{containerImage(container)}</td>
                    <td className="truncate">{publishedPorts(container.configuration?.publishedPorts)}</td>
                    <td>
                      <MiniMeter value={memoryPercent(stats)} label={formatBytes(stats?.memoryUsageBytes)} />
                    </td>
                    <td>
                      <button
                        className="row-action"
                        type="button"
                        title={isRunning(container) ? "Stop" : "Start"}
                        disabled={operatingId === id}
                        onClick={(event) => {
                          event.stopPropagation();
                          onAction(container, isRunning(container) ? "stop" : "start");
                        }}
                      >
                        {operatingId === id ? (
                          <Loader2 className="spin" size={16} />
                        ) : isRunning(container) ? (
                          <>
                            <Pause size={14} />
                            <span>Stop</span>
                          </>
                        ) : (
                          <>
                            <Play size={14} />
                            <span>Start</span>
                          </>
                        )}
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>
      {detail}
    </div>
  );
}

function ContainerDetail({
  container,
  stats,
  inspectJson,
  logs,
  bootLogs,
  logLines,
  loading,
  onBootLogsChange,
  onLogLinesChange,
  onReload,
}: {
  container: ContainerRecord;
  stats: StatsRecord | undefined;
  inspectJson: unknown;
  logs: string;
  bootLogs: boolean;
  logLines: number;
  loading: boolean;
  onBootLogsChange: (next: boolean) => void;
  onLogLinesChange: (next: number) => void;
  onReload: () => void;
}) {
  const id = containerId(container);
  const inspectText = useMemo(
    () => redactSensitive(JSON.stringify(inspectJson ?? {}, null, 2)),
    [inspectJson],
  );
  return (
    <section className="panel detail-panel">
      <div className="panel-header">
        <div>
          <h2>{id}</h2>
          <p>{containerImage(container)}</p>
        </div>
        <button className="icon-button" type="button" onClick={onReload} title="Reload detail">
          {loading ? <Loader2 className="spin" size={18} /> : <RefreshCw size={18} />}
        </button>
      </div>

      <div className="detail-grid">
        <InfoTile icon={Clock3} label="Created" value={formatDateTime(container.configuration?.creationDate)} />
        <InfoTile icon={Cpu} label="CPU" value={String(container.configuration?.resources?.cpus ?? "-")} />
        <InfoTile icon={HardDrive} label="Memory limit" value={formatBytes(container.configuration?.resources?.memoryInBytes)} />
        <InfoTile icon={Network} label="Ports" value={publishedPorts(container.configuration?.publishedPorts)} />
      </div>

      <section className="subsection">
        <h3>Resource snapshot</h3>
        <div className="resource-lines">
          <ResourceLine label="Memory" value={memoryPercent(stats)} detail={`${formatBytes(stats?.memoryUsageBytes)} / ${formatBytes(stats?.memoryLimitBytes)}`} />
          <ResourceText label="Network" value={`${formatBytes(stats?.networkRxBytes)} rx / ${formatBytes(stats?.networkTxBytes)} tx`} />
          <ResourceText label="Block I/O" value={`${formatBytes(stats?.blockReadBytes)} read / ${formatBytes(stats?.blockWriteBytes)} write`} />
          <ResourceText label="Processes" value={String(stats?.numProcesses ?? "-")} />
        </div>
      </section>

      <section className="subsection">
        <div className="section-toolbar">
          <h3>Logs</h3>
          <label className="toggle">
            <input
              type="checkbox"
              checked={bootLogs}
              onChange={(event) => onBootLogsChange(event.currentTarget.checked)}
            />
            <span>Boot</span>
          </label>
          <select value={logLines} onChange={(event) => onLogLinesChange(Number(event.currentTarget.value))}>
            <option value={100}>100</option>
            <option value={200}>200</option>
            <option value={500}>500</option>
            <option value={1000}>1000</option>
          </select>
        </div>
        <pre className="log-view">{logs || "No log output"}</pre>
      </section>

      <section className="subsection">
        <h3>Inspect JSON</h3>
        <pre className="json-view">{inspectText}</pre>
      </section>
    </section>
  );
}

function ImagesView({
  images,
  volumes,
  networks,
}: {
  images: ImageRecord[];
  volumes: unknown[];
  networks: unknown[];
}) {
  return (
    <div className="resource-layout">
      <section className="panel table-panel">
        <div className="panel-header">
          <div>
            <h2>Images</h2>
            <p>{images.length} local images.</p>
          </div>
        </div>
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Name</th>
                <th>Platform</th>
                <th>Created</th>
                <th>Size</th>
              </tr>
            </thead>
            <tbody>
              {images.map((image) => (
                <tr key={image.id ?? imageName(image)}>
                  <td className="truncate">{imageName(image)}</td>
                  <td>{imagePlatforms(image)}</td>
                  <td>{formatDateTime(image.configuration?.creationDate)}</td>
                  <td>{formatBytes(image.variants?.[0]?.size ?? image.configuration?.descriptor?.size)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="panel">
        <div className="panel-header">
          <div>
            <h2>Storage and network</h2>
            <p>Read-only inventory for related resources.</p>
          </div>
        </div>
        <div className="compact-list">
          <InventoryGroup title="Volumes" items={volumes} />
          <InventoryGroup title="Networks" items={networks} />
        </div>
      </section>
    </div>
  );
}

function ActivityView({ activity }: { activity: ActivityRecord[] }) {
  return (
    <section className="panel table-panel">
      <div className="panel-header">
        <div>
          <h2>Command activity</h2>
          <p>Local audit trail written by the desktop app.</p>
        </div>
      </div>
      <div className="table-scroll">
        <table>
          <thead>
            <tr>
              <th>Time</th>
              <th>Action</th>
              <th>Result</th>
              <th>Duration</th>
              <th>Approval</th>
              <th>Command</th>
            </tr>
          </thead>
          <tbody>
            {activity.map((record) => (
              <tr key={record.id}>
                <td>{formatDateTime(record.startedAtMs)}</td>
                <td>{record.action}</td>
                <td>
                  <StatusPill label={record.success ? "ok" : "failed"} tone={record.success ? "good" : "bad"} />
                </td>
                <td>{record.durationMs} ms</td>
                <td className="truncate" title={record.approvalReason ?? ""}>
                  {record.approvalId ? record.requestedBy ?? "approved" : "-"}
                </td>
                <td className="truncate">{record.command}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function PanelErrors({ errors }: { errors: Array<CommandFailure | null | undefined> }) {
  const visible = errors.filter((item): item is CommandFailure => Boolean(item));
  if (visible.length === 0) return null;
  return (
    <div className="error-stack">
      {visible.map((failure) => (
        <div className="banner warn" key={`${failure.kind}-${failure.command}`}>
          <CircleAlert size={18} aria-hidden="true" />
          <span>{summarizeError(failure)}</span>
        </div>
      ))}
    </div>
  );
}

function Metric({
  icon: Icon,
  label,
  value,
  detail,
}: {
  icon: typeof Server;
  label: string;
  value: string;
  detail: string;
}) {
  return (
    <section className="metric">
      <Icon size={22} aria-hidden="true" />
      <div>
        <span>{label}</span>
        <strong>{value}</strong>
        <small>{detail}</small>
      </div>
    </section>
  );
}

function StatusPill({ label, tone }: { label: string; tone: "good" | "warn" | "bad" | "muted" }) {
  return (
    <span className={`status-pill ${tone}`}>
      {tone === "good" ? <CircleCheck size={13} /> : <span className="status-dot" />}
      {label}
    </span>
  );
}

function MiniMeter({ value, label }: { value: number; label: string }) {
  return (
    <div className="mini-meter" title={`${value}%`}>
      <span style={{ width: `${value}%` }} />
      <em>{label}</em>
    </div>
  );
}

function InfoTile({ icon: Icon, label, value }: { icon: typeof Clock3; label: string; value: string }) {
  return (
    <div className="info-tile">
      <Icon size={16} aria-hidden="true" />
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function ResourceLine({ label, value, detail }: { label: string; value: number; detail: string }) {
  return (
    <div className="resource-line">
      <div>
        <span>{label}</span>
        <strong>{detail}</strong>
      </div>
      <MiniMeter value={value} label={`${value}%`} />
    </div>
  );
}

function ResourceText({ label, value }: { label: string; value: string }) {
  return (
    <div className="resource-text">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function InventoryGroup({ title, items }: { title: string; items: unknown[] }) {
  return (
    <div>
      <h3>{title}</h3>
      {items.length === 0 ? (
        <p className="muted-text">No records</p>
      ) : (
        <ul>
          {items.map((item, index) => (
            <li key={index}>
              <ChevronRight size={14} />
              <span>{readInventoryName(item)}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function readInventoryName(item: unknown): string {
  if (item && typeof item === "object") {
    const record = item as Record<string, unknown>;
    return String(record.name ?? record.id ?? JSON.stringify(record));
  }
  return String(item);
}

export default App;
