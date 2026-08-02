import { computed, ref } from "vue";
import { defineStore } from "pinia";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { AgentLease, CaptureFilter, WindowCandidate } from "../types";

export const useWindowsStore = defineStore("windows", () => {
  const windows = ref<WindowCandidate[]>([]);
  const leases = ref<AgentLease[]>([]);
  const search = ref("");
  const filter = ref<CaptureFilter>("all");
  const selectedId = ref<string>();
  const collapsedGroups = ref(new Set<string>());
  const loading = ref(false);
  const error = ref("");
  const outputRoot = ref("");

  const counts = computed(() => {
    const result = new Map<string, number>();
    for (const lease of leases.value) {
      result.set(lease.targetKey, (result.get(lease.targetKey) ?? 0) + 1);
    }
    return result;
  });

  const filtered = computed(() => {
    const keyword = search.value.trim().toLocaleLowerCase();
    return windows.value.filter((window) => {
      const count = counts.value.get(`${window.nativeTarget.kind}:${window.nativeTarget.value}`) ?? 0;
      if (filter.value === "idle" && count > 0) return false;
      if (filter.value === "capturing" && count === 0) return false;
      if (!keyword) return true;
      return `${window.application.displayName} ${window.title}`.toLocaleLowerCase().includes(keyword);
    });
  });

  const groups = computed(() => {
    const result = new Map<string, { application: WindowCandidate["application"]; windows: WindowCandidate[] }>();
    for (const window of filtered.value) {
      const group = result.get(window.application.groupId) ?? {
        application: window.application,
        windows: [],
      };
      group.windows.push(window);
      result.set(window.application.groupId, group);
    }
    return [...result.entries()].sort((a, b) =>
      a[1].application.displayName.localeCompare(b[1].application.displayName, "zh-CN"),
    );
  });

  const selected = computed(() => windows.value.find((window) => window.candidateId === selectedId.value));
  const captureWindowCount = computed(() => windows.value.filter((window) => countFor(window) > 0).length);

  function countFor(window: WindowCandidate): number {
    return counts.value.get(`${window.nativeTarget.kind}:${window.nativeTarget.value}`) ?? 0;
  }

  function thumbnailUrl(path?: string): string | undefined {
    return path ? convertFileSrc(path) : undefined;
  }

  async function refresh(): Promise<void> {
    loading.value = true;
    error.value = "";
    try {
      windows.value = await invoke<WindowCandidate[]>("enumerate_windows");
      await refreshLeases();
      if (selectedId.value && !windows.value.some((item) => item.candidateId === selectedId.value)) {
        selectedId.value = undefined;
      }
    } catch (reason) {
      error.value = String(reason);
    } finally {
      loading.value = false;
    }
  }

  async function refreshLeases(): Promise<void> {
    leases.value = await invoke<AgentLease[]>("list_agents");
  }

  async function loadDefaultOutput(): Promise<void> {
    outputRoot.value = await invoke<string>("default_output_directory");
  }

  async function startSelected(): Promise<void> {
    if (!selected.value) return;
    error.value = "";
    try {
      await invoke("start_capture", {
        request: {
          target: selected.value.nativeTarget,
          title: selected.value.title,
          outputRoot: outputRoot.value,
          fps: 10,
          segmentSeconds: 5,
          bitrateKbps: 2048,
        },
      });
      await refreshLeases();
      if (filter.value === "idle") {
        selectedId.value = undefined;
      }
    } catch (reason) {
      error.value = String(reason);
    }
  }

  function toggleGroup(groupId: string): void {
    const next = new Set(collapsedGroups.value);
    next.has(groupId) ? next.delete(groupId) : next.add(groupId);
    collapsedGroups.value = next;
  }

  function collapseAll(): void {
    collapsedGroups.value = new Set(groups.value.map(([id]) => id));
  }

  function expandAll(): void {
    collapsedGroups.value = new Set();
  }

  function isCollapsed(groupId: string): boolean {
    return !search.value && collapsedGroups.value.has(groupId);
  }

  return {
    windows, leases, search, filter, selectedId, loading, error, outputRoot,
    groups, selected, captureWindowCount, filtered,
    countFor, thumbnailUrl, refresh, refreshLeases, loadDefaultOutput, startSelected,
    toggleGroup, collapseAll, expandAll, isCollapsed,
  };
});
