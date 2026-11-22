<!--
  Panneau « Extensions » d'une version de PHP :
  - profils (Laravel, Symfony, WordPress) avec aperçu des changements ;
  - extensions PECL à télécharger (xdebug, redis…) ;
  - liste filtrable avec interrupteurs activer / désactiver.
  Chaque modification est sauvegardée, vérifiée par PHP et annulée si elle échoue.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { t, tErreur } from "../i18n";
  import { api, type ExtensionInfo, type InstallProgress, type PeclEntry, type Profile, type ProfilePlan } from "../lib/api";
  import { formatOctets } from "../lib/format";

  let { version, ini, fermer }: { version: string; ini: string; fermer: () => void } = $props();

  let extensions = $state<ExtensionInfo[]>([]);
  let pecl = $state<PeclEntry[]>([]);
  let profils = $state<Profile[]>([]);
  let filtre = $state("");
  let voirIntegrees = $state(false);
  let chargement = $state(true);
  let erreur = $state<string | null>(null);
  let info = $state<string | null>(null);
  /** Extension (ou profil) en cours de modification. */
  let occupe = $state<string | null>(null);
  let progression = $state<InstallProgress | null>(null);
  let plan = $state<ProfilePlan | null>(null);

  async function charger() {
    try {
      extensions = await api.phpExtensions(version);
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      chargement = false;
    }
  }

  onMount(() => {
    charger();
    api.peclList().then((l) => (pecl = l));
    api.profiles().then((l) => (profils = l));
    const off = api.onInstallProgress((p) => {
      if (occupe) progression = p;
    });
    return () => { off.then((f) => f()); };
  });

  async function executer(cle: string, f: () => Promise<string | void>) {
    erreur = info = null;
    occupe = cle;
    progression = null;
    try {
      const msg = await f();
      if (msg) info = msg;
    } catch (e) {
      erreur = tErreur(e);
    } finally {
      occupe = null;
      progression = null;
      await charger();
    }
  }

  function basculer(e: ExtensionInfo) {
    const activer = e.state !== "active";
    executer(e.name, async () => {
      await api.phpExtensionSet(version, e.name, activer);
      return t(activer ? "ext.activee" : "ext.desactivee", { nom: e.name });
    });
  }

  function installerPecl(nom: string) {
    executer(nom, async () => {
      const r = await api.peclInstall(version, nom);
      return t("ext.pecl_ok", { nom, version: r.version });
    });
  }

  async function voirPlan(p: Profile) {
    erreur = info = null;
    try {
      plan = await api.profilePlan(version, p.id);
    } catch (e) {
      erreur = tErreur(e);
    }
  }

  function appliquer(p: ProfilePlan) {
    plan = null;
    executer(`profil-${p.profil}`, async () => {
      const r = await api.profileApply(version, p.profil);
      let msg = t("ext.profil_ok", { profil: nomProfil(p.profil) });
      for (const [ext, err] of r.echecs_pecl) msg += "\n" + t("ext.profil_echec_pecl", { nom: ext, detail: tErreur(err) });
      return msg;
    });
  }

  const nomProfil = (id: string) => profils.find((p) => p.id === id)?.nom ?? id;
  const installee = (nom: string) => extensions.some((e) => e.name === nom);

  const visibles = $derived(
    extensions.filter(
      (e) =>
        (voirIntegrees || e.state !== "integree") &&
        (!filtre || e.name.includes(filtre.toLowerCase()) || (e.description ?? "").toLowerCase().includes(filtre.toLowerCase())),
    ),
  );
  const nbActives = $derived(extensions.filter((e) => e.state === "active").length);
  const nbErreurs = $derived(extensions.filter((e) => e.state === "erreur").length);

  function libelleProgression(p: InstallProgress): string {
    if (p.phase === "telechargement" && p.total) return t("catalogue.telechargement", { fait: formatOctets(p.done), total: formatOctets(p.total) });
    return t(`catalogue.phase.${p.phase}`);
  }
</script>

