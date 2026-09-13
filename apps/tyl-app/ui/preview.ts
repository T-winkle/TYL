// Development-only fixture. Not a production Vite entry, never contacts translation services.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
const query = new URLSearchParams(location.search);
mockWindows(query.get("page") === "settings" ? "settings" : "popup");
const theme = query.get("theme") ?? "light";
const colorScheme = query.get("scheme") ?? "indigo";
const language = query.get("lang") ?? "zh-CN";
const scenario = query.get("case") ?? "dict";
const engines = query.get("allEngines") === "true"
  ? ["bing", "youdao", "transmart", "yandex", "iciba", "google", "mymemory", "llm"]
  : ["bing", "youdao", "transmart"];
const cfg = {
  language,
  hotkey: "alt+t",
  engines,
  result_display: "tabs",
  theme,
  color_scheme: colorScheme,
  show_source: query.get("sourceText") === "true",
  dictionary: { enabled: true, provider: "auto" },
  gpu_acceleration: false,
  log_level: "info",
  proxy: { mode: "system", url: "" },
  llm: {
    enabled: false,
    api_key: "",
    base_url: "https://api.openai.com/v1",
    model: "gpt-4o-mini",
  },
};
const calls: { command: string; args: unknown }[] = [];
Object.assign(window, { __tylPreview: { calls, emit } });
mockIPC(
  (command, args) => {
    calls.push({ command, args });
    if (command === "get_settings") return structuredClone(cfg);
    if (command === "get_autostart") return false;
    if (command === "test_llm") return query.get("testError") === "true" ? Promise.reject("AI HTTP 401：API Key 无效或已过期") : 200;
    if (command === "get_gpu_status") return { requested: false, software: true, external_override: false };
    if (command === "save_settings") Object.assign(cfg, args?.settings);
    if (command === "replace_selection" && query.get("replaceHidden") === "true")
      return emit("tyl://popup-hidden", {}).then(() => {
        throw new Error("原选区已变化，请重新划词后再替换");
      });
    if (command === "replace_selection")
      throw new Error("预览模式不会修改其他应用");
    if (command === "pronunciation_audio")
      throw new Error("预览模式不播放网络音频");
    if (command === "translate_one")
      setTimeout(
        () =>
          emit("tyl://translate", {
            request_id: args?.request_id,
            service: args?.engine,
            phase: "done",
            text: "这是切换标签后按需请求的译文。",
          }),
        120,
      );
  },
  { shouldMockEvents: true },
);
if (query.get("page") === "settings") {
  await import("./src/settings/main");
} else {
  await import("./src/popup/main");
  await new Promise((resolve) => setTimeout(resolve, 100));
  const isWord = scenario === "dict";
  const original =
    scenario === "short"
      ? "  Good design matters.  "
      : "  Well-designed tools should help people read with less effort.\nThey should preserve the meaning of the original text while making the translation clear, natural, and easy to understand.  ";
  await emit("tyl://captured", {
    request_id: 42,
    text: isWord ? "consider" : original.trim().replace("\n", " "),
    source: { kind: query.get("source") ?? "uia", restored: true },
    elapsed_ms: 34,
    target_exe: "notepad",
    engines,
    result_display: scenario === "stacked" ? "stacked" : "tabs",
    grow_upward: false,
    theme,
    color_scheme: colorScheme,
    language,
    show_source: query.get("sourceText") === "true",
    is_word: isWord,
    dictionary: true,
    can_replace: query.get("editable") === "true",
  });
  const translation =
    scenario === "short"
      ? "好的设计很重要。"
      : "设计出色的工具应当帮助人们更轻松地阅读。在保留原文含义的同时，让译文清晰、自然、易于理解。\n\n界面应根据内容自动调整高度，将更多空间留给翻译结果。用户可以随时展开原文、切换翻译引擎，或直接复制译文继续工作。";
  if (isWord) {
    await emit("tyl://translate", {
      request_id: 42,
      service: "dict",
      phase: "dict",
      text: JSON.stringify({
        word: "consider",
        phonetic_uk: "kənˈsɪdə(r)",
        phonetic_us: "kənˈsɪdər",
        exam_types: ["CET4", "CET6", "考研", "IELTS"],
        senses: [
          { pos: "v.", definitions: ["仔细考虑；斟酌；思考"] },
          { pos: "vt.", definitions: ["认为；把……看作；顾及；体谅"] },
          { pos: "vi.", definitions: ["考虑；细想"] },
          {
            pos: "短语",
            definitions: [
              "consider doing sth. 考虑做某事",
              "all things considered 从各方面考虑",
            ],
          },
        ],
      }),
    });
  }
  for (const service of isWord ? [] : scenario === "stacked" ? engines : ["bing"]) {
    await emit("tyl://translate", {
      request_id: 42,
      service,
      phase: scenario === "error" ? "error" : "done",
      text:
        scenario === "error"
          ? "network timeout"
          : scenario === "long"
            ? translation.repeat(6)
            : translation,
    });
  }
}
