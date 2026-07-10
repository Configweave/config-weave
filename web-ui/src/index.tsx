import { render } from "solid-js/web";
// Token CSS must load before component styles (cascade order).
import "@forge/tokens/tokens.css";
import "@forge/tokens/base.css";
import "@forge/ui/styles.css";
import "@forge/code/styles.css";
import "@forge/term/styles.css";
import "@forge/desktop/styles.css";
import "./app.css";
import App from "./App";

render(() => <App />, document.getElementById("root")!);
