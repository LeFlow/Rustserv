# Rustserv â€” guide du projet

RemplaÃ§ant de Laragon : stack de dÃ©veloppement web locale, **portable** (un seul dossier,
pas de registre), pour Windows 10/11. Tauri 2 (Rust) + Svelte 5 (TypeScript).

## Commandes

| Action | Commande (PowerShell, Ã  la racine) |
|---|---|
| Lancer en dev (fenÃªtre + rechargement Ã  chaud ; compile aussi le helper) | `pnpm tauri dev` |
| Tests du noyau Rust | `cargo test -p rustserv-core` |
| VÃ©rification des types du front | `pnpm check` |
| Build de l'interface seule | `pnpm build` |
| Build release (exe portable) | `pnpm tauri build` â†’ `target/release/rustserv.exe` |

En dev, la racine portable est `./portable/` (ignorÃ©e par git). Surchargeable avec `RUSTSERV_ROOT`.
Niveau de log : variable `RUSTSERV_LOG` (ex. `debug`, `rustserv_core=trace`).

## Architecture

```
crates/core    bibliothÃ¨que sans Tauri : catalog/ process/ php/ services orchestrator settings sites templates/
crates/app     couche Tauri fine : fenÃªtre, tray, commandes â†’ appelle core
crates/helper  rustserv-helper.exe, lancÃ© avec UAC uniquement si besoin (hostsâ€¦)
ui/            interface Svelte 5 ; ui/src/lib/api.ts = seul point d'appel au backend
catalog/       catalog.json de rÃ©fÃ©rence (copiÃ© dans la racine portable)
```

Dossier portable : `bin/ etc/ data/ www/ ssl/ logs/ tmp/ catalog.json`.
- `bin/<paquet>/<paquet>-<version>/` : binaires **non modifiÃ©s** (remplaÃ§ables), + marqueur
  `.rustserv-install.json`. Ex. `bin/php/php-8.3.37`, `bin/nginx/nginx-1.30.7`.
- `tmp/downloads` (archives en cours, `.part`), `tmp/extract` (nettoyÃ© automatiquement).
- Toutes les configs modifiÃ©es/gÃ©nÃ©rÃ©es vont dans `etc/` (php.ini dans `etc/php/<version>/php.ini`).
- Profil WebView2 dans `tmp/webview` (pas dans %LOCALAPPDATA%).

## Conventions

- **Erreurs** : `rustserv_core::Error` (thiserror). Jamais de texte utilisateur cÃ´tÃ© Rust :
  chaque variante a un `code()` (ex. `port.occupe`) + `params()`. SÃ©rialisÃ©e en
  `{code, params, detail}`, traduite cÃ´tÃ© UI par `tErreur()` via `erreurs.<code>` dans
  `ui/src/i18n/fr.json`. Toute nouvelle variante â‡’ ajouter sa traduction.
