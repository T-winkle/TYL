import { render } from "solid-js/web";
import { isMac } from "../shared/platform";
import { Settings } from "./Settings";
import "./settings.css";

if (isMac) document.documentElement.dataset.platform = "macos";
render(() => <Settings />, document.getElementById("root")!);
