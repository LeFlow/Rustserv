<!--
  Assistant d'import depuis Laragon / WAMP.
  1. détection (lecture seule) ; 2. choix des sites, nom, PHP, conversion des adresses ;
  3. import avec suivi en direct ; 4. résultat par site.
  Rien n'est modifié chez Laragon ou WAMP.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { t, tErreur } from "../i18n";
  import { api, type Detection, type ProgressionImport, type ResultatImport, nomMoteur, type BaseVersion } from "../lib/api";

  let { fermer, termine }: { fermer: () => void; termine: () => void } = $props();

  interface Ligne {
    choisi: boolean;
    nom: string;
    php: string;
    convertir: boolean;
    mariadb: string;
  }

  let detection = $state<Detection | null>(null);
  let lignes = $state<Ligne[]>([]);
  let branches = $state<string[]>([]);
  /** Bases possibles pour un site importé : MariaDB ou MySQL (les bases d'origine sont MySQL). */
  let mariadbs = $state<BaseVersion[]>([]);
  let erreur = $state<string | null>(null);
  let enCours = $state(false);
  let etapes = $state<Record<string, ProgressionImport>>({});
  let resultats = $state<ResultatImport[] | null>(null);

  onMount(() => {
    Promise.all([api.migrationDetect(), api.phpOverview(), api.basesVersions()])
      .then(([d, php, m]) => {
        detection = d;
        mariadbs = m.filter((b) => b.moteur !== "postgresql");
        branches = php.versions.filter((v) => v.active).map((v) => v.branch);
        lignes = d.sites.map((s) => {
          const src = d.sources.find((x) => x.kind === s.source);
          const php = src?.php && branches.includes(src.php) ? src.php : "";
          return { choisi: false, nom: s.nom_propose, php, convertir: s.type_site === "wordpress", mariadb: "" };
        });
      })
      .catch((e) => (erreur = tErreur(e)));
    const off = api.onMigration((p) => (etapes[p.site] = p));
    return () => { off.then((f) => f()); };
  });

  const NOM_VALIDE = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

  /** Message d'erreur pour le nom choisi (null si correct). */
  function problemeNom(i: number): string | null {
    const n = lignes[i].nom;
    if (!NOM_VALIDE.test(n)) return t("import.nom_invalide");
    if (detection?.existants.includes(n)) return t("import.nom_existe");
    if (lignes.some((l, j) => j !== i && l.choisi && l.nom === n)) return t("import.nom_double");
    return null;
  }

  const choisis = $derived(lignes.map((l, i) => ({ l, i })).filter((x) => x.l.choisi));
  const valide = $derived(choisis.length > 0 && choisis.every((x) => problemeNom(x.i) === null));

  async function importer() {
    if (!detection) return;
    erreur = null;
    enCours = true;
    etapes = {};
    try {
      const items = choisis.map(({ l, i }) => {
        const s = detection!.sites[i];
        return { source: s.source, dossier: s.dossier, nom: l.nom, type_site: s.type_site, base: s.base_trouvee ? s.base : null, php: l.php === "" ? null : l.php, convertir_url: l.convertir, mariadb: l.mariadb === "" ? null : l.mariadb };
      });
      resultats = await api.migrationRun(items);
      termine();
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      enCours = false;
    }
  }

  const nomSource = (k: string) => (k === "wamp" ? "WAMP" : "Laragon");
</script>

