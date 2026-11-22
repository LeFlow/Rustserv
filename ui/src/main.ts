import { mount } from "svelte";
import App from "./App.svelte";
import { t } from "./i18n";
import "./styles.css";

const cible = document.getElementById("app")!;

// L'interface ne fonctionne que dans la fenêtre Rustserv (pas dans un navigateur
// ouvert sur http://localhost:5173) : le backend Rust n'y est pas accessible.
if ("__TAURI_INTERNALS__" in window) {
  mount(App, { target: cible });
} else {
  cible.innerHTML = `<p style="padding:24px">${t("app.hors_tauri")}</p>`;
}
