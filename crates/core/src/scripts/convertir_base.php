<?php
// ---------------------------------------------------------------------------
// Rustserv : conversion d'une base MySQL / MariaDB <-> PostgreSQL.
//
// Usage : php convertir_base.php <config.json>
//   { "source": {"moteur": "mariadb|mysql|postgresql", "port": 3306, "base": "app"},
//     "cible":  {"moteur": "...", "port": 5432, "base": "app"} }
//
// 1. lit la structure de la source (tables, colonnes, clé primaire, index, clés étrangères) ;
// 2. crée les tables dans la cible avec les types équivalents ;
// 3. copie les données par lots (curseur côté serveur, pas de chargement en mémoire) ;
// 4. recrée index et clés étrangères, recale les compteurs automatiques.
//
// Ce qui ne se convertit pas automatiquement (vues, procédures, déclencheurs, événements)
// est listé dans le rapport. Sortie : un objet JSON sur stdout (rapport ou erreur).
// Les avertissements sont des codes (traduits par l'interface), jamais des phrases.
// ---------------------------------------------------------------------------

declare(strict_types=1);
error_reporting(E_ALL);
ini_set('display_errors', 'stderr');
set_time_limit(0);

const LOT_PARAMS = 20000;          // paramètres max par INSERT (limite PDO : 65535)
const LOT_LIGNES = 500;

$debut = microtime(true);
$avertissements = [];
function avertir(string $code, array $params = []): void
{
    global $avertissements;
    $avertissements[] = ['code' => $code, 'params' => (object) $params];
}

function sortir(array $json, int $code = 0): void
{
    echo json_encode($json, JSON_UNESCAPED_UNICODE | JSON_INVALID_UTF8_SUBSTITUTE), PHP_EOL;
    exit($code);
}

set_exception_handler(function (Throwable $e) {
    sortir(['erreur' => 'conversion', 'detail' => $e->getMessage()], 1);
});

$cfg = json_decode((string) file_get_contents($argv[1] ?? ''), true);
if (!is_array($cfg) || !isset($cfg['source'], $cfg['cible'])) {
    sortir(['erreur' => 'conversion', 'detail' => 'configuration illisible'], 1);
}
$estPg = fn(array $c): bool => $c['moteur'] === 'postgresql';

function connexion(array $c): PDO
{
    $opts = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC];
    if ($c['moteur'] === 'postgresql') {
        $p = new PDO("pgsql:host=127.0.0.1;port={$c['port']};dbname={$c['base']}", 'postgres', '', $opts);
        $p->exec("SET client_encoding TO 'UTF8'");
        $p->exec("SET TIME ZONE 'UTC'");
        return $p;
    }
    $p = new PDO("mysql:host=127.0.0.1;port={$c['port']};dbname={$c['base']};charset=utf8mb4", 'root', '', $opts);
    // Mode permissif : dates « zéro », valeurs tronquées… comme le font les vieux sites.
    $p->exec("SET SESSION sql_mode = ''");
    $p->exec("SET time_zone = '+00:00'");
    return $p;
}

$src = connexion($cfg['source']);
$dst = connexion($cfg['cible']);
$srcPg = $estPg($cfg['source']);
$dstPg = $estPg($cfg['cible']);

// La cible doit être vide : on ne mélange jamais avec des données existantes.
$nbCible = $dstPg
    ? (int) $dst->query("SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'")->fetchColumn()
    : (int) $dst->query('SELECT count(*) FROM information_schema.tables WHERE table_schema = DATABASE()')->fetchColumn();
if ($nbCible > 0) {
    sortir(['erreur' => 'cible_non_vide', 'detail' => (string) $nbCible], 1);
}

// ===========================================================================
// Modèle neutre d'une colonne :
//   nom, type (bool, int16, int32, int64, decimal, float32, float64, char, varchar, text,
//   binary, date, time, datetime, datetime_tz, json, uuid, enum), longueur, precision,
//   echelle, frac (fractions de seconde), nullable, auto, valeurs (enum),
//   defaut : null | ['k' => 'lit', 'v' => '…'] | ['k' => 'now']
// ===========================================================================

function defaut_mysql(?string $d, bool $mariadb, string $extra): ?array
{
    if ($d === null) {
        return null;
    }
    if ($mariadb) {
        if (strtoupper($d) === 'NULL') {
            return null;
        }
        if (strlen($d) >= 2 && $d[0] === "'" && substr($d, -1) === "'") {
            return ['k' => 'lit', 'v' => str_replace("''", "'", substr($d, 1, -1))];
        }
    }
    if (preg_match('/^current_timestamp(\(\d*\))?$/i', $d) || preg_match('/^now\(\)$/i', $d)) {
        return ['k' => 'now'];
    }
    if (stripos($extra, 'DEFAULT_GENERATED') !== false && !is_numeric($d)) {
        return ['k' => 'expr', 'v' => $d];
    }
    if ($mariadb && !is_numeric($d)) {
        return ['k' => 'expr', 'v' => $d];
    }
    return ['k' => 'lit', 'v' => $d];
}