<section class="panneau">
  <header>
    <h3>{t("ext.titre", { version })}</h3>
    <span class="muted petit">{t("ext.resume", { actives: nbActives, total: extensions.length })}</span>
    <span class="espace"></span>
    <button class="lien" onclick={() => api.openPath(ini).catch((e) => (erreur = tErreur(e)))}>{t("ext.ouvrir_ini")}</button>
    <button onclick={fermer}>✕</button>
  </header>

  {#if erreur}<pre class="erreur">{erreur}</pre>{/if}
  {#if info}<pre class="info">{info}</pre>{/if}
  {#if occupe}
    <p class="encours">⟳ {progression ? `${progression.package} : ${libelleProgression(progression)}` : t("ext.verification")}</p>
  {/if}
  {#if nbErreurs}<p class="erreur">{t("ext.en_erreur", { n: nbErreurs })}</p>{/if}

  <h4>{t("ext.profils")}</h4>
  <div class="profils">
    {#each profils as p (p.id)}
      <button disabled={!!occupe} title={p.description} onclick={() => voirPlan(p)}>{p.nom}</button>
    {/each}
  </div>

  {#if plan}
    <div class="plan">
      <strong>{t("ext.plan_titre", { profil: nomProfil(plan.profil) })}</strong>
      {#if plan.a_activer.length === 0 && plan.a_telecharger.length === 0 && plan.reglages.length === 0}
        <p>{t("ext.plan_rien")}</p>
      {:else}
        <ul>
          {#if plan.a_activer.length}<li>{t("ext.plan_activer")} <b>{plan.a_activer.join(", ")}</b></li>{/if}
          {#if plan.a_telecharger.length}<li>{t("ext.plan_telecharger")} <b>{plan.a_telecharger.join(", ")}</b></li>{/if}
          {#each plan.reglages as r}<li class="mono">{r.cle} : {r.avant ?? t("ext.non_defini")} → {r.apres}</li>{/each}
        </ul>
      {/if}
      {#if plan.indisponibles.length}<p class="avert">{t("ext.plan_indispo", { liste: plan.indisponibles.join(", ") })}</p>{/if}
      <p class="muted petit">{t("ext.plan_deja", { n: plan.deja_ok.length })}</p>
      <div class="boutons">
        {#if plan.a_activer.length || plan.a_telecharger.length || plan.reglages.length}
          {@const p = plan}
          <button class="primaire" onclick={() => appliquer(p)}>{t("ext.appliquer")}</button>
        {/if}
        <button onclick={() => (plan = null)}>{t("actions.annuler")}</button>
      </div>
    </div>
  {/if}

  <h4>{t("ext.pecl")}</h4>
  <div class="pecl">
    {#each pecl.filter((p) => !installee(p.name)) as p (p.name)}
      <button disabled={!!occupe} title={p.description ?? ""} onclick={() => installerPecl(p.name)}>⬇ {p.name}</button>
    {:else}
      <span class="muted petit">{t("ext.pecl_toutes")}</span>
    {/each}
  </div>

  <h4>{t("ext.liste")}</h4>
  <div class="filtres">
    <input type="search" placeholder={t("ext.filtrer")} bind:value={filtre} />
    <label class="petit"><input type="checkbox" bind:checked={voirIntegrees} /> {t("ext.voir_integrees")}</label>
  </div>

  {#if chargement}
    <p class="muted">{t("ext.chargement")}</p>
  {:else}
    <table>
      <tbody>
        {#each visibles as e (e.name)}
          <tr class={e.state}>
            <td class="interrupteur">
              {#if e.state === "integree"}
                <span class="muted petit" title={t("ext.integree_aide")}>{t("ext.etat.integree")}</span>
              {:else}
                <label class="switch" title={t(`ext.etat.${e.state}`)}>
                  <input type="checkbox" checked={e.state === "active" || e.state === "erreur"} disabled={!!occupe} onchange={() => basculer(e)} />
                  <span></span>
                </label>
              {/if}
            </td>
            <td>
              <strong class="mono">{e.name}</strong>
              {#if e.zend}<span class="badge">zend</span>{/if}
              {#if e.pecl}<span class="badge">PECL {e.pecl}</span>{/if}
              {#if occupe === e.name}<span class="muted petit"> ⟳</span>{/if}
              {#if e.description}<div class="muted petit">{e.description}</div>{/if}
              {#if e.dependances.length}<div class="muted petit">{t("ext.depend", { liste: e.dependances.join(", ") })}</div>{/if}
              {#if e.requiert}<div class="avert petit">{t("ext.requiert", { lib: e.requiert })}</div>{/if}
              {#if e.erreur}<div class="erreur petit">{e.erreur}</div>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</section>

<style>
  .panneau { background: var(--bg-card); border: 1px solid var(--accent); border-radius: 8px; padding: 14px 18px; margin-top: 16px; }
  header { display: flex; align-items: center; gap: 10px; }
  h3 { margin: 0; }
  h4 { margin: 16px 0 6px; font-size: 13px; color: var(--muted); text-transform: uppercase; letter-spacing: 0.04em; }
  .espace { flex: 1; }
  .profils, .pecl, .boutons { display: flex; gap: 8px; flex-wrap: wrap; }
  .plan { border: 1px dashed var(--border); border-radius: 6px; padding: 10px 14px; margin-top: 10px; }
  .plan ul { margin: 6px 0; padding-left: 20px; }
  .filtres { display: flex; gap: 12px; align-items: center; margin-bottom: 6px; }
  input[type="search"] { font: inherit; padding: 5px 8px; border: 1px solid var(--border); border-radius: 6px; background: var(--bg); color: var(--fg); width: 260px; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: 6px 8px; border-top: 1px solid var(--border); vertical-align: top; }
  td.interrupteur { width: 70px; padding-top: 8px; }
  tr.inactive strong { opacity: 0.7; }
  .badge { margin-left: 6px; }
  .petit { font-size: 12px; }
  .erreur { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .info { color: var(--ok); white-space: pre-wrap; font-family: inherit; }
  .avert { color: var(--accent); }
  .encours { color: var(--accent); font-weight: 600; }
  /* Interrupteur */
  .switch { position: relative; display: inline-block; width: 38px; height: 20px; }
  .switch input { opacity: 0; width: 0; height: 0; }
  .switch span { position: absolute; inset: 0; background: var(--bg-hover); border-radius: 20px; transition: 0.2s; cursor: pointer; }
  .switch span::before { content: ""; position: absolute; width: 14px; height: 14px; left: 3px; top: 3px; background: white; border-radius: 50%; transition: 0.2s; }
  .switch input:checked + span { background: var(--ok); }
  tr.erreur .switch input:checked + span { background: var(--danger); }
  .switch input:checked + span::before { transform: translateX(18px); }
  .switch input:disabled + span { opacity: 0.5; cursor: default; }
</style>
