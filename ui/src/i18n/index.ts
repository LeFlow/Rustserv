// Mini système de traduction.
//
// - Les textes sont dans des fichiers JSON par langue (fr.json, plus tard en.json…).
// - Les clés sont hiérarchiques : "erreurs.port.occupe".
// - Les paramètres s'écrivent {nom} dans le texte : "Le port {port} est utilisé".
//
// Le Rust renvoie des erreurs { code, params } : `tErreur()` les traduit.

import fr from "./fr.json";

type Dict = { [k: string]: string | Dict };
const langues: Record<string, Dict> = { fr };
let courante: Dict = langues.fr;

/** Change la langue (si elle existe). */
export function setLangue(code: string) {
  courante = langues[code] ?? langues.fr;
}

function lookup(dict: Dict, cle: string): string | undefined {
  let node: string | Dict | undefined = dict;
  for (const part of cle.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = node[part];
  }
  return typeof node === "string" ? node : undefined;
}

/** Traduit une clé ; renvoie la clé elle-même si elle est introuvable (facile à repérer). */
export function t(cle: string, params: Record<string, unknown> = {}): string {
  const texte = lookup(courante, cle) ?? lookup(langues.fr, cle) ?? cle;
  return texte.replace(/\{(\w+)\}/g, (_, nom) => String(params[nom] ?? `{${nom}}`));
}

/** Erreur telle qu'envoyée par le backend Rust. */
export interface ErreurBackend {
  code: string;
  params: Record<string, unknown>;
  detail: string;
}

/** Traduit une erreur renvoyée par `invoke()` (ou n'importe quelle exception). */
export function tErreur(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as ErreurBackend;
    const cle = `erreurs.${err.code}`;
    const msg = t(cle, err.params);
    return msg === cle ? t("erreurs.inconnue", { detail: err.detail }) : msg;
  }
  return t("erreurs.inconnue", { detail: String(e) });
}
