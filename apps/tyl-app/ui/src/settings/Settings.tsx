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
import { Logo } from "../shared/Logo";
import { applyTheme, type ColorScheme, type Theme } from "../shared/theme";
import { installContextMenuGuard } from "../shared/contextMenu";
import {
  applyLanguage,
  localizedError,
  tr,
  type LanguagePreference,
} from "../shared/i18n";
import {
  languageLabel,
  TRANSLATION_LANGUAGES,
  type TranslationLanguage,
} from "../shared/languages";

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
interface LanguageRoutingSettings {
  mode: "smart" | "fixed_target" | "fixed_pair";
  primary: TranslationLanguage;
  secondary: TranslationLanguage;
  source: TranslationLanguage;
  target: TranslationLanguage;
}
interface SettingsData {
  language: LanguagePreference;
  hotkey: string;
  /** 有序启用引擎；第一个 = 主引擎（弹窗 tab 顺序同此） */
  engines: string[];
  result_display: "tabs" | "stacked";
  language_routing: LanguageRoutingSettings;
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
const ALL_ENGINE_IDS = [
  "bing",
  "youdao",
  "transmart",
  "yandex",
  "iciba",
  "google",
  "mymemory",
  "llm",
];

function engineMeta(id: string): { label: string; desc: string } {
  return {
    bing: { label: tr("必应", "Bing"), desc: tr("通用文本与日常阅读", "General text and everyday reading") },
    youdao: { label: tr("有道", "Youdao"), desc: tr("短文本翻译，免配置内部接口", "Short text via a key-free internal endpoint") },
    transmart: { label: tr("腾讯", "Tencent"), desc: tr("长句与技术文本", "Long sentences and technical text") },
    yandex: { label: "Yandex", desc: tr("多语言及长文翻译，免配置", "Key-free multilingual and long-text translation") },
    iciba: { label: tr("金山", "Kingsoft"), desc: tr("中短文本，国内访问较快", "Short and medium text; fast access in China") },
    google: { label: "Google", desc: tr("多语言翻译，部分网络需代理", "Multilingual translation; some networks require a proxy") },
    mymemory: { label: "MyMemory", desc: tr("社区翻译记忆库", "Community translation memory") },
    llm: { label: "AI", desc: tr("自配大模型，流式输出（见 AI 模型页）", "Bring your own model with streaming output") },
  }[id] ?? { label: id, desc: "" };
}

function palettes(): { id: ColorScheme; label: string }[] {
  return [
    { id: "jade", label: tr("松石", "Jade") },
    { id: "indigo", label: tr("靛蓝", "Indigo") },
    { id: "plum", label: tr("岩紫", "Plum") },
  ];
}

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

function pageMeta(tab: Tab): { eyebrow: string; title: string; desc: string } {
  return {
    general: {
      eyebrow: "GENERAL",
      title: tr("通用", "General"),
      desc: tr("管理划词方式、快捷键与系统行为", "Manage capture, shortcuts, and system behavior"),
    },
    translate: {
      eyebrow: "TRANSLATION",
      title: tr("翻译", "Translation"),
      desc: tr("选择结果布局并安排翻译引擎优先级", "Choose a result layout and translation engine priority"),
    },
    network: {
      eyebrow: "NETWORK",
      title: tr("网络", "Network"),
      desc: tr("配置翻译服务使用的连接与代理方式", "Configure connectivity and proxy settings"),
    },
    llm: {
      eyebrow: "AI MODEL",
      title: tr("AI 模型", "AI Model"),
      desc: tr("接入兼容 OpenAI 协议的大语言模型", "Connect an OpenAI-compatible language model"),
    },
  }[tab];
}

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
    if (!s()) return;
    applyLanguage(s()!.language ?? "system");
    applyTheme(s()!.theme ?? "system", s()!.color_scheme ?? "indigo");
    document.title = tr("TYL 设置", "TYL Settings");
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
        .catch((e) => showError(localizedError(e, "Could not move the window")));
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
      applyLanguage(initial.language ?? "system");
      setS(initial);
      setSaved(JSON.stringify(initial));
      setSavedGpu(initial.gpu_acceleration ?? false);
      setGpuStatus(await invoke<GpuStatus>("get_gpu_status"));
      setAutostart(await invoke<boolean>("get_autostart"));
    } catch (e) {
      showError(localizedError(e, "Could not load settings"));
    }
  });

  const toggleAutostart = async (on: boolean) => {
    setAutostart(on);
    try {
      await invoke("set_autostart", { enabled: on });
      flashStatus(on ? tr("开机自启已开启", "Launch at startup enabled") : tr("开机自启已关闭", "Launch at startup disabled"));
    } catch (e) {
      setAutostart(!on);
      showError(localizedError(e, "Could not update startup setting"));
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
    if (p.llm || p.language) setLlmTest(null);
    window.clearTimeout(statusTimer);
    setStatus("");
    setStatusError(false);
    setS((prev) => (prev ? { ...prev, ...p } : prev));
  };

  const patchDictionary = (p: Partial<DictionarySettings>) => {
    patch({ dictionary: { ...s()!.dictionary, ...p } });
  };

  const patchLanguageRouting = (p: Partial<LanguageRoutingSettings>) => {
    patch({ language_routing: { ...s()!.language_routing, ...p } });
  };

  const setRoutingLanguage = (
    field: "primary" | "secondary" | "source" | "target",
    value: TranslationLanguage,
  ) => {
    const routing = s()!.language_routing;
    const paired = field === "primary" ? "secondary"
      : field === "secondary" ? "primary"
        : field === "source" ? "target" : "source";
    const next: LanguageRoutingSettings = { ...routing, [field]: value };
    if (next[paired] === value) next[paired] = routing[field];
    patch({ language_routing: next });
  };

  const testLlm = async () => {
    if (!s() || testingLlm()) return;
    const config = { ...s()!.llm };
    setTestingLlm(true);
    setLlmTest(null);
    try {
      const elapsedMs = await invoke<number>("test_llm", { config });
      const text = `${tr("连接成功，已收到译文", "Connected and received a translation")} (${(elapsedMs / 1000).toFixed(1)} ${tr("秒", "s")})`;
      if (JSON.stringify(config) === JSON.stringify(s()!.llm)) setLlmTest({ text, error: false });
    } catch (error) {
      if (JSON.stringify(config) === JSON.stringify(s()!.llm)) {
        setLlmTest({ text: localizedError(error, "Could not connect to the AI provider"), error: true });
      }
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
      showError(tr("热键不能为空", "The shortcut cannot be empty"));
      return;
    }
    if (cur.proxy.mode === "manual") {
      const proxy = cur.proxy.url.trim();
      if (!proxy) {
        showError(tr("代理地址不能为空（或改用其他模式）", "Enter a proxy address or choose another mode"));
        return;
      }
      if (!/^(https?|socks5):\/\//i.test(proxy)) {
        showError(tr("代理地址需以 http://、https:// 或 socks5:// 开头", "The proxy address must start with http://, https://, or socks5://"));
        return;
      }
    }
    try {
      setSaving(true);
      setStatus("");
      setStatusError(false);
      await invoke("save_settings", { settings: cur });
      setSaved(JSON.stringify(cur));
      setSavedGpu(cur.gpu_acceleration);
      flashStatus(restartRequired()
        ? tr("已保存，重启后切换渲染模式", "Saved. Restart TYL to change rendering mode")
        : tr("设置已保存", "Settings saved"));
    } catch (e) {
      showError(localizedError(e, "Could not save settings"));
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
          showError(tr("请加至少一个修饰键（Ctrl/Alt/Shift）", "Add at least one modifier (Ctrl/Alt/Shift)"));
          return;
        }
        patch({ hotkey: [...mods, key].join("+") });
      } else {
        showError(`${tr("不支持的键", "Unsupported key")}: ${k}`);
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

  const tabs = (): { id: Tab; label: string; icon: IconName }[] => [
    { id: "general", label: tr("通用", "General"), icon: "settings" },
    { id: "translate", label: tr("翻译", "Translation"), icon: "translate" },
    { id: "network", label: tr("网络", "Network"), icon: "network" },
    { id: "llm", label: tr("AI 模型", "AI Model"), icon: "ai" },
  ];

  return (
    <>
      <div class="window-chrome">
        <button
          class="window-close"
          aria-label={tr("关闭设置", "Close settings")}
          title={tr("关闭设置", "Close settings")}
          onClick={() =>
            void getCurrentWindow()
              .close()
              .catch((e) => showError(localizedError(e, "Could not close the settings window")))
          }
        >
          <Icon name="close" size={16} />
        </button>
      </div>
      <Show
        when={s()}
        fallback={<div class="loading">{status() || tr("正在加载设置…", "Loading settings…")}</div>}
      >
        {(cur) => (
          <div class="settings">
            {/* 侧边导航 */}
            <nav class="nav" onPointerDown={beginWindowDrag}>
              <div class="nav-brand" title={tr("拖动此处移动窗口", "Drag here to move the window")}>
                <Logo class="nav-logo" label={tr("TYL 标志", "TYL logo")} />
                <div class="nav-brand-copy">
                  <div class="nav-title">TYL</div>
                  <div class="nav-sub">{tr("划词翻译", "Selection Translator")}</div>
                </div>
              </div>
              <div class="nav-items">
                <For each={tabs()}>
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
                <span>{tr("阅读，不止一种语言", "Read beyond one language")}</span>
                <small>TYL · 0.1.0</small>
              </div>
            </nav>

            {/* 内容区 */}
            <div class="content">
              <header
                class="page-header"
                onPointerDown={beginWindowDrag}
              >
                <div class="page-head" title={tr("拖动此处移动窗口", "Drag here to move the window")}>
                  <div class="page-eyebrow">{pageMeta(tab()).eyebrow}</div>
                  <h1>{pageMeta(tab()).title}</h1>
                  <p>{pageMeta(tab()).desc}</p>
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
                    <span>{saving()
                      ? tr("正在保存…", "Saving…")
                      : statusError()
                        ? tr("需要检查", "Needs attention")
                        : status() || (dirty() ? tr("未保存", "Unsaved") : tr("已保存", "Saved"))}</span>
                  </div>
                  <button
                    class="primary"
                    disabled={!dirty() || saving()}
                    onClick={() => void save()}
                  >
                    {saving() ? tr("正在保存…", "Saving…") : tr("保存更改", "Save changes")}
                  </button>
                </div>
              </header>
              <Show when={statusError()}>
                <div class="settings-message" role="alert">{status()}</div>
              </Show>
              <Show when={restartRequired()}>
                <div class="restart-notice" role="status">
                  <strong>{tr("渲染模式将在重启后生效", "Rendering changes take effect after restart")}</strong>
                  <span>{tr("请从托盘退出 TYL 后重新打开。仅关闭设置窗口不会重启应用。", "Quit TYL from the tray, then open it again. Closing Settings alone does not restart the app.")}</span>
                </div>
              </Show>
              <div class="content-scroll" ref={contentScroll}>
                <Show when={tab() === "general"}>
                  <section class="group">
                    <div class="group-title">{tr("外观", "Appearance")}</div>
                    <div class="row">
                      <div class="row-text">
                        <label class="label" for="ui-language">{tr("界面语言", "Language")}</label>
                        <div class="hint">{tr("默认跟随系统，也可固定为指定语言", "Follow the system by default, or choose a language")}</div>
                      </div>
                      <div class="select-wrap language-select-wrap">
                        <select
                          id="ui-language"
                          class="input language-select"
                          value={cur().language ?? "system"}
                          onChange={(event) => patch({ language: event.currentTarget.value as LanguagePreference })}
                        >
                          <option value="system">{tr("跟随系统", "System default")}</option>
                          <option value="zh-CN">简体中文</option>
                          <option value="en-US">English</option>
                        </select>
                        <span class="select-chevron"><Icon name="down" size={15} /></span>
                      </div>
                    </div>
                    <div class="row theme-row">
                      <div class="row-text">
                        <div class="label">{tr("界面主题", "Theme")}</div>
                        <div class="hint">{tr("弹窗与设置窗口使用相同的配色", "Use the same theme for the popup and Settings")}</div>
                      </div>
                      <div class="theme-options">
                        <For
                          each={
                            [
                              {
                                id: "system",
                                label: tr("跟随系统", "System"),
                                icon: "monitor",
                              },
                              { id: "light", label: tr("浅色", "Light"), icon: "sun" },
                              { id: "dark", label: tr("深色", "Dark"), icon: "moon" },
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
                        <div class="label">{tr("主题色", "Accent color")}</div>
                        <div class="hint">{tr("用于强调色、选中状态与操作反馈", "Used for highlights, selections, and feedback")}</div>
                      </div>
                      <div class="palette-options" aria-label={tr("主题色", "Accent color")}>
                        <For each={palettes()}>
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
                    <div class="group-title">{tr("划词热键", "Capture shortcut")}</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">{tr("触发组合键", "Keyboard shortcut")}</div>
                        <div class="hint">{tr("保存后重启应用生效", "Restart the app after saving")}</div>
                      </div>
                      <button
                        class={`hotkey-btn ${recording() ? "recording" : ""}`}
                        onClick={() =>
                          recording() ? cancelRecord() : startRecord()
                        }
                      >
                        {recording()
                          ? tr("按下组合键…（Esc 取消）", "Press a key combination… (Esc to cancel)")
                          : formatHotkey(cur().hotkey) || tr("未设置", "Not set")}
                      </button>
                    </div>
                    <p class="hint">
                      {tr("建议 Alt/Shift 系组合，避免与常用软件冲突。", "Alt or Shift combinations are less likely to conflict with other apps.")}
                    </p>
                  </section>

                  <section class="group">
                    <div class="group-title">{tr("取词", "Text capture")}</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">{tr("显示选中原文", "Show selected text")}</div>
                        <div class="hint">{tr("默认关闭，为译文保留更多阅读空间", "Off by default to leave more room for translations")}</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={cur().show_source}
                        aria-label={tr("显示选中原文", "Show selected text")}
                        onChange={(e) =>
                          patch({ show_source: e.currentTarget.checked })
                        }
                      />
                    </div>
                    <p class="hint capture-hint">
                      {tr("优先直接读取选区，必要时临时使用剪贴板并还原。终端仅直接读取，不模拟 Ctrl+C，避免中断正在运行的命令。", "TYL reads the selection directly when possible. If needed, it temporarily uses and restores the clipboard. In terminals, it never simulates Ctrl+C, so running commands stay safe.")}
                    </p>
                  </section>

                  <section class="group">
                    <div class="group-title">{tr("系统", "System")}</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">{tr("GPU 硬件加速", "GPU acceleration")}</div>
                        <div class="hint" id="gpu-hint">{tr("默认关闭以减少显存占用；开启可利用 GPU 渲染。保存后重启生效。", "Off by default to reduce video memory. Enable for GPU rendering; restart after saving.")}</div>
                        <Show when={gpuStatus()}>
                          <div class="hint gpu-current" id="gpu-current">
                            {tr("本次启动", "This session")}: {gpuStatus()!.software ? tr("软件渲染", "Software rendering") : tr("GPU 硬件加速", "GPU acceleration")}
                            {gpuStatus()!.external_override ? tr(" · 外部启动参数禁用了 GPU", " · GPU disabled by launch arguments") : ""}
                          </div>
                        </Show>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={cur().gpu_acceleration ?? false}
                        aria-label={tr("GPU 硬件加速", "GPU acceleration")}
                        aria-describedby="gpu-hint gpu-current"
                        onChange={(e) => patch({ gpu_acceleration: e.currentTarget.checked })}
                      />
                    </div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">{tr("开机自启", "Launch at startup")}</div>
                        <div class="hint">{tr("登录 Windows 后自动驻留后台", "Run in the background after signing in to Windows")}</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        checked={autostart()}
                        aria-label={tr("开机自启", "Launch at startup")}
                        onChange={(e) =>
                          void toggleAutostart(e.currentTarget.checked)
                        }
                      />
                    </div>
                    <div class="row">
                      <div class="row-text">
                        <label class="label" for="log-level">{tr("日志等级", "Log level")}</label>
                        <div class="hint">{tr("默认记录运行状态和异常，保存后立即生效", "Records runtime status and errors by default; applies immediately")}</div>
                      </div>
                      <div class="log-level-wrap">
                        <select id="log-level" class="input log-level" value={cur().log_level ?? "info"} onChange={(e) => patch({ log_level: e.currentTarget.value as SettingsData["log_level"] })}>
                          <option value="off">{tr("关闭", "Off")} · OFF</option>
                          <option value="error">{tr("错误", "Error")} · ERROR</option>
                          <option value="warn">{tr("警告", "Warning")} · WARN</option>
                          <option value="info">{tr("信息", "Info")} · INFO</option>
                          <option value="debug">{tr("调试", "Debug")} · DEBUG</option>
                          <option value="trace">{tr("跟踪", "Trace")} · TRACE</option>
                        </select>
                        <span class="log-level-chevron"><Icon name="down" size={15} /></span>
                      </div>
                    </div>
                    <p class="hint">{tr("日志写入程序目录的 tyl.log；目录不可写时使用系统临时目录。调试/跟踪用于排查问题，不记录密钥或选中文本。", "Logs are written to tyl.log beside the app, or to the system temporary folder if that location is read-only. Debug and Trace help diagnose issues; keys and selected text are never logged.")}</p>
                  </section>
                </Show>

                <Show when={tab() === "translate"}>
                  <section class="group language-routing-group">
                    <div class="group-title">{tr("翻译方向", "Translation direction")}</div>
                    <div class="seg routing-modes" role="radiogroup" aria-label={tr("翻译方向模式", "Translation direction mode")}>
                      {[
                        ["smart", tr("智能互译", "Smart")],
                        ["fixed_target", tr("固定目标", "Fixed target")],
                        ["fixed_pair", tr("固定语言对", "Fixed pair")],
                      ].map(([mode, label]) => (
                        <button
                          type="button"
                          role="radio"
                          aria-checked={cur().language_routing.mode === mode}
                          classList={{ active: cur().language_routing.mode === mode }}
                          onClick={() => patchLanguageRouting({ mode: mode as LanguageRoutingSettings["mode"] })}
                        >
                          {label}
                        </button>
                      ))}
                    </div>
                    <Show when={cur().language_routing.mode === "smart"}>
                      <p class="hint routing-hint">
                        {tr("主语言文本译为第二语言，其他语言统一译为主语言。默认即“中文 → 英文，其他 → 中文”。", "Primary-language text goes to the secondary language; every other language goes to the primary language.")}
                      </p>
                      <div class="language-pair">
                        <LanguageSelect id="routing-primary" label={tr("主语言", "Primary")} value={cur().language_routing.primary} onChange={(value) => setRoutingLanguage("primary", value)} />
                        <span class="language-pair-mark" aria-hidden="true">⇄</span>
                        <LanguageSelect id="routing-secondary" label={tr("第二语言", "Secondary")} value={cur().language_routing.secondary} onChange={(value) => setRoutingLanguage("secondary", value)} />
                      </div>
                    </Show>
                    <Show when={cur().language_routing.mode === "fixed_target"}>
                      <p class="hint routing-hint">{tr("自动识别原文语言，并始终翻译为指定语言。", "Detect the source automatically and always translate to the selected language.")}</p>
                      <div class="language-pair">
                        <div class="routing-auto-language">
                          <span>{tr("源语言", "Source")}</span>
                          <strong>{languageLabel("auto")}</strong>
                        </div>
                        <span class="language-pair-mark" aria-hidden="true">→</span>
                        <LanguageSelect id="routing-target" label={tr("目标语言", "Target")} value={cur().language_routing.target} onChange={(value) => setRoutingLanguage("target", value)} />
                      </div>
                    </Show>
                    <Show when={cur().language_routing.mode === "fixed_pair"}>
                      <p class="hint routing-hint">{tr("原文和译文语言均固定，适合稳定的双语工作流。", "Fix both languages for a predictable bilingual workflow.")}</p>
                      <div class="language-pair">
                        <LanguageSelect id="routing-source" label={tr("源语言", "Source")} value={cur().language_routing.source} onChange={(value) => setRoutingLanguage("source", value)} />
                        <span class="language-pair-mark" aria-hidden="true">→</span>
                        <LanguageSelect id="routing-pair-target" label={tr("目标语言", "Target")} value={cur().language_routing.target} onChange={(value) => setRoutingLanguage("target", value)} />
                      </div>
                    </Show>
                  </section>

                  <section class="group display-group">
                    <div class="group-title">{tr("结果呈现", "Result layout")}</div>
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
                          <strong>{tr("标签切换", "Tabs")}</strong>
                          <small>{tr("首个引擎立即翻译，其余按需请求", "Translate with the first engine; load others on demand")}</small>
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
                          <strong>{tr("全部展开", "Show all")}</strong>
                          <small>{tr("并行请求全部引擎，纵向对照结果", "Run every engine in parallel and compare vertically")}</small>
                        </span>
                        <span class="mode-check" aria-hidden="true">✓</span>
                      </button>
                    </div>
                  </section>

                  <section class="group">
                    <div class="group-title">{tr("翻译引擎", "Translation engines")}</div>
                    <p class="hint">
                      {cur().result_display === "tabs"
                        ? tr("排在第一位的引擎划词后立即翻译，其余引擎切换时按需请求。", "The first engine starts immediately; other engines load when selected.")
                        : tr("划词后并行请求所有已启用引擎，并按以下顺序展示结果。", "All enabled engines run in parallel and appear in this order.")}
                      {tr(" 使用箭头调整顺序。", " Use the arrows to change the order.")}
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
                                {engineMeta(id).label}
                                <Show when={i() === 0}>
                                  <span class="preferred-badge">
                                    {cur().result_display === "tabs"
                                      ? tr("首选引擎", "Primary")
                                      : tr("优先展示", "Shown first")}
                                  </span>
                                </Show>
                              </div>
                              <div class="hint">
                                {engineMeta(id).desc}
                              </div>
                            </div>
                            <div class="engine-ops">
                              <button
                                class="icon-btn"
                                title={tr("上移", "Move up")}
                                disabled={i() === 0}
                                onClick={() => moveEngine(i(), -1)}
                              >
                                <Icon name="up" size={15} />
                              </button>
                              <button
                                class="icon-btn"
                                title={tr("下移", "Move down")}
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
                                title={tr("关闭（至少保留一个引擎）", "Disable (keep at least one engine)")}
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
                            + {engineMeta(id).label}
                          </button>
                        )}
                      </For>
                    </div>
                  </section>

                  <section class="group dictionary-group">
                    <div class="group-title">{tr("词典服务", "Dictionary")}</div>
                    <div class="row">
                      <div class="row-text">
                        <div class="label">{tr("单词词典卡片", "Word dictionary card")}</div>
                        <div class="hint">{tr("选中英文单词时显示音标、词性与释义", "Show pronunciation, part of speech, and definitions for English words")}</div>
                      </div>
                      <input
                        type="checkbox"
                        class="switch"
                        aria-label={tr("单词词典卡片", "Word dictionary card")}
                        checked={cur().dictionary.enabled}
                        onChange={(e) => patchDictionary({ enabled: e.currentTarget.checked })}
                      />
                    </div>
                    <div class="row dictionary-provider-row" classList={{ disabled: !cur().dictionary.enabled }}>
                      <div class="row-text">
                        <label class="label" for="dictionary-provider">{tr("词典数据源", "Dictionary provider")}</label>
                        <div class="hint">
                          {cur().dictionary.provider === "auto"
                            ? tr("自动按有道 → 金山 → 必应的顺序降级", "Automatically try Youdao → Kingsoft → Bing")
                            : tr("固定使用所选词典，失败时不切换数据源", "Use only the selected dictionary with no fallback")}
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
                          <option value="auto">{tr("自动推荐", "Automatic")}</option>
                          <option value="youdao">{tr("有道词典", "Youdao Dictionary")}</option>
                          <option value="iciba">{tr("金山词典", "Kingsoft Dictionary")}</option>
                          <option value="bing">{tr("必应词典", "Bing Dictionary")}</option>
                        </select>
                        <span class="select-chevron"><Icon name="down" size={15} /></span>
                      </div>
                    </div>
                  </section>
                </Show>

                <Show when={tab() === "network"}>
                  <section class="group">
                    <div class="group-title">{tr("网络代理", "Network proxy")}</div>
                    <div class="seg">
                      {[
                        ["off", tr("直连", "Direct")],
                        ["system", tr("跟随系统", "System")],
                        ["manual", tr("手动", "Manual")],
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
                        placeholder={tr("http://127.0.0.1:7890 或 socks5://…", "http://127.0.0.1:7890 or socks5://…")}
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
                      <p class="hint">{tr("翻译与词典请求全部经此代理。", "All translation and dictionary requests use this proxy.")}</p>
                    </Show>
                    <Show when={cur().proxy.mode === "system"}>
                      <p class="hint">
                        {tr("自动使用 Windows 系统代理（Internet 选项），随系统开关联动。", "Automatically use the Windows system proxy (Internet Options) and follow its current state.")}
                      </p>
                    </Show>
                  </section>
                </Show>

                <Show when={tab() === "llm"}>
                  <section class="group">
                    <div class="group-title">{tr("AI 模型（OpenAI 兼容）", "AI Model (OpenAI compatible)")}</div>
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
                      {tr("支持供应商提供的 Base URL 或完整 /chat/completions 地址。", "Supports a provider Base URL or a complete /chat/completions endpoint.")}
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
                          aria-label={showApiKey() ? tr("隐藏 API Key", "Hide API Key") : tr("显示 API Key", "Show API Key")}
                          title={showApiKey() ? tr("隐藏 API Key", "Hide API Key") : tr("显示 API Key", "Show API Key")}
                          aria-pressed={showApiKey()}
                          onClick={() => setShowApiKey(!showApiKey())}>
                          <Icon name={showApiKey() ? "eyeOff" : "eye"} size={18} />
                        </button>
                      </div>
                    </div>
                    <label class="field">
                      <span class="field-label">{tr("模型", "Model")}</span>
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
                      {tr("请在翻译页启用 AI 引擎；标签模式下，首选 AI 失败后会尝试其他启用的引擎。", "Enable the AI engine on the Translation page. In tab mode, TYL tries another enabled engine if the primary AI engine fails.")}
                    </p>
                    <div class="connection-test">
                      <button class="test-connection" disabled={testingLlm() || !cur().llm.api_key.trim() || !cur().llm.model.trim()} onClick={() => void testLlm()}>
                        {testingLlm() ? tr("正在测试…", "Testing…") : tr("测试连接", "Test connection")}
                      </button>
                      <span class="hint">{tr("使用当前填写的配置发送固定短句，不保存设置；会消耗少量 API 额度。", "Sends a fixed short sentence using the current fields without saving. This uses a small amount of API credit.")}</span>
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

function LanguageSelect(props: {
  id: string;
  label: string;
  value: TranslationLanguage;
  onChange: (value: TranslationLanguage) => void;
}) {
  return (
    <label class="routing-language" for={props.id}>
      <span>{props.label}</span>
      <span class="select-wrap">
        <select
          id={props.id}
          class="input"
          value={props.value}
          onChange={(event) =>
            props.onChange(event.currentTarget.value as TranslationLanguage)
          }
        >
          <For each={TRANSLATION_LANGUAGES}>
            {(language) => (
              <option value={language}>{languageLabel(language)}</option>
            )}
          </For>
        </select>
        <span class="select-chevron">
          <Icon name="down" size={15} />
        </span>
      </span>
    </label>
  );
}
