# Rustserv — guide du projet

Remplaçant de Laragon : stack de développement web locale, **portable** (un seul dossier,
pas de registre), pour Windows 10/11. Tauri 2 (Rust) + Svelte 5 (TypeScript).

## Commandes

| Action | Commande (PowerShell, à la racine) |
|---|---|
| Lancer en dev (fenêtre + rechargement à chaud) | `pnpm tauri dev` |
| Tests du noyau Rust | `cargo test -p rustserv-core` |
| Vérification des types du front | `pnpm check` |
| Build de l'interface seule | `pnpm build` |
| Build release (exe portable) | `pnpm tauri build` → `target/release/rustserv.exe` |

En dev, la racine portable est `./portable/` (ignorée par git). Surchargeable avec `RUSTSERV_ROOT`.
Niveau de log : variable `RUSTSERV_LOG` (ex. `debug`, `rustserv_core=trace`).

## Architecture

```
crates/core    bibliothèque sans Tauri : catalog/ process/ php/ services orchestrator settings sites templates/
crates/app     couche Tauri fine : fenêtre, tray, commandes → appelle core
crates/helper  rustserv-helper.exe, lancé avec UAC uniquement si besoin (hosts…)
ui/            interface Svelte 5 ; ui/src/lib/api.ts = seul point d'appel au backend
catalog/       catalog.json de référence (copié dans la racine portable)
```

Dossier portable : `bin/ etc/ data/ www/ ssl/ logs/ tmp/ catalog.json`.
- `bin/<paquet>/<paquet>-<version>/` : binaires **non modifiés** (remplaçables), + marqueur
  `.rustserv-install.json`. Ex. `bin/php/php-8.3.37`, `bin/nginx/nginx-1.30.7`.
- `tmp/downloads` (archives en cours, `.part`), `tmp/extract` (nettoyé automatiquement).
- Toutes les configs modifiées/générées vont dans `etc/` (php.ini dans `etc/php/<version>/php.ini`).
- Profil WebView2 dans `tmp/webview` (pas dans %LOCALAPPDATA%).

## Conventions

- **Erreurs** : `rustserv_core::Error` (thiserror). Jamais de texte utilisateur côté Rust :
  chaque variante a un `code()` (ex. `port.occupe`) + `params()`. Sérialisée en
  `{code, params, detail}`, traduite côté UI par `tErreur()` via `erreurs.<code>` dans
  `ui/src/i18n/fr.json`. Toute nouvelle variante ⇒ ajouter sa traduction.
