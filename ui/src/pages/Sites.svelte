<!--
  Page Sites : chaque dossier de www/ est un site. On y choisit la version
  de PHP du site. (Les domaines .test et le HTTPS arrivent en phase 4.)
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { t, tErreur } from "../i18n";
  import { api, type PhpVersionView, type SiteView } from "../lib/api";

  let sites = $state<SiteView[]>([]);
  let branches = $state<string[]>([]);
  let defaut = $state<PhpVersionView | null>(null);
  let erreur = $state<string | null>(null);
  let occupe = $state<Record<string, boolean>>({});

  async function charger() {
    try {
      const [s, php] = await Promise.all([api.sitesList(), api.phpOverview()]);
      sites = s;
      branches = php.versions.filter((v) => v.active).map((v) => v.branch);
      defaut = php.versions.find((v) => v.default) ?? null;
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  onMount(charger);

  async function choisir(site: SiteView, valeur: string) {
    erreur = null;
    occupe[site.name] = true;
    try {
      await api.siteSetPhp(site.name, valeur === "" ? null : valeur);
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      occupe[site.name] = false;
      await charger();
    }
  }
</script>

<div class="barre">
  <button onclick={charger}>⟳ {t("sites.actualiser")}</button>
  <span class="muted petit">{t("sites.aide")}</span>
</div>

{#if erreur}<pre class="erreur">{erreur}</pre>{/if}

{#if sites.length === 0}
  <p class="muted">{t("sites.aucun")}</p>
{:else}
  <table class="carte">
    <thead>
      <tr><th>{t("sites.col_site")}</th><th>{t("sites.col_dossier")}</th><th>PHP</th><th></th></tr>
    </thead>
    <tbody>
      {#each sites as s (s.name)}
        <tr>
          <td>
            <strong>{s.name}</strong>
            <div class="muted petit">{s.host} <span class="bientot">({t("sites.domaine_bientot")})</span></div>
          </td>
          <td class="mono petit" title={s.dir}>{s.root.replace(s.dir, "") || "/"}</td>
          <td>
            <select disabled={occupe[s.name]} value={s.php_choice ?? ""} onchange={(e) => choisir(s, e.currentTarget.value)}>
              <option value="">{t("sites.php_defaut", { version: defaut?.branch ?? "—" })}</option>
              {#each branches as b}<option value={b}>PHP {b}</option>{/each}
              {#if s.php_choice && !branches.includes(s.php_choice)}
                <option value={s.php_choice}>PHP {s.php_choice} ({t("sites.non_installe")})</option>
              {/if}
            </select>
            {#if s.php_missing}<div class="avert petit">{t("sites.php_manquant", { branche: s.php_choice ?? "", version: s.php_version ?? "—" })}</div>{/if}
            {#if occupe[s.name]}<span class="muted petit">{t("sites.application")}</span>{/if}
          </td>
          <td class="actions">
            <button class="lien" onclick={() => openUrl(s.local_url)}>{t("sites.ouvrir")} ↗</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .barre { display: flex; gap: 12px; align-items: center; margin-bottom: 14px; }
  table.carte { width: 100%; border-collapse: collapse; background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-top: 1px solid var(--border); vertical-align: middle; }
  select { font: inherit; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  .actions { text-align: right; }
  .petit { font-size: 12px; }
  .bientot { font-style: italic; }
  .avert { color: var(--accent); }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
</style>
