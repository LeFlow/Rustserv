<!--
  Page Paramètres : serveur web, ports, pool PHP, domaine, fuseau horaire.
  Les réglages s'appliquent au prochain (re)démarrage des services.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type Settings } from "../lib/api";

  let { racine }: { racine: string } = $props();

  let reglages = $state<Settings | null>(null);
  let avertissement = $state<string | null>(null);
  let message = $state<{ ok: boolean; texte: string } | null>(null);

  onMount(async () => {
    try {
      const v = await api.settingsGet();
      reglages = v.settings;
      if (v.warning) avertissement = tErreur(v.warning);
    } catch (e) {
      message = { ok: false, texte: tErreur(e) };
    }
  });

  async function enregistrer(ev: SubmitEvent) {
    ev.preventDefault();
    if (!reglages) return;
    try {
      // $state.snapshot : copie « simple » de l'objet réactif pour l'envoyer au backend.
      await api.settingsSave($state.snapshot(reglages));
      message = { ok: true, texte: t("parametres.enregistre") };
    } catch (e) {
      message = { ok: false, texte: tErreur(e) };
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
      <label>{t("parametres.tld")} <input type="text" bind:value={reglages.tld} /></label>
      <p class="muted petit">{t("parametres.tld_aide", { tld: reglages.tld })}</p>
    </fieldset>

    <div class="pied">
      <button class="primaire" type="submit">{t("actions.enregistrer")}</button>
      {#if message}<span class:ok={message.ok} class:erreur={!message.ok}>{message.texte}</span>{/if}
    </div>
  </form>
  <p class="muted mono petit">{t("app.racine", { racine })}</p>
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
  .pied { display: flex; gap: 12px; align-items: center; }
  .ok { color: var(--ok); }
  .erreur { color: var(--danger); }
  .avertissement { color: var(--accent); }
</style>
