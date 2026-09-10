import { render } from "solid-js/web";
import { invoke } from "@tauri-apps/api/core";
import { Popup } from "./Popup";
import "./popup.css";

// 挂载点缺失直接把原因写进日志再抛出（不再无声死掉）
const root = document.getElementById("root");
if (!root) {
  void invoke("frontend_log", { message: "[popup] 错误: #root 不存在" }).catch(() => {});
  throw new Error("#root missing");
}

render(() => <Popup />, root);