- **i18n** : textes UI uniquement dans `ui/src/i18n/<langue>.json`, appel `t("cle", {params})`.
- **Code Windows** isolÃ© dans `crates/core/src/platform/` (prÃ©voir un Ã©quivalent unix plus tard).
- Commentaires et noms mÃ©tier en franÃ§ais ; code idiomatique Rust/TS.
- Tests unitaires obligatoires pour : gestionnaire php.ini, catalogue.
- L'app ne tourne jamais en admin ; toute modif systÃ¨me passe par le helper et doit Ãªtre rÃ©versible.
- TypeScript reste en 6.x tant que svelte-check ne supporte pas TS 7.
- Le test `error::tests::toutes_les_erreurs_sont_traduites` Ã©choue si une erreur n'a pas
  sa traduction dans fr.json (ou si un `{param}` n'est pas fourni).
- Tests rÃ©seau rÃ©els : `cargo test -p rustserv-core --test reseau -- --ignored --nocapture`
  (vÃ©rifie que toutes les URLs du catalogue rÃ©pondent, installe Nginx).

## Services : points techniques vÃ©rifiÃ©s (ne pas Â« simplifier Â»)

- Lancement : `platform::spawn_in_job` crÃ©e le processus **suspendu**, l'ajoute au Job, puis le
  relance (sinon les sous-processus crÃ©Ã©s immÃ©diatement, workers nginx/Apache, Ã©chappent au Job).
- ArrÃªts propres : nginx `-s quit` ; Apache Ã©vÃ©nement `ap<pid>_shutdown` ; MariaDB Ã©vÃ©nement
  `MySQLShutdown<pid>` ; php-cgi kill. VÃ©rifiÃ© : fichiers pid supprimÃ©s, Â« Shutdown complete Â».
- nginx lancÃ© avec `-p <racine>/` (il ouvre `logs/` au dÃ©marrage) et `-c etc/nginx/nginx.conf`.
- Apache + php-cgi : `ProxyFCGIBackendType GENERIC` + `ProxyFCGISetEnvIf â€¦ SCRIPT_FILENAME`,
  et noms de balancer **avec `/` final** dans `<Proxy>` et `SetHandler` (sinon erreur 500).
- `Paths::new` normalise la racine : un `..` dans SCRIPT_FILENAME â†’ Â« No input file specified Â».
- php-cgi : `PHP_FCGI_MAX_REQUESTS=0`, PATH prÃ©fixÃ© par le dossier PHP, `cgi.force_redirect=0`.
- php.ini : crÃ©Ã© une seule fois depuis php.ini-development, jamais Ã©crasÃ© ensuite.
- Groupes UI (`orchestrator.rs`) : dÃ©marrer `web` dÃ©marre `php` ; arrÃªter `php` arrÃªte `web`.

## PHP multi-versions (phase 2)

- Les choix se font par **branche** (Â« 8.3 Â») : `settings.versions.php` (dÃ©faut), `settings.site_php`
  (site â†’ branche). Une seule version **active** par branche (`Stack::php_for_branch`) : celle de
  `settings.php_pins[branche]` si installÃ©e, sinon la plus rÃ©cente installÃ©e. Les ports dÃ©pendent
  de la branche, donc deux versions d'une mÃªme branche ne tournent jamais ensemble.
- `Stack::php_needed` = dÃ©faut + PHP des sites existants ; seules ces versions tournent.
- `Orchestrator::apply` reconfigure Ã  chaud aprÃ¨s un changement (arrÃªte les pools inutiles,
  dÃ©marre les nouveaux, redÃ©marre le serveur web) **sans dÃ©marrer ce qui Ã©tait arrÃªtÃ©**.
- http://localhost/<dossier>/ utilise dÃ©jÃ  le PHP du site (nginx : `location ^~ /<dossier>/`
  imbriquÃ©e ; Apache : `<Directory>` + `SetHandler` dans le vhost par dÃ©faut).
- Mise Ã  jour de branche : `Orchestrator::upgrade_php_branch` (install â†’ `php::migrate_ini`
  (remplace les chemins de l'ancien dossier) â†’ `php::copy_extra_dlls` (DLL absentes de la nouvelle
  version) â†’ `php::check` (`php -m`, tout Â« PHP Startup Â» = Ã©chec) â†’ apply â†’ suppression
  facultative). Ã‰chec de validation â‡’ retour arriÃ¨re complet. L'ancien php.ini reste dans etc/.
- Nouvelles versions : `catalog::php_remote` lit releases.json (NTS x64), `Catalog::merge` +
  `save` dans catalog.json. Recherche automatique une fois par session Ã  l'ouverture de la page PHP.

## Extensions PHP (phase 3)

- Ã‰tat (`php::extensions::scan`) = DLL de `ext/` + lignes du php.ini + `php -n -m` (intÃ©grÃ©es,
  non dÃ©sactivables) + `php -c ini -m` (rÃ©ellement chargÃ©es â†’ dÃ©tecte Â« activÃ©e mais en erreur Â»).
  Alias module/DLL : `gd2` (PHP 7.4) â†’ module `gd`, `oci8_19` â†’ `oci8` (`module_name`, `dll_for`).
- Toute modification passe par `php::transaction::modify` : sauvegarde dans
  `etc/php/<v>/backups/` (10 max) â†’ changement â†’ `php -m` â†’ restauration si avertissement.
- DÃ©pendances et ordre (`DEPENDANCES`) : activer exif active mbstring **avant** ; dÃ©sactiver une
  extension requise est refusÃ© (`extension.requise`). Zend : opcache, xdebug.
- OPcache intÃ©grÃ© (PHP â‰¥ 8.5) : pilotÃ© par `opcache.enable`. Activer OPcache rÃ¨gle aussi
  `opcache.revalidate_freq=0` (code modifiÃ© visible immÃ©diatement en dev).
