// Appels au backend Rust, typés. Toute l'interface passe par ce fichier
// (jamais d'invoke() direct dans les composants) : un seul endroit à modifier
// si une commande change.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ErreurBackend } from "../i18n";

export interface AppInfo {
  version: string;
  racine: string;
}

export type Categorie = "web" | "database" | "php" | "tool";

export interface Build {
  thread_safe: boolean;
  compiler: string;
  arch: string;
}

export interface ReleaseView {
  version: string;
  branch: string;
  build: Build | null;
  installed: boolean;
  installing: boolean;
}

export interface PackageView {
  id: string;
  name: string;
  category: Categorie;
  releases: ReleaseView[];
}

export interface CatalogView {
  updated: string;
  packages: PackageView[];
  warning: ErreurBackend | null;
}

export interface InstalledPackage {
  package: string;
  version: string;
  sha256: string;
  installed_at: number;
  path: string;
}

export type InstallPhase = "telechargement" | "extraction" | "termine";

export interface InstallProgress {
  package: string;
  version: string;
  phase: InstallPhase;
  done: number;
  total: number | null;
}

// ------------------------------------------------------------ services

export type Groupe = "web" | "php" | "mariadb";

export type EtatService =
  | { etat: "arrete" }
  | { etat: "demarrage" }
  | { etat: "en_cours"; pid: number; depuis: number }
  | { etat: "arret" }
  | { etat: "erreur"; erreur: ErreurBackend };

export interface StatusEvent {
  id: string;
  state: EtatService;
}

export interface GroupView {
  group: Groupe;
  software: string;
  version: string | null;
  installed: boolean;
  ports: number[];
  state: EtatService;
  members: StatusEvent[];
  log_file: string;
}

// ------------------------------------------------------------ réglages

export interface Settings {
  web_server: "nginx" | "apache";
  ports: { http: number; https: number; mariadb: number };
  versions: { nginx: string | null; apache: string | null; mariadb: string | null; php: string | null };
  php_pool_size: number;
  tld: string;
  timezone: string;
}

export interface SettingsView {
  settings: Settings;
  warning: ErreurBackend | null;
  /** Domaines de sites autorisés (le premier est recommandé). */
  tlds: string[];
}

export interface SaveResult {
  /** Le domaine des sites a changé : les domaines doivent être redéclarés dans Windows. */
  domaine_change: boolean;
  /** Des services tournent avec les anciens réglages. */
  redemarrage_requis: boolean;
}

// ------------------------------------------------------------ PHP & sites

export interface PhpVersionView {
  version: string;
  branch: string;
  active: boolean;
  default: boolean;
  pinned: boolean;
  running: boolean;
  ports: number[];
  update: string | null;
  sites: string[];
  ini: string;
}

export interface PhpView {
  versions: PhpVersionView[];
  last_check: number | null;
}

export interface MigrationReport {
  ini_repris: boolean;
  dll_copiees: string[];
}

export interface SiteView {
  name: string;
  host: string;
  dir: string;
  root: string;
  php_choice: string | null;
  php_version: string | null;
  php_missing: boolean;
  local_url: string;
}

// ------------------------------------------------------------ extensions

export type EtatExtension = "active" | "inactive" | "integree" | "erreur";

export interface ExtensionInfo {
  name: string;
  state: EtatExtension;
  zend: boolean;
  description: string | null;
  dependances: string[];
  requiert: string | null;
  erreur: string | null;
  pecl: string | null;
}

export interface PeclEntry {
  name: string;
  description: string | null;
}

export interface PeclRecord {
  version: string;
  url: string;
  sha256: string;
  files: string[];
}

export interface Profile {
  id: string;
  nom: string;
  description: string;
  extensions: string[];
  pecl: string[];
  ini: Record<string, string>;
}

export interface ProfilePlan {
  profil: string;
  deja_ok: string[];
  a_activer: string[];
  a_telecharger: string[];
  reglages: { cle: string; avant: string | null; apres: string }[];
  indisponibles: string[];
}

export interface ProfileResult {
  plan: ProfilePlan;
  echecs_pecl: [string, ErreurBackend][];
}

// ------------------------------------------------------------ domaines, HTTPS, terminal

export interface HostsStatus {
  declares: string[];
  manquants: string[];
  en_trop: string[];
}

export interface CaInfo {
  nom: string;
  cree_le: number;
  expire_le: number;
  empreinte: string;
}

export interface SslStatus {
  ca: CaInfo | null;
  installee: boolean;
  ca_pem: string;
}

export interface TerminalInfo {
  id: number;
  cwd: string;
  php: string | null;
}

// ------------------------------------------------------------ compatibilité

export type TypeTest = "phpunit" | "pest" | "script";
export type StatutTest = "reussi" | "echec" | "erreur" | "delai_depasse";

export interface RunResult {
  version: string;
  statut: StatutTest;
  code: number | null;
  duree_ms: number;
  sortie: string;
  resume: { tests: number; assertions: number; echecs: number; erreurs: number; ignores: number } | null;
  empreinte: string;
}

export interface ToolsStatus {
  installes: boolean;
  phpcs: string | null;
  phpcompatibility: string | null;
  phpstan: string | null;
}

export interface RapportMessage {
  ligne: number;
  gravite: "erreur" | "avertissement";
  message: string;
  regle: string | null;
}

