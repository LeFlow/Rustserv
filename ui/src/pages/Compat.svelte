<!--
  Page Compatibilité (phase 5) :
  - onglet « Tests multi-versions » : PHPUnit, Pest ou un script lancés en parallèle
    sur plusieurs versions de PHP, avec tableau comparatif ;
  - onglet « Analyse de code » : PHPCompatibility (fonctions supprimées/dépréciées
    entre deux versions) et PHPStan (analyse de types pour une version cible).
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type PhpVersionView, type Rapport, type RunResult, type SiteView, type ToolsStatus, type TypeTest } from "../lib/api";
  import RapportAnalyse from "./RapportAnalyse.svelte";

  /** Branches connues de PHPCompatibility (cibles d'analyse). */
  const BRANCHES = ["5.6", "7.0", "7.1", "7.2", "7.3", "7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5", "8.6"];

  let onglet = $state<"tests" | "analyse">("tests");
  let sites = $state<SiteView[]>([]);
  let phps = $state<PhpVersionView[]>([]);
  let site = $state("");
  let erreur = $state<string | null>(null);
  /** PHPUnit/Pest absent du projet : proposer de l'installer. */
  let frameworkAbsent = $state<TypeTest | null>(null);
  let installationFramework = $state(false);
  let info = $state<string | null>(null);

  // --- Tests
  let type = $state<TypeTest>("phpunit");
  let script = $state("");
  let choisies = $state<Record<string, boolean>>({});
  let resultats = $state<RunResult[] | null>(null);
  let enCours = $state(false);
  let sortieVisible = $state<string | null>(null);

  // --- Analyse
  let outils = $state<ToolsStatus | null>(null);
  let installation = $state(false);
  let min = $state("7.4");
  let max = $state("");
  let niveau = $state(5);
  let cible = $state("8.3");
  let analyse = $state<"phpcs" | "phpstan" | null>(null);
  let rapport = $state<Rapport | null>(null);

  onMount(async () => {
    try {
      [sites, phps, outils] = await Promise.all([api.sitesList(), api.phpOverview().then((v) => v.versions), api.compatToolsStatus()]);
      site = sites[0]?.name ?? "";
      for (const v of phps) choisies[v.version] = v.active;
      const plusRecente = phps.find((v) => v.active)?.branch;
      if (plusRecente) cible = plusRecente;
    } catch (e) {
      erreur = tErreur(e);
    }
  });

  const versionsChoisies = $derived(phps.filter((v) => choisies[v.version]).map((v) => v.version));

  async function lancer() {
    erreur = info = null;
    frameworkAbsent = null;
    resultats = null;
    sortieVisible = null;
    enCours = true;
    try {
      resultats = await api.compatRun(site, type, type === "script" ? script : null, versionsChoisies);
    } catch (e) {
      const code = (e as { code?: string })?.code;
      if (code === "compat.phpunit_absent" || code === "compat.pest_absent") {
        frameworkAbsent = type;
      } else {
        erreur = tErreur(e);
      }
    } finally {
      enCours = false;
    }
  }

  /** Installe PHPUnit/Pest dans le projet puis relance les tests. */
  async function installerFramework() {
    if (!frameworkAbsent) return;
    erreur = null;
    installationFramework = true;
    try {
      const v = await api.compatInstallFramework(site, frameworkAbsent, versionsChoisies);
      info = t("compat.framework_installe", { outil: nomFramework(frameworkAbsent), version: v });
      frameworkAbsent = null;
      await lancer();
      info = t("compat.framework_installe", { outil: nomFramework(type), version: v });
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      installationFramework = false;
    }
  }

  const nomFramework = (k: TypeTest) => (k === "pest" ? "Pest" : "PHPUnit");
  const plusAncienne = $derived(
    [...versionsChoisies].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))[0] ?? "",
  );

  async function installerOutils() {
    erreur = null;
    installation = true;
    try {
      outils = await api.compatToolsInstall();
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      installation = false;
    }
  }

  async function analyser(outil: "phpcs" | "phpstan") {
    erreur = null;
    rapport = null;
    analyse = outil;
    try {
      rapport = outil === "phpcs" ? await api.compatPhpcs(site, min, max === "" ? null : max) : await api.compatPhpstan(site, niveau, cible);
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      analyse = null;
    }
  }

  /** Pour un script : les versions qui produisent la même sortie reçoivent la même lettre. */
  const groupes = $derived.by(() => {
    const m = new Map<string, string>();
    for (const r of resultats ?? []) if (!m.has(r.empreinte)) m.set(r.empreinte, String.fromCharCode(65 + m.size));
    return m;
  });
  const sortiesDifferentes = $derived(groupes.size > 1);
  const secondes = (ms: number) => (ms / 1000).toLocaleString("fr-FR", { maximumFractionDigits: 2 });