- PECL : builds officiels `downloads.php.net/~windows/pecl/releases/<ext>/<v>/php_<ext>-<v>-<branche>-nts-<vsXX>-x64.zip`
  (versions stables, 6 plus rÃ©centes examinÃ©es). **Pas de SHA256 publiÃ©** : HTTPS + empreinte
  calculÃ©e et enregistrÃ©e dans `etc/php/<v>/pecl.json` + validation `php -m`. `php_<ext>.dll` â†’ ext/,
  autres DLL (ImageMagickâ€¦) â†’ dossier PHP sans Ã©craser les DLL officielles. Rien pour PHP 8.6 (fin 2026).
- Profils (`php/profiles.json`) : additifs, plan affichÃ© avant application ; Ã©checs PECL signalÃ©s
  sans bloquer le reste ; une seule transaction php.ini.
- AprÃ¨s modification, seuls les php-cgi **de cette version** sont relancÃ©s, s'ils tournaient.

## Sites, HTTPS et terminal (phase 4)

- **hosts** : `crates/helper` = lib (logique pure, sans dÃ©pendance, testÃ©e) + `rustserv-helper.exe`
  (`windows_subsystem`, lancÃ© via `ShellExecuteExW` verbe `runas` â†’ fenÃªtre UAC). Seul le bloc
  `# BEGIN Rustserv â€¦ # END Rustserv` est rÃ©Ã©crit ; BOM/CRLF/entrÃ©es tierces conservÃ©s ;
  copie `hosts.rustserv.bak`. Domaines acceptÃ©s uniquement en `.test .localhost .local .internal .lan`
  (le helper Ã©levÃ© lit un fichier Ã©crit par l'app : il ne doit jamais pouvoir dÃ©tourner un vrai site).
  Erreurs du helper â†’ `<demande>.err` (sortie d'un processus Ã©levÃ© illisible). UAC refusÃ© â†’ `admin.refuse`.
- Le helper doit Ãªtre Ã  cÃ´tÃ© de `rustserv.exe` : `pnpm dev`/`pnpm build` le compilent d'abord.
- Le bandeau Â« domaines Ã  mettre Ã  jour Â» ne dÃ©clenche jamais l'UAC tout seul : clic utilisateur.
- **HTTPS** (`ssl.rs`, rcgen 0.14) : CA ECDSA P-256 10 ans dans `ssl/ca/` (clÃ©, PEM, DER, ca.json),
  installÃ©e dans **CurrentUser\Root** (confirmation Windows, pas d'admin). L'Ã©metteur est reconstruit
  avec `Issuer::new(ca_params(info), key)` (pas besoin de x509-parser). Certificats de site 825 j
  (`site.test` + `*.site.test`, `localhost` + 127.0.0.1), renouvelÃ©s Ã  < 30 j ou si la CA change.
  HTTPS actif â‡” CA prÃ©sente : nginx `listen 443 ssl` + `http2 on`, Apache vhosts `*:443` via macros.
- **Terminal** (`terminal.rs`, portable-pty/ConPTY) : PowerShell (pwsh si prÃ©sent) dans le dossier du
  site, PATH = PHP du site + Composer + MariaDB/bin, `PHPRC` = etc/php/<v>. ConPTY envoie `ESC[6n`
  au dÃ©marrage et **attend la rÃ©ponse** (xterm.js rÃ©pond ; un test doit rÃ©pondre lui-mÃªme).
  DÃ©codage UTF-8 par morceaux (`decode_utf8`). La page Terminal reste montÃ©e (masquÃ©e) entre pages.
- **Composer** : entrÃ©e catalogue `file: "composer.phar"` (fichier unique, pas d'archive) ; un `.phar`
  reÃ§oit un lanceur `<nom>.bat` (`@php "%~dp0composer.phar" %*`).

## RÃ©trocompatibilitÃ© (phase 5, `compat/`)

- `runner` : PHPUnit (`vendor/phpunit/phpunit/phpunit`), Pest (`vendor/pestphp/pest/bin/pest`) ou
  script (relatif au site, `..` interdit) lancÃ©s **en parallÃ¨le** avec `php.exe -c <ini>` de chaque
  version. `--log-junit` â†’ rÃ©sumÃ© (tests, assertions, Ã©checs, erreurs). Sans phpunit.xml ni argument,
  `tests/` est passÃ© (sinon PHPUnit affiche son aide, code 2). Script : sorties comparÃ©es par empreinte.
  Statut : code 0 = rÃ©ussi ; tests exÃ©cutÃ©s + code â‰  0 = Ã©chec ; sinon erreur. Sortie : 256 Ko max (fin).
- `tools` : Composer installe dans `<racine>/tools/` (COMPOSER_HOME = tmp/composer, portable) :
  PHP_CodeSniffer ^4, **PHPCompatibility ^10.0@alpha** (la 9.3.5 stable de 2019 ignore PHP 8),
  PHPStan ^2.1, plugin dealerdirect. Composer installÃ© depuis le catalogue s'il manque.
  Les outils tournent avec le PHP installÃ© le plus rÃ©cent.
- Installation de PHPUnit/Pest dans un site (`ops::install_test_framework`) : `composer require --dev`
  exÃ©cutÃ© avec la **plus ancienne** version de PHP cochÃ©e â†’ Composer choisit la version compatible
  (ex. PHPUnit 9.6 si 7.4 est cochÃ©e). Pest : `allow-plugins.pestphp/pest-plugin` autorisÃ© avant ;
  un `composer.json` minimal `{}` est crÃ©Ã© s'il manque (`composer config` en exige un).
- `analysis` : phpcs `--standard=PHPCompatibility --runtime-set testVersion 7.4-8.5 --report=json`
  (vendor, node_modules, storage, var/cache ignorÃ©s) ; PHPStan avec une config neon temporaire
  (`phpVersion` 80300, `excludePaths.analyse` + `scanDirectories` vendor). JSON extrait entre le premier
  `{` et le dernier `}` (des avertissements peuvent prÃ©cÃ©der). Rapport commun `Rapport`.

## Gestion des bases : Adminer

- Choix validÃ© par l'utilisateur (2026-11-22) : **Adminer** seul (phpMyAdmin 5.2.3 limitÃ© Ã  PHP < 8.4,
  HeidiSQL Ã©cartÃ© pour l'instant). Catalogue : fichier unique `adminer.php` (SHA256 = digest GitHub).
- Servi sous `/adminer/` (pas dans www/) par le vhost par dÃ©faut, avec le PHP par dÃ©faut, **accÃ¨s
  local uniquement** (nginx `allow 127.0.0.1; deny all;`, Apache `Require local`) car connectÃ© en root.
- Lanceur gÃ©nÃ©rÃ© `etc/adminer/index.php` (gabarit `adminer.php.j2`) : `adminer_object()` Ã©tend
  `\Adminer\Adminer` (credentials, `login()` â†’ true pour root sans mot de passe,
  `verifyLoginToken()` â†’ false) et simule `$_POST['auth']` Ã  la premiÃ¨re visite â†’ redirection 303
  vers `?server=127.0.0.1:<port>&username=root`. Apache : `Alias` + `ProxyFCGISetEnvIf` SCRIPT_FILENAME
  dans le `<Directory>` (le rÃ©glage global DOCUMENT_ROOT+SCRIPT_NAME ne convient pas Ã  un Alias).
- Bouton Â« GÃ©rer les bases Â» (carte MariaDB) : installe Adminer, dÃ©marre MariaDB + web si besoin.

## Import depuis Laragon / WAMP (`migration/`)

- Choix utilisateur (2026-11-22) : adresses WordPress **converties en .test** ; doublons entre
  sources **renommÃ©s** (suffixe -laragon / -wamp, modifiable).
- `detect` (lecture seule) : C:\laragon (bin/mysql/mysql-X, data/mysql-<branche>, usr/laragon.ini) et
  C:\wamp64 (bin/mysql|mariadb/*/data, wampmanager.conf). Noms de bases dÃ©codÃ©s (`@002d` â†’ `-`).
  Base d'un site lue dans wp-config.php (DB_NAME) ou .env (DB_DATABASE).
- `run` : **rien n'est modifiÃ© chez Laragon/WAMP**. Pour chaque serveur d'origine : copie des donnÃ©es
  (sans binlog.*), dÃ©marrage du MySQL **d'origine** sur la copie (`--no-defaults`, port libre,
  `--mysqlx=OFF --disable-log-bin`, `--init-file` qui vide le mot de passe root **de la copie** :
  WAMP a un root protÃ©gÃ©), mysqldump, arrÃªt. âš  MySQL 8+ Windows lance un processus **moniteur** :
  le serveur temporaire est lancÃ© dans un **Job Object** (sinon orphelin). DÃ©marrage jusqu'Ã  5 min.
- Import : `mariadb < dump` dans une base libre (`nom`, `nom_2`â€¦) ; repli si collation `utf8mb4_0900_*`
  inconnue. wp-config.php / .env adaptÃ©s (copie `*.avant-rustserv`). Conversion : WP-CLI
  `search-replace //ancien //nouveau` (+ variante JSON `\/\/`), `option update siteurl/home`,
  .htaccess RewriteBase corrigÃ©. Refus si Laragon/WAMP tourne encore (ports 80/443/3306/3307).
- âš  WordPress : les constantes `WP_HOME`/`WP_SITEURL` de wp-config.php **priment sur la base**.
  `adapter_adresses` les met Ã  jour **avant** la base (sinon WordPress compare avec l'ancienne
  constante), convertit depuis l'adresse en base ET celle de la constante, puis fixe
  siteurl/home en SQL. `adapter_site` (bouton Â« Adapter les adresses Â» de la page Sites) refait
  cette conversion vers le domaine actuel : Ã  utiliser aprÃ¨s un changement de domaine.
- http://localhost/<site>/ ne convient pas Ã  WordPress (une seule adresse canonique).
- Test rÃ©el `import_laragon_et_wamp_reel` : agence-web (Laragon, MySQL 8.4) et claude (WAMP, MySQL 9.1,
  localhost/claude â†’ claude.test, page WordPress 200) â€” utilise les vraies installations, en lecture.

## MariaDB multi-versions (par site)

- Comme PHP : choix par **branche** (`settings.site_mariadb` : site â†’ Â« 11.4 Â»). Chaque branche a son
  dossier de donnÃ©es (`data/mariadb-<branche>`), son instance et son port. Instance **par dÃ©faut** :
  service `mariadb`, port des rÃ©glages (3306), `etc/mariadb/my.ini`, `logs/mariadb-error.log`.
  Autres : service `mariadb-<branche>`, port Â« 3 + majeure + mineure Â» (11.4 â†’ 31104, 10.11 â†’ 31011),
  `etc/mariadb/my-<branche>.ini`, journaux/tmp nommÃ©s d'aprÃ¨s l'instance (variable `instance` du gabarit).
- `Stack::mariadb_needed` = dÃ©faut + versions des sites existants ; seules celles-ci dÃ©marrent
  (`Group::Mariadb`), `apply` arrÃªte les instances devenues inutiles.
- Changer la MariaDB d'un site (`site_set_mariadb`) : les deux instances dÃ©marrent
  (`Orchestrator::ensure_mariadb`), `migration::run::deplacer_base` copie la base (mariadb-dump de
  la source â†’ import ; refus si la base existe dÃ©jÃ  dans la cible ; repli collations `uca1400`/`0900`),
  puis `configurer_port` met Ã  jour DB_HOST (wp-config) / DB_PORT (.env). L'ancienne base est conservÃ©e.
- Adminer : `?port=31104` pour se connecter Ã  une autre instance. Terminal : client MariaDB de la
  version du site + `MYSQL_TCP_PORT`. Import Laragon/WAMP : version MariaDB choisissable par site.
- Test rÃ©el : `mariadb_multi_versions_par_site` (`target/pile-mariadb`, 12.3 â†’ 11.4).

## MySQL et PostgreSQL (moteurs de bases, `db.rs`)

- `Moteur` (Mariadb / Mysql / Postgresql) : branche Â« 11.4 Â», Â« 8.4 Â», mais **majeure seule** pour
  PostgreSQL (Â« 17 Â»). DonnÃ©es `data/<moteur>-<branche>`, instance par dÃ©faut = service `<moteur>`
  + port des rÃ©glages (3306 / **3307** / 5432) ; autres : `<moteur>-<branche>`, port
  MariaDB 3MMmm, MySQL 4MMmm (8.0 â†’ 40800), PostgreSQL 5MM00 (16 â†’ 51600).
- Choix par site : `settings.site_db` = Â« mysql:8.4 Â» (`ChoixBase`) ; l'ancien `site_mariadb`
  (site â†’ Â« 11.4 Â») est converti au chargement. Sans choix : MariaDB par dÃ©faut (sinon MySQL, PG).
- Groupes `mysql` / `postgresql` (cartes Services affichÃ©es seulement si installÃ©s).
- MySQL : `mysqld --initialize-insecure` + `--init-file` crÃ©ant **root@127.0.0.1 et root@::1**
  (sinon ERROR 1130 : skip-name-resolve + connexion TCP). Gabarit `mysql.ini.j2` (`mysqlx=OFF`,
  `disable-log-bin`). ArrÃªt : `mysqladmin shutdown` (processus moniteur : l'Ã©vÃ©nement nommÃ© ne
  vise pas le bon PID). VÃ©rifiÃ© : Â« Shutdown complete Â».
- PostgreSQL : `initdb -U postgres -A trust -E UTF8 --no-locale`, `postgres -D â€¦ -p â€¦ -c
  listen_addresses=127.0.0.1`, arrÃªt `pg_ctl stop -m fast -w`. **`ready_check` = `pg_isready`** :
  PostgreSQL ouvre son port avant d'accepter les connexions (rÃ©cupÃ©ration aprÃ¨s arrÃªt brutal).
- Catalogue : `exclude` (motifs Â« *.pdb Â», Â« pgAdmin 4/ Â») : MySQL 511 Mo au lieu de 1,2 Go,
  PostgreSQL 150 Mo au lieu de 1 Go. Ni MySQL (MD5 seul) ni EDB ne publient de SHA256 :
  empreintes **calculÃ©es** par nous. Pas encore de recherche automatique des nouvelles versions.
- Migrer la base d'un site (`site_set_base`) : MariaDB â†” MySQL = copie (`deplacer_base`,
  collations converties) ; vers/depuis PostgreSQL = **conversion avec donnÃ©es** (`changer_de_moteur`
  â†’ `conversion::convertir`), `.env` : `DB_CONNECTION` (pgsql/mysql), `DB_USERNAME` postgres/root.
  WordPress + PostgreSQL refusÃ© (`base.wordpress_postgresql`). Passage Ã  PG : `pgsql` + `pdo_pgsql`
  activÃ©es dans le PHP du site. Choisir explicitement la base par dÃ©faut = `None` (sinon la branche
  deviendrait Â« secondaire Â» et la version par dÃ©faut basculerait). L'ancienne base est conservÃ©e.
- **Version par dÃ©faut d'un moteur** (`Stack::pick`) : la plus rÃ©cente avec donnÃ©es, en **excluant les
  branches choisies explicitement pour des sites** (bug corrigÃ© : attribuer MariaDB 13.0 Ã  un site
  faisait basculer tous les autres sites vers une instance sans leurs bases).
- **Conversion** (`conversion.rs` + `scripts/convertir_base.php`, PHP/PDO) : lancÃ©e avec le PHP â‰¥ 8
  le plus rÃ©cent en `-n` (pdo_mysql + pdo_pgsql seulement, PATH = dossier PHP pour libpq.dll).
  ModÃ¨le neutre de colonnes ; tinyint(1) â†” boolean, AUTO_INCREMENT â†” IDENTITY (sÃ©quence recalÃ©e),
  enum â†’ varchar + CHECK (PG) / ENUM (MySQL), dates Â« zÃ©ro Â» â†’ NULL (colonne rendue facultative),
  bytea â†” longblob, timestamptz â†’ datetime UTC, TEXT indexÃ© â†’ varchar(255) cÃ´tÃ© MySQL. DonnÃ©es par
  lots (curseur serveur PG, requÃªte non bufferisÃ©e MySQL). Index puis clÃ©s Ã©trangÃ¨res Ã  la fin
  (Ã©chec â†’ avertissement). Cible non vide â†’ `base.cible_non_vide`. Avertissements = **codes**
  traduits par l'UI (`conversion.avert.*`) ; vues / procÃ©dures / dÃ©clencheurs / Ã©vÃ©nements listÃ©s.
- **Type de site** (`site_type.rs`) : WordPress (wp-includes/version.php), composer.lock (Statamic,
  October, Craft, Shopware, Drupal, TYPO3, Magento avant Laravel/Symfonyâ€¦), fichiers caractÃ©ristiques
  (Joomla, PrestaShop, Moodleâ€¦), sinon Â« php Â» / Â« statique Â». Logos UI : `simple-icons` (CC0,
  import nommÃ© â†’ seuls les logos utilisÃ©s sont inclus, +17 Ko gzip), `components/LogoSite.svelte`.
- Page Sites : base actuelle (texte) â†’ liste des bases installÃ©es â†’ bouton Â« Migrer Â» (actif si la
  cible diffÃ¨re), confirmation (voile + dialogue), rapport de conversion.
- Adminer : `?port=â€¦&driver=pgsql` (utilisateur postgres) ; pgsql activÃ©e dans le PHP par dÃ©faut.
- Terminal : client du moteur du site, `PGHOST/PGPORT/PGUSER` pour PostgreSQL.
- Import Laragon/WAMP : cible MariaDB **ou MySQL** (pas PostgreSQL).
- Tests rÃ©els : `mysql_et_postgresql_reels`, `adminer_mysql_et_postgresql` (`target/pile-bases` ;
  archives rÃ©utilisÃ©es depuis `target/telechargements-db` si prÃ©sentes).

## Version portable et dÃ©placement du dossier

- Installation de l'utilisateur : **`C:\Rustserv\`** (copie de `portable/` sans tmp/logs + `rustserv.exe`
  + `rustserv-helper.exe` de `target/release`). Ne jamais faire tourner l'app de dev et la portable
  en mÃªme temps (mÃªmes ports). Mettre Ã  jour la portable = rebuild (`pnpm tauri build`) puis recopier
  **uniquement** les deux .exe (les donnÃ©es restent).
- `relocation::relocate` (au dÃ©marrage, avant tout) : `etc/racine.txt` mÃ©morise le dossier ; s'il a
  changÃ©, l'ancien chemin (formes `C:\` et `C:/`) est remplacÃ© dans `etc/**` (ini, conf, php, neon,
  json + sauvegardes `php.ini.<horodatage>`) et `data/*/my.ini`. VÃ©rifiÃ© sur la vraie copie (7 fichiers).
- CompatibilitÃ© : annulation des tests/analyses via `compat::ou_annuler` (kill_on_drop des processus).

## Catalogue (`catalog/catalog.json`)

- SchÃ©ma 1. SHA256 **obligatoire**, URLs **https** obligatoires, `mirrors` essayÃ©s dans l'ordre.
- `subdir` : sous-dossier de l'archive Ã  installer (Apache Lounge : `Apache24`) ; sinon le dossier
  unique de premier niveau est retirÃ© automatiquement (nginx, mariadb) ; PHP est Ã  plat.
- `vcruntime_min` : version minimale de vcruntime140.dll (VS16 â†’ 14.20, VS17 â†’ 14.30, VS18 â†’ 14.50).
- Sources : PHP = `windows.php.net/downloads/releases/releases.json` (+ `/archives/`) ;
  Nginx = nginx.org (pas de SHA publiÃ© : calculÃ© par nous) ; Apache = apachelounge.com
  (`<zip>.txt` contient le SHA256) ; MariaDB = `downloads.mariadb.org/rest-api/mariadb/<branche>/latest/`.
- Catalogue intÃ©grÃ© copiÃ© dans la racine portable au 1er lancement ; ensuite, Ã  **chaque dÃ©marrage**,
  `Catalog::absorb` y **ajoute** les paquets/versions nouveaux de la version intÃ©grÃ©e (rien n'est
  retirÃ© ni modifiÃ©). Bug corrigÃ© : un catalog.json de la phase 1 n'avait jamais reÃ§u Composer.
  Si `catalog.json` est corrompu â†’ repli sur le catalogue intÃ©grÃ© + avertissement dans l'UI.
- Mises Ã  jour (`catalog::remote::fetch_all`) : php (releases.json), nginx (download.html, versions
  stables Ã  mineure paire, SHA256 **calculÃ©**), apache (Apache Lounge + .txt SHA256), mariadb (REST API,
  branches Stable), composer (/versions + .sha256sum), adminer et wp-cli (API GitHub, `digest`).
  Bouton dans Catalogue + vÃ©rification auto au dÃ©marrage si > 24 h (`app/src/verification.rs`,
  `etc/catalog-verification.json`). Badges Â« â¬† disponible Â» / Â« nouveau Â».
- MariaDB : `Stack::pick` sans choix explicite garde la version la plus rÃ©cente **dont le dossier de
  donnÃ©es existe** (installer une nouvelle branche ne bascule pas vers une base vide).

## Choix techniques validÃ©s

- Serveur web : Nginx par dÃ©faut, Apache en option. Gabarits `minijinja`.
- PHP : php-cgi NTS en FastCGI, **pool de 4 processus par version** (php-cgi Windows ne traite
  qu'une requÃªte Ã  la fois), ports par plage : PHP X.Y â†’ `9XY0`â€“`9XY3` (8.3 â†’ 9830-9833,
  7.4 â†’ 9740-9743). `PHP_FCGI_MAX_REQUESTS=0` + redÃ©marrage auto par le superviseur.
- Processus enfants dans un Job Object Windows (`KILL_ON_JOB_CLOSE`) : pas d'orphelins.
- DNS `.test` : fichier hosts via helper (bloc `# BEGIN Rustserv`/`# END Rustserv`) ;
  DNS intÃ©grÃ© + rÃ¨gle NRPT en option plus tard (port 53 dÃ©jÃ  pris par svchost sur la machine de dev).
- SSL : CA locale gÃ©nÃ©rÃ©e avec `rcgen`, installÃ©e dans CurrentUser\Root (sans admin).
- Ports par dÃ©faut : 80 / 443 / 3306 (Laragon arrÃªtÃ© pendant le dev ; il est dans C:\laragon).

## Avancement

### Phase 1 â€” noyau âœ… (2026-11-22)
- [x] 1. Squelette : workspace, Tauri + Svelte, tray, racine portable, erreurs i18n, logs
- [x] 2. Catalogue : modÃ¨le, tÃ©lÃ©chargement, SHA256, extraction, progression, vcruntime, page UI
  - reste : proposer la mise Ã  jour quand le catalogue intÃ©grÃ© est plus rÃ©cent que le fichier portable
- [x] 3. Superviseur (`process/`), Job Object, ports (propriÃ©taire + plages rÃ©servÃ©es), relance auto
- [x] 4. Gabarits minijinja, Nginx + pool php-cgi, Apache (balancer + .htaccess), sites www/ auto
- [x] 5. MariaDB : init auto `data/mariadb-<branche>`, arrÃªt propre par Ã©vÃ©nement
- [x] 6. Ã‰cran Services (groupes web/php/mariadb, journaux) + page ParamÃ¨tres

Tests rÃ©els (binaires tÃ©lÃ©chargÃ©s dans `target/pile-test`, ~150 Mo) :
`cargo test -p rustserv-core --test pile_reelle -- --ignored --nocapture --test-threads=1`

### Phase 2 â€” PHP multi-versions âœ… (2026-11-22)
- [x] Plusieurs versions en parallÃ¨le, une par branche, pool par branche
- [x] Association site â†’ branche (page Sites), PHP par dÃ©faut, Ã©pinglage (page PHP)
- [x] Mise Ã  jour de branche en un clic avec reprise php.ini/DLL, validation et retour arriÃ¨re
- [x] DÃ©tection des nouvelles versions (windows.php.net/releases.json)
- Tests rÃ©els : `multi_versions_php_par_site`, `mise_a_jour_de_branche_reelle` (dossier `target/pile-maj`)

### Phase 3 â€” Extensions PHP âœ… (2026-11-22)
- [x] Scan ext/*.dll + php.ini + php -m : active / inactive / intÃ©grÃ©e / en erreur
- [x] Activer/dÃ©sactiver (extension= / zend_extension=), sauvegarde, validation, restauration
- [x] PECL : xdebug, apcu, redis, igbinary, imagick, mongodb, pcov, yaml
- [x] DÃ©pendances et ordre de chargement, DLL tierces
- [x] Profils Laravel / Symfony / WordPress avec aperÃ§u
- Test rÃ©el : `extensions_pecl_et_profils_reels` (dossier `target/pile-ext`)

### Phase 4 â€” Sites, hosts, HTTPS, terminal âœ… (2026-11-22)
- [x] Domaines `.test` via fichier hosts (helper UAC, bloc balisÃ©, validation stricte)
- [x] HTTPS : CA locale + certificats par site, nginx (HTTP/2) et Apache
- [x] Terminal intÃ©grÃ© (xterm.js + ConPTY) avec le PHP du site, Composer, client MariaDB
- [x] Composer dans le catalogue
- Non testÃ© automatiquement (fenÃªtres Windows Ã  cliquer) : UAC du fichier hosts,
  confirmation d'installation de la CA. TestÃ©s : helper sans droits, HTTPS avec CA approuvÃ©e par le client.
- Option non faite : DNS local + rÃ¨gle NRPT (le fichier hosts suffit).
- Tests rÃ©els : `https_nginx_et_apache` (`target/pile-https`), `terminal_avec_le_php_du_site`

### Phase 5 â€” Tests de rÃ©trocompatibilitÃ© âœ… (2026-11-22)
- [x] Script / PHPUnit / Pest sur plusieurs versions en parallÃ¨le, tableau comparatif
- [x] PHPCompatibility (PHP_CodeSniffer) et PHPStan, rapports par fichier
- Test rÃ©el : `retrocompatibilite_reelle` (projet Â« historique Â» dans `target/pile-test`)
- Non fait : annulation d'une analyse en cours (dÃ©lais max : tests 10 min, analyses 15 min)

### Pistes pour la suite
- BoÃ®te de dialogue native si l'app ne peut pas dÃ©marrer (aujourd'hui : message console)
- Annulation des analyses ; DNS local + NRPT (sous-domaines sans fichier hosts)
- Les tests rÃ©els partagent `target/pile-test` : les lancer avec `--test-threads=1`, machine peu chargÃ©e
