<!--
  Logo du type de site reconnu (WordPress, Laravel…), avec sa version en infobulle.
  Tracés SVG de Simple Icons (licence CC0) ; les marques appartiennent à leurs propriétaires.
  Logos noirs (Symfony, Kirby, Grav) : couleur du texte, pour rester visibles en thème sombre.
-->
<script lang="ts">
  import {
    siCakephp, siCodeigniter, siCraftcms, siDolibarr, siDrupal, siGrav, siHtml5, siJoomla, siKirby, siLaravel,
    siMoodle, siNextcloud, siOctobercms, siPhp, siPhpbb, siPrestashop, siShopware, siStatamic, siSymfony,
    siTypo3, siWordpress, siYii,
  } from "simple-icons";
  import type { TypeDetecte } from "../lib/api";

  let { type }: { type: TypeDetecte } = $props();

  type Icone = { title: string; path: string; hex: string };
  const icones: Record<string, Icone> = {
    wordpress: siWordpress, laravel: siLaravel, symfony: siSymfony, drupal: siDrupal, joomla: siJoomla,
    prestashop: siPrestashop, codeigniter: siCodeigniter, cakephp: siCakephp, yii: siYii, craftcms: siCraftcms,
    statamic: siStatamic, typo3: siTypo3, shopware: siShopware, moodle: siMoodle, nextcloud: siNextcloud,
    grav: siGrav, kirby: siKirby, octobercms: siOctobercms, dolibarr: siDolibarr, phpbb: siPhpbb, php: siPhp,
    statique: siHtml5,
  };
  /** Noms affichés des types sans icône (ou au nom plus lisible que celui de l'icône). */
  const noms: Record<string, string> = { magento: "Magento", php: "PHP", statique: "HTML", octobercms: "October CMS", craftcms: "Craft CMS" };

  const icone = $derived(icones[type.framework]);
  const nom = $derived(noms[type.framework] ?? icone?.title ?? type.framework);
  const titre = $derived(type.version ? `${nom} ${type.version}` : nom);
  const sombre = $derived(icone && parseInt(icone.hex, 16) < 0x333333);
</script>

{#if icone}
  <svg class="logo" role="img" viewBox="0 0 24 24" aria-label={titre} style:fill={sombre ? "currentColor" : `#${icone.hex}`}>
    <title>{titre}</title>
    <path d={icone.path} />
  </svg>
{:else}
  <!-- Pas d'icône libre (ex. Magento) : initiale. -->
  <span class="logo lettre" title={titre}>{nom.slice(0, 1)}</span>
{/if}

<style>
  .logo { width: 22px; height: 22px; display: block; }
  .lettre { display: grid; place-items: center; border-radius: 5px; background: #ee672f; color: white; font-weight: 700; font-size: 13px; }
</style>
