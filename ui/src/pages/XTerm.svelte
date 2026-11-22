<!--
  Un terminal xterm.js relié à un pseudo-terminal Windows (PowerShell) côté Rust.
  Le terminal Rust est créé au montage et fermé au démontage.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { t, tErreur } from "../i18n";
  import { api } from "../lib/api";

  let { site, visible, termine }: { site: string | null; visible: boolean; termine: (info: string) => void } = $props();

  let conteneur: HTMLDivElement;
  let erreur = $state<string | null>(null);
  let entete = $state("");
  let fit: FitAddon | null = null;
  let term: Terminal | null = null;
  let id: number | null = null;

  /** Recalcule la taille quand l'onglet devient visible ou que la fenêtre change. */
  function ajuster() {
    if (!fit || !term || !visible) return;
    try {
      fit.fit();
      if (id !== null) api.terminalResize(id, term.cols, term.rows);
    } catch {
      /* conteneur pas encore mesurable */
    }
  }

  $effect(() => {
    if (visible) requestAnimationFrame(() => { ajuster(); term?.focus(); });
  });

  onMount(() => {
    const sombre = window.matchMedia("(prefers-color-scheme: dark)").matches;
    term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, monospace',
      fontSize: 13,
      cursorBlink: true,
      theme: sombre ? { background: "#15181e" } : { background: "#1d2330", foreground: "#e6e8ec" },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(conteneur);
    fit.fit();

    const offs: Promise<() => void>[] = [];
    let ferme = false;
    offs.push(api.onTerminalOutput((e) => { if (e.id === id) term?.write(e.data); }));
    offs.push(api.onTerminalExit((e) => {
      if (e === id) {
        term?.write(`\r\n\x1b[90m${t("terminal.termine")}\x1b[0m\r\n`);
        id = null;
        termine(t("terminal.termine"));
      }
    }));
    // Saisie clavier → shell (xterm.js répond aussi aux requêtes de ConPTY, ex. position du curseur).
    const saisie = term.onData((d) => { if (id !== null) api.terminalWrite(id, d); });

    api
      .terminalOpen(site, term.cols, term.rows)
      .then((info) => {
        if (ferme) { api.terminalClose(info.id); return; }
        id = info.id;
        entete = info.php ? t("terminal.entete_php", { dossier: info.cwd, php: info.php }) : t("terminal.entete", { dossier: info.cwd });
      })
      .catch((e) => (erreur = tErreur(e)));

    const obs = new ResizeObserver(() => ajuster());
    obs.observe(conteneur);

    return () => {
      ferme = true;
      obs.disconnect();
      saisie.dispose();
      offs.forEach((p) => p.then((f) => f()));
      if (id !== null) api.terminalClose(id);
      term?.dispose();
    };
  });
</script>

<div class="xterm-bloc" class:cache={!visible}>
  {#if entete}<div class="entete mono">{entete}</div>{/if}
  {#if erreur}<pre class="erreur">{erreur}</pre>{/if}
  <div class="ecran" bind:this={conteneur}></div>
</div>

<style>
  .xterm-bloc { display: flex; flex-direction: column; height: 100%; }
  .cache { display: none; }
  .entete { font-size: 11px; color: var(--muted); padding: 4px 2px; }
  .ecran { flex: 1; min-height: 0; background: #1d2330; border-radius: 6px; padding: 6px; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
</style>
