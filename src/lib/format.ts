import type {
  CommandFailure,
  ContainerRecord,
  ImageRecord,
  PanelResult,
  PublishedPort,
  StatsRecord,
} from "./types";

export function panelData<T>(panel: PanelResult<T> | null | undefined, fallback: T): T {
  return panel?.ok && panel.data != null ? panel.data : fallback;
}

export function containerId(container: ContainerRecord): string {
  return container.id ?? container.configuration?.id ?? "unknown";
}

export function containerImage(container: ContainerRecord): string {
  return container.configuration?.image?.reference ?? "unknown";
}

export function containerState(container: ContainerRecord): string {
  return container.status?.state ?? "unknown";
}

export function isRunning(container: ContainerRecord): boolean {
  return containerState(container).toLowerCase() === "running";
}

export function isProtectedContainer(container: ContainerRecord): boolean {
  const labels = container.configuration?.labels ?? {};
  return (
    labels["com.apple.container.plugin"] != null ||
    labels["com.apple.container.resource.role"] != null ||
    containerId(container) === "buildkit"
  );
}

export function imageName(image: ImageRecord): string {
  return image.configuration?.name ?? image.id ?? "unnamed";
}

export function imagePlatforms(image: ImageRecord): string {
  const variants = image.variants ?? [];
  const values = variants
    .map((variant) => {
      const platform = variant.platform;
      if (!platform?.os || !platform?.architecture) return null;
      return [platform.os, platform.architecture, platform.variant].filter(Boolean).join("/");
    })
    .filter((value): value is string => Boolean(value));
  return values.length > 0 ? Array.from(new Set(values)).join(", ") : "unknown";
}

export function publishedPorts(ports: PublishedPort[] | undefined): string {
  if (!ports || ports.length === 0) return "none";
  return ports
    .map((port) => {
      const host = [port.hostAddress ?? "127.0.0.1", port.hostPort ?? "?"].join(":");
      const target = [port.containerPort ?? "?", port.proto ?? "tcp"].join("/");
      return `${host} -> ${target}`;
    })
    .join(", ");
}

export function formatBytes(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return "-";
  if (value === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1);
  const scaled = value / 1024 ** exponent;
  return `${scaled >= 10 || exponent === 0 ? scaled.toFixed(0) : scaled.toFixed(1)} ${units[exponent]}`;
}

export function memoryPercent(stats: StatsRecord | undefined): number {
  if (!stats?.memoryUsageBytes || !stats.memoryLimitBytes) return 0;
  return Math.min(100, Math.round((stats.memoryUsageBytes / stats.memoryLimitBytes) * 100));
}

export function formatDateTime(value: string | number | null | undefined): string {
  if (value == null || value === "") return "-";
  const date = typeof value === "number" ? new Date(value) : new Date(value);
  if (Number.isNaN(date.getTime())) return "-";
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

export function summarizeError(error: CommandFailure | null | undefined): string {
  if (!error) return "";
  const detail = error.stderr.trim() || error.message;
  return `${error.kind}: ${detail}`;
}

export function statusTone(state: string): "good" | "warn" | "bad" | "muted" {
  switch (state.toLowerCase()) {
    case "running":
      return "good";
    case "created":
    case "stopped":
      return "warn";
    case "exited":
    case "failed":
      return "bad";
    default:
      return "muted";
  }
}
