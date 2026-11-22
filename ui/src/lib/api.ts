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

export const api = {
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
  serviceLog: (group: Groupe) => invoke<string>("service_log", { group }),
  onServiceStatus: (cb: (e: StatusEvent) => void): Promise<UnlistenFn> =>
    listen<StatusEvent>("service-status", (e) => cb(e.payload)),
  settingsGet: () => invoke<SettingsView>("settings_get"),
  settingsSave: (settings: Settings) => invoke<void>("settings_save", { settings }),

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
