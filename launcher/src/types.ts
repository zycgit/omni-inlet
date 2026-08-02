export interface ApplicationInfo {
  groupId: string;
  displayName: string;
  processId?: number;
  iconPath?: string;
}

export interface NativeTarget {
  kind: string;
  value: string;
}

export interface WindowCandidate {
  candidateId: string;
  application: ApplicationInfo;
  title: string;
  visible: boolean;
  capturable: boolean;
  unavailableReason?: string;
  thumbnailPath?: string;
  width: number;
  height: number;
  nativeTarget: NativeTarget;
}

export interface AgentLease {
  agentId: string;
  jobId: string;
  pid: number;
  targetKey: string;
  outputDirectory: string;
  state: "starting" | "capturing" | "suspended" | "unresponsive";
  startedAtUnixMs: number;
  heartbeatAtUnixMs: number;
  segments: number;
  recordedDurationMs: number;
}

export type CaptureFilter = "all" | "idle" | "capturing";
