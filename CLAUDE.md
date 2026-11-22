# Rustserv — guide du projet

Remplaçant de Laragon : stack de développement web locale, **portable** (un seul dossier,
pas de registre), pour Windows 10/11. Tauri 2 (Rust) + Svelte 5 (TypeScript).

## Commandes

| Action | Commande (PowerShell, à la racine) |
|---|---|
| Lancer en dev (fenêtre + rechargement à chaud ; compile aussi le helper) | `pnpm tauri dev` |
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

## PHP multi-versions (phase 2)

- Les choix se font par **branche** (« 8.3 ») : `settings.versions.php` (défaut), `settings.site_php`
  (site → branche). Une seule version **active** par branche (`Stack::php_for_branch`) : celle de
  `settings.php_pins[branche]` si installée, sinon la plus récente installée. Les ports dépendent
  de la branche, donc deux versions d'une même branche ne tournent jamais ensemble.
- `Stack::php_needed` = défaut + PHP des sites existants ; seules ces versions tournent.
- `Orchestrator::apply` reconfigure à chaud après un changement (arrête les pools inutiles,
  démarre les nouveaux, redémarre le serveur web) **sans démarrer ce qui était arrêté**.
- http://localhost/<dossier>/ utilise déjà le PHP du site (nginx : `location ^~ /<dossier>/`
  imbriquée ; Apache : `<Directory>` + `SetHandler` dans le vhost par défaut).
- Mise à jour de branche : `Orchestrator::upgrade_php_branch` (install → `php::migrate_ini`
  (remplace les chemins de l'ancien dossier) → `php::copy_extra_dlls` (DLL absentes de la nouvelle
  version) → `php::check` (`php -m`, tout « PHP Startup » = échec) → apply → suppression
  facultative). Échec de validation ⇒ retour arrière complet. L'ancien php.ini reste dans etc/.
- Nouvelles versions : `catalog::php_remote` lit releases.json (NTS x64), `Catalog::merge` +
  `save` dans catalog.json. Recherche automatique une fois par session à l'ouverture de la page PHP.

## Extensions PHP (phase 3)

- État (`php::extensions::scan`) = DLL de `ext/` + lignes du php.ini + `php -n -m` (intégrées,
  non désactivables) + `php -c ini -m` (réellement chargées → détecte « activée mais en erreur »).
  Alias module/DLL : `gd2` (PHP 7.4) → module `gd`, `oci8_19` → `oci8` (`module_name`, `dll_for`).
- Toute modification passe par `php::transaction::modify` : sauvegarde dans
  `etc/php/<v>/backups/` (10 max) → changement → `php -m` → restauration si avertissement.
- Dépendances et ordre (`DEPENDANCES`) : activer exif active mbstring **avant** ; désactiver une
  extension requise est refusé (`extension.requise`). Zend : opcache, xdebug.
- OPcache intégré (PHP ≥ 8.5) : piloté par `opcache.enable`. Activer OPcache règle aussi
  `opcache.revalidate_freq=0` (code modifié visible immédiatement en dev).
- PECL : builds officiels `downloads.php.net/~windows/pecl/releases/<ext>/<v>/php_<ext>-<v>-<branche>-nts-<vsXX>-x64.zip`
  (versions stables, 6 plus récentes examinées). **Pas de SHA256 publié** : HTTPS + empreinte
  calculée et enregistrée dans `etc/php/<v>/pecl.json` + validation `php -m`. `php_<ext>.dll` → ext/,
  autres DLL (ImageMagick…) → dossier PHP sans écraser les DLL officielles. Rien pour PHP 8.6 (fin 2026).
- Profils (`php/profiles.json`) : additifs, plan affiché avant application ; échecs PECL signalés
  sans bloquer le reste ; une seule transaction php.ini.
- Après modification, seuls les php-cgi **de cette version** sont relancés, s'ils tournaient.

## Sites, HTTPS et terminal (phase 4)

