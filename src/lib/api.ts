import { invoke } from "@tauri-apps/api/core";
import { redactSensitive } from "./format";
import type {
  ActivityRecord,
  OperationResult,
  Snapshot,
  StopApprovalChallenge,
  StopApprovalInput,
  TextResult,
} from "./types";

type TauriWindow = Window & { __TAURI_INTERNALS__?: unknown };

const now = Date.now();

const mockActivity: ActivityRecord[] = [
  {
    id: "mock-activity-1",
    action: "stats_once",
    command: "/usr/local/bin/container stats --format json --no-stream",
    startedAtMs: now - 25_000,
    durationMs: 68,
    success: true,
    exitCode: 0,
    stderr: "",
  },
  {
    id: "mock-activity-2",
    action: "containers_list",
    command: "/usr/local/bin/container list --all --format json",
    startedAtMs: now - 38_000,
    durationMs: 44,
    success: true,
    exitCode: 0,
    stderr: "",
  },
];

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as TauriWindow);
}

export async function getSnapshot(): Promise<Snapshot> {
  if (!isTauriRuntime()) return delayed(mockSnapshot);
  return invoke<Snapshot>("get_snapshot");
}

export async function inspectContainer(containerId: string): Promise<unknown> {
  if (!isTauriRuntime()) return delayed(mockSnapshot.containers.data);
  return invoke("inspect_container", { containerId });
}

export async function getContainerLogs(
  containerId: string,
  boot: boolean,
  lines: number,
): Promise<TextResult> {
  if (!isTauriRuntime()) {
    return delayed({
      text: [
        `mock log for ${containerId}`,
        "server listening on 0.0.0.0:5188",
        "health check completed",
      ].join("\n"),
      command: `/usr/local/bin/container logs -n ${lines} ${containerId}`,
      durationMs: 12,
    });
  }
  return invoke<TextResult>("get_container_logs", { containerId, boot, lines });
}

export async function startContainer(containerId: string): Promise<OperationResult> {
  if (!isTauriRuntime()) {
    mockActivity.unshift(mockOperation("container_start", containerId, true));
    return delayed({ stdout: "", stderr: "", command: "mock start", durationMs: 10 });
  }
  return invoke<OperationResult>("start_container", { containerId });
}

export async function requestStopApproval(containerId: string): Promise<StopApprovalChallenge> {
  if (!isTauriRuntime()) {
    return delayed({
      approvalId: `mock-${Date.now()}`,
      containerId,
      requiredPhrase: `STOP ${containerId}`,
      expiresAtMs: Date.now() + 120_000,
    });
  }
  return invoke<StopApprovalChallenge>("request_stop_approval", { containerId });
}

export async function stopContainer(
  containerId: string,
  approval: StopApprovalInput,
): Promise<OperationResult> {
  if (!isTauriRuntime()) {
    mockActivity.unshift(mockOperation("container_stop", containerId, true, approval));
    return delayed({ stdout: "", stderr: "", command: "mock stop", durationMs: 10 });
  }
  return invoke<OperationResult>("stop_container", { containerId, approval });
}

export async function getActivity(): Promise<ActivityRecord[]> {
  if (!isTauriRuntime()) return delayed(mockActivity);
  return invoke<ActivityRecord[]>("get_activity");
}

function delayed<T>(value: T): Promise<T> {
  return new Promise((resolve) => {
    window.setTimeout(() => resolve(structuredClone(value)), 120);
  });
}

function mockOperation(
  action: string,
  containerId: string,
  success: boolean,
  approval?: StopApprovalInput,
): ActivityRecord {
  return {
    id: `${Date.now()}-${action}`,
    action,
    command: `/usr/local/bin/container ${action.endsWith("start") ? "start" : "stop"} ${containerId}`,
    startedAtMs: Date.now(),
    durationMs: 10,
    success,
    exitCode: success ? 0 : 1,
    stderr: "",
    requestedBy: approval ? "local-user" : undefined,
    approvalId: approval?.approvalId,
    approvalReason: approval ? redactSensitive(approval.reason) : undefined,
  };
}

