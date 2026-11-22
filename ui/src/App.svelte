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
  import Compat from "./pages/Compat.svelte";
  import Parametres from "./pages/Parametres.svelte";
  import Php from "./pages/Php.svelte";
  import Services from "./pages/Services.svelte";
  import Sites from "./pages/Sites.svelte";
  import Terminal from "./pages/Terminal.svelte";

  type Page = "services" | "php" | "sites" | "terminal" | "compat" | "catalogue" | "parametres";
  const pages: Page[] = ["services", "php", "sites", "terminal", "compat", "catalogue", "parametres"];

  let page = $state<Page>("services");
  let info = $state<AppInfo | null>(null);
  let erreur = $state<string | null>(null);

  api.appInfo().then((i) => (info = i)).catch((e) => (erreur = tErreur(e)));
</script>

<div class="layout">
  <nav>
    <!-- Logo : version couleur en thème clair, monochrome blanc en thème sombre
         (le mot « Rust » noir serait invisible sur fond sombre). -->
    <h1 class="logo">
      <img class="clair" src="/img/logo-horizontal.png" alt={t("app.nom")} />
      <img class="sombre" src="/img/logo-monochrome.png" alt={t("app.nom")} />
    </h1>
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
    {:else if page === "compat"}
      <Compat />
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
  nav h1.logo { margin: 0 6px 18px; line-height: 0; }
  nav h1.logo img { width: 100%; height: auto; }
  nav h1.logo .sombre { display: none; filter: invert(1); }
  @media (prefers-color-scheme: dark) {
    nav h1.logo .clair { display: none; }
    nav h1.logo .sombre { display: block; }
  }
  nav button { text-align: left; background: none; border: 0; color: var(--fg); padding: 8px 10px; border-radius: 6px; cursor: pointer; font: inherit; }
  nav button:hover { background: var(--bg-hover); }
  nav button.active { background: var(--accent); color: white; }
  footer { margin-top: auto; font-size: 12px; color: var(--muted); padding: 0 8px; }
  main { padding: 24px 32px; overflow: auto; }
  .erreur { color: var(--danger); }
  .cache { display: none; }
</style>
