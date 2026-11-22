<!--
  Page Paramètres : serveur web, ports, pool PHP, domaine, fuseau horaire.
  Les réglages s'appliquent au prochain (re)démarrage des services.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type Settings } from "../lib/api";

  let { racine, allerAuxSites }: { racine: string; allerAuxSites: () => void } = $props();

  let reglages = $state<Settings | null>(null);
  let tlds = $state<string[]>([]);
  let avertissement = $state<string | null>(null);
  let message = $state<{ ok: boolean; texte: string } | null>(null);
  /** Le domaine vient de changer : proposer d'aller déclarer les nouveaux domaines. */
  let domaineChange = $state(false);
  let enCours = $state(false);

  onMount(async () => {
    try {
      const v = await api.settingsGet();
      reglages = v.settings;
      tlds = v.tlds;
      if (v.warning) avertissement = tErreur(v.warning);
    } catch (e) {
      message = { ok: false, texte: tErreur(e) };
    }
  });

  async function enregistrer(ev: SubmitEvent) {
    ev.preventDefault();
    if (!reglages) return;
    enCours = true;
    domaineChange = false;
    try {
      // $state.snapshot : copie « simple » de l'objet réactif pour l'envoyer au backend.
      const r = await api.settingsSave($state.snapshot(reglages));
      domaineChange = r.domaine_change;
      let texte = t(r.domaine_change ? "parametres.enregistre_domaine" : "parametres.enregistre_simple");
      if (r.redemarrage_requis) texte += " " + t("parametres.redemarrage_requis");
      message = { ok: true, texte };
    } catch (e) {
      message = { ok: false, texte: tErreur(e) };
    } finally {
      enCours = false;
    }
  }
</script>

{#if avertissement}<p class="avertissement">{t("parametres.repli")} {avertissement}</p>{/if}

{#if reglages}
  <form onsubmit={enregistrer}>
    <fieldset>
      <legend>{t("parametres.serveur_web")}</legend>
      <label><input type="radio" bind:group={reglages.web_server} value="nginx" /> Nginx <span class="muted">— {t("parametres.nginx_aide")}</span></label>
      <label><input type="radio" bind:group={reglages.web_server} value="apache" /> Apache <span class="muted">— {t("parametres.apache_aide")}</span></label>
    </fieldset>

    <fieldset>
      <legend>{t("parametres.ports")}</legend>
      <div class="grille">
        <label>HTTP <input type="number" min="1" max="65535" bind:value={reglages.ports.http} /></label>
        <label>HTTPS <input type="number" min="1" max="65535" bind:value={reglages.ports.https} /></label>
        <label>MariaDB <input type="number" min="1" max="65535" bind:value={reglages.ports.mariadb} /></label>
        <label>MySQL <input type="number" min="1" max="65535" bind:value={reglages.ports.mysql} /></label>
        <label>PostgreSQL <input type="number" min="1" max="65535" bind:value={reglages.ports.postgresql} /></label>
      </div>
    </fieldset>

    <fieldset>
      <legend>PHP</legend>
      <div class="grille">
        <label>{t("parametres.pool")} <input type="number" min="1" max="10" bind:value={reglages.php_pool_size} /></label>
        <label>{t("parametres.fuseau")} <input type="text" bind:value={reglages.timezone} /></label>
      </div>
      <p class="muted petit">{t("parametres.pool_aide")}</p>
    </fieldset>

    <fieldset>
      <legend>{t("parametres.sites")}</legend>
      <label for="tld">{t("parametres.tld")}</label>
      <!-- Liste fermée (domaines réservés au local) : aucune saisie invalide possible. -->
      <select id="tld" bind:value={reglages.tld}>
        {#each tlds as d, i}
          <option value={d}>.{d}{i === 0 ? ` — ${t("parametres.recommande")}` : ""}</option>
        {/each}
      </select>
      <p class="apercu mono">www/monsite → monsite.{reglages.tld}</p>
      <p class="muted petit">{t(`parametres.tld_desc.${reglages.tld}`)}</p>
      <p class="muted petit">{t("parametres.tld_pourquoi")}</p>
    </fieldset>

    <div class="pied">
      <button class="primaire" type="submit" disabled={enCours}>{enCours ? t("parametres.application") : t("actions.enregistrer")}</button>
      {#if message}<span class:ok={message.ok} class:erreur={!message.ok}>{message.texte}</span>{/if}
      {#if domaineChange}<button type="button" class="lien" onclick={allerAuxSites}>{t("parametres.aller_sites")} →</button>{/if}
    </div>
  </form>
  <!-- À propos -->
  <section class="apropos">
    <img src="/img/logo-complet.png" alt="Rustserv" />
    <p>{t("parametres.apropos")}</p>
    <p class="muted mono petit">{t("app.racine", { racine })}</p>
  </section>
{/if}

<style>
  form { max-width: 640px; }
  fieldset { border: 1px solid var(--border); border-radius: 8px; padding: 12px 16px; margin: 0 0 14px; background: var(--bg-card); }
  legend { font-weight: 600; padding: 0 6px; }
  label { display: block; margin: 6px 0; }
  .grille { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  .grille label { display: flex; flex-direction: column; gap: 4px; }
  input[type="number"], input[type="text"] { font: inherit; padding: 5px 8px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); }
  .petit { font-size: 12px; margin: 6px 0 0; }
  select { font: inherit; padding: 5px 8px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); min-width: 260px; margin-top: 4px; }
  .apercu { margin: 8px 0 0; color: var(--accent); }
  .pied { display: flex; gap: 12px; align-items: center; }
  .ok { color: var(--ok); }
  .erreur { color: var(--danger); }
  .avertissement { color: var(--accent); }
  .apropos { max-width: 640px; margin-top: 24px; padding-top: 16px; border-top: 1px solid var(--border); text-align: center; }
  .apropos img { width: 320px; max-width: 100%; height: auto; }
  .apropos p { margin: 6px 0; }
</style>