- **i18n** : textes UI uniquement dans `ui/src/i18n/<langue>.json`, appel `t("cle", {params})`.
- **Code Windows** isolé dans `crates/core/src/platform/` (prévoir un équivalent unix plus tard).
- Commentaires et noms métier en français ; code idiomatique Rust/TS.
- Tests unitaires obligatoires pour : gestionnaire php.ini, catalogue.
- L'app ne tourne jamais en admin ; toute modif système passe par le helper et doit être réversible.
- TypeScript reste en 6.x tant que svelte-check ne supporte pas TS 7.
- Le test `error::tests::toutes_les_erreurs_sont_traduites` échoue si une erreur n'a pas
  sa traduction dans fr.json (ou si un `{param}` n'est pas fourni).
- Tests réseau réels : `cargo test -p rustserv-core --test reseau -- --ignored --nocapture`
  (vérifie que toutes les URLs du catalogue répondent, installe Nginx).

## Services : points techniques vérifiés (ne pas « simplifier »)

- Lancement : `platform::spawn_in_job` crée le processus **suspendu**, l'ajoute au Job, puis le
  relance (sinon les sous-processus créés immédiatement, workers nginx/Apache, échappent au Job).
- Arrêts propres : nginx `-s quit` ; Apache événement `ap<pid>_shutdown` ; MariaDB événement
  `MySQLShutdown<pid>` ; php-cgi kill. Vérifié : fichiers pid supprimés, « Shutdown complete ».
- nginx lancé avec `-p <racine>/` (il ouvre `logs/` au démarrage) et `-c etc/nginx/nginx.conf`.
- Apache + php-cgi : `ProxyFCGIBackendType GENERIC` + `ProxyFCGISetEnvIf … SCRIPT_FILENAME`,
  et noms de balancer **avec `/` final** dans `<Proxy>` et `SetHandler` (sinon erreur 500).
- `Paths::new` normalise la racine : un `..` dans SCRIPT_FILENAME → « No input file specified ».
- php-cgi : `PHP_FCGI_MAX_REQUESTS=0`, PATH préfixé par le dossier PHP, `cgi.force_redirect=0`.
- php.ini : créé une seule fois depuis php.ini-development, jamais écrasé ensuite.
- Groupes UI (`orchestrator.rs`) : démarrer `web` démarre `php` ; arrêter `php` arrête `web`.

## Catalogue (`catalog/catalog.json`)

- Schéma 1. SHA256 **obligatoire**, URLs **https** obligatoires, `mirrors` essayés dans l'ordre.
- `subdir` : sous-dossier de l'archive à installer (Apache Lounge : `Apache24`) ; sinon le dossier
  unique de premier niveau est retiré automatiquement (nginx, mariadb) ; PHP est à plat.
- `vcruntime_min` : version minimale de vcruntime140.dll (VS16 → 14.20, VS17 → 14.30, VS18 → 14.50).
- Sources : PHP = `windows.php.net/downloads/releases/releases.json` (+ `/archives/`) ;
  Nginx = nginx.org (pas de SHA publié : calculé par nous) ; Apache = apachelounge.com
  (`<zip>.txt` contient le SHA256) ; MariaDB = `downloads.mariadb.org/rest-api/mariadb/<branche>/latest/`.
- Mise à jour du catalogue intégré : copié dans la racine portable au 1er lancement seulement.
  Si `catalog.json` est corrompu → repli sur le catalogue intégré + avertissement dans l'UI.

## Choix techniques validés

- Serveur web : Nginx par défaut, Apache en option. Gabarits `minijinja`.
- PHP : php-cgi NTS en FastCGI, **pool de 4 processus par version** (php-cgi Windows ne traite
  qu'une requête à la fois), ports par plage : PHP X.Y → `9XY0`–`9XY3` (8.3 → 9830-9833,
  7.4 → 9740-9743). `PHP_FCGI_MAX_REQUESTS=0` + redémarrage auto par le superviseur.
- Processus enfants dans un Job Object Windows (`KILL_ON_JOB_CLOSE`) : pas d'orphelins.
- DNS `.test` : fichier hosts via helper (bloc `# BEGIN Rustserv`/`# END Rustserv`) ;
  DNS intégré + règle NRPT en option plus tard (port 53 déjà pris par svchost sur la machine de dev).
- SSL : CA locale générée avec `rcgen`, installée dans CurrentUser\Root (sans admin).
- Ports par défaut : 80 / 443 / 3306 (Laragon arrêté pendant le dev ; il est dans C:\laragon).

## Avancement

### Phase 1 — noyau ✅ (2026-11-22)
- [x] 1. Squelette : workspace, Tauri + Svelte, tray, racine portable, erreurs i18n, logs
- [x] 2. Catalogue : modèle, téléchargement, SHA256, extraction, progression, vcruntime, page UI
  - reste : proposer la mise à jour quand le catalogue intégré est plus récent que le fichier portable
- [x] 3. Superviseur (`process/`), Job Object, ports (propriétaire + plages réservées), relance auto
- [x] 4. Gabarits minijinja, Nginx + pool php-cgi, Apache (balancer + .htaccess), sites www/ auto
- [x] 5. MariaDB : init auto `data/mariadb-<branche>`, arrêt propre par événement
- [x] 6. Écran Services (groupes web/php/mariadb, journaux) + page Paramètres

Tests réels (binaires téléchargés dans `target/pile-test`, ~150 Mo) :
`cargo test -p rustserv-core --test pile_reelle -- --ignored --nocapture --test-threads=1`

### Phases suivantes
- [ ] Phase 2 — PHP multi-versions
- [ ] Phase 3 — Extensions PHP
- [ ] Phase 4 — Sites, hosts, HTTPS, terminal
- [ ] Phase 5 — Tests de rétrocompatibilité
