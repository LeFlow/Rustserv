<!--
  Affichage d'un rapport d'analyse (PHPCompatibility ou PHPStan) :
  totaux, puis un bloc repliable par fichier avec ses messages.
-->
<script lang="ts">
  import { t } from "../i18n";
  import type { Rapport } from "../lib/api";

  let { rapport }: { rapport: Rapport } = $props();
  /** Fichiers dépliés (tous dépliés par défaut s'il y en a peu). */
  let ouverts = $state<Record<string, boolean>>({});
  const deplie = (chemin: string) => ouverts[chemin] ?? rapport.fichiers.length <= 5;
  const secondes = (ms: number) => (ms / 1000).toLocaleString("fr-FR", { maximumFractionDigits: 1 });
</script>

<div class="rapport">
  <div class="resume">
    {#if rapport.erreurs === 0 && rapport.avertissements === 0}
      <span class="ok">✓ {t("compat.aucun_probleme", { outil: rapport.outil, cible: rapport.cible })}</span>
    {:else}
      <strong>{rapport.outil}</strong> · {rapport.cible} :
      <span class="err">{t("compat.n_erreurs", { n: rapport.erreurs })}</span>
      {#if rapport.avertissements}<span class="avert">{t("compat.n_avertissements", { n: rapport.avertissements })}</span>{/if}
      <span class="muted">{t("compat.n_fichiers", { n: rapport.fichiers.length })}</span>
    {/if}
    <span class="muted petit">· {secondes(rapport.duree_ms)} s</span>
  </div>

  {#each rapport.general as g}<pre class="general">{g}</pre>{/each}

  {#each rapport.fichiers as f (f.chemin)}
    <div class="fichier">
      <button class="entete" onclick={() => (ouverts[f.chemin] = !deplie(f.chemin))}>
        <span>{deplie(f.chemin) ? "▾" : "▸"}</span>
        <span class="mono">{f.chemin}</span>
        <span class="muted petit">({f.messages.length})</span>
      </button>
      {#if deplie(f.chemin)}
        <table>
          <tbody>
            {#each f.messages as m, i (i)}
              <tr>
                <td class="ligne mono">{m.ligne}</td>
                <td><span class="pastille {m.gravite}">{t(`compat.gravite.${m.gravite}`)}</span></td>
                <td>
                  {m.message}
                  {#if m.regle}<div class="muted mono petit">{m.regle}</div>{/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  {/each}
</div>

<style>
  .rapport { margin-top: 12px; }
  .resume { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; margin-bottom: 8px; }
  .ok { color: var(--ok); font-weight: 600; }
  .err { color: var(--danger); font-weight: 600; }
  .avert { color: #ca8a04; font-weight: 600; }
  .general { color: var(--danger); white-space: pre-wrap; font-family: inherit; }
  .fichier { border: 1px solid var(--border); border-radius: 6px; margin-bottom: 6px; background: var(--bg-card); }
  .entete { display: flex; gap: 8px; width: 100%; text-align: left; border: 0; background: none; padding: 6px 10px; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: 5px 10px; border-top: 1px solid var(--border); vertical-align: top; font-size: 13px; }
  td.ligne { width: 50px; text-align: right; color: var(--muted); }
  .pastille { font-size: 11px; padding: 1px 7px; border-radius: 10px; color: white; white-space: nowrap; }
  .pastille.erreur { background: var(--danger); }
  .pastille.avertissement { background: #ca8a04; }
  .petit { font-size: 12px; }
</style>
