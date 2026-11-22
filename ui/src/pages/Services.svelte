<!--
  Écran Services : état de chaque groupe (serveur web, PHP, MariaDB),
  boutons démarrer / arrêter / redémarrer et journaux.
  L'état se met à jour en direct grâce à l'événement « service-status ».
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { t, tErreur } from "../i18n";
  import { api, type GroupView, type Groupe } from "../lib/api";
  import { formatPorts, groupesPhp } from "../lib/format";

  let { allerAuCatalogue }: { allerAuCatalogue: () => void } = $props();

  let groupes = $state<GroupView[]>([]);
  let erreur = $state<string | null>(null);
  /** Groupe pour lequel une action est en cours (boutons désactivés). */
  let occupe = $state<Record<string, boolean>>({});
  let toutOccupe = $state(false);
  /** Réglages enregistrés pendant que des services tournaient. */
  let enAttente = $state(false);
  /** Journal affiché par groupe (null = replié). */
  let journaux = $state<Record<string, string | null>>({});

  async function charger() {
    try {
      groupes = await api.servicesOverview();
      enAttente = await api.restartPending();
      for (const g of groupes) {
        if (journaux[g.group] != null) journaux[g.group] = await api.serviceLog(g.group);
      }
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  // Plusieurs événements arrivent d'un coup (pool PHP) : on regroupe les rafraîchissements.
  let minuteur: ReturnType<typeof setTimeout> | undefined;
  function rafraichirBientot() {
    clearTimeout(minuteur);
    minuteur = setTimeout(charger, 150);
  }

  onMount(() => {
    charger();
    const off = api.onServiceStatus(rafraichirBientot);
    return () => { off.then((f) => f()); clearTimeout(minuteur); };
  });

  async function action(g: Groupe, f: (g: Groupe) => Promise<void>) {
    erreur = null;
    occupe[g] = true;
    try {
      await f(g);
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      occupe[g] = false;
      await charger();
    }
  }

  async function tout(f: () => Promise<void>) {
    erreur = null;
    toutOccupe = true;
    try {
      await f();
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      toutOccupe = false;
      await charger();
    }
  }

  /** Ouvre Adminer (installé et services démarrés automatiquement si besoin). */
  async function gererBases() {
    erreur = null;
    occupe.adminer = true;
    try {
      await openUrl(await api.adminerOpen());
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      occupe.adminer = false;
      await charger();
    }
  }

  async function basculerJournal(g: Groupe) {
    journaux[g] = journaux[g] == null ? await api.serviceLog(g) : null;
  }

  const enMarche = (g: GroupView) => g.state.etat === "en_cours";
  const enTransition = (g: GroupView) => g.state.etat === "demarrage" || g.state.etat === "arret";
  const actif = (g: GroupView) => enMarche(g) || enTransition(g);

  /** Durée lisible depuis un horodatage Unix. */
  function depuis(ts: number): string {
    const s = Math.max(0, Math.floor(Date.now() / 1000 - ts));
    if (s < 60) return t("services.depuis_s", { n: s });
    if (s < 3600) return t("services.depuis_min", { n: Math.floor(s / 60) });
    return t("services.depuis_h", { n: Math.floor(s / 3600) });
  }

  const webPort = $derived(groupes.find((g) => g.group === "web")?.ports[0] ?? 80);
  const urlAccueil = $derived(webPort === 80 ? "http://localhost/" : `http://localhost:${webPort}/`);
  const webEnMarche = $derived(groupes.some((g) => g.group === "web" && enMarche(g)));
  const unEnMarche = $derived(groupes.some((g) => enMarche(g)));
</script>

<div class="barre-actions">
  <button class="primaire" disabled={toutOccupe} onclick={() => tout(api.startAll)}>▶ {t("services.tout_demarrer")}</button>
  {#if unEnMarche}
    <button disabled={toutOccupe} title={t("services.tout_redemarrer_aide")} onclick={() => tout(async () => { await api.restartAll(); })}>⟳ {t("services.tout_redemarrer")}</button>
  {/if}
  <button disabled={toutOccupe} onclick={() => tout(api.stopAll)}>■ {t("services.tout_arreter")}</button>
  <span class="espace"></span>
  <button class="lien" disabled={!webEnMarche} onclick={() => openUrl(urlAccueil)}>{t("services.ouvrir", { url: urlAccueil })} ↗</button>
</div>

{#if enAttente && unEnMarche}
  <div class="bandeau">
    <span>⚠ {t("services.reglages_en_attente")}</span>
    <button class="primaire" disabled={toutOccupe} onclick={() => tout(async () => { await api.restartAll(); })}>⟳ {t("services.appliquer_maintenant")}</button>
  </div>
{/if}

{#if erreur}<pre class="erreur">{erreur}</pre>{/if}

{#each groupes as g (g.group)}
  <section class="carte">
    <div class="ligne">
      <span class="pastille {g.state.etat}" title={t(`services.etat.${g.state.etat}`)}></span>
      <div class="titre">
        <strong>{t(`services.groupe.${g.group}`, { logiciel: g.software === "apache" ? "Apache" : "Nginx" })}</strong>
        <span class="muted">
          {#if g.version}{g.software} {g.version}{/if}
          {#if g.ports.length}· {t(g.ports.length > 1 ? "services.ports" : "services.port", { ports: formatPorts(g.ports) })}{/if}
        </span>
      </div>
      <div class="etat">
        {t(`services.etat.${g.state.etat}`)}
        {#if g.state.etat === "en_cours"}<span class="muted">· {depuis(g.state.depuis)}</span>{/if}
      </div>
      <div class="boutons">
        {#if !g.installed}
          <button onclick={allerAuCatalogue}>{t("services.installer")}</button>
        {:else if actif(g)}
          <button disabled={occupe[g.group] || enTransition(g)} onclick={() => action(g.group, api.serviceRestart)}>⟳ {t("actions.redemarrer")}</button>
          <button disabled={occupe[g.group] || g.state.etat === "arret"} onclick={() => action(g.group, api.serviceStop)}>■ {t("actions.arreter")}</button>
        {:else}
          <button class="primaire" disabled={occupe[g.group]} onclick={() => action(g.group, api.serviceStart)}>▶ {t("actions.demarrer")}</button>
        {/if}
        <button class="lien" onclick={() => basculerJournal(g.group)}>{journaux[g.group] == null ? t("services.voir_journal") : t("services.masquer_journal")}</button>
      </div>
    </div>

    {#if g.state.etat === "erreur"}
      <pre class="erreur">{tErreur(g.state.erreur)}</pre>
    {/if}

    {#if g.group === "php" && g.members.length > 1}
      <!-- Un groupe de voyants par version de PHP (un voyant par processus). -->
      <div class="pool">
        {#each groupesPhp(g.members) as [version, membres] (version)}
          <span class="version-pool">
            <span class="mono">PHP {version}</span>
            {#each membres as m (m.id)}
              <span class="pastille petite {m.state.etat}" title={`${m.id} : ${t(`services.etat.${m.state.etat}`)}`}></span>
            {/each}
          </span>
        {/each}
        <span class="muted">{t("services.pool")}</span>
      </div>
    {/if}

    {#if g.group === "mariadb" && g.installed}
      <div class="bas">
        <button disabled={occupe.adminer} title={t("services.adminer_aide")} onclick={gererBases}>🗄 {occupe.adminer ? t("services.adminer_ouverture") : t("services.adminer")}</button>
      </div>
    {/if}

    {#if journaux[g.group] != null}
      <pre class="journal mono">{journaux[g.group] || t("services.journal_vide")}</pre>
      <p class="muted mono petit">{g.log_file}</p>
    {/if}
  </section>
{/each}

<style>
  .barre-actions { display: flex; gap: 8px; align-items: center; margin-bottom: 16px; }
  .espace { flex: 1; }
  .bandeau { display: flex; justify-content: space-between; align-items: center; gap: 16px; padding: 10px 16px; margin-bottom: 12px; border-radius: 8px; border: 1px solid var(--accent); background: var(--bg-card); }
  .carte { background: var(--bg-card); border: 1px solid var(--border); border-radius: 8px; padding: 14px 18px; margin-bottom: 12px; }
  .ligne { display: flex; align-items: center; gap: 14px; }
  .titre { display: flex; flex-direction: column; width: 340px; flex-shrink: 0; }
  .etat { flex: 1; }
  .boutons { display: flex; gap: 6px; align-items: center; }
  .pastille { width: 12px; height: 12px; border-radius: 50%; background: var(--muted); flex-shrink: 0; }
  .pastille.petite { width: 8px; height: 8px; display: inline-block; margin-right: 4px; }
  .pastille.en_cours { background: var(--ok); box-shadow: 0 0 6px var(--ok); }
  .pastille.demarrage, .pastille.arret { background: #eab308; animation: clignote 1s infinite; }
  .pastille.erreur { background: var(--danger); }
  @keyframes clignote { 50% { opacity: 0.4; } }
  .pool { display: flex; flex-direction: column; align-items: flex-start; gap: 4px; margin: 10px 0 0 26px; font-size: 12px; }
  .bas { display: flex; justify-content: flex-end; margin-top: 8px; }
  .version-pool { display: inline-flex; align-items: center; gap: 3px; margin-right: 8px; }
  .version-pool .mono { margin-right: 4px; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; margin: 10px 0 0; }
  .journal { background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 10px; max-height: 260px; overflow: auto; white-space: pre-wrap; margin: 12px 0 4px; }
  .petit { font-size: 11px; margin: 0; }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