</script>

<div class="haut">
  <label>{t("compat.site")}
    <select bind:value={site}>
      {#each sites as s (s.name)}<option value={s.name}>{s.name}</option>{/each}
    </select>
  </label>
  <div class="onglets">
    <button class:actif={onglet === "tests"} onclick={() => (onglet = "tests")}>{t("compat.onglet_tests")}</button>
    <button class:actif={onglet === "analyse"} onclick={() => (onglet = "analyse")}>{t("compat.onglet_analyse")}</button>
  </div>
</div>

{#if sites.length === 0}<p class="muted">{t("sites.aucun")}</p>{/if}
{#if erreur}<pre class="erreur">{erreur}</pre>{/if}
{#if info}<p class="info">{info}</p>{/if}

{#if onglet === "tests" && frameworkAbsent}
  <div class="bandeau">
    <div>
      <strong>{t("compat.framework_absent", { outil: nomFramework(frameworkAbsent), site })}</strong>
      <div class="petit">{t("compat.framework_explication", { outil: nomFramework(frameworkAbsent), version: plusAncienne })}</div>
      <div class="muted petit">{t("compat.framework_composer")}</div>
    </div>
    <button class="primaire" disabled={installationFramework} onclick={installerFramework}>
      {installationFramework ? t("compat.framework_installation") : `⬇ ${t("compat.framework_installer", { outil: nomFramework(frameworkAbsent) })}`}
    </button>
  </div>
{/if}

{#if onglet === "tests" && sites.length}
  <section class="carte">
    <div class="ligne">
      <label>{t("compat.type")}
        <select bind:value={type}>
          <option value="phpunit">PHPUnit</option>
          <option value="pest">Pest</option>
          <option value="script">{t("compat.script")}</option>
        </select>
      </label>
      {#if type === "script"}
        <label class="grand">{t("compat.fichier")}
          <input type="text" placeholder="ex. index.php ou scripts/test.php" bind:value={script} />
        </label>
      {/if}
    </div>
    <div class="versions">
      <span>{t("compat.versions")}</span>
      {#each phps as v (v.version)}
        <label class="case"><input type="checkbox" bind:checked={choisies[v.version]} /> PHP {v.version}</label>
      {/each}
    </div>
    <button class="primaire" disabled={enCours || !site || versionsChoisies.length === 0 || (type === "script" && !script.trim())} onclick={lancer}>
      {enCours ? t("compat.en_cours", { n: versionsChoisies.length }) : `▶ ${t("compat.lancer", { n: versionsChoisies.length })}`}
    </button>
    <p class="muted petit">{t(`compat.aide_${type}`)}</p>
  </section>

  {#if resultats}
    <table class="carte resultats">
      <thead>
        <tr>
          <th>PHP</th><th>{t("compat.col_statut")}</th><th>{t("compat.col_duree")}</th>
          <th>{type === "script" ? t("compat.col_sortie") : t("compat.col_tests")}</th><th></th>
        </tr>
      </thead>
      <tbody>
        {#each resultats as r (r.version)}
          <tr>
            <td class="mono"><strong>{r.version}</strong></td>
            <td><span class="statut {r.statut}">{t(`compat.statut.${r.statut}`)}</span></td>
            <td class="mono">{secondes(r.duree_ms)} s</td>
            <td>
              {#if r.resume}
                {t("compat.resume", { tests: r.resume.tests, assertions: r.resume.assertions })}
                {#if r.resume.echecs}<span class="err"> · {t("compat.echecs", { n: r.resume.echecs })}</span>{/if}
                {#if r.resume.erreurs}<span class="err"> · {t("compat.erreurs", { n: r.resume.erreurs })}</span>{/if}
                {#if r.resume.ignores}<span class="muted"> · {t("compat.ignores", { n: r.resume.ignores })}</span>{/if}
              {:else if type === "script"}
                <span class="groupe">{t("compat.sortie_groupe", { lettre: groupes.get(r.empreinte) ?? "?" })}</span>
              {:else}
                <span class="muted">—</span>
              {/if}
            </td>
            <td class="actions">
              <button class="lien" onclick={() => (sortieVisible = sortieVisible === r.version ? null : r.version)}>
                {sortieVisible === r.version ? t("compat.masquer_sortie") : t("compat.voir_sortie")}
              </button>
            </td>
          </tr>
          {#if sortieVisible === r.version}
            <tr><td colspan="5"><pre class="sortie mono">{r.sortie || t("services.journal_vide")}</pre></td></tr>
          {/if}
        {/each}
      </tbody>
    </table>
    {#if type === "script"}
      <p class={sortiesDifferentes ? "avert" : "ok"}>{sortiesDifferentes ? t("compat.sorties_differentes", { n: groupes.size }) : t("compat.sorties_identiques")}</p>
    {/if}
  {/if}
{/if}

{#if onglet === "analyse" && sites.length}
  <section class="carte">
    <h4>{t("compat.outils")}</h4>
    {#if outils?.installes}
      <p class="petit">
        PHP_CodeSniffer {outils.phpcs} · PHPCompatibility {outils.phpcompatibility} · PHPStan {outils.phpstan}
        <button class="lien" disabled={installation} onclick={installerOutils}>{installation ? t("compat.installation") : t("compat.mettre_a_jour")}</button>
      </p>
      {#if outils.phpcompatibility?.includes("alpha")}<p class="muted petit">ⓘ {t("compat.alpha")}</p>{/if}
    {:else}
      <p class="petit">{t("compat.outils_absents")}</p>
      <button class="primaire" disabled={installation} onclick={installerOutils}>{installation ? t("compat.installation") : `⬇ ${t("compat.installer_outils")}`}</button>
    {/if}
  </section>

  {#if outils?.installes}
    <section class="carte">
      <h4>PHPCompatibility</h4>
      <p class="muted petit">{t("compat.phpcs_aide")}</p>
      <div class="ligne">
        <label>{t("compat.de")} <select bind:value={min}>{#each BRANCHES as b}<option value={b}>PHP {b}</option>{/each}</select></label>
        <label>{t("compat.a")}
          <select bind:value={max}>
            <option value="">{t("compat.toutes_suivantes")}</option>
            {#each BRANCHES.filter((b) => b >= min) as b}<option value={b}>PHP {b}</option>{/each}
          </select>
        </label>
        <button class="primaire" disabled={analyse !== null} onclick={() => analyser("phpcs")}>{analyse === "phpcs" ? t("compat.analyse_en_cours") : `🔍 ${t("compat.analyser")}`}</button>
      </div>
    </section>

    <section class="carte">
      <h4>PHPStan</h4>
      <p class="muted petit">{t("compat.phpstan_aide")}</p>
      <div class="ligne">
        <label>{t("compat.version_cible")} <select bind:value={cible}>{#each BRANCHES.filter((b) => b >= "7.4") as b}<option value={b}>PHP {b}</option>{/each}</select></label>
        <label>{t("compat.niveau")}
          <select bind:value={niveau}>{#each Array.from({ length: 11 }, (_, i) => i) as n}<option value={n}>{n}{n === 0 ? ` (${t("compat.souple")})` : n === 10 ? ` (${t("compat.strict")})` : ""}</option>{/each}</select>
        </label>
        <button class="primaire" disabled={analyse !== null} onclick={() => analyser("phpstan")}>{analyse === "phpstan" ? t("compat.analyse_en_cours") : `🔍 ${t("compat.analyser")}`}</button>
      </div>
    </section>

    {#if rapport}<RapportAnalyse {rapport} />{/if}
  {/if}
{/if}

<style>
  .haut { display: flex; justify-content: space-between; align-items: end; gap: 12px; margin-bottom: 12px; }
  .onglets { display: flex; gap: 4px; }
  .onglets button.actif { background: var(--accent); color: white; border-color: var(--accent); }
  .carte { background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; padding: 12px 16px; margin-bottom: 12px; }
  h4 { margin: 0 0 6px; }
  .ligne { display: flex; gap: 14px; align-items: end; flex-wrap: wrap; margin-bottom: 10px; }
  label { display: flex; flex-direction: column; gap: 4px; font-size: 13px; }
  label.case { flex-direction: row; align-items: center; gap: 5px; }
  label.grand { flex: 1; min-width: 260px; }
  .versions { display: flex; gap: 14px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; font-size: 13px; }
  select, input[type="text"] { font: inherit; padding: 5px 8px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  table.resultats { width: 100%; border-collapse: collapse; padding: 0; overflow: hidden; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-top: 1px solid var(--border); vertical-align: middle; }
  .actions { text-align: right; }
  .statut { font-size: 12px; padding: 2px 9px; border-radius: 10px; color: white; }
  .statut.reussi { background: var(--ok); }
  .statut.echec { background: #ca8a04; }
  .statut.erreur, .statut.delai_depasse { background: var(--danger); }
  .groupe { font-weight: 600; }
  .bandeau { display: flex; justify-content: space-between; align-items: center; gap: 16px; padding: 12px 16px; margin-bottom: 12px; border-radius: 8px; border: 1px solid var(--accent); background: var(--bg-card); }
  .info { color: var(--ok); }
  .sortie { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 10px; max-height: 360px; overflow: auto; white-space: pre-wrap; margin: 0; }
  .err { color: var(--danger); }
  .ok { color: var(--ok); }
  .avert { color: #ca8a04; }
  .petit { font-size: 12px; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
</style>
