<!--
  Page PHP : versions installées, version par défaut, épinglage,
  mises à jour de branche (8.3.12 → 8.3.15) et détection des nouvelles versions.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type InstallProgress, type PhpVersionView, type PhpView } from "../lib/api";
  import { formatOctets } from "../lib/format";
  import Extensions from "./Extensions.svelte";

  let { allerAuCatalogue }: { allerAuCatalogue: () => void } = $props();
  /** Version dont le panneau Extensions est ouvert. */
  let extensionsDe = $state<PhpVersionView | null>(null);

  let vue = $state<PhpView | null>(null);
  let erreur = $state<string | null>(null);
  let info = $state<string | null>(null);
  let recherche = $state(false);
  /** Branche en cours de mise à jour → progression. */
  let maj = $state<Record<string, InstallProgress | "validation">>({});
  /** Option « conserver l'ancienne version » par branche (cochée par défaut, plus prudent). */
  let conserver = $state<Record<string, boolean>>({});

  async function charger() {
    try {
      vue = await api.phpOverview();
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  async function rechercher() {
    recherche = true;
    erreur = info = null;
    try {
      const ajout = await api.phpCheckUpdates();
      info = ajout.length ? t("php.nouvelles", { versions: ajout.join(", ") }) : t("php.a_jour");
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      recherche = false;
      await charger();
    }
  }

  onMount(() => {
    charger().then(() => {
      // Recherche automatique une fois par session.
      if (vue && vue.last_check === null) rechercher();
    });
    const off = api.onInstallProgress((p) => {
      if (p.package !== "php") return;
      const branche = p.version.split(".").slice(0, 2).join(".");
      if (branche in maj) maj[branche] = p.phase === "termine" ? "validation" : p;
    });
    return () => { off.then((f) => f()); };
  });

  async function action(f: () => Promise<unknown>) {
    erreur = info = null;
    try {
      await f();
    } catch (e) {
      erreur = tErreur(e);
    }
    await charger();
  }

  async function mettreAJour(v: PhpVersionView) {
    erreur = info = null;
    maj[v.branch] = "validation";
    try {
      const r = await api.phpUpgrade(v.branch, conserver[v.branch] ?? true);
      info = t("php.mis_a_jour", { de: v.version, vers: v.update ?? "" });
      if (r.dll_copiees.length) info += " " + t("php.dll_copiees", { liste: r.dll_copiees.join(", ") });
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      delete maj[v.branch];
      await charger();
    }
  }

  function libelleMaj(p: InstallProgress | "validation"): string {
    if (p === "validation") return t("php.validation");
    if (p.phase === "telechargement" && p.total) return t("catalogue.telechargement", { fait: formatOctets(p.done), total: formatOctets(p.total) });
    return t(`catalogue.phase.${p.phase}`);
  }

  function depuis(ts: number): string {
    return new Date(ts * 1000).toLocaleTimeString("fr-FR", { hour: "2-digit", minute: "2-digit" });
  }
</script>

<div class="barre">
  <button disabled={recherche} onclick={rechercher}>{recherche ? t("php.recherche_en_cours") : t("php.rechercher")}</button>
  <button class="lien" onclick={allerAuCatalogue}>{t("php.installer_autre")}</button>
  <span class="espace"></span>
  {#if vue?.last_check}<span class="muted petit">{t("php.derniere_recherche", { heure: depuis(vue.last_check) })}</span>{/if}
</div>

{#if erreur}<pre class="erreur">{erreur}</pre>{/if}
{#if info}<p class="info">{info}</p>{/if}

{#if vue && vue.versions.length === 0}
  <p class="muted">{t("php.aucune")}</p>
{/if}

{#if vue && vue.versions.length}
  <table class="carte">
    <thead>
      <tr>
        <th>{t("php.col_version")}</th>
        <th>{t("php.col_etat")}</th>
        <th>{t("php.col_sites")}</th>
        <th>{t("php.col_ports")}</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#each vue.versions as v (v.version)}
        <tr class:inactive={!v.active}>
          <td>
            <strong>PHP {v.version}</strong>
            {#if v.default}<span class="badge defaut">{t("php.par_defaut")}</span>{/if}
            {#if v.pinned}<span class="badge">📌 {t("php.epinglee")}</span>{/if}
          </td>
          <td>
            {#if !v.active}<span class="muted">{t("php.inactive")}</span>
            {:else if v.running}<span class="ok">● {t("services.etat.en_cours")}</span>
            {:else}<span class="muted">○ {t("services.etat.arrete")}</span>{/if}
          </td>
          <td class="petit">{v.sites.length ? v.sites.join(", ") : "—"}</td>
          <td class="mono petit">{v.active ? `${v.ports[0]}-${v.ports[v.ports.length - 1]}` : "—"}</td>
          <td class="actions">
            <button class:primaire={extensionsDe?.version === v.version} onclick={() => (extensionsDe = extensionsDe?.version === v.version ? null : v)}>{t("php.extensions")}</button>
            {#if v.active && !v.default}
              <button onclick={() => action(() => api.phpSetDefault(v.branch))}>{t("php.definir_defaut")}</button>
            {/if}
            {#if !v.active}
              <button onclick={() => action(() => api.phpPin(v.branch, v.version))}>{t("php.utiliser")}</button>
            {:else if v.pinned}
              <button onclick={() => action(() => api.phpPin(v.branch, null))}>{t("php.desepingler")}</button>
            {/if}
          </td>
        </tr>
        {#if v.active && v.update}
          <tr class="ligne-maj">
            <td colspan="5">
              {#if maj[v.branch]}
                <span class="maj">⟳ {libelleMaj(maj[v.branch])}</span>
              {:else}
                <span class="maj">⬆ {t("php.maj_dispo", { version: v.update })}</span>
                <label class="petit"><input type="checkbox" checked={conserver[v.branch] ?? true} onchange={(e) => (conserver[v.branch] = e.currentTarget.checked)} /> {t("php.conserver", { version: v.version })}</label>
                <button class="primaire" onclick={() => mettreAJour(v)}>{t("php.mettre_a_jour", { version: v.update })}</button>
              {/if}
            </td>
          </tr>
        {/if}
      {/each}
    </tbody>
  </table>
  <p class="muted petit">{t("php.explication")}</p>
{/if}

{#if extensionsDe}
  {#key extensionsDe.version}
    <Extensions version={extensionsDe.version} ini={extensionsDe.ini} fermer={() => (extensionsDe = null)} />
  {/key}
{/if}

<style>
  .barre { display: flex; gap: 10px; align-items: center; margin-bottom: 14px; }
  .espace { flex: 1; }
  table.carte { width: 100%; border-collapse: collapse; background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; overflow: hidden; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-top: 1px solid var(--border); vertical-align: middle; }
  tr.inactive td { opacity: 0.6; }
  .ligne-maj td { border-top: 0; padding-top: 0; display: table-cell; }
  .ligne-maj td > * { margin-right: 12px; }
  .maj { color: var(--accent); font-weight: 600; }
  .actions { text-align: right; white-space: nowrap; }
  .actions button { margin-left: 4px; }
  .badge { margin-left: 6px; }
  .badge.defaut { background: var(--accent); color: white; }
  .ok { color: var(--ok); }
  .petit { font-size: 12px; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .info { color: var(--ok); }
</style>