function lire_mysql(PDO $p): array
{
    $mariadb = stripos((string) $p->query('SELECT VERSION()')->fetchColumn(), 'mariadb') !== false;
    $tables = [];
    foreach ($p->query("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME") as $r) {
        $tables[$r['TABLE_NAME']] = ['nom' => $r['TABLE_NAME'], 'colonnes' => [], 'pk' => [], 'index' => [], 'fk' => []];
    }
    $q = $p->query('SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, CHARACTER_MAXIMUM_LENGTH, NUMERIC_PRECISION, NUMERIC_SCALE,
        DATETIME_PRECISION, IS_NULLABLE, COLUMN_DEFAULT, EXTRA FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = DATABASE()
        ORDER BY TABLE_NAME, ORDINAL_POSITION');
    foreach ($q as $c) {
        if (!isset($tables[$c['TABLE_NAME']])) {
            continue;
        }
        $t = strtolower($c['DATA_TYPE']);
        $ct = strtolower($c['COLUMN_TYPE']);
        $non_signe = str_contains($ct, 'unsigned');
        $col = [
            'nom' => $c['COLUMN_NAME'], 'longueur' => null, 'precision' => null, 'echelle' => null, 'frac' => (int) $c['DATETIME_PRECISION'],
            'nullable' => $c['IS_NULLABLE'] === 'YES', 'auto' => stripos($c['EXTRA'], 'auto_increment') !== false, 'valeurs' => [],
            'defaut' => defaut_mysql($c['COLUMN_DEFAULT'], $mariadb, (string) $c['EXTRA']),
        ];
        switch ($t) {
            case 'tinyint':
                $col['type'] = preg_match('/^tinyint\(1\)/', $ct) ? 'bool' : 'int16';
                break;
            case 'bit':
                $col['type'] = $ct === 'bit(1)' ? 'bool' : 'int64';
                break;
            case 'smallint':
                $col['type'] = $non_signe ? 'int32' : 'int16';
                break;
            case 'mediumint':
                $col['type'] = 'int32';
                break;
            case 'int':
            case 'integer':
                $col['type'] = $non_signe ? 'int64' : 'int32';
                break;
            case 'bigint':
                $col['type'] = 'int64';
                break;
            case 'decimal':
            case 'numeric':
                $col['type'] = 'decimal';
                $col['precision'] = (int) $c['NUMERIC_PRECISION'];
                $col['echelle'] = (int) $c['NUMERIC_SCALE'];
                break;
            case 'float':
                $col['type'] = 'float32';
                break;
            case 'double':
            case 'real':
                $col['type'] = 'float64';
                break;
            case 'char':
                $col['type'] = 'char';
                $col['longueur'] = max(1, (int) $c['CHARACTER_MAXIMUM_LENGTH']);
                break;
            case 'varchar':
                $col['type'] = 'varchar';
                $col['longueur'] = max(1, (int) $c['CHARACTER_MAXIMUM_LENGTH']);
                break;
            case 'tinytext':
            case 'text':
            case 'mediumtext':
            case 'longtext':
                $col['type'] = 'text';
                break;
            case 'binary':
            case 'varbinary':
            case 'tinyblob':
            case 'blob':
            case 'mediumblob':
            case 'longblob':
                $col['type'] = 'binary';
                break;
            case 'date':
                $col['type'] = 'date';
                break;
            case 'time':
                $col['type'] = 'time';
                break;
            case 'datetime':
            case 'timestamp':
                $col['type'] = 'datetime';
                break;
            case 'year':
                $col['type'] = 'int16';
                break;
            case 'json':
                $col['type'] = 'json';
                break;
            case 'enum':
                $col['type'] = 'enum';
                preg_match_all("/'((?:[^']|'')*)'/", $c['COLUMN_TYPE'], $m);
                $col['valeurs'] = array_map(fn($v) => str_replace("''", "'", $v), $m[1]);
                break;
            case 'set':
                $col['type'] = 'text';
                break;
            case 'uuid':
                // MariaDB 10.7+
                $col['type'] = 'uuid';
                break;
            case 'inet4':
            case 'inet6':
                $col['type'] = 'varchar';
                $col['longueur'] = 64;
                break;
            default:
                // Types spatiaux, etc. : données gardées en binaire.
                $col['type'] = 'binary';
                avertir('type_inconnu', ['table' => $c['TABLE_NAME'], 'colonne' => $c['COLUMN_NAME'], 'type' => $t]);
        }
        if ($t === 'bigint' && $non_signe && !$col['auto']) {
            // bigint unsigned : au-delà de 9,2 × 10^18 la valeur ne tiendrait pas (rare).
            $col['type'] = 'int64';
        }
        $tables[$c['TABLE_NAME']]['colonnes'][] = $col;
    }
    // Clés primaires et index.
    $idx = [];
    foreach ($p->query('SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME, INDEX_TYPE FROM information_schema.STATISTICS
        WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX') as $r) {
        if (!isset($tables[$r['TABLE_NAME']])) {
            continue;
        }
        if ($r['INDEX_NAME'] === 'PRIMARY') {
            $tables[$r['TABLE_NAME']]['pk'][] = $r['COLUMN_NAME'];
            continue;
        }
        if (in_array($r['INDEX_TYPE'], ['FULLTEXT', 'SPATIAL'], true)) {
            $cle = $r['TABLE_NAME'] . '.' . $r['INDEX_NAME'];
            if (!isset($idx[$cle])) {
                avertir('index_ignore', ['table' => $r['TABLE_NAME'], 'index' => $r['INDEX_NAME'], 'type' => $r['INDEX_TYPE']]);
            }
            $idx[$cle] = true;
            continue;
        }
        $k = $r['INDEX_NAME'];
        $tables[$r['TABLE_NAME']]['index'][$k]['nom'] = $k;
        $tables[$r['TABLE_NAME']]['index'][$k]['unique'] = (int) $r['NON_UNIQUE'] === 0;
        $tables[$r['TABLE_NAME']]['index'][$k]['colonnes'][] = $r['COLUMN_NAME'];
    }
    // Clés étrangères.
    $q = $p->query('SELECT k.CONSTRAINT_NAME, k.TABLE_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME,
        r.UPDATE_RULE, r.DELETE_RULE FROM information_schema.KEY_COLUMN_USAGE k
        JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME
        WHERE k.TABLE_SCHEMA = DATABASE() AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION');
    foreach ($q as $r) {
        if (!isset($tables[$r['TABLE_NAME']])) {
            continue;
        }
        $f = &$tables[$r['TABLE_NAME']]['fk'][$r['CONSTRAINT_NAME']];
        $f['nom'] = $r['CONSTRAINT_NAME'];
        $f['table_ref'] = $r['REFERENCED_TABLE_NAME'];
        $f['colonnes'][] = $r['COLUMN_NAME'];
        $f['colonnes_ref'][] = $r['REFERENCED_COLUMN_NAME'];
        $f['maj'] = $r['UPDATE_RULE'];
        $f['suppr'] = $r['DELETE_RULE'];
        unset($f);
    }
    $autres = [
        'vues' => $p->query("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'VIEW'")->fetchAll(PDO::FETCH_COLUMN),
        'procedures' => $p->query('SELECT ROUTINE_NAME FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = DATABASE()')->fetchAll(PDO::FETCH_COLUMN),
        'declencheurs' => $p->query('SELECT TRIGGER_NAME FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = DATABASE()')->fetchAll(PDO::FETCH_COLUMN),
        'evenements' => $p->query('SELECT EVENT_NAME FROM information_schema.EVENTS WHERE EVENT_SCHEMA = DATABASE()')->fetchAll(PDO::FETCH_COLUMN),
    ];
    return [$tables, $autres];
}

function defaut_pg(?string $d): ?array
{
    if ($d === null) {
        return null;
    }
    if (preg_match("/^'((?:[^']|'')*)'(::[\\w\\s\\.\"\\[\\]]+)?$/s", $d, $m)) {
        return ['k' => 'lit', 'v' => str_replace("''", "'", $m[1])];
    }
    if (preg_match('/^\(?(-?[\d.]+)\)?(::[\w\s]+)?$/', $d, $m)) {
        return ['k' => 'lit', 'v' => $m[1]];
    }
    if (in_array(strtolower($d), ['true', 'false'], true)) {
        return ['k' => 'lit', 'v' => strtolower($d) === 'true' ? '1' : '0'];
    }
    if (preg_match('/^NULL(::.*)?$/i', $d)) {
        return null;
    }
    if (preg_match('/^(now\(\)|current_timestamp(\(\d*\))?|localtimestamp|\(?now\(\)\)?::timestamp.*)$/i', $d)) {
        return ['k' => 'now'];
    }
    return ['k' => 'expr', 'v' => $d];
}

function lire_pg(PDO $p): array
{
    $tables = [];
    foreach ($p->query("SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name") as $r) {
        $tables[$r['table_name']] = ['nom' => $r['table_name'], 'colonnes' => [], 'pk' => [], 'index' => [], 'fk' => []];
    }
    $enums = [];
    foreach ($p->query("SELECT t.typname, e.enumlabel FROM pg_type t JOIN pg_enum e ON e.enumtypid = t.oid ORDER BY t.typname, e.enumsortorder") as $r) {
        $enums[$r['typname']][] = $r['enumlabel'];
    }
    $q = $p->query("SELECT table_name, column_name, data_type, udt_name, character_maximum_length, numeric_precision, numeric_scale,
        datetime_precision, is_nullable, column_default, is_identity FROM information_schema.columns WHERE table_schema = 'public'
        ORDER BY table_name, ordinal_position");
    foreach ($q as $c) {
        if (!isset($tables[$c['table_name']])) {
            continue;
        }
        $t = $c['data_type'];
        $auto = $c['is_identity'] === 'YES' || str_starts_with((string) $c['column_default'], 'nextval(');
        $col = [
            'nom' => $c['column_name'], 'longueur' => $c['character_maximum_length'] !== null ? (int) $c['character_maximum_length'] : null,
            'precision' => null, 'echelle' => null, 'frac' => min(6, (int) ($c['datetime_precision'] ?? 0)),
            'nullable' => $c['is_nullable'] === 'YES', 'auto' => $auto, 'valeurs' => [],
            'defaut' => $auto ? null : defaut_pg($c['column_default']),
        ];
        $types = [
            'boolean' => 'bool', 'smallint' => 'int16', 'integer' => 'int32', 'bigint' => 'int64', 'real' => 'float32',
            'double precision' => 'float64', 'character' => 'char', 'character varying' => 'varchar', 'text' => 'text',
            'bytea' => 'binary', 'date' => 'date', 'time without time zone' => 'time', 'time with time zone' => 'time',
            'timestamp without time zone' => 'datetime', 'timestamp with time zone' => 'datetime_tz', 'json' => 'json',
            'jsonb' => 'json', 'uuid' => 'uuid', 'money' => 'decimal', 'inet' => 'varchar', 'cidr' => 'varchar',
            'macaddr' => 'varchar', 'interval' => 'varchar', 'xml' => 'text', 'citext' => 'text',
        ];
        if ($t === 'numeric') {
            $col['type'] = 'decimal';
            $col['precision'] = $c['numeric_precision'] !== null ? (int) $c['numeric_precision'] : 65;
            $col['echelle'] = $c['numeric_scale'] !== null ? (int) $c['numeric_scale'] : 30;
        } elseif ($t === 'USER-DEFINED' && isset($enums[$c['udt_name']])) {
            $col['type'] = 'enum';
            $col['valeurs'] = $enums[$c['udt_name']];
        } elseif (isset($types[$t])) {
            $col['type'] = $types[$t];
            if ($t === 'money') {
                [$col['precision'], $col['echelle']] = [19, 2];
            }
            if (in_array($t, ['inet', 'cidr', 'macaddr', 'interval'], true)) {
                $col['longueur'] = 64;
            }
        } else {
            // Tableaux, types géométriques… : texte (représentation PostgreSQL).
            $col['type'] = 'text';
            avertir('type_inconnu', ['table' => $c['table_name'], 'colonne' => $c['column_name'], 'type' => $t === 'USER-DEFINED' ? $c['udt_name'] : $t]);
        }
        if ($col['defaut'] !== null && $col['defaut']['k'] === 'expr') {
            avertir('defaut_ignore', ['table' => $c['table_name'], 'colonne' => $c['column_name'], 'defaut' => $col['defaut']['v']]);
            $col['defaut'] = null;
        }
        $tables[$c['table_name']]['colonnes'][] = $col;
    }
    $q = $p->query("SELECT t.relname AS tbl, i.relname AS idx, ix.indisunique AS uniq, ix.indisprimary AS prim, a.attname AS col
        FROM pg_index ix JOIN pg_class t ON t.oid = ix.indrelid JOIN pg_class i ON i.oid = ix.indexrelid
        JOIN pg_namespace n ON n.oid = t.relnamespace
        CROSS JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS k(attnum, ord)
        JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum
        WHERE n.nspname = 'public' ORDER BY t.relname, i.relname, k.ord");
    foreach ($q as $r) {
        if (!isset($tables[$r['tbl']])) {
            continue;
        }
        if ($r['prim']) {
            $tables[$r['tbl']]['pk'][] = $r['col'];
            continue;
        }
        $k = $r['idx'];
        $tables[$r['tbl']]['index'][$k]['nom'] = $k;
        $tables[$r['tbl']]['index'][$k]['unique'] = (bool) $r['uniq'];
        $tables[$r['tbl']]['index'][$k]['colonnes'][] = $r['col'];
    }
    $actions = ['a' => 'NO ACTION', 'r' => 'RESTRICT', 'c' => 'CASCADE', 'n' => 'SET NULL', 'd' => 'SET DEFAULT'];
    $q = $p->query("SELECT con.conname, cl.relname AS tbl, a.attname AS col, rcl.relname AS reftbl, ra.attname AS refcol,
        con.confupdtype, con.confdeltype
        FROM pg_constraint con JOIN pg_class cl ON cl.oid = con.conrelid JOIN pg_class rcl ON rcl.oid = con.confrelid
        JOIN pg_namespace n ON n.oid = cl.relnamespace
        CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(attnum, refattnum, ord)
        JOIN pg_attribute a ON a.attrelid = cl.oid AND a.attnum = k.attnum
        JOIN pg_attribute ra ON ra.attrelid = rcl.oid AND ra.attnum = k.refattnum
        WHERE con.contype = 'f' AND n.nspname = 'public' ORDER BY cl.relname, con.conname, k.ord");
    foreach ($q as $r) {
        if (!isset($tables[$r['tbl']])) {
            continue;
        }
        $f = &$tables[$r['tbl']]['fk'][$r['conname']];
        $f['nom'] = $r['conname'];
        $f['table_ref'] = $r['reftbl'];
        $f['colonnes'][] = $r['col'];
        $f['colonnes_ref'][] = $r['refcol'];
        $f['maj'] = $actions[$r['confupdtype']] ?? 'NO ACTION';
        $f['suppr'] = $actions[$r['confdeltype']] ?? 'NO ACTION';
        unset($f);
    }
    $autres = [
        'vues' => $p->query("SELECT table_name FROM information_schema.views WHERE table_schema = 'public'")->fetchAll(PDO::FETCH_COLUMN),
        'procedures' => $p->query("SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
            LEFT JOIN pg_depend d ON d.objid = p.oid AND d.deptype = 'e' WHERE n.nspname = 'public' AND d.objid IS NULL")->fetchAll(PDO::FETCH_COLUMN),
        'declencheurs' => $p->query("SELECT DISTINCT trigger_name FROM information_schema.triggers WHERE trigger_schema = 'public'")->fetchAll(PDO::FETCH_COLUMN),
        'evenements' => [],
    ];
    return [$tables, $autres];
}

// ===========================================================================
// Écriture
// ===========================================================================

function qi(string $n, bool $pg): string
{
    return $pg ? '"' . str_replace('"', '""', $n) . '"' : '`' . str_replace('`', '``', $n) . '`';
}

/** Nom d'index / contrainte unique dans la base cible (PostgreSQL : unique par schéma, 63 car.). */
function nom_objet(string $table, string $nom, int $max): string
{
    $n = str_starts_with($nom, $table . '_') ? $nom : $table . '_' . $nom;
    return strlen($n) <= $max ? $n : substr($n, 0, $max - 9) . '_' . substr(md5($n), 0, 8);
}

function type_pg(array $c): string
{
    $id = $c['auto'] && in_array($c['type'], ['int16', 'int32', 'int64'], true) ? ' GENERATED BY DEFAULT AS IDENTITY' : '';
    switch ($c['type']) {
        case 'bool': return 'boolean';
        case 'int16': return 'smallint' . $id;
        case 'int32': return 'integer' . $id;
        case 'int64': return 'bigint' . $id;
        case 'decimal': return 'numeric(' . max(1, min(1000, $c['precision'])) . ',' . max(0, (int) $c['echelle']) . ')';
        case 'float32': return 'real';
        case 'float64': return 'double precision';
        case 'char': return 'char(' . $c['longueur'] . ')';
        case 'varchar': return $c['longueur'] ? 'varchar(' . $c['longueur'] . ')' : 'varchar';
        case 'text': return 'text';
        case 'binary': return 'bytea';
        case 'date': return 'date';
        case 'time': return 'time(' . min(6, $c['frac']) . ')';
        case 'datetime': return 'timestamp(' . min(6, $c['frac']) . ')';
        case 'datetime_tz': return 'timestamp(' . min(6, $c['frac']) . ') with time zone';
        case 'json': return 'json';
        case 'uuid': return 'uuid';
        case 'enum': return 'varchar(255)';
    }
    return 'text';
}

function type_mysql(array $c, bool $dansIndex): string
{
    switch ($c['type']) {
        case 'bool': return 'tinyint(1)';
        case 'int16': return 'smallint';
        case 'int32': return 'int';
        case 'int64': return 'bigint';
        case 'decimal': return 'decimal(' . max(1, min(65, $c['precision'])) . ',' . max(0, min(30, (int) $c['echelle'], min(65, $c['precision']))) . ')';
        case 'float32': return 'float';
        case 'float64': return 'double';
        case 'char': return $c['longueur'] <= 255 ? 'char(' . $c['longueur'] . ')' : 'varchar(' . $c['longueur'] . ')';
        case 'varchar':
            if (!$c['longueur']) {
                return $dansIndex ? 'varchar(255)' : 'longtext';
            }
            return $c['longueur'] <= 16000 ? 'varchar(' . $c['longueur'] . ')' : 'longtext';
        case 'text': return $dansIndex ? 'varchar(255)' : 'longtext';
        case 'binary': return 'longblob';
        case 'date': return 'date';
        case 'time': return 'time(' . min(6, $c['frac']) . ')';
        case 'datetime':
        case 'datetime_tz': return 'datetime(' . min(6, $c['frac']) . ')';
        case 'json': return 'json';
        case 'uuid': return 'char(36)';
        case 'enum':
            return 'enum(' . implode(',', array_map(fn($v) => "'" . str_replace("'", "''", $v) . "'", $c['valeurs'] ?: [''])) . ')';
    }
    return 'longtext';
}

function defaut_sql(array $c, bool $pg, PDO $dst, string $table): string
{
    $d = $c['defaut'];
    if ($d === null) {
        return '';
    }
    if ($d['k'] === 'now') {
        return in_array($c['type'], ['datetime', 'datetime_tz', 'date'], true) ? ' DEFAULT CURRENT_TIMESTAMP' : '';
    }
    if ($d['k'] === 'expr') {
        avertir('defaut_ignore', ['table' => $table, 'colonne' => $c['nom'], 'defaut' => $d['v']]);
        return '';
    }
    $v = $d['v'];
    if (!$pg && in_array($c['type'], ['text', 'binary', 'json'], true)) {
        // MySQL < 8.0.13 refuse un défaut sur TEXT/BLOB/JSON.
        if ($v !== '') {
            avertir('defaut_ignore', ['table' => $table, 'colonne' => $c['nom'], 'defaut' => $v]);
        }
        return '';
    }
    if (in_array($c['type'], ['date', 'datetime', 'datetime_tz'], true) && str_starts_with($v, '0000-00-00')) {
        return '';
    }
    switch ($c['type']) {
        case 'bool':
            return $pg ? ' DEFAULT ' . (in_array(strtolower($v), ['1', 'true', "b'1'"], true) ? 'true' : 'false') : ' DEFAULT ' . (in_array(strtolower($v), ['1', 'true', "b'1'"], true) ? '1' : '0');
        case 'int16':
        case 'int32':
        case 'int64':
        case 'decimal':
        case 'float32':
        case 'float64':
            return is_numeric($v) ? ' DEFAULT ' . $v : '';
    }
    return ' DEFAULT ' . $dst->quote($v);
}

function est_date_zero($v): bool
{
    return is_string($v) && str_starts_with($v, '0000-00-00');
}

// ---------------------------------------------------------------------------
[$tables, $autres] = $srcPg ? lire_pg($src) : lire_mysql($src);

// Dates « zéro » (MySQL) : n'existent pas en PostgreSQL → NULL (colonne rendue facultative).
if (!$srcPg && $dstPg) {
    foreach ($tables as $tn => &$t) {
        foreach ($t['colonnes'] as &$c) {
            if (!in_array($c['type'], ['date', 'datetime'], true)) {
                continue;
            }
            $zeroDefaut = $c['defaut'] !== null && $c['defaut']['k'] === 'lit' && str_starts_with($c['defaut']['v'], '0000-00-00');
            $n = (int) $src->query('SELECT count(*) FROM ' . qi($tn, false) . ' WHERE ' . qi($c['nom'], false) . " < '0001-01-01' OR " . qi($c['nom'], false) . " LIKE '0000-00-00%'")->fetchColumn();
            if ($n > 0 || $zeroDefaut) {
                if (!$c['nullable']) {
                    $c['nullable'] = true;
                }
                if ($zeroDefaut) {
                    $c['defaut'] = null;
                }
                if ($n > 0) {
                    avertir('dates_zero', ['table' => $tn, 'colonne' => $c['nom'], 'n' => $n]);
                }
            }
        }
        unset($c);
    }
    unset($t);
}

// Colonnes utilisées dans un index ou une clé (MySQL : TEXT non indexable sans longueur).
$indexees = [];
foreach ($tables as $tn => $t) {
    foreach (array_merge($t['pk'], ...array_map(fn($i) => $i['colonnes'], array_values($t['index'])), ...array_map(fn($f) => $f['colonnes'], array_values($t['fk']))) as $cn) {
        $indexees[$tn][$cn] = true;
    }
}

// 1. Tables
foreach ($tables as $tn => $t) {
    $defs = [];
    foreach ($t['colonnes'] as $c) {
        $dansIndex = isset($indexees[$tn][$c['nom']]);
        $type = $dstPg ? type_pg($c) : type_mysql($c, $dansIndex);
        $def = qi($c['nom'], $dstPg) . ' ' . $type;
        if (!$c['nullable']) {
            $def .= ' NOT NULL';
        }
        if (!$c['auto']) {
            $def .= defaut_sql($c, $dstPg, $dst, $tn);
        } elseif (!$dstPg) {
            $def .= ' AUTO_INCREMENT';
        }
        if ($dstPg && $c['type'] === 'enum' && $c['valeurs']) {
            $def .= ' CHECK (' . qi($c['nom'], true) . ' IN (' . implode(', ', array_map(fn($v) => $dst->quote($v), $c['valeurs'])) . '))';
        }
        $defs[] = $def;
    }
    if ($t['pk']) {
        $defs[] = 'PRIMARY KEY (' . implode(', ', array_map(fn($n) => qi($n, $dstPg), $t['pk'])) . ')';
    }
    if (!$dstPg) {
        // AUTO_INCREMENT doit être en tête d'une clé.
        foreach ($t['colonnes'] as $c) {
            if ($c['auto'] && ($t['pk'][0] ?? null) !== $c['nom']) {
                $defs[] = 'KEY ' . qi(nom_objet($tn, $c['nom'] . '_auto', 64), false) . ' (' . qi($c['nom'], false) . ')';
            }
        }
    }
    $sql = 'CREATE TABLE ' . qi($tn, $dstPg) . " (\n  " . implode(",\n  ", $defs) . "\n)";
    if (!$dstPg) {
        $sql .= ' ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci';
    }
    $dst->exec($sql);
    if (!$t['pk']) {
        avertir('sans_cle_primaire', ['table' => $tn]);
    }
}

// 2. Données
if (!$dstPg) {
    $dst->exec('SET FOREIGN_KEY_CHECKS = 0');
    $dst->exec('SET UNIQUE_CHECKS = 0');
}
$resume = [];
$nulTexte = [];
foreach ($tables as $tn => $t) {
    $cols = $t['colonnes'];
    $noms = array_map(fn($c) => qi($c['nom'], $srcPg), $cols);
    $select = 'SELECT ' . implode(', ', $noms) . ' FROM ' . qi($tn, $srcPg);
    $parLot = max(1, min(LOT_LIGNES, intdiv(LOT_PARAMS, max(1, count($cols)))));
    $insertTete = 'INSERT INTO ' . qi($tn, $dstPg) . ' (' . implode(', ', array_map(fn($c) => qi($c['nom'], $dstPg), $cols)) . ') VALUES ';
    $place = '(' . implode(', ', array_fill(0, count($cols), '?')) . ')';
    $stmts = [];
    $lot = [];
    $total = 0;

    $ecrire = function (array $lignes) use (&$stmts, $dst, $insertTete, $place, $cols, $dstPg) {
        $n = count($lignes);
        $stmts[$n] ??= $dst->prepare($insertTete . implode(', ', array_fill(0, $n, $place)));
        $s = $stmts[$n];
        $i = 1;
        foreach ($lignes as $l) {
            foreach ($cols as $k => $c) {
                $v = $l[$k];
                if ($v === null) {
                    $s->bindValue($i++, null, PDO::PARAM_NULL);
                } elseif ($c['type'] === 'binary') {
                    $s->bindValue($i++, $v, PDO::PARAM_LOB);
                } elseif ($c['type'] === 'bool') {
                    $b = is_bool($v) ? $v : !in_array(strtolower((string) $v), ['0', 'f', 'false', '', "\0"], true);
                    $dstPg ? $s->bindValue($i++, $b, PDO::PARAM_BOOL) : $s->bindValue($i++, $b ? 1 : 0, PDO::PARAM_INT);
                } else {
                    $s->bindValue($i++, (string) $v, PDO::PARAM_STR);
                }
            }
        }
        $s->execute();
    };

    $convertir = function (array $r) use ($cols, $srcPg, $dstPg, $tn, &$nulTexte): array {
        $out = [];
        foreach ($cols as $c) {
            $v = $r[$c['nom']];
            if (is_resource($v)) {
                $v = stream_get_contents($v);
            }
            if ($v !== null) {
                if (in_array($c['type'], ['date', 'datetime', 'datetime_tz'], true) && est_date_zero($v)) {
                    $v = null;
                } elseif ($c['type'] === 'datetime_tz' && !$dstPg) {
                    // PostgreSQL renvoie « 2024-05-01 10:00:00+00 » (session en UTC) : MySQL n'a pas de fuseau.
                    $v = preg_replace('/([+-]\d{2}(:?\d{2})?)$/', '', (string) $v);
                } elseif ($dstPg && in_array($c['type'], ['text', 'varchar', 'char', 'json', 'enum'], true) && is_string($v) && str_contains($v, "\0")) {
                    // PostgreSQL refuse le caractère nul dans un texte.
                    $v = str_replace("\0", '', $v);
                    $nulTexte[$tn . '.' . $c['nom']] = ($nulTexte[$tn . '.' . $c['nom']] ?? 0) + 1;
                }
            }
            $out[] = $v;
        }
        return $out;
    };

    $dst->beginTransaction();
    if ($srcPg) {
        // Curseur côté serveur : les grosses tables ne sont pas chargées en mémoire.
        $src->beginTransaction();
        $src->exec('DECLARE rustserv_c NO SCROLL CURSOR FOR ' . $select);
        while (true) {
            $rows = $src->query('FETCH 1000 FROM rustserv_c')->fetchAll();
            if (!$rows) {
                break;
            }
            foreach ($rows as $r) {
                $lot[] = $convertir($r);
                if (count($lot) >= $parLot) {
                    $ecrire($lot);
                    $total += count($lot);
                    $lot = [];
                }
            }
        }
        $src->exec('CLOSE rustserv_c');
        $src->commit();
    } else {
        $src->setAttribute(PDO::MYSQL_ATTR_USE_BUFFERED_QUERY, false);
        $q = $src->query($select);
        while (($r = $q->fetch()) !== false) {
            $lot[] = $convertir($r);
            if (count($lot) >= $parLot) {
                $ecrire($lot);
                $total += count($lot);
                $lot = [];
            }
        }
        $q->closeCursor();
        $src->setAttribute(PDO::MYSQL_ATTR_USE_BUFFERED_QUERY, true);
    }
    if ($lot) {
        $ecrire($lot);
        $total += count($lot);
    }
    $dst->commit();
    $resume[] = ['nom' => $tn, 'lignes' => $total];
}
foreach ($nulTexte as $cc => $n) {
    [$tt, $cn] = explode('.', $cc, 2);
    avertir('caractere_nul', ['table' => $tt, 'colonne' => $cn, 'n' => $n]);
}

// 3. Compteurs automatiques (PostgreSQL : la séquence repart après le plus grand identifiant).
foreach ($tables as $tn => $t) {
    foreach ($t['colonnes'] as $c) {
        if (!$c['auto']) {
            continue;
        }
        if ($dstPg) {
            $dst->query('SELECT setval(pg_get_serial_sequence(' . $dst->quote(qi($tn, true)) . ', ' . $dst->quote($c['nom']) . '), '
                . 'COALESCE((SELECT MAX(' . qi($c['nom'], true) . ') FROM ' . qi($tn, true) . '), 0) + 1, false)');
        }
        // MySQL / MariaDB recalent AUTO_INCREMENT tout seuls après insertion d'identifiants explicites.
    }
}

// 4. Index puis clés étrangères (une clé impossible, ex. lignes orphelines, est signalée).
foreach ($tables as $tn => $t) {
    foreach ($t['index'] as $i) {
        $cols = implode(', ', array_map(function ($n) use ($tn, $t, $dstPg) {
            if (!$dstPg) {
                foreach ($t['colonnes'] as $c) {
                    if ($c['nom'] === $n && in_array($c['type'], ['binary', 'json'], true)) {
                        return qi($n, false) . '(191)';
                    }
                }
            }
            return qi($n, $dstPg);
        }, $i['colonnes']));
        $nom = nom_objet($tn, $i['nom'], $dstPg ? 63 : 64);
        try {
            $dst->exec('CREATE ' . ($i['unique'] ? 'UNIQUE ' : '') . 'INDEX ' . qi($nom, $dstPg) . ' ON ' . qi($tn, $dstPg) . ' (' . $cols . ')');
        } catch (PDOException $e) {
            avertir('index_impossible', ['table' => $tn, 'index' => $i['nom'], 'detail' => $e->getMessage()]);
        }
    }
}
if (!$dstPg) {
    $dst->exec('SET FOREIGN_KEY_CHECKS = 1');
    $dst->exec('SET UNIQUE_CHECKS = 1');
}
foreach ($tables as $tn => $t) {
    foreach ($t['fk'] as $f) {
        if (!isset($tables[$f['table_ref']])) {
            continue;
        }
        $sql = 'ALTER TABLE ' . qi($tn, $dstPg) . ' ADD CONSTRAINT ' . qi(nom_objet($tn, $f['nom'], $dstPg ? 63 : 64), $dstPg)
            . ' FOREIGN KEY (' . implode(', ', array_map(fn($n) => qi($n, $dstPg), $f['colonnes'])) . ')'
            . ' REFERENCES ' . qi($f['table_ref'], $dstPg) . ' (' . implode(', ', array_map(fn($n) => qi($n, $dstPg), $f['colonnes_ref'])) . ')'
            . ' ON DELETE ' . $f['suppr'] . ' ON UPDATE ' . $f['maj'];
        try {
            $dst->exec($sql);
        } catch (PDOException $e) {
            avertir('cle_etrangere_impossible', ['table' => $tn, 'cle' => $f['nom'], 'detail' => $e->getMessage()]);
        }
    }
}

sortir([
    'tables' => $resume,
    'avertissements' => $avertissements,
    'non_convertis' => $autres,
    'secondes' => round(microtime(true) - $debut, 2),
]);
