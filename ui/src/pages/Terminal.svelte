<!--
  Page Terminal : onglets de terminaux PowerShell, chacun réglé sur le PHP
  d'un site (PATH, php.ini), avec Composer et le client de la base du site (MariaDB, MySQL ou psql) disponibles.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n";
  import { api, type SiteView } from "../lib/api";
  import { fermerOnglet, nouveauTerminal, terminaux } from "../lib/terminaux.svelte";
  import XTerm from "./XTerm.svelte";

  let { visible }: { visible: boolean } = $props();
  let sites = $state<SiteView[]>([]);
  let choix = $state("");

  onMount(() => {
    api.sitesList().then((s) => (sites = s));
  });

  function ouvrir() {
    const site = choix === "" ? null : choix;
    const s = sites.find((x) => x.name === site);
    nouveauTerminal(site, s ? `${s.name} (PHP ${s.php_version ?? "—"})` : "www");
  }
</script>

<div class="page">
  <div class="barre">
    <select bind:value={choix}>
      <option value="">{t("terminal.dossier_www")}</option>
      {#each sites as s (s.name)}<option value={s.name}>{s.name} — PHP {s.php_version ?? "—"}</option>{/each}
    </select>
    <button class="primaire" onclick={ouvrir}>＋ {t("terminal.nouveau")}</button>
    <span class="muted petit">{t("terminal.aide")}</span>
  </div>

  {#if terminaux.onglets.length}
    <div class="onglets">
      {#each terminaux.onglets as o (o.cle)}
        <div class="onglet" class:actif={terminaux.actif === o.cle}>
          <button class="titre" onclick={() => (terminaux.actif = o.cle)}>{o.titre}</button>
          <button class="fermer" title={t("terminal.fermer")} onclick={() => fermerOnglet(o.cle)}>✕</button>
        </div>
      {/each}
    </div>
    <div class="zone">
      {#each terminaux.onglets as o (o.cle)}
        <XTerm site={o.site} visible={visible && terminaux.actif === o.cle} termine={() => {}} />
      {/each}
    </div>
  {:else}
    <p class="muted">{t("terminal.vide")}</p>
  {/if}
</div>

<style>
  .page { display: flex; flex-direction: column; height: calc(100vh - 110px); }
  .barre { display: flex; gap: 10px; align-items: center; margin-bottom: 10px; }
  select { font: inherit; padding: 5px 8px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  .onglets { display: flex; gap: 4px; flex-wrap: wrap; }
  .onglet { display: flex; align-items: center; border: 1px solid var(--border); border-bottom: 0; border-radius: 6px 6px 0 0; background: var(--bg-card); }
  .onglet.actif { background: var(--accent); }
  .onglet.actif button { color: white; }
  .onglet button { border: 0; background: none; padding: 5px 10px; }
  .onglet .fermer { padding: 5px 8px 5px 0; opacity: 0.7; }
  .zone { flex: 1; min-height: 0; }
  .petit { font-size: 12px; }
</style>
