<!--
  Page Sites : chaque dossier de www/ est un site.
  - domaines .test : déclarés dans le fichier hosts de Windows (fenêtre administrateur) ;
  - HTTPS : autorité de certification locale approuvée par Windows ;
  - version de PHP par site, ouverture dans le navigateur, terminal dans le dossier.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { t, tErreur } from "../i18n";
  import { api, nomMoteur, type BaseVersion, type HostsStatus, type PhpVersionView, type RapportConversion, type SiteView, type SslStatus } from "../lib/api";
  import LogoSite from "../components/LogoSite.svelte";
  import { nouveauTerminal } from "../lib/terminaux.svelte";
  import Import from "./Import.svelte";

  let { allerAuTerminal }: { allerAuTerminal: () => void } = $props();

  let sites = $state<SiteView[]>([]);
  let branches = $state<string[]>([]);
  let defaut = $state<PhpVersionView | null>(null);
  let hosts = $state<HostsStatus | null>(null);
  let ssl = $state<SslStatus | null>(null);
  let ports = $state({ http: 80, https: 443 });
  let erreur = $state<string | null>(null);
  let info = $state<string | null>(null);
  let occupe = $state<Record<string, boolean>>({});
  let importOuvert = $state(false);
  let bases = $state<BaseVersion[]>([]);
  /** Base par défaut des sites sans choix (MariaDB par défaut, sinon MySQL, sinon PostgreSQL). */
  const baseDefaut = $derived((["mariadb", "mysql", "postgresql"] as const).map((m) => bases.find((b) => b.moteur === m && b.defaut)).find((b) => b));
  /** Site dont le menu « Ouvrir » est déplié (un seul à la fois). */
  let menuOuvert = $state<string | null>(null);

  function ouvrir(url: string) {
    menuOuvert = null;
    openUrl(url);
  }

  // ------------------------------------------------------------ migration de base
  /** Base cible choisie dans la liste, par site (par défaut : la base actuelle). */
  let cibles = $state<Record<string, string>>({});
  /** Migration en attente de confirmation. */
  let confirmation = $state<{ site: SiteView; cible: BaseVersion } | null>(null);
  /** Rapport de la dernière conversion vers / depuis PostgreSQL. */
  let rapport = $state<{ site: string; de: string; vers: string; r: RapportConversion } | null>(null);

  const libelleBase = (choix: string | null) => {
    const b = bases.find((x) => x.choix === choix);
    return b ? `${nomMoteur(b.moteur)} ${b.branch}` : (choix ?? "—");
  };
  const moteurDe = (choix: string | null) => choix?.split(":")[0] ?? null;
  /** Changement de famille (MariaDB / MySQL ↔ PostgreSQL) : conversion des données. */
  const estConversion = (s: SiteView, cible: BaseVersion) => (moteurDe(s.db_actuel) === "postgresql") !== (cible.moteur === "postgresql");

  function demanderMigration(s: SiteView) {
    const cible = bases.find((b) => b.choix === cibles[s.name]);
    if (cible && cible.choix !== s.db_actuel) confirmation = { site: s, cible };
  }

  function migrer() {
    if (!confirmation) return;
    const { site: s, cible } = confirmation;
    confirmation = null;
    rapport = null;
    executer("db-" + s.name, async () => {
      const r = await api.siteSetBase(s.name, cible.choix);
      if (r.rapport) rapport = { site: s.name, de: r.de, vers: r.vers, r: r.rapport };
      if (r.de === r.vers) return t("sites.db_inchange", { site: s.name });
      if (r.copiee) return t("sites.db_deplacee", { site: s.name, base: r.base ?? "", de: r.de, vers: r.vers, port: r.port });
      if (r.rapport) return t("sites.db_convertie", { site: s.name, base: r.base ?? "", de: r.de, vers: r.vers, port: r.port });
      if (r.base) return t("sites.db_vide", { site: s.name, base: r.base, vers: r.vers, port: r.port });
      return t("sites.db_sans_base", { site: s.name, vers: r.vers });
    });
  }

  async function charger() {
    try {
      const [s, php, h, c, r, m] = await Promise.all([api.sitesList(), api.phpOverview(), api.hostsStatus(), api.sslStatus(), api.settingsGet(), api.basesVersions()]);
      bases = m;
      sites = s;
      // Chaque liste revient sur la base actuelle du site.
      cibles = Object.fromEntries(s.map((x) => [x.name, x.db_actuel ?? ""]));
      branches = php.versions.filter((v) => v.active).map((v) => v.branch);
      defaut = php.versions.find((v) => v.default) ?? null;
      hosts = h;
      ssl = c;
      ports = { http: r.settings.ports.http, https: r.settings.ports.https };
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  onMount(charger);

  async function executer(cle: string, f: () => Promise<string | void>) {
    erreur = info = null;
    occupe[cle] = true;
    try {
      const m = await f();
      if (m) info = m;
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      occupe[cle] = false;
      await charger();
    }
  }

  const declare = (s: SiteView) => hosts?.declares.includes(s.host) ?? false;
  const httpsActif = $derived(!!ssl?.ca && ssl.installee);
  const suffixe = (port: number, defautPort: number) => (port === defautPort ? "" : `:${port}`);
  const urlSite = (s: SiteView, https: boolean) =>
    https ? `https://${s.host}${suffixe(ports.https, 443)}/` : `http://${s.host}${suffixe(ports.http, 80)}/`;
  const aDeclarer = $derived((hosts?.manquants.length ?? 0) + (hosts?.en_trop.length ?? 0));

  function terminal(s: SiteView) {
    nouveauTerminal(s.name, `${s.name} (PHP ${s.php_version ?? "—"})`);
    allerAuTerminal();
  }
</script>

<div class="barre">
  <button disabled={occupe.refresh} onclick={() => executer("refresh", async () => { await api.sitesRefresh(); return t("sites.actualise"); })}>⟳ {t("sites.actualiser")}</button>
  <button onclick={() => (importOuvert = true)}>⬇ {t("import.ouvrir")}</button>
  <span class="muted petit">{t("sites.aide")}</span>
</div>

{#if importOuvert}
  <Import fermer={() => (importOuvert = false)} termine={charger} />
{/if}

<svelte:window onclick={() => (menuOuvert = null)} />

{#if erreur}<pre class="erreur">{erreur}</pre>{/if}
{#if info}<p class="info">{info}</p>{/if}

<!-- Domaines .test -->
{#if hosts && aDeclarer > 0}
  <div class="bandeau">
    <div>
      <strong>{t("sites.hosts_titre", { n: aDeclarer })}</strong>
      <div class="petit">
        {#if hosts.manquants.length}{t("sites.hosts_ajout", { liste: hosts.manquants.join(", ") })}{/if}
        {#if hosts.en_trop.length}{t("sites.hosts_retrait", { liste: hosts.en_trop.join(", ") })}{/if}
      </div>
      <div class="muted petit">{t("sites.hosts_uac")}</div>
    </div>
    <button class="primaire" disabled={occupe.hosts} onclick={() => executer("hosts", async () => { await api.hostsSync(); return t("sites.hosts_ok"); })}>
      🛡 {occupe.hosts ? t("sites.hosts_attente") : t("sites.hosts_bouton")}
    </button>
  </div>
{/if}

<!-- HTTPS -->
{#if ssl}
  <div class="carte https">
    {#if httpsActif}
      <div>
        <strong>🔒 {t("sites.https_actif")}</strong>
        <div class="muted petit">{t("sites.https_ca", { nom: ssl.ca?.nom ?? "" })}</div>
      </div>
      <button class="danger" disabled={occupe.ssl} onclick={() => executer("ssl", async () => { await api.sslDisable(); return t("sites.https_desactive"); })}>{t("sites.https_desactiver")}</button>
    {:else}
      <div>
        <strong>{t("sites.https_titre")}</strong>
        <div class="muted petit">{t("sites.https_explication")}</div>
      </div>
      <button class="primaire" disabled={occupe.ssl} onclick={() => executer("ssl", async () => { await api.sslEnable(); return t("sites.https_ok"); })}>
        🔒 {occupe.ssl ? t("sites.https_attente") : t("sites.https_activer")}
      </button>
    {/if}
  </div>
{/if}

{#if rapport}
  <!-- Rapport de conversion vers / depuis PostgreSQL. -->
  <section class="carte rapport">
    <div class="rapport-tete">
      <strong>{t("sites.rapport_titre", { site: rapport.site, de: rapport.de, vers: rapport.vers })}</strong>
      <button class="lien" onclick={() => (rapport = null)}>✕ {t("sites.fermer")}</button>
    </div>
    <p>{t("sites.rapport_resume", { tables: rapport.r.tables.length, lignes: rapport.r.tables.reduce((n, x) => n + x.lignes, 0), secondes: rapport.r.secondes })}</p>
    {#if rapport.r.avertissements.length}
      <p class="avert">{t("sites.rapport_avertissements")}</p>
      <ul>{#each rapport.r.avertissements as a}<li>{t(`conversion.avert.${a.code}`, a.params)}</li>{/each}</ul>
    {/if}
    {#each ["vues", "procedures", "declencheurs", "evenements"] as const as k}
      {#if rapport.r.non_convertis[k].length}
        <p class="avert">{t(`conversion.non_convertis.${k}`, { liste: rapport.r.non_convertis[k].join(", ") })}</p>
      {/if}
    {/each}
    {#if !rapport.r.avertissements.length && !Object.values(rapport.r.non_convertis).some((l) => l.length)}
      <p class="info">{t("sites.rapport_parfait")}</p>
    {/if}
    <details>
      <summary class="muted">{t("sites.rapport_tables")}</summary>
      <ul class="mono petit">{#each rapport.r.tables as tb}<li>{tb.nom} : {tb.lignes}</li>{/each}</ul>
    </details>
  </section>
{/if}

{#if confirmation}
  {@const c = confirmation}
  <!-- Confirmation avant migration. -->
  <div class="voile" role="presentation" onclick={() => (confirmation = null)}>
    <div class="dialogue carte" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === "Escape" && (confirmation = null)}>
      <h3>{t("sites.migrer_titre", { site: c.site.name })}</h3>
      <p><strong>{libelleBase(c.site.db_actuel)}</strong> → <strong>{nomMoteur(c.cible.moteur)} {c.cible.branch}</strong> (port {c.cible.port})</p>
      <p>{estConversion(c.site, c.cible) ? t("sites.migrer_conversion") : t("sites.migrer_copie")}</p>
      <p class="muted petit">{t("sites.migrer_conserve")}</p>
      <div class="dialogue-boutons">
        <button onclick={() => (confirmation = null)}>{t("sites.annuler")}</button>
        <button class="primaire" onclick={migrer}>⇄ {t("sites.migrer")}</button>
      </div>
    </div>
  </div>
{/if}

{#if sites.length === 0}
  <p class="muted">{t("sites.aucun")}</p>
{:else}
  <table class="carte">
    <thead>
      <tr><th>{t("sites.col_site")}</th><th class="col-type"></th><th>PHP</th><th>{t("sites.col_base")}</th><th>{t("sites.col_ouvrir")}</th><th></th></tr>
    </thead>
    <tbody>
      {#each sites as s (s.name)}
        <tr>
          <td>
            <strong>{s.name}</strong>
            <div class="muted petit mono" title={s.root}>{s.host}{s.root !== s.dir ? ` → ${s.root.replace(s.dir, "").replaceAll("\\", "/")}` : ""}</div>
          </td>
          <td class="col-type">{#if s.type_site}<LogoSite type={s.type_site} />{/if}</td>
          <td>
            <select disabled={occupe[s.name]} value={s.php_choice ?? ""} onchange={(e) => executer(s.name, () => api.siteSetPhp(s.name, e.currentTarget.value === "" ? null : e.currentTarget.value))}>
              <option value="">{t("sites.php_defaut", { version: defaut?.branch ?? "—" })}</option>
              {#each branches as b}<option value={b}>PHP {b}</option>{/each}
              {#if s.php_choice && !branches.includes(s.php_choice)}
                <option value={s.php_choice}>PHP {s.php_choice} ({t("sites.non_installe")})</option>
              {/if}
            </select>
            {#if s.php_missing}<div class="avert petit">{t("sites.php_manquant", { branche: s.php_choice ?? "", version: s.php_version ?? "—" })}</div>{/if}
          </td>
          <td>
            <!-- Base actuelle (texte) → base cible (liste) → bouton, actif seulement si la cible diffère. -->
            <div class="base">
              <span class="actuelle" title={s.db_version ? `${nomMoteur(s.db_moteur)} ${s.db_version}` : ""}>
                {libelleBase(s.db_actuel)}{#if s.db_defaut}<span class="muted"> ({t("services.par_defaut")})</span>{/if}
              </span>
              {#if bases.length > 1}
                <span class="muted">→</span>
                <select bind:value={cibles[s.name]} disabled={occupe["db-" + s.name]} title={t("sites.db_aide")}>
                  {#each bases as b (b.choix)}
                    <!-- WordPress ne fonctionne pas avec PostgreSQL. -->
                    <option value={b.choix} disabled={s.wordpress && b.moteur === "postgresql"}>
                      {nomMoteur(b.moteur)} {b.branch}{b.choix === baseDefaut?.choix ? ` (${t("services.par_defaut")})` : ""}
                    </option>
                  {/each}
                </select>
                <button disabled={occupe["db-" + s.name] || !cibles[s.name] || cibles[s.name] === s.db_actuel} onclick={() => demanderMigration(s)}>
                  ⇄ {occupe["db-" + s.name] ? t("sites.db_en_cours") : t("sites.migrer")}
                </button>
              {/if}
            </div>
          </td>
          <td class="liens">
            <!-- Menu « Ouvrir » : adresses du site et dossier, regroupés pour garder une ligne compacte. -->
            <div class="menu">
              <button onclick={(e) => { e.stopPropagation(); menuOuvert = menuOuvert === s.name ? null : s.name; }}>{t("sites.ouvrir_menu")} ▾</button>
              {#if menuOuvert === s.name}
                <div class="menu-liste">
                  {#if declare(s)}
                    {#if httpsActif}<button onclick={() => ouvrir(urlSite(s, true))}>🔒 {urlSite(s, true)}</button>{/if}
                    <button onclick={() => ouvrir(urlSite(s, false))}>🌐 {urlSite(s, false)}</button>
                  {:else}
                    <span class="muted petit" title={t("sites.domaine_non_declare")}>{t("sites.domaine_a_declarer")}</span>
                  {/if}
                  <button onclick={() => ouvrir(s.local_url)}>🖥 {s.local_url}</button>
                  <hr />
                  <button onclick={() => { menuOuvert = null; api.openPath(s.dir).catch((e) => (erreur = tErreur(e))); }}>📁 {t("sites.ouvrir_dossier")}</button>
                </div>
              {/if}
            </div>
          </td>
          <td class="actions">
            {#if s.wordpress}
              <button title={t("sites.wp_fix_aide")} disabled={occupe["wp-" + s.name]} onclick={() => executer("wp-" + s.name, async () => {
                const r = await api.siteWpFix(s.name);
                return r.ancienne ? t("sites.wp_fix_ok", { site: s.name, de: r.ancienne, vers: r.nouvelle }) : t("sites.wp_fix_rien", { site: s.name, url: r.nouvelle });
              })}>🔧 {occupe["wp-" + s.name] ? t("sites.wp_fix_en_cours") : t("sites.wp_fix")}</button>
            {/if}
            <button onclick={() => terminal(s)}>&gt;_ {t("sites.terminal")}</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

<style>
  .barre { display: flex; gap: 12px; align-items: center; margin-bottom: 14px; }
  .bandeau { display: flex; justify-content: space-between; align-items: center; gap: 16px; padding: 12px 16px; margin-bottom: 12px; border-radius: 8px; border: 1px solid var(--accent); background: var(--bg-card); }
  .carte { background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; }
  .https { display: flex; justify-content: space-between; align-items: center; gap: 16px; padding: 12px 16px; margin-bottom: 12px; }
  /* Pas d'overflow: hidden (il couperait le menu « Ouvrir ») : coins arrondis via border-collapse: separate. */
  table.carte { width: 100%; border-collapse: separate; border-spacing: 0; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-top: 1px solid var(--border); vertical-align: middle; }
  select { font: inherit; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  .menu { position: relative; display: inline-block; }
  .menu > button { white-space: nowrap; }
  .menu-liste { position: absolute; right: 0; top: calc(100% + 4px); z-index: 10; min-width: 260px; display: flex; flex-direction: column; padding: 6px; background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; box-shadow: 0 6px 20px rgba(0, 0, 0, 0.18); }
  .menu-liste button { border: 0; background: none; text-align: left; padding: 6px 10px; border-radius: 6px; white-space: nowrap; }
  .menu-liste button:hover { background: var(--bg-hover); }
  .menu-liste hr { border: 0; border-top: 1px solid var(--border); margin: 4px 0; width: 100%; }
  .menu-liste span { padding: 6px 10px; }
  .actions { text-align: right; white-space: nowrap; }
  .col-type { width: 22px; padding-left: 0; padding-right: 0; }
  .base { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  .base .actuelle { white-space: nowrap; }
  .base button { white-space: nowrap; }
  .rapport { padding: 12px 16px; margin-bottom: 12px; }
  .rapport-tete { display: flex; justify-content: space-between; align-items: center; }
  .rapport ul { margin: 4px 0 8px; padding-left: 20px; }
  .voile { position: fixed; inset: 0; z-index: 50; background: rgba(0, 0, 0, 0.35); display: grid; place-items: center; }
  .dialogue { max-width: 520px; padding: 18px 20px; box-shadow: 0 10px 30px rgba(0, 0, 0, 0.3); }
  .dialogue h3 { margin: 0 0 10px; }
  .dialogue-boutons { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  .petit { font-size: 12px; }
  .avert { color: var(--accent); }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .info { color: var(--ok); }
</style>
