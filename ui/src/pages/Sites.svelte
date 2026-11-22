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
  import { api, type HostsStatus, type MariadbVersion, type PhpVersionView, type SiteView, type SslStatus } from "../lib/api";
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
  let mariadbs = $state<MariadbVersion[]>([]);

  /** Change la MariaDB d'un site : sa base est copiée vers la nouvelle version. */
  function choisirMariadb(s: SiteView, valeur: string) {
    executer("db-" + s.name, async () => {
      const r = await api.siteSetMariadb(s.name, valeur === "" ? null : valeur);
      if (r.de === r.vers) return t("sites.db_inchange", { site: s.name });
      return r.base
        ? t("sites.db_deplacee", { site: s.name, base: r.base, de: r.de, vers: r.vers, port: r.port })
        : t("sites.db_sans_base", { site: s.name, vers: r.vers });
    });
  }

  async function charger() {
    try {
      const [s, php, h, c, r, m] = await Promise.all([api.sitesList(), api.phpOverview(), api.hostsStatus(), api.sslStatus(), api.settingsGet(), api.mariadbVersions()]);
      mariadbs = m;
      sites = s;
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

{#if sites.length === 0}
  <p class="muted">{t("sites.aucun")}</p>
{:else}
  <table class="carte">
    <thead>
      <tr><th>{t("sites.col_site")}</th><th>PHP</th><th>MariaDB</th><th>{t("sites.col_ouvrir")}</th><th></th></tr>
    </thead>
    <tbody>
      {#each sites as s (s.name)}
        <tr>
          <td>
            <strong>{s.name}</strong>
            <div class="muted petit mono" title={s.root}>{s.host}{s.root !== s.dir ? ` → ${s.root.replace(s.dir, "").replaceAll("\\", "/")}` : ""}</div>
          </td>
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
            {#if mariadbs.length > 1}
              <select disabled={occupe["db-" + s.name]} value={s.mariadb_choice ?? ""} title={t("sites.db_aide")} onchange={(e) => choisirMariadb(s, e.currentTarget.value)}>
                <option value="">{t("sites.php_defaut", { version: mariadbs.find((x) => x.defaut)?.branch ?? "—" })}</option>
                {#each mariadbs.filter((x) => !x.defaut) as m}<option value={m.branch}>MariaDB {m.branch}</option>{/each}
              </select>
              {#if occupe["db-" + s.name]}<div class="muted petit">{t("sites.db_en_cours")}</div>{/if}
            {:else}
              <span class="muted petit">{s.mariadb_version ?? "—"}</span>
            {/if}
          </td>
          <td class="liens">
            {#if declare(s)}
              <button class="lien" onclick={() => openUrl(urlSite(s, false))}>http ↗</button>
              {#if httpsActif}<button class="lien" onclick={() => openUrl(urlSite(s, true))}>🔒 https ↗</button>{/if}
            {:else}
              <span class="muted petit" title={t("sites.domaine_non_declare")}>{t("sites.domaine_a_declarer")}</span>
            {/if}
            <button class="lien" onclick={() => openUrl(s.local_url)}>localhost ↗</button>
          </td>
          <td class="actions">
            {#if s.wordpress}
              <button title={t("sites.wp_fix_aide")} disabled={occupe["wp-" + s.name]} onclick={() => executer("wp-" + s.name, async () => {
                const r = await api.siteWpFix(s.name);
                return r.ancienne ? t("sites.wp_fix_ok", { site: s.name, de: r.ancienne, vers: r.nouvelle }) : t("sites.wp_fix_rien", { site: s.name, url: r.nouvelle });
              })}>🔧 {occupe["wp-" + s.name] ? t("sites.wp_fix_en_cours") : t("sites.wp_fix")}</button>
            {/if}
            <button onclick={() => terminal(s)}>&gt;_ {t("sites.terminal")}</button>
            <button class="lien" title={s.dir} onclick={() => api.openPath(s.dir).catch((e) => (erreur = tErreur(e)))}>📁</button>
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
  table.carte { width: 100%; border-collapse: collapse; overflow: hidden; }
  th { text-align: left; font-weight: 600; font-size: 12px; color: var(--muted); padding: 8px 12px; border-bottom: 1px solid var(--border); }
  td { padding: 8px 12px; border-top: 1px solid var(--border); vertical-align: middle; }
  select { font: inherit; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--border); background: var(--bg); color: var(--fg); }
  .liens button { margin-right: 6px; }
  .actions { text-align: right; white-space: nowrap; }
  .petit { font-size: 12px; }
  .avert { color: var(--accent); }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .info { color: var(--ok); }
</style>