- **hosts** : `crates/helper` = lib (logique pure, sans dépendance, testée) + `rustserv-helper.exe`
  (`windows_subsystem`, lancé via `ShellExecuteExW` verbe `runas` → fenêtre UAC). Seul le bloc
  `# BEGIN Rustserv … # END Rustserv` est réécrit ; BOM/CRLF/entrées tierces conservés ;
  copie `hosts.rustserv.bak`. Domaines acceptés uniquement en `.test .localhost .local .internal .lan`
  (le helper élevé lit un fichier écrit par l'app : il ne doit jamais pouvoir détourner un vrai site).
  Erreurs du helper → `<demande>.err` (sortie d'un processus élevé illisible). UAC refusé → `admin.refuse`.
- Le helper doit être à côté de `rustserv.exe` : `pnpm dev`/`pnpm build` le compilent d'abord.
- Le bandeau « domaines à mettre à jour » ne déclenche jamais l'UAC tout seul : clic utilisateur.
- **HTTPS** (`ssl.rs`, rcgen 0.14) : CA ECDSA P-256 10 ans dans `ssl/ca/` (clé, PEM, DER, ca.json),
  installée dans **CurrentUser\Root** (confirmation Windows, pas d'admin). L'émetteur est reconstruit
  avec `Issuer::new(ca_params(info), key)` (pas besoin de x509-parser). Certificats de site 825 j
  (`site.test` + `*.site.test`, `localhost` + 127.0.0.1), renouvelés à < 30 j ou si la CA change.
  HTTPS actif ⇔ CA présente : nginx `listen 443 ssl` + `http2 on`, Apache vhosts `*:443` via macros.
- **Terminal** (`terminal.rs`, portable-pty/ConPTY) : PowerShell (pwsh si présent) dans le dossier du
  site, PATH = PHP du site + Composer + MariaDB/bin, `PHPRC` = etc/php/<v>. ConPTY envoie `ESC[6n`
  au démarrage et **attend la réponse** (xterm.js répond ; un test doit répondre lui-même).
  Décodage UTF-8 par morceaux (`decode_utf8`). La page Terminal reste montée (masquée) entre pages.
- **Composer** : entrée catalogue `file: "composer.phar"` (fichier unique, pas d'archive) ; un `.phar`
  reçoit un lanceur `<nom>.bat` (`@php "%~dp0composer.phar" %*`).

## Rétrocompatibilité (phase 5, `compat/`)

- `runner` : PHPUnit (`vendor/phpunit/phpunit/phpunit`), Pest (`vendor/pestphp/pest/bin/pest`) ou
  script (relatif au site, `..` interdit) lancés **en parallèle** avec `php.exe -c <ini>` de chaque
  version. `--log-junit` → résumé (tests, assertions, échecs, erreurs). Sans phpunit.xml ni argument,
  `tests/` est passé (sinon PHPUnit affiche son aide, code 2). Script : sorties comparées par empreinte.
  Statut : code 0 = réussi ; tests exécutés + code ≠ 0 = échec ; sinon erreur. Sortie : 256 Ko max (fin).
- `tools` : Composer installe dans `<racine>/tools/` (COMPOSER_HOME = tmp/composer, portable) :
  PHP_CodeSniffer ^4, **PHPCompatibility ^10.0@alpha** (la 9.3.5 stable de 2019 ignore PHP 8),
  PHPStan ^2.1, plugin dealerdirect. Composer installé depuis le catalogue s'il manque.
  Les outils tournent avec le PHP installé le plus récent.
- Installation de PHPUnit/Pest dans un site (`ops::install_test_framework`) : `composer require --dev`
  exécuté avec la **plus ancienne** version de PHP cochée → Composer choisit la version compatible
  (ex. PHPUnit 9.6 si 7.4 est cochée). Pest : `allow-plugins.pestphp/pest-plugin` autorisé avant ;
  un `composer.json` minimal `{}` est créé s'il manque (`composer config` en exige un).
- `analysis` : phpcs `--standard=PHPCompatibility --runtime-set testVersion 7.4-8.5 --report=json`
  (vendor, node_modules, storage, var/cache ignorés) ; PHPStan avec une config neon temporaire
  (`phpVersion` 80300, `excludePaths.analyse` + `scanDirectories` vendor). JSON extrait entre le premier
  `{` et le dernier `}` (des avertissements peuvent précéder). Rapport commun `Rapport`.

## Gestion des bases : Adminer

- Choix validé par l'utilisateur (2026-11-22) : **Adminer** seul (phpMyAdmin 5.2.3 limité à PHP < 8.4,
  HeidiSQL écarté pour l'instant). Catalogue : fichier unique `adminer.php` (SHA256 = digest GitHub).
- Servi sous `/adminer/` (pas dans www/) par le vhost par défaut, avec le PHP par défaut, **accès
  local uniquement** (nginx `allow 127.0.0.1; deny all;`, Apache `Require local`) car connecté en root.
- Lanceur généré `etc/adminer/index.php` (gabarit `adminer.php.j2`) : `adminer_object()` étend
  `\Adminer\Adminer` (credentials, `login()` → true pour root sans mot de passe,
  `verifyLoginToken()` → false) et simule `$_POST['auth']` à la première visite → redirection 303
  vers `?server=127.0.0.1:<port>&username=root`. Apache : `Alias` + `ProxyFCGISetEnvIf` SCRIPT_FILENAME
  dans le `<Directory>` (le réglage global DOCUMENT_ROOT+SCRIPT_NAME ne convient pas à un Alias).
- Bouton « Gérer les bases » (carte MariaDB) : installe Adminer, démarre MariaDB + web si besoin.

## Import depuis Laragon / WAMP (`migration/`)

- Choix utilisateur (2026-11-22) : adresses WordPress **converties en .test** ; doublons entre
  sources **renommés** (suffixe -laragon / -wamp, modifiable).
- `detect` (lecture seule) : C:\laragon (bin/mysql/mysql-X, data/mysql-<branche>, usr/laragon.ini) et
  C:\wamp64 (bin/mysql|mariadb/*/data, wampmanager.conf). Noms de bases décodés (`@002d` → `-`).
  Base d'un site lue dans wp-config.php (DB_NAME) ou .env (DB_DATABASE).
- `run` : **rien n'est modifié chez Laragon/WAMP**. Pour chaque serveur d'origine : copie des données
  (sans binlog.*), démarrage du MySQL **d'origine** sur la copie (`--no-defaults`, port libre,
  `--mysqlx=OFF --disable-log-bin`, `--init-file` qui vide le mot de passe root **de la copie** :
  WAMP a un root protégé), mysqldump, arrêt. ⚠ MySQL 8+ Windows lance un processus **moniteur** :
  le serveur temporaire est lancé dans un **Job Object** (sinon orphelin). Démarrage jusqu'à 5 min.
- Import : `mariadb < dump` dans une base libre (`nom`, `nom_2`…) ; repli si collation `utf8mb4_0900_*`
  inconnue. wp-config.php / .env adaptés (copie `*.avant-rustserv`). Conversion : WP-CLI
  `search-replace //ancien //nouveau` (+ variante JSON `\/\/`), `option update siteurl/home`,
  .htaccess RewriteBase corrigé. Refus si Laragon/WAMP tourne encore (ports 80/443/3306/3307).
- ⚠ WordPress : les constantes `WP_HOME`/`WP_SITEURL` de wp-config.php **priment sur la base**.
  `adapter_adresses` les met à jour **avant** la base (sinon WordPress compare avec l'ancienne
  constante), convertit depuis l'adresse en base ET celle de la constante, puis fixe
  siteurl/home en SQL. `adapter_site` (bouton « Adapter les adresses » de la page Sites) refait
  cette conversion vers le domaine actuel : à utiliser après un changement de domaine.
- http://localhost/<site>/ ne convient pas à WordPress (une seule adresse canonique).
- Test réel `import_laragon_et_wamp_reel` : agence-web (Laragon, MySQL 8.4) et claude (WAMP, MySQL 9.1,
  localhost/claude → claude.test, page WordPress 200) — utilise les vraies installations, en lecture.

## MariaDB multi-versions (par site)

- Comme PHP : choix par **branche** (`settings.site_mariadb` : site → « 11.4 »). Chaque branche a son
  dossier de données (`data/mariadb-<branche>`), son instance et son port. Instance **par défaut** :
  service `mariadb`, port des réglages (3306), `etc/mariadb/my.ini`, `logs/mariadb-error.log`.
  Autres : service `mariadb-<branche>`, port « 3 + majeure + mineure » (11.4 → 31104, 10.11 → 31011),
  `etc/mariadb/my-<branche>.ini`, journaux/tmp nommés d'après l'instance (variable `instance` du gabarit).
- `Stack::mariadb_needed` = défaut + versions des sites existants ; seules celles-ci démarrent
  (`Group::Mariadb`), `apply` arrête les instances devenues inutiles.
- Changer la MariaDB d'un site (`site_set_mariadb`) : les deux instances démarrent
  (`Orchestrator::ensure_mariadb`), `migration::run::deplacer_base` copie la base (mariadb-dump de
  la source → import ; refus si la base existe déjà dans la cible ; repli collations `uca1400`/`0900`),
  puis `configurer_port` met à jour DB_HOST (wp-config) / DB_PORT (.env). L'ancienne base est conservée.
- Adminer : `?port=31104` pour se connecter à une autre instance. Terminal : client MariaDB de la
  version du site + `MYSQL_TCP_PORT`. Import Laragon/WAMP : version MariaDB choisissable par site.
- Test réel : `mariadb_multi_versions_par_site` (`target/pile-mariadb`, 12.3 → 11.4).

## Catalogue (`catalog/catalog.json`)

- Schéma 1. SHA256 **obligatoire**, URLs **https** obligatoires, `mirrors` essayés dans l'ordre.
- `subdir` : sous-dossier de l'archive à installer (Apache Lounge : `Apache24`) ; sinon le dossier
  unique de premier niveau est retiré automatiquement (nginx, mariadb) ; PHP est à plat.
- `vcruntime_min` : version minimale de vcruntime140.dll (VS16 → 14.20, VS17 → 14.30, VS18 → 14.50).
- Sources : PHP = `windows.php.net/downloads/releases/releases.json` (+ `/archives/`) ;
  Nginx = nginx.org (pas de SHA publié : calculé par nous) ; Apache = apachelounge.com
  (`<zip>.txt` contient le SHA256) ; MariaDB = `downloads.mariadb.org/rest-api/mariadb/<branche>/latest/`.
- Catalogue intégré copié dans la racine portable au 1er lancement ; ensuite, à **chaque démarrage**,
  `Catalog::absorb` y **ajoute** les paquets/versions nouveaux de la version intégrée (rien n'est
  retiré ni modifié). Bug corrigé : un catalog.json de la phase 1 n'avait jamais reçu Composer.
  Si `catalog.json` est corrompu → repli sur le catalogue intégré + avertissement dans l'UI.
- Mises à jour (`catalog::remote::fetch_all`) : php (releases.json), nginx (download.html, versions
  stables à mineure paire, SHA256 **calculé**), apache (Apache Lounge + .txt SHA256), mariadb (REST API,
  branches Stable), composer (/versions + .sha256sum), adminer et wp-cli (API GitHub, `digest`).
  Bouton dans Catalogue + vérification auto au démarrage si > 24 h (`app/src/verification.rs`,
  `etc/catalog-verification.json`). Badges « ⬆ disponible » / « nouveau ».
- MariaDB : `Stack::pick` sans choix explicite garde la version la plus récente **dont le dossier de
  données existe** (installer une nouvelle branche ne bascule pas vers une base vide).

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

### Phase 2 — PHP multi-versions ✅ (2026-11-22)
- [x] Plusieurs versions en parallèle, une par branche, pool par branche
- [x] Association site → branche (page Sites), PHP par défaut, épinglage (page PHP)
- [x] Mise à jour de branche en un clic avec reprise php.ini/DLL, validation et retour arrière
- [x] Détection des nouvelles versions (windows.php.net/releases.json)
- Tests réels : `multi_versions_php_par_site`, `mise_a_jour_de_branche_reelle` (dossier `target/pile-maj`)

### Phase 3 — Extensions PHP ✅ (2026-11-22)
- [x] Scan ext/*.dll + php.ini + php -m : active / inactive / intégrée / en erreur
- [x] Activer/désactiver (extension= / zend_extension=), sauvegarde, validation, restauration
- [x] PECL : xdebug, apcu, redis, igbinary, imagick, mongodb, pcov, yaml
- [x] Dépendances et ordre de chargement, DLL tierces
- [x] Profils Laravel / Symfony / WordPress avec aperçu
- Test réel : `extensions_pecl_et_profils_reels` (dossier `target/pile-ext`)

### Phase 4 — Sites, hosts, HTTPS, terminal ✅ (2026-11-22)
- [x] Domaines `.test` via fichier hosts (helper UAC, bloc balisé, validation stricte)
- [x] HTTPS : CA locale + certificats par site, nginx (HTTP/2) et Apache
- [x] Terminal intégré (xterm.js + ConPTY) avec le PHP du site, Composer, client MariaDB
- [x] Composer dans le catalogue
- Non testé automatiquement (fenêtres Windows à cliquer) : UAC du fichier hosts,
  confirmation d'installation de la CA. Testés : helper sans droits, HTTPS avec CA approuvée par le client.
- Option non faite : DNS local + règle NRPT (le fichier hosts suffit).
- Tests réels : `https_nginx_et_apache` (`target/pile-https`), `terminal_avec_le_php_du_site`

### Phase 5 — Tests de rétrocompatibilité ✅ (2026-11-22)
- [x] Script / PHPUnit / Pest sur plusieurs versions en parallèle, tableau comparatif
- [x] PHPCompatibility (PHP_CodeSniffer) et PHPStan, rapports par fichier
- Test réel : `retrocompatibilite_reelle` (projet « historique » dans `target/pile-test`)
- Non fait : annulation d'une analyse en cours (délais max : tests 10 min, analyses 15 min)

### Pistes pour la suite
- Boîte de dialogue native si l'app ne peut pas démarrer (aujourd'hui : message console)
- Annulation des analyses ; DNS local + NRPT (sous-domaines sans fichier hosts)
- Les tests réels partagent `target/pile-test` : les lancer avec `--test-threads=1`, machine peu chargée