<section class="panneau">
  <header>
    <h3>{t("import.titre")}</h3>
    <span class="espace"></span>
    <button onclick={fermer} disabled={enCours}>✕</button>
  </header>

  <p class="muted petit">{t("import.intro")}</p>
  {#if erreur}<pre class="erreur">{erreur}</pre>{/if}

  {#if detection && detection.sources.length === 0}
    <p>{t("import.rien")}</p>
  {:else if detection && !resultats}
    <div class="sources">
      {#each detection.sources as s}
        <span class="badge">{nomSource(s.kind)} · {s.root} · {s.serveurs.map((x) => `${x.label} (${t("import.n_bases", { n: x.bases.length })})`).join(", ") || t("import.sans_base")}</span>
      {/each}
    </div>

    <table>
      <thead>
        <tr><th></th><th>{t("import.col_origine")}</th><th>{t("import.col_base")}</th><th>{t("import.col_nom")}</th><th>PHP</th>{#if mariadbs.length > 1}<th>{t("import.col_base_cible")}</th>{/if}<th>{t("import.col_adresses")}</th></tr>
      </thead>
      <tbody>
        {#each detection.sites as s, i (s.source + s.dossier)}
          {@const pb = lignes[i]?.choisi ? problemeNom(i) : null}
          <tr class:choisi={lignes[i]?.choisi}>
            <td><input type="checkbox" bind:checked={lignes[i].choisi} disabled={enCours} /></td>
            <td>
              <strong>{s.nom}</strong> <span class="badge">{nomSource(s.source)}</span>
              <div class="muted petit">{t(`import.type.${s.type_site}`)}</div>
            </td>
            <td class="petit">
              {#if s.base && s.base_trouvee}✓ {s.base}
              {:else if s.base}<span class="avert">⚠ {t("import.base_absente", { base: s.base })}</span>
              {:else}<span class="muted">—</span>{/if}
            </td>
            <td>
              <input class="nom mono" type="text" bind:value={lignes[i].nom} disabled={enCours} />
              {#if pb}<div class="erreur petit">{pb}</div>{/if}
            </td>
            <td>
              <select bind:value={lignes[i].php} disabled={enCours}>
                <option value="">{t("import.php_defaut")}</option>
                {#each branches as b}<option value={b}>{b}</option>{/each}
              </select>
            </td>
            {#if mariadbs.length > 1}
              <td>
                <select bind:value={lignes[i].mariadb} disabled={enCours}>
                  <option value="">{t("import.php_defaut")}</option>
                  {#each mariadbs.filter((m) => !(m.moteur === "mariadb" && m.defaut)) as m}<option value={m.choix}>{nomMoteur(m.moteur)} {m.branch}</option>{/each}
                </select>
              </td>
            {/if}
            <td>
              {#if s.type_site === "wordpress"}
                <label class="petit"><input type="checkbox" bind:checked={lignes[i].convertir} disabled={enCours} /> → {lignes[i].nom}.test</label>
              {:else}<span class="muted petit">—</span>{/if}
            </td>
          </tr>
          {#if etapes[lignes[i]?.nom] && lignes[i].choisi}
            <tr><td></td><td colspan="6" class="etape petit">⟳ {t(`import.etape.${etapes[lignes[i].nom].etape}`)} {etapes[lignes[i].nom].detail}</td></tr>
          {/if}
        {/each}
      </tbody>
    </table>

    <div class="pied">
      <button class="primaire" disabled={!valide || enCours} onclick={importer}>
        {enCours ? t("import.en_cours") : `⬇ ${t("import.lancer", { n: choisis.length })}`}
      </button>
      <span class="muted petit">{t("import.avertissement")}</span>
    </div>
  {:else if !detection}
    <p class="muted">{t("import.detection")}</p>
  {/if}

  {#if resultats}
    <ul class="resultats">
      {#each resultats as r (r.nom)}
        <li>
          {#if r.ok}
            <span class="ok">✓</span> <strong>{r.nom}</strong>
            {#if r.base}<span class="muted petit">· {t("import.base", { base: r.base })}</span>{/if}
            {#if r.nouvelle_url}<button class="lien" onclick={() => openUrl(r.nouvelle_url!)}>{r.nouvelle_url} ↗</button>{/if}
          {:else}
            <span class="erreur">✗ <strong>{r.nom}</strong> : {r.erreur ? tErreur(r.erreur) : ""}</span>
          {/if}
        </li>
      {/each}
    </ul>
    <p class="petit">{t("import.suite")}</p>
  {/if}
</section>

<style>
  .panneau { background: var(--bg-card); border: 1px solid var(--accent); border-radius: 8px; padding: 14px 18px; margin-bottom: 16px; }
  header { display: flex; align-items: center; gap: 10px; }
  h3 { margin: 0; }
  .espace { flex: 1; }
  .sources { display: flex; gap: 8px; flex-wrap: wrap; margin: 8px 0 12px; }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 6px 8px; border-bottom: 1px solid var(--border); }
  td { padding: 6px 8px; border-top: 1px solid var(--border); vertical-align: middle; }
  tr.choisi { background: var(--bg-hover); }
  input.nom { font: inherit; padding: 4px 6px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); width: 180px; }
  select { font: inherit; padding: 4px 6px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  .etape { color: var(--accent); border-top: 0; padding-top: 0; }
  .pied { display: flex; gap: 12px; align-items: center; margin-top: 12px; }
  .resultats { padding-left: 4px; list-style: none; }
  .resultats li { margin: 6px 0; }
  .ok { color: var(--ok); font-weight: 700; }
  .avert { color: #ca8a04; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .petit { font-size: 12px; }
</style>