export interface Rapport {
  outil: string;
  cible: string;
  erreurs: number;
  avertissements: number;
  fichiers: { chemin: string; messages: RapportMessage[] }[];
  general: string[];
  duree_ms: number;
}

export const api = {
  compatToolsStatus: () => invoke<ToolsStatus>("compat_tools_status"),
  compatToolsInstall: () => invoke<ToolsStatus>("compat_tools_install"),
  compatRun: (site: string, kind: TypeTest, script: string | null, versions: string[]) =>
    invoke<RunResult[]>("compat_run", { site, kind, script, versions }),
  compatInstallFramework: (site: string, kind: TypeTest, versions: string[]) =>
    invoke<string>("compat_install_framework", { site, kind, versions }),
  compatPhpcs: (site: string, min: string, max: string | null) => invoke<Rapport>("compat_phpcs", { site, min, max }),
  compatPhpstan: (site: string, niveau: number, cible: string) => invoke<Rapport>("compat_phpstan", { site, niveau, cible }),

  hostsStatus: () => invoke<HostsStatus>("hosts_status"),
  hostsSync: () => invoke<HostsStatus>("hosts_sync"),
  sitesRefresh: () => invoke<void>("sites_refresh"),
  sslStatus: () => invoke<SslStatus>("ssl_status"),
  sslEnable: () => invoke<SslStatus>("ssl_enable"),
  sslDisable: () => invoke<SslStatus>("ssl_disable"),
  terminalOpen: (site: string | null, cols: number, rows: number) =>
    invoke<TerminalInfo>("terminal_open", { site, cols, rows }),
  terminalWrite: (id: number, data: string) => invoke<void>("terminal_write", { id, data }),
  terminalResize: (id: number, cols: number, rows: number) => invoke<void>("terminal_resize", { id, cols, rows }),
  terminalClose: (id: number) => invoke<void>("terminal_close", { id }),
  onTerminalOutput: (cb: (e: { id: number; data: string }) => void): Promise<UnlistenFn> =>
    listen<{ id: number; data: string }>("terminal-output", (e) => cb(e.payload)),
  onTerminalExit: (cb: (id: number) => void): Promise<UnlistenFn> => listen<number>("terminal-exit", (e) => cb(e.payload)),

  phpExtensions: (version: string) => invoke<ExtensionInfo[]>("php_extensions", { version }),
  phpExtensionSet: (version: string, name: string, enabled: boolean) =>
    invoke<void>("php_extension_set", { version, name, enabled }),
  peclList: () => invoke<PeclEntry[]>("php_pecl_list"),
  peclInstall: (version: string, name: string) => invoke<PeclRecord>("php_pecl_install", { version, name }),
  profiles: () => invoke<Profile[]>("php_profiles"),
  profilePlan: (version: string, profile: string) => invoke<ProfilePlan>("php_profile_plan", { version, profile }),
  profileApply: (version: string, profile: string) => invoke<ProfileResult>("php_profile_apply", { version, profile }),
  openPath: (path: string) => invoke<void>("open_path", { path }),

  phpOverview: () => invoke<PhpView>("php_overview"),
  phpSetDefault: (branch: string) => invoke<void>("php_set_default", { branch }),
  phpPin: (branch: string, version: string | null) => invoke<void>("php_pin", { branch, version }),
  phpCheckUpdates: () => invoke<string[]>("php_check_updates"),
  phpUpgrade: (branch: string, keepOld: boolean) => invoke<MigrationReport>("php_upgrade", { branch, keepOld }),
  sitesList: () => invoke<SiteView[]>("sites_list"),
  siteSetPhp: (site: string, branch: string | null) => invoke<void>("site_set_php", { site, branch }),

  servicesOverview: () => invoke<GroupView[]>("services_overview"),
  serviceStart: (group: Groupe) => invoke<void>("service_start", { group }),
  serviceStop: (group: Groupe) => invoke<void>("service_stop", { group }),
  serviceRestart: (group: Groupe) => invoke<void>("service_restart", { group }),
  startAll: () => invoke<void>("services_start_all"),
  stopAll: () => invoke<void>("services_stop_all"),
  restartAll: () => invoke<Groupe[]>("services_restart_all"),
  adminerOpen: () => invoke<string>("adminer_open"),
  restartPending: () => invoke<boolean>("services_restart_pending"),
  serviceLog: (group: Groupe) => invoke<string>("service_log", { group }),
  onServiceStatus: (cb: (e: StatusEvent) => void): Promise<UnlistenFn> =>
    listen<StatusEvent>("service-status", (e) => cb(e.payload)),
  settingsGet: () => invoke<SettingsView>("settings_get"),
  settingsSave: (settings: Settings) => invoke<SaveResult>("settings_save", { settings }),

  appInfo: () => invoke<AppInfo>("app_info"),
  catalogList: () => invoke<CatalogView>("catalog_list"),
  install: (pkg: string, version: string) =>
    invoke<InstalledPackage>("catalog_install", { package: pkg, version }),
  cancel: (pkg: string, version: string) => invoke<void>("catalog_cancel", { package: pkg, version }),
  uninstall: (pkg: string, version: string) => invoke<void>("catalog_uninstall", { package: pkg, version }),
  installedList: () => invoke<InstalledPackage[]>("installed_list"),
  onInstallProgress: (cb: (p: InstallProgress) => void): Promise<UnlistenFn> =>
    listen<InstallProgress>("install-progress", (e) => cb(e.payload)),
};