const mockSnapshot: Snapshot = {
  generatedAtMs: now,
  cliVersion: {
    ok: true,
    data: { text: "container CLI version 1.0.0 (build: release, commit: ee848e3)" },
    error: null,
  },
  systemStatus: {
    ok: true,
    data: {
      status: "running",
      apiServerAppName: "container-apiserver",
      apiServerVersion: "container-apiserver version 1.0.0",
      appRoot: "/Users/example/Library/Application Support/com.apple.container/",
      installRoot: "/usr/local/",
    },
    error: null,
  },
  containers: {
    ok: true,
    data: [
      {
        id: "buildkit",
        configuration: {
          id: "buildkit",
          image: { reference: "ghcr.io/apple/container-builder-shim/builder:0.12.0" },
          creationDate: new Date(now - 8_400_000).toISOString(),
          labels: {
            "com.apple.container.plugin": "builder",
            "com.apple.container.resource.role": "builder",
          },
          mounts: [{ destination: "/run", type: { tmpfs: {} } }],
          publishedPorts: [],
          resources: { cpus: 2, memoryInBytes: 2_147_483_648 },
          platform: { os: "linux", architecture: "arm64" },
        },
        status: {
          state: "running",
          startedDate: new Date(now - 8_200_000).toISOString(),
          networks: [{ network: "default", ipv4Address: "192.168.64.2/24" }],
        },
      },
      {
        id: "agentab-dev-up",
        configuration: {
          id: "agentab-dev-up",
          image: { reference: "agentab-dev:latest" },
          creationDate: new Date(now - 3_600_000).toISOString(),
          labels: {},
          mounts: [
            { source: "/Users/example/agentab", destination: "/workspace", type: { virtiofs: {} } },
            { source: "agentab-dev-data", destination: "/data/agentab", type: { volume: {} } },
          ],
          publishedPorts: [
            { hostAddress: "127.0.0.1", hostPort: 5189, containerPort: 5188, proto: "tcp" },
          ],
          resources: { cpus: 4, memoryInBytes: 1_073_741_824 },
          platform: { os: "linux", architecture: "arm64" },
        },
        status: {
          state: "running",
          startedDate: new Date(now - 3_500_000).toISOString(),
          networks: [{ network: "default", ipv4Address: "192.168.64.29/24" }],
        },
      },
    ],
    error: null,
  },
  images: {
    ok: true,
    data: [
      {
        id: "sha256:556bfd9d5df0cd841db6ec3cf84c9274212d6cb8d39f13cefe1c22c3a9487929",
        configuration: {
          name: "agentab-dev:latest",
          creationDate: new Date(now - 4_500_000).toISOString(),
          descriptor: { size: 375 },
        },
        variants: [{ platform: { os: "linux", architecture: "arm64" }, size: 399_833_366 }],
      },
    ],
    error: null,
  },
  volumes: { ok: true, data: [{ name: "agentab-dev-data" }], error: null },
  networks: { ok: true, data: [{ name: "default" }], error: null },
  stats: {
    ok: true,
    data: [
      {
        id: "buildkit",
        memoryUsageBytes: 1_590_747_136,
        memoryLimitBytes: 2_147_483_648,
        networkRxBytes: 77_878,
        networkTxBytes: 602,
        blockReadBytes: 381_988_864,
        blockWriteBytes: 3_613_618_176,
        numProcesses: 22,
      },
      {
        id: "agentab-dev-up",
        memoryUsageBytes: 426_602_496,
        memoryLimitBytes: 1_073_741_824,
        networkRxBytes: 690_428,
        networkTxBytes: 3_229_042,
        blockReadBytes: 176_514_048,
        blockWriteBytes: 1_384_448,
        numProcesses: 89,
      },
    ],
    error: null,
  },
};
