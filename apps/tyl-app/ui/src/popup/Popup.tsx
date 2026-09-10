import {
  createSignal,
  onCleanup,
  onMount,
  For,
  Show,
  createEffect,
} from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Icon } from "../shared/Icon";
import { applyTheme, type ColorScheme, type Theme } from "../shared/theme";
import { installContextMenuGuard } from "../shared/contextMenu";
import logo from "../shared/logo.svg";

interface CapturedPayload {
  request_id: number;
  text: string;
  source: { kind: string; restored?: boolean };
  elapsed_ms: number;
  target_exe: string | null;
  engines: string[];
  result_display: "tabs" | "stacked";
  grow_upward: boolean;
  theme: Theme;
  color_scheme: ColorScheme;
  show_source: boolean;
  is_word: boolean;
  dictionary?: boolean;
  can_replace: boolean;
  replace_requires_verification?: boolean;
  benchmark?: boolean;
}
interface TranslatePayload {
  request_id: number;
  phase: string;
  text: string;
  service: string;
}
interface DictEntry {
  word: string;
  phonetic_us: string | null;
  phonetic_uk: string | null;
  senses: { pos: string; definitions: string[] }[];
  exam_types?: string[];
}
interface EngineState {
  service: string;
  status: "loading" | "streaming" | "done" | "error";
  text: string;
  error?: string;
}
const LABELS: Record<string, string> = {
  bing: "必应",
  youdao: "有道",
  transmart: "腾讯",
  yandex: "Yandex",
  iciba: "金山",
  mymemory: "MyMemory",
  google: "Google",
  llm: "AI",
};

