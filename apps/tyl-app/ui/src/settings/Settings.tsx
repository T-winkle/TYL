import {
  createSignal,
  createEffect,
  onCleanup,
  onMount,
  Show,
  For,
} from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon, type IconName } from "../shared/Icon";
import { applyTheme, type ColorScheme, type Theme } from "../shared/theme";
import { installContextMenuGuard } from "../shared/contextMenu";
import logo from "../shared/logo.svg";

interface Llm {
  enabled: boolean;
  base_url: string;
  api_key: string;
  model: string;
}
interface Proxy {
  mode: string; // "off" | "system" | "manual"
  url: string;
}
interface DictionarySettings {
  enabled: boolean;
  provider: "auto" | "youdao" | "iciba" | "bing";
}
interface SettingsData {
  hotkey: string;
  /** 有序启用引擎；第一个 = 主引擎（弹窗 tab 顺序同此） */
  engines: string[];
  result_display: "tabs" | "stacked";
  theme: Theme;
  color_scheme: ColorScheme;
  show_source: boolean;
  llm: Llm;
  proxy: Proxy;
  dictionary: DictionarySettings;
  gpu_acceleration: boolean;
  log_level: "off" | "error" | "warn" | "info" | "debug" | "trace";
}

interface GpuStatus {
  requested: boolean;
  software: boolean;
  external_override: boolean;
}

type Tab = "general" | "translate" | "network" | "llm";

/** 引擎元数据（标签+说明）。顺序即追加时的展示顺序。 */
const ENGINE_META: Record<string, { label: string; desc: string }> = {
  bing: { label: "必应", desc: "通用文本与日常阅读" },
  youdao: { label: "有道", desc: "短文本翻译，免配置内部接口" },
  transmart: { label: "腾讯", desc: "长句与技术文本" },
  yandex: { label: "Yandex", desc: "多语言及长文翻译，免配置" },
  iciba: { label: "金山", desc: "中短文本，国内访问较快" },
  google: { label: "Google", desc: "多语言翻译，部分网络需代理" },
  mymemory: { label: "MyMemory", desc: "社区翻译记忆库" },
  llm: { label: "AI", desc: "自配大模型，流式输出（见 AI 模型页）" },
};
const ALL_ENGINE_IDS = Object.keys(ENGINE_META);
const PALETTES: { id: ColorScheme; label: string }[] = [
  { id: "jade", label: "松石" },
  { id: "indigo", label: "靛蓝" },
  { id: "plum", label: "岩紫" },
];

function formatHotkey(value: string) {
  const names: Record<string, string> = {
    ctrl: "Ctrl",
    control: "Ctrl",
    alt: "Alt",
    shift: "Shift",
    meta: "Win",
  };
  return value
    .split("+")
    .map((part) => names[part.toLowerCase()] ?? part.toUpperCase())
    .join("+");
}

const PAGE_META: Record<Tab, { eyebrow: string; title: string; desc: string }> =
  {
    general: {
      eyebrow: "GENERAL",
      title: "通用",
      desc: "管理划词方式、快捷键与系统行为",
    },
    translate: {
      eyebrow: "TRANSLATION",
      title: "翻译",
      desc: "选择结果布局并安排翻译引擎优先级",
    },
    network: {
      eyebrow: "NETWORK",
      title: "网络",
      desc: "配置翻译服务使用的连接与代理方式",
    },
    llm: {
      eyebrow: "AI MODEL",
      title: "AI 模型",
      desc: "接入兼容 OpenAI 协议的大语言模型",
    },
  };

