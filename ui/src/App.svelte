<!--
  Composant racine : barre latérale de navigation + page courante.
  Svelte 5 en bref :
  - `let x = $state(...)` : variable réactive (l'affichage se met à jour tout seul) ;
  - `{#if}`, `{#each}` : conditions et boucles dans le HTML ;
  - `onclick={...}` : gestionnaire d'événement.
-->
<script lang="ts">
  import { t, tErreur } from "./i18n";
  import { api, type AppInfo } from "./lib/api";
  import Catalogue from "./pages/Catalogue.svelte";
  import Parametres from "./pages/Parametres.svelte";
  import Php from "./pages/Php.svelte";
  import Services from "./pages/Services.svelte";
  import Sites from "./pages/Sites.svelte";
  import Terminal from "./pages/Terminal.svelte";

  type Page = "services" | "php" | "sites" | "terminal" | "catalogue" | "parametres";
  const pages: Page[] = ["services", "php", "sites", "terminal", "catalogue", "parametres"];

  let page = $state<Page>("services");
  let info = $state<AppInfo | null>(null);
  let erreur = $state<string | null>(null);

  api.appInfo().then((i) => (info = i)).catch((e) => (erreur = tErreur(e)));
</script>

<div class="layout">
  <nav>
    <h1>{t("app.nom")}</h1>
    {#each pages as p}
      <button class:active={page === p} onclick={() => (page = p)}>{t(`nav.${p}`)}</button>
    {/each}
    {#if info}
      <footer>{t("app.version", { version: info.version })}</footer>
    {/if}
  </nav>

  <main>
    <h2>{t(`nav.${page}`)}</h2>
    {#if erreur}<p class="erreur">{erreur}</p>{/if}
    {#if page === "services"}
      <Services allerAuCatalogue={() => (page = "catalogue")} />
    {:else if page === "php"}
      <Php allerAuCatalogue={() => (page = "catalogue")} />
    {:else if page === "sites"}
      <Sites allerAuTerminal={() => (page = "terminal")} />
    {:else if page === "catalogue"}
      <Catalogue />
    {:else if page === "parametres"}
      <Parametres racine={info?.racine ?? ""} allerAuxSites={() => (page = "sites")} />
    {/if}
    <!-- Toujours monté (simplement masqué) : les terminaux restent ouverts d'une page à l'autre. -->
    <div class:cache={page !== "terminal"}>
      <Terminal visible={page === "terminal"} />
    </div>
  </main>
</div>

<style>
  .layout { display: grid; grid-template-columns: 200px 1fr; height: 100vh; }
  nav { background: var(--bg-nav); display: flex; flex-direction: column; padding: 16px 10px; gap: 4px; border-right: 1px solid var(--border); }
  nav h1 { font-size: 18px; margin: 0 8px 16px; color: var(--accent); }
  nav button { text-align: left; background: none; border: 0; color: var(--fg); padding: 8px 10px; border-radius: 6px; cursor: pointer; font: inherit; }
  nav button:hover { background: var(--bg-hover); }
  nav button.active { background: var(--accent); color: white; }
  footer { margin-top: auto; font-size: 12px; color: var(--muted); padding: 0 8px; }
  main { padding: 24px 32px; overflow: auto; }
  .erreur { color: var(--danger); }
  .cache { display: none; }
</style>
