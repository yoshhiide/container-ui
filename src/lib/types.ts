export type CommandFailure = {
  kind: string;
  message: string;
  command: string;
  exitCode: number | null;
  stderr: string;
};

export type PanelResult<T = unknown> = {
  ok: boolean;
  data: T | null;
  error: CommandFailure | null;
};

export type Snapshot = {
  generatedAtMs: number;
  cliVersion: PanelResult<{ text: string }>;
  systemStatus: PanelResult<SystemStatus>;
  containers: PanelResult<ContainerRecord[]>;
  images: PanelResult<ImageRecord[]>;
  volumes: PanelResult<VolumeRecord[]>;
  networks: PanelResult<NetworkRecord[]>;
  stats: PanelResult<StatsRecord[]>;
};

export type ActivityRecord = {
  id: string;
  action: string;
  command: string;
  startedAtMs: number;
  durationMs: number;
  success: boolean;
  exitCode: number | null;
  stderr: string;
  requestedBy?: string;
  approvalId?: string;
  approvalReason?: string;
};

export type TextResult = {
  text: string;
  command: string;
  durationMs: number;
};

export type OperationResult = {
  stdout: string;
  stderr: string;
  command: string;
  durationMs: number;
};

export type StopApprovalChallenge = {
  approvalId: string;
  containerId: string;
  requiredPhrase: string;
  expiresAtMs: number;
};

export type StopApprovalInput = {
  approvalId: string;
  acknowledgement: string;
  reason: string;
};

export type SystemStatus = {
  status?: string;
  apiServerVersion?: string;
  apiServerAppName?: string;
  appRoot?: string;
  installRoot?: string;
  [key: string]: unknown;
};

export type ContainerRecord = {
  id?: string;
  configuration?: {
    id?: string;
    image?: {
      reference?: string;
      descriptor?: {
        digest?: string;
        size?: number;
      };
    };
    creationDate?: string;
    initProcess?: {
      arguments?: string[];
      environment?: string[];
      executable?: string;
      workingDirectory?: string;
    };
    labels?: Record<string, string>;
    mounts?: MountRecord[];
    networks?: Array<{
      network?: string;
      options?: Record<string, unknown>;
    }>;
    platform?: {
      architecture?: string;
      os?: string;
      variant?: string;
    };
    publishedPorts?: PublishedPort[];
    resources?: {
      cpus?: number;
      memoryInBytes?: number;
    };
    rosetta?: boolean;
    readOnly?: boolean;
  };
  status?: {
    state?: string;
    startedDate?: string;
    networks?: Array<{
      hostname?: string;
      network?: string;
      ipv4Address?: string;
      ipv6Address?: string;
      macAddress?: string;
      mtu?: number;
    }>;
  };
  [key: string]: unknown;
};

export type PublishedPort = {
  containerPort?: number;
  hostAddress?: string;
  hostPort?: number;
  proto?: string;
  count?: number;
};

export type MountRecord = {
  destination?: string;
  source?: string;
  options?: string[];
  type?: Record<string, unknown>;
};

export type ImageRecord = {
  id?: string;
  configuration?: {
    name?: string;
    creationDate?: string;
    descriptor?: {
      digest?: string;
      size?: number;
    };
  };
  variants?: Array<{
    digest?: string;
    platform?: {
      os?: string;
      architecture?: string;
      variant?: string;
    };
    size?: number;
  }>;
  [key: string]: unknown;
};

export type StatsRecord = {
  id?: string;
  cpuUsageUsec?: number;
  memoryUsageBytes?: number;
  memoryLimitBytes?: number;
  networkRxBytes?: number;
  networkTxBytes?: number;
  blockReadBytes?: number;
  blockWriteBytes?: number;
  numProcesses?: number;
};

export type VolumeRecord = {
  name?: string;
  [key: string]: unknown;
};

export type NetworkRecord = {
  name?: string;
  id?: string;
  [key: string]: unknown;
};
