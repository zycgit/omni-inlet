<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useWindowsStore } from "./stores/windows";

const store = useWindowsStore();
const permission = ref({ required: false, granted: true, settingsLabel: "" });
const permissionRequested = ref(false);
let leaseTimer: number | undefined;
let eventUnlisteners: UnlistenFn[] = [];

async function chooseOutput(): Promise<void> {
  const selected = await open({ directory: true, multiple: false, defaultPath: store.outputRoot });
  if (typeof selected === "string") store.outputRoot = selected;
}

onMounted(async () => {
  eventUnlisteners = [
    await listen("capture-event", () => {
      void store.refreshLeases();
    }),
    await listen("capture-exited", (event) => {
      store.reportCaptureExit(event.payload as { windowTitle?: string; exitCode?: number; error?: string });
      void store.refreshLeases();
    }),
  ];
  permission.value = await invoke("capture_permission_status");
  await store.loadDefaultOutput();
  if (permission.value.granted) await store.refresh();
  leaseTimer = window.setInterval(() => void store.refreshLeases(), 2_000);
});

async function requestPermission(): Promise<void> {
  const granted = await invoke<boolean>("request_capture_permission");
  permissionRequested.value = true;
  permission.value = { ...permission.value, granted };
  if (granted) await store.refresh();
}

onBeforeUnmount(() => {
  window.clearInterval(leaseTimer);
  for (const unlisten of eventUnlisteners) unlisten();
});
</script>

<template>
  <div class="shell">
    <div v-if="permission.required && !permission.granted" class="permission-overlay">
      <section class="permission-card">
        <div class="permission-icon">▣</div>
        <p class="eyebrow">MACOS 权限</p>
        <h2>允许 OmniInlet 读取窗口画面</h2>
        <p>窗口枚举、缩略图和视频采集都需要“屏幕与系统录音”权限。OmniInlet 只记录你明确选择的窗口。</p>
        <div class="settings-path">系统设置 → {{ permission.settingsLabel }}</div>
        <button class="primary" @click="requestPermission">授予屏幕录制权限</button>
        <p v-if="permissionRequested" class="restart-tip">完成系统设置后，请退出并重新启动 OmniInlet，使权限对采集子进程生效。</p>
      </section>
    </div>
    <header class="topbar">
      <div class="brand">
        <div class="brand-mark">OI</div>
        <div>
          <h1>OmniInlet</h1>
          <p>选择需要持续记录的聊天窗口</p>
        </div>
      </div>
      <button class="secondary refresh" :disabled="store.loading" @click="store.refresh">
        <span :class="{ spinning: store.loading }">↻</span>
        {{ store.loading ? "正在扫描" : "刷新窗口" }}
      </button>
    </header>

    <main>
      <section class="toolbar" aria-label="窗口筛选">
        <label class="search-box">
          <span>⌕</span>
          <input v-model="store.search" type="search" placeholder="搜索应用或窗口标题" />
          <kbd>⌘ K</kbd>
        </label>
        <div class="segments" role="group" aria-label="采集状态">
          <button :class="{ active: store.filter === 'all' }" @click="store.filter = 'all'">
            全部 <b>{{ store.windows.length }}</b>
          </button>
          <button :class="{ active: store.filter === 'idle' }" @click="store.filter = 'idle'">
            未采集 <b>{{ store.windows.length - store.captureWindowCount }}</b>
          </button>
          <button :class="{ active: store.filter === 'capturing' }" @click="store.filter = 'capturing'">
            正在采集 <b>{{ store.captureWindowCount }}</b>
          </button>
        </div>
      </section>

      <section class="result-header">
        <p>找到 {{ store.filtered.length }} 个窗口，按应用分为 {{ store.groups.length }} 组</p>
        <div>
          <button class="text-button" @click="store.expandAll">全部展开</button>
          <i></i>
          <button class="text-button" @click="store.collapseAll">全部折叠</button>
        </div>
      </section>

      <div v-if="store.error" class="error-banner" role="alert">
        <strong>无法完成操作</strong><span>{{ store.error }}</span>
      </div>

      <section v-if="!store.loading && store.groups.length === 0" class="empty-state">
        <div>▱</div>
        <h2>没有符合条件的窗口</h2>
        <p>请确认聊天软件窗口已经打开，或清除当前搜索和过滤条件。</p>
      </section>

      <section v-for="[groupId, group] in store.groups" :key="groupId" class="app-group">
        <button class="group-heading" @click="store.toggleGroup(groupId)">
          <span class="app-icon">{{ group.application.displayName.slice(0, 1).toUpperCase() }}</span>
          <span class="group-name">{{ group.application.displayName }}</span>
          <span class="group-summary">
            {{ group.windows.length }} 个窗口
            <template v-if="group.windows.filter((item) => store.countFor(item) > 0).length">
              · 正在采集 {{ group.windows.filter((item) => store.countFor(item) > 0).length }}
            </template>
          </span>
          <span class="chevron" :class="{ collapsed: store.isCollapsed(groupId) }">⌃</span>
        </button>

        <div v-if="!store.isCollapsed(groupId)" class="window-grid">
          <button
            v-for="windowItem in group.windows"
            :key="windowItem.candidateId"
            class="window-card"
            :class="{
              selected: store.selectedId === windowItem.candidateId,
              unavailable: !windowItem.capturable,
            }"
            :disabled="!windowItem.capturable"
            @click="store.selectedId = windowItem.candidateId"
          >
            <div class="thumbnail">
              <img
                v-if="store.thumbnailUrl(windowItem.thumbnailPath)"
                :src="store.thumbnailUrl(windowItem.thumbnailPath)"
                :alt="windowItem.title"
              />
              <div v-else class="thumbnail-placeholder">
                <span>{{ windowItem.application.displayName.slice(0, 2) }}</span>
                <small>{{ windowItem.width }} × {{ windowItem.height }}</small>
              </div>
              <span v-if="store.selectedId === windowItem.candidateId" class="selected-check">✓</span>
              <span v-if="store.countFor(windowItem)" class="capture-badge">
                正在采集<span v-if="store.countFor(windowItem) > 1"> ({{ store.countFor(windowItem) }})</span>
              </span>
              <span v-else-if="!windowItem.capturable" class="unavailable-badge">
                {{ windowItem.unavailableReason ?? "无可用画面" }}
              </span>
            </div>
            <div class="window-title" :title="windowItem.title">{{ windowItem.title }}</div>
          </button>
        </div>
      </section>
    </main>

    <footer class="action-bar">
      <div class="selection-summary">
        <span class="selection-label">当前选择</span>
        <strong>{{ store.selected?.title ?? "尚未选择窗口" }}</strong>
        <small v-if="store.selected">{{ store.selected.application.displayName }}</small>
      </div>
      <div class="output-field">
        <label>保存到</label>
        <button class="path-button" @click="chooseOutput">
          <span>{{ store.outputRoot }}</span><b>浏览</b>
        </button>
      </div>
      <button class="primary" :disabled="!store.selected?.capturable || !store.outputRoot || store.starting" @click="store.startSelected">
        <span>●</span> {{ store.starting ? "正在启动" : "开始采集" }}
      </button>
    </footer>
  </div>
</template>