export function Popup() {
  const [captured, setCaptured] = createSignal<CapturedPayload | null>(null);
  const [engines, setEngines] = createSignal<EngineState[]>([]);
  const [active, setActive] = createSignal("");
  const [dict, setDict] = createSignal<DictEntry | null>(null);
  const [dictPending, setDictPending] = createSignal(false);
  const [wordTranslations, setWordTranslations] = createSignal(false);
  const [sourceExpanded, setSourceExpanded] = createSignal(false);
  const [collapsed, setCollapsed] = createSignal<Set<string>>(new Set());
  const [visible, setVisible] = createSignal(false);
  const [entered, setEntered] = createSignal(false);
  const [notice, setNotice] = createSignal("");
  const [noticeError, setNoticeError] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [speaking, setSpeaking] = createSignal("");
  const [audioPhase, setAudioPhase] = createSignal<
    "idle" | "loading" | "playing"
  >("idle");
  const [width, setWidth] = createSignal(window.innerWidth);
  const unlisteners: UnlistenFn[] = [];
  const pending = new Map<number, TranslatePayload[]>();
  const audioCache = new Map<string, Blob>();
  let popupEl: HTMLDivElement | undefined;
  let resizeTimer = 0,
    noticeTimer = 0,
    enterFrame = 0,
    fitFrame = 0,
    lastHeight = 0,
    lastFitWidth = 0;
  let audio: HTMLAudioElement | undefined;
  let engineTabsEl: HTMLDivElement | undefined;
  let audioUrl: string | undefined;
  let audioToken = 0;
  let audioTimer = 0;
  let dragOrigin: { x: number; y: number } | undefined;
  const beginDrag = (event: PointerEvent) => {
    if (event.button !== 0 || (event.target as Element).closest("button"))
      return;
    event.preventDefault();
    dragOrigin = { x: event.clientX, y: event.clientY };
  };
  const engineIds = () => captured()?.engines ?? [];
  const current = () => engines().find((e) => e.service === active());
  const dictionaryView = () => (dictPending() || Boolean(dict())) && !wordTranslations();
  const stacked = () =>
    !dictionaryView() && captured()?.result_display === "stacked";
  const longSource = () => (captured()?.text.length ?? 0) > 100;
  const resultText = () => (dictionaryView() ? "" : (current()?.text ?? ""));
  const allResults = () =>
    engineIds()
      .map((id) => {
        const text = engines().find((e) => e.service === id)?.text;
        return text ? `${LABELS[id] ?? id}\n${text}` : "";
      })
      .filter(Boolean)
      .join("\n\n");
  const feedback = (message: string, error = false) => {
    window.clearTimeout(noticeTimer);
    setNotice(message);
    setNoticeError(error);
    noticeTimer = window.setTimeout(() => setNotice(""), error ? 6000 : 2200);
  };
  const stopAudio = () => {
    window.clearTimeout(audioTimer);
    audioToken++;
    audio?.pause();
    audio = undefined;
    if (audioUrl) URL.revokeObjectURL(audioUrl);
    audioUrl = undefined;
    setSpeaking("");
    setAudioPhase("idle");
  };
  const hide = async () => {
    stopAudio();
    setEntered(false);
    setVisible(false);
    await invoke("hide_popup");
  };
  const copy = async (text: string) => {
    const id = captured()?.request_id;
    try {
      await invoke("copy_translation", { text });
      if (id === captured()?.request_id) feedback("已复制");
    } catch (e) {
      if (id === captured()?.request_id) feedback(String(e), true);
    }
  };
  const replace = async (text: string) => {
    const payload = captured();
    if (!payload || !payload.can_replace || !text || busy()) return;
    setBusy(true);
    try {
      await invoke("replace_selection", {
        text,
        request_id: payload.request_id,
      });
      if (payload.request_id === captured()?.request_id) {
        setVisible(false);
        stopAudio();
      }
    } catch (e) {
      if (payload.request_id === captured()?.request_id) {
        // Returning to the editor can hide the popup before native verification fails.
        // The backend refocuses it on failure; restore the UI so the reason is visible.
        setVisible(true);
        setEntered(true);
        feedback(String(e), true);
      }
    } finally {
      setBusy(false);
    }
  };
  const pronounce = async (accent: "uk" | "us") => {
    if (speaking() === accent) {
      stopAudio();
      return;
    }
    stopAudio();
    const token = audioToken;
    const word = dict()?.word ?? captured()?.text;
    if (!word) return;
    setSpeaking(accent);
    setAudioPhase("loading");
    audioTimer = window.setTimeout(() => {
      if (token === audioToken) {
        stopAudio();
        feedback("发音加载超时，请重试", true);
      }
    }, 12000);
    try {
      const key = `${word}:${accent}`;
      let blob = audioCache.get(key);
      if (!blob) {
        const bytes = await invoke<number[]>("pronunciation_audio", {
          word,
          accent,
        });
        blob = new Blob([new Uint8Array(bytes)], { type: "audio/mpeg" });
        if (audioCache.size >= 24)
          audioCache.delete(audioCache.keys().next().value!);
        audioCache.set(key, blob);
      }
      if (token !== audioToken) return;
      audioUrl = URL.createObjectURL(blob);
      audio = new Audio(audioUrl);
      audio.onended = () => {
        if (token === audioToken) stopAudio();
      };
      audio.onerror = () => {
        if (token === audioToken) {
          stopAudio();
          feedback("发音播放失败，请重试", true);
        }
      };
      await audio.play();
      if (token !== audioToken) return;
      window.clearTimeout(audioTimer);
      setAudioPhase("playing");
      audioTimer = window.setTimeout(() => {
        if (token === audioToken) stopAudio();
      }, 20000);
      void invoke("frontend_log", {
        message: `[audio] ${accent} playback started`,
      }).catch(() => {});
    } catch (e) {
      if (token === audioToken) {
        stopAudio();
        feedback(String(e), true);
      }
    }
  };
  const applyEvent = (p: TranslatePayload) => {
    if (p.phase === "dict") {
      try {
        setDict(JSON.parse(p.text));
      } catch {
        feedback("词典暂不可用，正在加载引擎译文", true);
        showWordTranslations();
      }
      setDictPending(false);
      return;
    }
    if (p.service === "dict") {
      if (p.phase === "error") {
        setDictPending(false);
        showWordTranslations();
      }
      return;
    }
    setEngines((prev) => {
      const previous = prev.find((e) => e.service === p.service);
      // Final text wins over delayed start/chunk notifications.
      if (previous?.status === "done") return prev;
      const next: EngineState = {
        ...(previous ?? { service: p.service, status: "loading", text: "" }),
      };
      if (p.phase === "start") {
        next.status = "loading";
        next.text = "";
        next.error = undefined;
      }
      if (p.phase === "chunk") {
        next.status = "streaming";
        next.text += p.text;
      }
      if (p.phase === "done") {
        next.status = p.text.trim() ? "done" : "error";
        next.text = p.text;
        next.error = p.text.trim() ? undefined : "empty";
      }
      if (p.phase === "error") {
        next.status = "error";
        next.error = p.text;
      }
      return previous
        ? prev.map((e) => (e.service === p.service ? next : e))
        : [...prev, next];
    });
    if (
      p.phase === "done" &&
      p.service !== active() &&
      current()?.status === "error"
    )
      setActive(p.service);
  };
  const onTranslate = (p: TranslatePayload) => {
    const id = captured()?.request_id;
    if (id === undefined || p.request_id > id) {
      const events = pending.get(p.request_id) ?? [];
      events.push(p);
      pending.set(p.request_id, events);
      if (pending.size > 4) pending.delete(Math.min(...pending.keys()));
    } else if (p.request_id === id) applyEvent(p);
  };
  const present = (payload: CapturedPayload) => {
    stopAudio();
    window.clearTimeout(noticeTimer);
    window.cancelAnimationFrame(enterFrame);
    lastHeight = 0;
    setCaptured(payload);
    applyTheme(payload.theme ?? "system", payload.color_scheme ?? "indigo");
    setEngines([]);
    setActive(payload.engines?.[0] ?? "");
    setDict(null);
    setDictPending(payload.is_word && payload.dictionary !== false);
    setWordTranslations(false);
    setSourceExpanded(false);
    setCollapsed(new Set<string>());
    setNotice("");
    setVisible(true);
    setEntered(false);
    enterFrame = requestAnimationFrame(() => {
      enterFrame = requestAnimationFrame(() => {
        setEntered(true);
        if (payload.benchmark) {
          void invoke("frontend_log", {
            message: `[memory-bench] frame-ready id=${payload.request_id}`,
          });
        }
      });
    });
    const events = pending.get(payload.request_id) ?? [];
    for (const id of pending.keys())
      if (id <= payload.request_id) pending.delete(id);
    events.forEach(applyEvent);
  };
  const ensureEngine = (service: string) => {
    const payload = captured();
    if (!payload || !payload.engines.includes(service) || engines().some((e) => e.service === service)) return;
    setEngines((prev) => [...prev, { service, status: "loading", text: "" }]);
    void invoke("translate_one", {
      text: payload.text,
      engine: service,
      request_id: payload.request_id,
    }).catch((e) =>
      onTranslate({
        request_id: payload.request_id,
        service,
        phase: "error",
        text: String(e),
      }),
    );
  };
  const switchEngine = (service: string) => {
    setActive(service);
    ensureEngine(service);
    requestAnimationFrame(() => {
      engineTabsEl
        ?.querySelector<HTMLButtonElement>(`button[data-engine="${CSS.escape(service)}"]`)
        ?.scrollIntoView({ behavior: "smooth", block: "nearest", inline: "nearest" });
    });
  };
  const scrollEngineTabs = (event: WheelEvent) => {
    const tabs = engineTabsEl;
    if (!tabs || tabs.scrollWidth <= tabs.clientWidth) return;
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY)
      ? event.deltaX
      : event.deltaY;
    if (!delta) return;
    event.preventDefault();
    tabs.scrollBy({ left: delta, behavior: "smooth" });
  };
  const showWordTranslations = () => {
    setWordTranslations(true);
    if (captured()?.result_display === "stacked") {
      engineIds().forEach(ensureEngine);
    } else {
      ensureEngine(active() || engineIds()[0]);
    }
  };
  const toggleResult = (service: string) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(service)) next.delete(service);
      else next.add(service);
      return next;
    });
  };
  createEffect(() => {
    void engines();
    void dict();
    void dictPending();
    void active();
    void sourceExpanded();
    void collapsed();
    void wordTranslations();
    void width();
    if (!captured() || !visible() || resizeTimer) return;
    // Throttle rather than debounce: continuous streaming must still resize.
    resizeTimer = window.setTimeout(() => {
      fitFrame = requestAnimationFrame(() => {
        resizeTimer = 0;
        const payload = captured();
        if (!payload || !visible()) return;
        const fitWidth = width();
        const height = measureNaturalHeight(popupEl);
        if (
          height > 0 &&
          (Math.abs(height - lastHeight) >= 2 || fitWidth !== lastFitWidth)
        ) {
          lastHeight = height;
          lastFitWidth = fitWidth;
          void invoke("fit_popup_height", {
            height,
            viewport_width: fitWidth,
            grow_upward: payload.grow_upward,
          }).catch(() => {});
        }
      });
    }, 55);
  });
  onMount(async () => {
    unlisteners.push(installContextMenuGuard());
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
        5
      )
        return;
      cancelDrag();
      void getCurrentWindow()
        .startDragging()
        .catch(() => feedback("暂时无法移动窗口，请重试", true));
    };
    window.addEventListener("pointermove", moveDrag);
    window.addEventListener("pointerup", cancelDrag);
    window.addEventListener("pointercancel", cancelDrag);
    unlisteners.push(() => {
      window.removeEventListener("pointermove", moveDrag);
      window.removeEventListener("pointerup", cancelDrag);
      window.removeEventListener("pointercancel", cancelDrag);
    });
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        void hide();
      }
      if (
        (e.ctrlKey || e.metaKey) &&
        e.key.toLowerCase() === "c" &&
        !window.getSelection()?.toString()
      ) {
        const text = stacked() ? allResults() : resultText();
        if (text) {
          e.preventDefault();
          void copy(text);
        }
      }
    };
    const onResize = () => setWidth(window.innerWidth);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", onResize);
    unlisteners.push(
      () => window.removeEventListener("keydown", onKey),
      () => window.removeEventListener("resize", onResize),
    );
    unlisteners.push(
      await listen<CapturedPayload>("tyl://captured", (e) =>
        present(e.payload),
      ),
    );
    unlisteners.push(
      await listen<TranslatePayload>("tyl://translate", (e) =>
        onTranslate(e.payload),
      ),
    );
    unlisteners.push(
      await listen("tyl://popup-hidden", () => {
        stopAudio();
        cancelDrag();
        setVisible(false);
      }),
    );
    await invoke("frontend_ready");
  });
  onCleanup(() => {
    unlisteners.forEach((u) => u());
    stopAudio();
    window.clearTimeout(resizeTimer);
    window.clearTimeout(noticeTimer);
    window.cancelAnimationFrame(enterFrame);
    window.cancelAnimationFrame(fitFrame);
  });
  return (
    <Show when={visible() && captured()}>
      <div class="popup" classList={{ entered: entered() }} ref={popupEl}>
        <header class="bar" onPointerDown={beginDrag} title="拖动此处移动窗口">
          <span class="brand" title={captureDescription(captured()!)}>
            <img src={logo} alt="" draggable={false} />
            TYL
            <span class="brand-label">
              {dictionaryView() ? "词典" : "划词翻译"}
            </span>
          </span>
          <span class="capture-badge" title={captureDescription(captured()!)}>
            {captured()!.source.kind === "uia"
              ? "直接取词"
              : captured()!.source.kind === "clipboard"
                ? "剪贴板取词"
                : "手动输入"}
          </span>
          <span class="language-label">
            {isChinese(captured()!.text) ? "中文 → 英语" : "英语 → 中文"}
          </span>
          <button
            class="icon-button close"
            onClick={() => void hide()}
            title="关闭 · Esc"
            aria-label="关闭"
          >
            <Icon name="close" size={15} />
          </button>
        </header>
        <Show when={captured()!.show_source}>
          <section
            class="source"
            classList={{
              folded: longSource() && !sourceExpanded(),
              word: captured()!.is_word,
            }}
          >
            <Show when={longSource()}>
              <button
                class="source-toggle"
                aria-expanded={sourceExpanded()}
                onClick={() => setSourceExpanded((v) => !v)}
              >
                <span>
                  原文 <small>{captured()!.text.length} 字符</small>
                </span>
                <span>
                  {sourceExpanded() ? "收起" : "展开"}
                  <Icon name="chevron" size={13} />
                </span>
              </button>
            </Show>
            <div
              class="source-text"
              lang={isChinese(captured()!.text) ? "zh" : "en"}
            >
              {captured()!.text}
            </div>
          </section>
        </Show>
        <Show when={dict() || dictPending()}>
          <div class="word-bar">
            <div class="word-nav" role="tablist" aria-label="单词查询结果">
              <button
                role="tab"
                aria-selected={dictionaryView()}
                classList={{ active: dictionaryView() }}
                onClick={() => setWordTranslations(false)}
              >
                词典释义
              </button>
              <button
                role="tab"
                aria-selected={!dictionaryView()}
                classList={{ active: !dictionaryView() }}
                onClick={showWordTranslations}
              >
                引擎译文
              </button>
            </div>
            <Show when={!dictionaryView() && !stacked()}>
              <ResultActions
                text={resultText()}
                canReplace={
                  Boolean(captured()?.can_replace) &&
                  (dictionaryView() || current()?.status === "done")
                }
                requiresVerification={captured()?.replace_requires_verification}
                busy={busy()}
                onCopy={copy}
                onReplace={replace}
              />
            </Show>
          </div>
        </Show>
        <Show
          when={dictionaryView()}
          fallback={
            <div class="translation-area">
              <Show
                when={!stacked()}
                fallback={
                  <div class="results-stack">
                    <For each={engineIds()}>
                      {(id) => {
                        const state = () =>
                          engines().find((e) => e.service === id);
                        return (
                          <article
                            class="result-card"
                            classList={{ collapsed: collapsed().has(id) }}
                          >
                            <header class="result-head">
                              <span class="result-engine">
                                <span
                                  class="engine-status"
                                  data-status={state()?.status ?? "loading"}
                                />
                                {LABELS[id] ?? id}
                              </span>
                              <span class="result-state">
                                {statusLabel(state()?.status)}
                              </span>
                              <button
                                class="icon-button collapse-button"
                                classList={{ collapsed: collapsed().has(id) }}
                                title={
                                  collapsed().has(id) ? "展开结果" : "折叠结果"
                                }
                                aria-label={`${collapsed().has(id) ? "展开" : "折叠"}${LABELS[id] ?? id}译文`}
                                aria-expanded={!collapsed().has(id)}
                                onClick={() => toggleResult(id)}
                              >
                                <Icon name="chevron" size={14} />
                              </button>
                              <button
                                class="icon-button"
                                title={`复制${LABELS[id]}译文`}
                                aria-label={`复制${LABELS[id]}译文`}
                                disabled={!state()?.text}
                                onClick={() => void copy(state()!.text)}
                              >
                                <Icon name="copy" size={14} />
                              </button>
                              <button
                                class="icon-button"
                                title={
                                  captured()?.can_replace
                                    ? captured()?.replace_requires_verification
                                      ? "尝试替换原选区（先校验选中文本）"
                                      : "用此译文替换原选区（不影响剪贴板历史）"
                                    : "原选区只读或无法确认可编辑，请复制译文"
                                }
                                aria-label={`用${LABELS[id]}译文替换原文`}
                                disabled={
                                  !captured()?.can_replace ||
                                  state()?.status !== "done" ||
                                  busy()
                                }
                                onClick={() => void replace(state()!.text)}
                              >
                                <Icon name="replace" size={15} />
                              </button>
                            </header>
                            <Show when={!collapsed().has(id)}>
                              <div class="result-body">
                                <EngineContent state={state()} />
                              </div>
                            </Show>
                          </article>
                        );
                      }}
                    </For>
                  </div>
                }
              >
                <div class="engine-bar">
                  <div
                    class="engine-tabs"
                    role="tablist"
                    aria-label="翻译引擎"
                    ref={engineTabsEl}
                    onWheel={scrollEngineTabs}
                  >
                    <For each={engineIds()}>
                      {(id) => (
                        <button
                          role="tab"
                          data-engine={id}
                          aria-selected={active() === id}
                          classList={{ active: active() === id }}
                          onClick={() => switchEngine(id)}
                        >
                          <span
                            class="engine-status"
                            data-status={
                              engines().find((e) => e.service === id)?.status ??
                              "idle"
                            }
                          />
                          {LABELS[id] ?? id}
                        </button>
                      )}
                    </For>
                  </div>
                  <Show when={!dict()}>
                    <ResultActions
                      text={resultText()}
                      canReplace={
                        Boolean(captured()?.can_replace) &&
                        current()?.status === "done"
                      }
                      requiresVerification={captured()?.replace_requires_verification}
                      busy={busy()}
                      onCopy={copy}
                      onReplace={replace}
                    />
                  </Show>
                </div>
                <Show when={`${captured()?.request_id}:${active()}`} keyed>
                  {(_key) => (
                    <div class="translation" role="tabpanel">
                      <EngineContent state={current()} />
                    </div>
                  )}
                </Show>
              </Show>
            </div>
          }
        >
          <section class="dict">
            <Show when={!dictPending()} fallback={
              <div class="dict-loading" role="status" aria-label="正在查询词典">
                <h1 class="dict-word">{captured()!.text}</h1>
                <div class="loading-block"><i /><i /><i /></div>
                <p class="dict-loading-label">正在查询词典…</p>
              </div>
            }>
            <h1 class="dict-word">{dict()!.word}</h1>
            <div class="pronunciations">
              <For each={["uk", "us"] as const}>
                {(accent) => (
                  <button
                    class="pronunciation"
                    classList={{
                      playing:
                        speaking() === accent && audioPhase() === "playing",
                    }}
                    aria-busy={
                      speaking() === accent && audioPhase() === "loading"
                    }
                    aria-pressed={speaking() === accent}
                    onClick={() => void pronounce(accent)}
                    title={accent === "uk" ? "播放英式发音" : "播放美式发音"}
                    aria-label={
                      accent === "uk" ? "播放英式发音" : "播放美式发音"
                    }
                  >
                    <span class="accent-label">
                      {accent === "uk" ? "英" : "美"}
                    </span>
                    <span class="phonetic">
                      {(
                        accent === "uk"
                          ? dict()!.phonetic_uk
                          : dict()!.phonetic_us
                      )
                        ? `/${accent === "uk" ? dict()!.phonetic_uk : dict()!.phonetic_us}/`
                        : "点击发音"}
                    </span>
                    <span class="sound-icon">
                      <Icon name="sound" size={14} />
                    </span>
                  </button>
                )}
              </For>
            </div>
            <Show when={dict()?.exam_types?.length}>
              <div class="dict-exams">
                <For each={dict()!.exam_types}>
                  {(label) => <span>{label}</span>}
                </For>
              </div>
            </Show>
            <div class="dict-senses">
              <For each={dict()!.senses}>
                {(sense) => (
                  <div class="dict-sense">
                    <span class="dict-pos">{sense.pos || "释义"}</span>
                    <span>{sense.definitions.join("；")}</span>
                  </div>
                )}
              </For>
            </div>
            </Show>
          </section>
        </Show>
        <Show when={notice()}>
          <div
            class="notice"
            classList={{ error: noticeError() }}
            role="status"
          >
            {!noticeError() && <Icon name="check" size={13} />}
            {notice()}
          </div>
        </Show>
      </div>
    </Show>
  );
}
function ResultActions(props: {
  text: string;
  canReplace: boolean;
  requiresVerification?: boolean;
  busy: boolean;
  onCopy: (text: string) => Promise<void>;
  onReplace: (text: string) => Promise<void>;
}) {
  return (
    <div class="result-actions" aria-label="结果操作">
      <button
        class="icon-button"
        title="复制当前结果 · Ctrl+C"
        aria-label="复制当前结果"
        disabled={!props.text}
        onClick={() => void props.onCopy(props.text)}
      >
        <Icon name="copy" size={14} />
      </button>
      <button
        class="icon-button"
        title={
          props.canReplace
            ? props.requiresVerification
              ? "尝试替换原选区（先校验选中文本）"
              : "替换原选区（不影响剪贴板历史）"
            : "原选区只读或无法确认可编辑"
        }
        aria-label="用当前结果替换原文"
        disabled={!props.text || !props.canReplace || props.busy}
        onClick={() => void props.onReplace(props.text)}
      >
        <Icon name="replace" size={15} />
      </button>
    </div>
  );
}
function EngineContent(props: { state?: EngineState }) {
  return (
    <Show
      when={props.state?.text}
      fallback={
        <Show
          when={props.state?.status === "error"}
          fallback={
            <div class="loading-block" aria-label="正在翻译">
              <i />
              <i />
              <i />
            </div>
          }
        >
          <div class="error-row">
            <span>{friendlyError(props.state?.error)}</span>
            <small>可切换其他引擎，或检查网络与配置。</small>
          </div>
        </Show>
      }
    >
      <span class="result-text">{props.state?.text}</span>
      <Show when={props.state?.status === "streaming"}>
        <span class="caret" />
      </Show>
      <Show when={props.state?.status === "error"}>
        <div class="partial-error">连接中断，已保留收到的内容</div>
      </Show>
    </Show>
  );
}
function isChinese(text: string) {
  return /[\u3400-\u9fff]/.test(text);
}
function statusLabel(status?: EngineState["status"]) {
  return status === "done"
    ? "翻译完成"
    : status === "error"
      ? "暂不可用"
      : status === "streaming"
        ? "正在生成"
        : "正在翻译";
}
function friendlyError(error = "") {
  if (error.startsWith("AI ")) return error;
  if (/API Key|API key/.test(error)) return "请先配置 AI 密钥";
  if (/network|dns|connect|timeout/.test(error)) return "暂时无法连接翻译服务";
  return error === "empty" ? "此引擎未返回译文" : "此引擎暂不可用";
}
function captureDescription(p: CapturedPayload) {
  const method =
    p.source.kind === "uia"
      ? "直接读取选中文本，未使用剪贴板"
      : p.source.restored
        ? "已读取选区并恢复原剪贴板"
        : "通过剪贴板读取选中文本";
  return `${method}${p.target_exe ? ` · ${p.target_exe}` : ""}`;
}
function measureNaturalHeight(popup?: HTMLDivElement): number {
  if (!popup?.clientWidth) return 0;
  const probe = document.createElement("div");
  probe.style.cssText = `position:fixed;left:-10000px;top:0;width:${popup.offsetWidth}px;visibility:hidden;pointer-events:none;contain:layout style`;
  const clone = popup.cloneNode(true) as HTMLDivElement;
  clone.classList.add("measurement-probe");
  probe.appendChild(clone);
  document.body.appendChild(probe);
  const height = clone.getBoundingClientRect().height;
  probe.remove();
  const style = getComputedStyle(document.getElementById("root")!);
  return Math.ceil(
    height + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + 2,
  );
}
