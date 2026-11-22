<!--
  Page Catalogue : liste des paquets et versions, installation avec barre de
  progression, annulation et désinstallation.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type CatalogView, type InstallProgress, type ReleaseView } from "../lib/api";
  import { formatOctets } from "../lib/format";
  import type { Verification } from "../lib/api";

  let catalogue = $state<CatalogView | null>(null);
  let erreur = $state<string | null>(null);
  /** Progression par clé « paquet@version ». */
  let progression = $state<Record<string, InstallProgress>>({});
  /** Erreur par clé « paquet@version ». */
  let erreursLigne = $state<Record<string, string>>({});
  /** Versions affichées repliées : on montre par défaut la plus récente de chaque branche. */
  let toutAfficher = $state<Record<string, boolean>>({});
  let recherche = $state(false);
  let resultatRecherche = $state<string | null>(null);

  /** Résumé lisible d'une recherche de mises à jour. */
  function resumer(v: Verification): string {
    let m = v.ajouts.length ? t("catalogue.maj_trouvees", { liste: v.ajouts.join(", ") }) : t("catalogue.maj_aucune");
    if (v.erreurs.length) m += " " + t("catalogue.maj_erreurs", { liste: v.erreurs.map(([p]) => p).join(", ") });
    return m;
  }

  async function rechercher() {
    recherche = true;
    resultatRecherche = null;
    try {
      resultatRecherche = resumer(await api.catalogCheckUpdates());
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      recherche = false;
      await charger();
    }
  }

  const dateHeure = (ts: number) => new Date(ts * 1000).toLocaleString("fr-FR", { dateStyle: "short", timeStyle: "short" });

  const cle = (p: string, v: string) => `${p}@${v}`;

  async function charger() {
    try {
      catalogue = await api.catalogList();
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  onMount(() => {
    charger();
    const off = api.onInstallProgress((p) => {
      progression[cle(p.package, p.version)] = p;
    });
    // Vérification automatique au démarrage terminée : affichage des nouveautés.
    const off2 = api.onCatalogUpdated((v) => {
      resultatRecherche = resumer(v);
      charger();
    });
    return () => { off.then((f) => f()); off2.then((f) => f()); };
  });

  async function installer(pkg: string, r: ReleaseView) {
    const k = cle(pkg, r.version);
    delete erreursLigne[k];
    r.installing = true;
    try {
      await api.install(pkg, r.version);
    } catch (e) {
      // Une annulation volontaire n'est pas une erreur à afficher en rouge.
      if (!(e && typeof e === "object" && "code" in e && e.code === "annule")) {
        erreursLigne[k] = tErreur(e);
      }
    } finally {
      delete progression[k];
      await charger();
    }
  }

  async function desinstaller(pkg: string, r: ReleaseView) {
    const k = cle(pkg, r.version);
    try {
      await api.uninstall(pkg, r.version);
    } catch (e) {
      erreursLigne[k] = tErreur(e);
    }
    await charger();
  }

  /** Pourcentage (0-100) ou null si la taille totale est inconnue. */
  function pourcent(p: InstallProgress): number | null {
    return p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : null;
  }

  function libellePhase(p: InstallProgress): string {
    if (p.phase === "telechargement") {
      return p.total
        ? t("catalogue.telechargement", { fait: formatOctets(p.done), total: formatOctets(p.total) })
        : t("catalogue.telechargement_inconnu", { fait: formatOctets(p.done) });
    }
    return t(`catalogue.phase.${p.phase}`);
  }

  /** Versions visibles : la plus récente de chaque branche, sauf si « tout afficher ». */
  function visibles(id: string, releases: ReleaseView[]): ReleaseView[] {
    if (toutAfficher[id]) return releases;
    const vues = new Set<string>();
    return releases.filter((r) => {
      if (r.installed || r.installing) return true;
      if (vues.has(r.branch)) return false;
      vues.add(r.branch);
      return true;
    });
  }
</script>

{#if erreur}<p class="erreur">{erreur}</p>{/if}
{#if catalogue?.warning}<p class="avertissement">{t("catalogue.repli")} {tErreur(catalogue.warning)}</p>{/if}

{#if catalogue}
  <div class="barre">
    <button disabled={recherche} onclick={rechercher}>{recherche ? t("catalogue.maj_recherche") : `⟳ ${t("catalogue.maj_bouton")}`}</button>
    <span class="muted petit">
      {catalogue.derniere_verification ? t("catalogue.maj_derniere", { date: dateHeure(catalogue.derniere_verification) }) : t("catalogue.maj_jamais")}
      · {t("catalogue.maj_auto")}
    </span>
  </div>
  {#if resultatRecherche}<p class="info">{resultatRecherche}</p>{/if}
  {#each catalogue.packages as pkg, i (pkg.id)}
    {#if i === 0 || catalogue.packages[i - 1].category !== pkg.category}
      <h3 class="section">{t(`catalogue.sections.${pkg.category}`)}</h3>
    {/if}
    <section class="carte">
      <header>
        <h3>{pkg.name}</h3>
        <span class="badge">{t(`catalogue.categorie.${pkg.category}`)}</span>
        {#if pkg.maj}<span class="badge maj">⬆ {t("catalogue.maj_dispo", { version: pkg.maj })}</span>{/if}
      </header>
      {#if pkg.maj && pkg.id === "mariadb"}<p class="muted petit">{t("catalogue.maj_mariadb")}</p>{/if}
      {#if pkg.maj && pkg.id === "php"}<p class="muted petit">{t("catalogue.maj_php")}</p>{/if}
      <table>
        <tbody>
          {#each visibles(pkg.id, pkg.releases) as r (r.version)}
            {@const k = cle(pkg.id, r.version)}
            {@const p = progression[k]}
            <tr>
              <td class="version">{r.version}{#if r.nouveau} <span class="badge nouveau">{t("catalogue.nouveau")}</span>{/if}</td>
              <td class="muted mono">
                {#if r.build}{r.build.thread_safe ? "TS" : "NTS"} · {r.build.compiler.toUpperCase()} · {r.build.arch}{/if}
              </td>
              <td class="etat">
                {#if r.installing || p}
                  <div class="progression">
                    <div class="barre"><div class="rempli" class:indetermine={!p || pourcent(p) === null} style:width={p && pourcent(p) !== null ? `${pourcent(p)}%` : "30%"}></div></div>
                    <span class="muted">{p ? libellePhase(p) : t("catalogue.phase.preparation")}</span>
                  </div>
                {:else if r.installed}
                  <span class="ok">✓ {t("catalogue.installe")}</span>
                {/if}
                {#if erreursLigne[k]}<div class="erreur">{erreursLigne[k]}</div>{/if}
              </td>
              <td class="actions">
                {#if r.installing || p}
                  <button onclick={() => api.cancel(pkg.id, r.version)}>{t("actions.annuler")}</button>
                {:else if r.installed}
                  <button class="danger" onclick={() => desinstaller(pkg.id, r)}>{t("actions.supprimer")}</button>
                {:else}
                  <button class="primaire" onclick={() => installer(pkg.id, r)}>{t("actions.installer")}</button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if pkg.releases.length > visibles(pkg.id, pkg.releases).length || toutAfficher[pkg.id]}
        <button class="lien" onclick={() => (toutAfficher[pkg.id] = !toutAfficher[pkg.id])}>
          {toutAfficher[pkg.id] ? t("catalogue.moins") : t("catalogue.toutes", { n: pkg.releases.length })}
        </button>
      {/if}
    </section>
  {/each}
{/if}

<style>
  h3.section { font-size: 12px; text-transform: uppercase; letter-spacing: 0.05em; color: var(--muted); margin: 22px 0 8px; }
  h3.section:first-of-type { margin-top: 8px; }
  .carte { background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; padding: 14px 18px; margin-bottom: 14px; }
  header { display: flex; align-items: center; gap: 10px; margin-bottom: 6px; }
  h3 { margin: 0; font-size: 15px; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: 6px 8px; border-top: 1px solid var(--border); vertical-align: middle; }
  td.version { font-weight: 600; width: 90px; }
  td.etat { width: 45%; }
  td.actions { text-align: right; width: 120px; }
  .progression { display: flex; flex-direction: column; gap: 3px; }
  .barre { height: 6px; background: var(--bg-hover); border-radius: 3px; overflow: hidden; }
  .rempli { height: 100%; background: var(--accent); transition: width 0.2s; }
  .rempli.indetermine { animation: va-et-vient 1.2s infinite ease-in-out; }
  @keyframes va-et-vient { 0% { margin-left: 0; } 50% { margin-left: 70%; } 100% { margin-left: 0; } }
  .ok { color: var(--ok); }
  .erreur { color: var(--danger); font-size: 12px; margin-top: 4px; }
  .avertissement { color: var(--accent); }
  .barre { display: flex; gap: 12px; align-items: center; margin-bottom: 10px; flex-wrap: wrap; }
  .info { color: var(--ok); }
  .petit { font-size: 12px; }
  .badge.maj { background: var(--accent); color: white; }
  .badge.nouveau { background: var(--ok); color: white; font-weight: 400; }
  td.version { width: 160px; }
</style>