export function Settings() {
  const [s, setS] = createSignal<SettingsData | null>(null);
  const [status, setStatus] = createSignal("");
  const [statusError, setStatusError] = createSignal(false);
  const [recording, setRecording] = createSignal(false);
  const [autostart, setAutostart] = createSignal(false);
  const [tab, setTab] = createSignal<Tab>("general");
  const [saved, setSaved] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [gpuStatus, setGpuStatus] = createSignal<GpuStatus | null>(null);
  const [savedGpu, setSavedGpu] = createSignal(false);
  const [testingLlm, setTestingLlm] = createSignal(false);
  const [showApiKey, setShowApiKey] = createSignal(false);
  createEffect(() => {
    if (tab() !== "llm") setShowApiKey(false);
  });
  const [llmTest, setLlmTest] = createSignal<{ text: string; error: boolean } | null>(null);
  const restartRequired = () => gpuStatus() !== null && savedGpu() !== gpuStatus()!.requested;
  let statusTimer = 0;
  let restoreScrollFrame = 0;
  let contentScroll: HTMLDivElement | undefined;
  let removeContextMenu = () => {};
  let removeWindowDrag = () => {};
  let dragOrigin: { x: number; y: number } | undefined;
  const scrollOffsets: Record<Tab, number> = {
    general: 0,
    translate: 0,
    network: 0,
    llm: 0,
  };
  let removeRecording: (() => void) | undefined;
  const dirty = () => Boolean(s()) && JSON.stringify(s()) !== saved();
  const beginWindowDrag = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const target = event.target;
    if (
      target instanceof Element &&
      target.closest("button, input, select, textarea, a, [role='button']")
    )
      return;
    event.preventDefault();
    dragOrigin = { x: event.clientX, y: event.clientY };
  };
  createEffect(() => {
    if (s()) applyTheme(s()!.theme ?? "system", s()!.color_scheme ?? "indigo");
  });
  onCleanup(() => {
    window.clearTimeout(statusTimer);
    window.cancelAnimationFrame(restoreScrollFrame);
    removeContextMenu();
    removeWindowDrag();
    removeRecording?.();
  });

  onMount(async () => {
    removeContextMenu = installContextMenuGuard();
    const cancelDrag = () => {
      dragOrigin = undefined;
    };
    const moveDrag = (event: PointerEvent) => {
      if (!dragOrigin) return;
      if (!(event.buttons & 1)) {
        cancelDrag();
        return;
      }
      if (
        Math.hypot(event.clientX - dragOrigin.x, event.clientY - dragOrigin.y) <
        4
      )
        return;
      cancelDrag();
      void getCurrentWindow()
        .startDragging()
        .catch((e) => showError(`移动窗口失败: ${String(e)}`));
    };
    window.addEventListener("pointermove", moveDrag);
    window.addEventListener("pointerup", cancelDrag);
    window.addEventListener("pointercancel", cancelDrag);
    removeWindowDrag = () => {
      window.removeEventListener("pointermove", moveDrag);
      window.removeEventListener("pointerup", cancelDrag);
      window.removeEventListener("pointercancel", cancelDrag);
    };
    try {
      const initial = await invoke<SettingsData>("get_settings");
      setS(initial);
      setSaved(JSON.stringify(initial));
      setSavedGpu(initial.gpu_acceleration ?? false);
      setGpuStatus(await invoke<GpuStatus>("get_gpu_status"));
      setAutostart(await invoke<boolean>("get_autostart"));
    } catch (e) {
      showError(`加载失败: ${String(e)}`);
    }
  });

  const toggleAutostart = async (on: boolean) => {
    setAutostart(on);
    try {
      await invoke("set_autostart", { enabled: on });
      flashStatus(on ? "开机自启已开启" : "开机自启已关闭");
    } catch (e) {
      setAutostart(!on);
      showError(`自启设置失败: ${String(e)}`);
    }
  };

  const flashStatus = (msg: string) => {
    window.clearTimeout(statusTimer);
    setStatusError(false);
    setStatus(msg);
    statusTimer = window.setTimeout(() => setStatus(""), 2400);
  };

  const showError = (msg: string) => {
    window.clearTimeout(statusTimer);
    setStatusError(true);
    setStatus(msg);
  };

  const patch = (p: Partial<SettingsData>) => {
    if (p.llm) setLlmTest(null);
    window.clearTimeout(statusTimer);
    setStatus("");
    setStatusError(false);
    setS((prev) => (prev ? { ...prev, ...p } : prev));
  };

  const patchDictionary = (p: Partial<DictionarySettings>) => {
    patch({ dictionary: { ...s()!.dictionary, ...p } });
  };

  const testLlm = async () => {
    if (!s() || testingLlm()) return;
    const config = { ...s()!.llm };
    setTestingLlm(true);
    setLlmTest(null);
    try {
      const text = await invoke<string>("test_llm", { config });
      if (JSON.stringify(config) === JSON.stringify(s()!.llm)) setLlmTest({ text, error: false });
    } catch (error) {
      if (JSON.stringify(config) === JSON.stringify(s()!.llm)) setLlmTest({ text: String(error), error: true });
    } finally { setTestingLlm(false); }
  };

  /** 引擎上移/下移（dir = -1/+1） */
  const moveEngine = (index: number, dir: -1 | 1) => {
    const list = [...s()!.engines];
    const j = index + dir;
    if (j < 0 || j >= list.length) return;
    [list[index], list[j]] = [list[j], list[index]];
    patch({ engines: list });
  };

  /** 引擎开关（至少保留一个） */
  const toggleEngine = (id: string, on: boolean) => {
    const cur = s()!;
    if (on) {
      patch({ engines: [...cur.engines, id] });
    } else if (cur.engines.length > 1) {
      patch({ engines: cur.engines.filter((e) => e !== id) });
    }
  };

  const save = async () => {
    const cur = s();
    if (!cur || saving()) return;
    if (!cur.hotkey.trim()) {
      showError("热键不能为空");
      return;
    }
    try {
      setSaving(true);
      setStatus("");
      setStatusError(false);
      await invoke("save_settings", { settings: cur });
      setSaved(JSON.stringify(cur));
      setSavedGpu(cur.gpu_acceleration);
      flashStatus(restartRequired() ? "已保存，重启后切换渲染模式" : "设置已保存");
    } catch (e) {
      showError(`保存失败: ${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  /** 热键录制：捕获下一组按键组合。 */
  const startRecord = () => {
    removeRecording?.();
    setRecording(true);
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const k = e.key;
      if (k === "Escape") {
        cancelRecord();
        return;
      }
      if (["Control", "Alt", "Shift", "Meta"].includes(k)) return;
      const mods: string[] = [];
      if (e.ctrlKey) mods.push("ctrl");
      if (e.altKey) mods.push("alt");
      if (e.shiftKey) mods.push("shift");
      if (e.metaKey) mods.push("meta");
      const key = /^[a-z0-9]$/i.test(k) ? k.toLowerCase() : k;
      if (/^[a-z0-9]$/.test(key) || /^F\d{1,2}$/.test(key)) {
        if (mods.length === 0 && /^[a-z]$/.test(key)) {
          showError("请加至少一个修饰键（Ctrl/Alt/Shift）");
          return;
        }
        patch({ hotkey: [...mods, key].join("+") });
      } else {
        showError(`不支持的键: ${k}`);
      }
      setRecording(false);
      window.removeEventListener("keydown", onKey, true);
    };
    window.addEventListener("keydown", onKey, true);
    removeRecording = () => window.removeEventListener("keydown", onKey, true);
  };

  const cancelRecord = () => {
    setRecording(false);
    removeRecording?.();
  };

  const switchTab = (next: Tab) => {
    const current = tab();
    if (next === current) return;
    if (contentScroll) scrollOffsets[current] = contentScroll.scrollTop;
    setTab(next);
    window.cancelAnimationFrame(restoreScrollFrame);
    restoreScrollFrame = window.requestAnimationFrame(() => {
      if (contentScroll) contentScroll.scrollTop = scrollOffsets[next];
    });
  };

  const TABS: { id: Tab; label: string; icon: IconName }[] = [
    { id: "general", label: "通用", icon: "settings" },
    { id: "translate", label: "翻译", icon: "translate" },
    { id: "network", label: "网络", icon: "network" },
    { id: "llm", label: "AI 模型", icon: "ai" },
  ];

  return (
    <>
      <div class="window-chrome">
        <button
          class="window-close"
          aria-label="关闭设置"
          title="关闭设置"
          onClick={() =>
            void getCurrentWindow()
              .close()
              .catch((e) => showError(`关闭失败: ${String(e)}`))
          }
        >
          <Icon name="close" size={16} />
        </button>
      </div>
      <Show
        when={s()}
        fallback={<div class="loading">{status() || "正在加载设置…"}</div>}
      >
        {(cur) => (
          <div class="settings">
            {/* 侧边导航 */}
            <nav class="nav" onPointerDown={beginWindowDrag}>
              <div class="nav-brand" title="拖动此处移动窗口">
                <img class="nav-logo" src={logo} alt="TYL 标志" />
                <div class="nav-brand-copy">
                  <div class="nav-title">TYL</div>
                  <div class="nav-sub">划词翻译</div>
                </div>
              </div>
              <div class="nav-items">
                <For each={TABS}>
                  {(t) => (
                    <button
                      classList={{ active: tab() === t.id }}
                      aria-current={tab() === t.id ? "page" : undefined}
                      onClick={() => {
                        cancelRecord();
                        switchTab(t.id);
                      }}
                    >
                      <Icon name={t.icon} size={17} />
                      {t.label}
                    </button>
                  )}
                </For>
              </div>
              <div class="nav-footer">
                <span>阅读，不止一种语言</span>
                <small>TYL · 0.1.0</small>
              </div>
            </nav>

            {/* 内容区 */}
            <div class="content">
              <header
                class="page-header"
                onPointerDown={beginWindowDrag}
              >
                <div class="page-head" title="拖动此处移动窗口">
                  <div class="page-eyebrow">{PAGE_META[tab()].eyebrow}</div>
                  <h1>{PAGE_META[tab()].title}</h1>
                  <p>{PAGE_META[tab()].desc}</p>
                </div>
                <div class="page-actions">
                  <div
                    class="status"
                    role="status"
                    data-state={statusError() ? "error" : dirty() ? "dirty" : "saved"}
                  >
                    <Show when={!dirty() && !statusError() && !saving()}>
                      <Icon name="check" size={14} />
                    </Show>
                    <span>{saving() ? "正在保存…" : statusError() ? "需要检查" : status() || (dirty() ? "未保存" : "已保存")}</span>
                  </div>
                  <button
                    class="primary"
                    disabled={!dirty() || saving()}
                    onClick={() => void save()}
                  >
                    {saving() ? "正在保存…" : "保存更改"}
                  </button>
                </div>
              </header>
              <Show when={statusError()}>
                <div class="settings-message" role="alert">{status()}</div>
              </Show>
              <Show when={restartRequired()}>
                <div class="restart-notice" role="status">
                  <strong>渲染模式将在重启后生效</strong>
                  <span>请从托盘退出 TYL 后重新打开。仅关闭设置窗口不会重启应用。</span>
                </div>
              </Show>
              <div class="content-scroll" ref={contentScroll}>
                <Show when={tab() === "general"}>
                  <section class="group">
                    <div class="group-title">外观</div>
                    <div class="row theme-row">
                      <div class="row-text">
                        <div class="label">界面主题</div>
                        <div class="hint">弹窗与设置窗口使用相同的配色</div>
                      </div>
                      <div class="theme-options">
                        <For
                          each={
                            [
                              {
                                id: "system",
                                label: "跟随系统",
                                icon: "monitor",
                              },
                              { id: "light", label: "浅色", icon: "sun" },
                              { id: "dark", label: "深色", icon: "moon" },
                            ] as const
                          }
                        >
                          {(option) => (
                            <button
                              classList={{ active: cur().theme === option.id }}
                              aria-pressed={cur().theme === option.id}
                              onClick={() => patch({ theme: option.id })}
                            >
                              <Icon name={option.icon} size={16} />
                              {option.label}
                              <Show when={cur().theme === option.id}>
                                <Icon name="check" size={13} />
                              </Show>
                            </button>
                          )}
                        </For>
                      </div>
                    </div>
                    <div class="row palette-row">
                      <div class="row-text">
                        <div class="label">主题色</div>
                        <div class="hint">用于强调色、选中状态与操作反馈</div>
                      </div>
                      <div class="palette-options" aria-label="主题色">
                        <For each={PALETTES}>
                          {(option) => (
                            <button
                              classList={{
                                active: cur().color_scheme === option.id,
                              }}
                              data-palette={option.id}
                              aria-label={option.label}
                              aria-pressed={cur().color_scheme === option.id}
                              title={option.label}
                              onClick={() => patch({ color_scheme: option.id })}
                            >
                              <span />
                            </button>
                          )}
                        </For>
                      </div>
                    </div>
                  </section>
                  <section class="group">
                    <div class="group-title">划词热键</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">触发组合键</div>
                        <div class="hint">保存后重启应用生效</div>
                      </div>
                      <button
                        class={`hotkey-btn ${recording() ? "recording" : ""}`}
                        onClick={() =>
                          recording() ? cancelRecord() : startRecord()
                        }
                      >
                        {recording()
                          ? "按下组合键…（Esc 取消）"
                          : formatHotkey(cur().hotkey) || "未设置"}
                      </button>
                    </div>
                    <p class="hint">
                      建议 Alt/Shift 系组合，避免与常用软件冲突。
                    </p>
                  </section>

                  <section class="group">
                    <div class="group-title">取词</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">显示选中原文</div>
                        <div class="hint">默认关闭，为译文保留更多阅读空间</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={cur().show_source}
                        aria-label="显示选中原文"
                        onChange={(e) =>
                          patch({ show_source: e.currentTarget.checked })
                        }
                      />
                    </div>
                    <p class="hint capture-hint">
                      优先直接读取选区，必要时临时使用剪贴板并还原。终端仅直接读取，不模拟 Ctrl+C，避免中断正在运行的命令。
                    </p>
                  </section>

                  <section class="group">
                    <div class="group-title">系统</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">GPU 硬件加速</div>
                        <div class="hint" id="gpu-hint">默认关闭以减少显存占用；开启可利用 GPU 渲染。保存后重启生效。</div>
                        <Show when={gpuStatus()}>
                          <div class="hint gpu-current" id="gpu-current">
                            本次启动：{gpuStatus()!.software ? "软件渲染" : "GPU 硬件加速"}
                            {gpuStatus()!.external_override ? " · 外部启动参数禁用了 GPU" : ""}
                          </div>
                        </Show>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={cur().gpu_acceleration ?? false}
                        aria-label="GPU 硬件加速"
                        aria-describedby="gpu-hint gpu-current"
                        onChange={(e) => patch({ gpu_acceleration: e.currentTarget.checked })}
                      />
                    </div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">开机自启</div>
                        <div class="hint">登录 Windows 后自动驻留后台</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={autostart()}
                        aria-label="开机自启"
                        onChange={(e) =>
                          void toggleAutostart(e.currentTarget.checked)
                        }
                      />
                    </div>
                    <div class="row">
                      <div class="row-text">
                        <label class="label" for="log-level">日志等级</label>
                        <div class="hint">默认记录运行状态和异常，保存后立即生效</div>
                      </div>
                      <div class="log-level-wrap">
                        <select id="log-level" class="input log-level" value={cur().log_level ?? "info"} onChange={(e) => patch({ log_level: e.currentTarget.value as SettingsData["log_level"] })}>
                          <option value="off">关闭 · OFF</option>
                          <option value="error">错误 · ERROR</option>
                          <option value="warn">警告 · WARN</option>
                          <option value="info">信息 · INFO</option>
                          <option value="debug">调试 · DEBUG</option>
                          <option value="trace">跟踪 · TRACE</option>
                        </select>
                        <span class="log-level-chevron"><Icon name="down" size={15} /></span>
                      </div>
                    </div>
                    <p class="hint">日志写入程序目录的 tyl.log；目录不可写时使用系统临时目录。调试/跟踪用于排查问题，不记录密钥或选中文本。</p>
                  </section>
                </Show>

                <Show when={tab() === "translate"}>
                  <section class="group display-group">
                    <div class="group-title">结果呈现</div>
                    <div class="display-options">
                      <button
                        type="button"
                        class="display-option"
                        classList={{ active: cur().result_display === "tabs" }}
                        aria-pressed={cur().result_display === "tabs"}
                        onClick={() => patch({ result_display: "tabs" })}
                      >
                        <span
                          class="mode-preview preview-tabs"
                          aria-hidden="true"
                        >
                          <i />
                          <i />
                          <i />
                          <b />
                          <b />
                        </span>
                        <span class="mode-copy">
                          <strong>标签切换</strong>
                          <small>首个引擎立即翻译，其余按需请求</small>
                        </span>
                        <span class="mode-check" aria-hidden="true">✓</span>
                      </button>
                      <button
                        type="button"
                        class="display-option"
                        classList={{
                          active: cur().result_display === "stacked",
                        }}
                        aria-pressed={cur().result_display === "stacked"}
                        onClick={() => patch({ result_display: "stacked" })}
                      >
                        <span
                          class="mode-preview preview-stacked"
                          aria-hidden="true"
                        >
                          <i />
                          <i />
                          <i />
                        </span>
                        <span class="mode-copy">
                          <strong>全部展开</strong>
                          <small>并行请求全部引擎，纵向对照结果</small>
                        </span>
                        <span class="mode-check" aria-hidden="true">✓</span>
                      </button>
                    </div>
                  </section>

                  <section class="group">
                    <div class="group-title">翻译引擎</div>
                    <p class="hint">
                      {cur().result_display === "tabs"
                        ? "排在第一位的引擎划词后立即翻译，其余引擎切换时按需请求。"
                        : "划词后并行请求所有已启用引擎，并按以下顺序展示结果。"}
                      使用箭头调整顺序。
                    </p>

                    {/* 已启用（有序） */}
                    <div class="engine-list">
                      <For each={cur().engines}>
                        {(id, i) => (
                          <div class="engine-item">
                            <span class="engine-order">
                              {String(i() + 1).padStart(2, "0")}
                            </span>
                            <div class="row-text">
                              <div class="label engine-label">
                                {ENGINE_META[id]?.label ?? id}
                                <Show when={i() === 0}>
                                  <span class="preferred-badge">
                                    {cur().result_display === "tabs"
                                      ? "首选引擎"
                                      : "优先展示"}
                                  </span>
                                </Show>
                              </div>
                              <div class="hint">
                                {ENGINE_META[id]?.desc ?? ""}
                              </div>
                            </div>
                            <div class="engine-ops">
                              <button
                                class="icon-btn"
                                title="上移"
                                disabled={i() === 0}
                                onClick={() => moveEngine(i(), -1)}
                              >
                                <Icon name="up" size={15} />
                              </button>
                              <button
                                class="icon-btn"
                                title="下移"
                                disabled={i() === cur().engines.length - 1}
                                onClick={() => moveEngine(i(), 1)}
                              >
                                <Icon name="down" size={15} />
                              </button>
                              <input
                                type="checkbox"
                                class="switch"
                                checked={true}
                                disabled={cur().engines.length <= 1}
                                title="关闭（至少保留一个引擎）"
                                onChange={() => toggleEngine(id, false)}
                              />
                            </div>
                          </div>
                        )}
                      </For>
                    </div>

                    {/* 未启用：一行小开关追加 */}
                    <div class="engine-add">
                      <For
                        each={ALL_ENGINE_IDS.filter(
                          (id) => !cur().engines.includes(id),
                        )}
                      >
                        {(id) => (
                          <button
                            class="chip"
                            onClick={() => toggleEngine(id, true)}
                          >
                            + {ENGINE_META[id]?.label ?? id}
                          </button>
                        )}
                      </For>
                    </div>
                  </section>

                  <section class="group dictionary-group">
                    <div class="group-title">词典服务</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">单词词典卡片</div>
                        <div class="hint">选中英文单词时显示音标、词性与释义</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        aria-label="单词词典卡片"
                        checked={cur().dictionary.enabled}
                        onChange={(e) => patchDictionary({ enabled: e.currentTarget.checked })}
                      />
                    </div>
                    <div class="row dictionary-provider-row" classList={{ disabled: !cur().dictionary.enabled }}>
                      <div class="row-text">
                        <label class="label" for="dictionary-provider">词典数据源</label>
                        <div class="hint">
                          {cur().dictionary.provider === "auto"
                            ? "自动按有道 → 金山 → 必应的顺序降级"
                            : "固定使用所选词典，失败时不切换数据源"}
                        </div>
                      </div>
                      <div class="select-wrap dictionary-select-wrap">
                        <select
                          id="dictionary-provider"
                          class="input dictionary-select"
                          value={cur().dictionary.provider}
                          disabled={!cur().dictionary.enabled}
                          onChange={(e) => patchDictionary({ provider: e.currentTarget.value as DictionarySettings["provider"] })}
                        >
                          <option value="auto">自动推荐</option>
                          <option value="youdao">有道词典</option>
                          <option value="iciba">金山词典</option>
                          <option value="bing">必应词典</option>
                        </select>
                        <span class="select-chevron"><Icon name="down" size={15} /></span>
                      </div>
                    </div>
                  </section>
                </Show>

                <Show when={tab() === "network"}>
                  <section class="group">
                    <div class="group-title">网络代理</div>
                    <div class="seg">
                      {[
                        ["off", "直连"],
                        ["system", "跟随系统"],
                        ["manual", "手动"],
                      ].map(([v, label]) => (
                        <button
                          classList={{ active: cur().proxy.mode === v }}
                          onClick={() =>
                            patch({ proxy: { ...cur().proxy, mode: v } })
                          }
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                    <Show when={cur().proxy.mode === "manual"}>
                      <input
                        class="input"
                        type="text"
                        placeholder="http://127.0.0.1:7890 或 socks5://…"
                        value={cur().proxy.url}
                        onInput={(e) =>
                          patch({
                            proxy: {
                              ...cur().proxy,
                              url: e.currentTarget.value,
                            },
                          })
                        }
                      />
                      <p class="hint">翻译与词典请求全部经此代理。</p>
                    </Show>
                    <Show when={cur().proxy.mode === "system"}>
                      <p class="hint">
                        自动使用 Windows 系统代理（Internet
                        选项），随系统开关联动。
                      </p>
                    </Show>
                  </section>
                </Show>

                <Show when={tab() === "llm"}>
                  <section class="group">
                    <div class="group-title">AI 模型（OpenAI 兼容）</div>
                    <label class="field">
                      <span class="field-label">Base URL</span>
                      <input
                        class="input"
                        type="text"
                        placeholder="https://api.deepseek.com/v1"
                        value={cur().llm.base_url}
                        onInput={(e) =>
                          patch({
                            llm: {
                              ...cur().llm,
                              base_url: e.currentTarget.value,
                            },
                          })
                        }
                      />
                    </label>
                    <p class="hint">
                      支持供应商提供的 Base URL 或完整 /chat/completions 地址。
                    </p>
                    <div class="field">
                      <label class="field-label" for="llm-api-key">API Key</label>
                      <div class="secret-input">
                      <input
                        id="llm-api-key"
                        class="input"
                        type={showApiKey() ? "text" : "password"}
                        autocomplete="off"
                        spellcheck={false}
                        placeholder="sk-••••••••"
                        value={cur().llm.api_key}
                        onInput={(e) =>
                          patch({
                            llm: {
                              ...cur().llm,
                              api_key: e.currentTarget.value,
                            },
                          })
                        }
                      />
                        <button type="button" class="secret-toggle"
                          aria-label={showApiKey() ? "隐藏 API Key" : "显示 API Key"}
                          title={showApiKey() ? "隐藏 API Key" : "显示 API Key"}
                          aria-pressed={showApiKey()}
                          onClick={() => setShowApiKey(!showApiKey())}>
                          <Icon name={showApiKey() ? "eyeOff" : "eye"} size={18} />
                        </button>
                      </div>
                    </div>
                    <label class="field">
                      <span class="field-label">模型</span>
                      <input
                        class="input"
                        type="text"
                        placeholder="deepseek-chat / gpt-4o-mini"
                        value={cur().llm.model}
                        onInput={(e) =>
                          patch({
                            llm: { ...cur().llm, model: e.currentTarget.value },
                          })
                        }
                      />
                    </label>
                    <p class="hint">
                      请在翻译页启用 AI 引擎；标签模式下，首选
                      AI 失败后会尝试其他启用的引擎。
                    </p>
                    <div class="connection-test">
                      <button class="test-connection" disabled={testingLlm() || !cur().llm.api_key.trim() || !cur().llm.model.trim()} onClick={() => void testLlm()}>
                        {testingLlm() ? "正在测试…" : "测试连接"}
                      </button>
                      <span class="hint">使用当前填写的配置发送固定短句，不保存设置；会消耗少量 API 额度。</span>
                    </div>
                    <Show when={llmTest()}><p class="connection-result" classList={{ error: llmTest()!.error }} role="status">{llmTest()!.text}</p></Show>
                  </section>
                </Show>
              </div>

            </div>
          </div>
        )}
      </Show>
    </>
  );
}
