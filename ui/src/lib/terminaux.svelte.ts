// État partagé des terminaux (onglets), pour pouvoir en ouvrir un depuis la page
// Sites (« Terminal ici ») et l'afficher dans la page Terminal.

export interface Onglet {
  /** Identifiant local de l'onglet (le terminal Rust est créé à l'affichage). */
  cle: number;
  site: string | null;
  titre: string;
}

let compteur = 0;

export const terminaux = $state<{ onglets: Onglet[]; actif: number | null }>({ onglets: [], actif: null });

/** Ajoute un onglet (le terminal démarre quand la page Terminal l'affiche). */
export function nouveauTerminal(site: string | null, titre: string) {
  const cle = ++compteur;
  terminaux.onglets.push({ cle, site, titre });
  terminaux.actif = cle;
}

export function fermerOnglet(cle: number) {
  const i = terminaux.onglets.findIndex((o) => o.cle === cle);
  if (i < 0) return;
  terminaux.onglets.splice(i, 1);
  if (terminaux.actif === cle) terminaux.actif = terminaux.onglets[Math.max(0, i - 1)]?.cle ?? null;
}
