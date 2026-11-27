<?php
// Run: php tests/test.php (starts `php -S` on a free port). Exits non-zero on failure.
declare(strict_types=1);
require __DIR__ . '/../src/OpenLens.php';

use OpenLens\OpenLens;

function check(bool $ok, string $what): void
{
    if (!$ok) {
        fwrite(STDERR, "FAIL: $what\n");
        exit(1);
    }
    echo "ok - $what\n";
}

$log = tempnam(sys_get_temp_dir(), 'ol');
$sock = stream_socket_server('tcp://127.0.0.1:0');
$port = (int) substr(strrchr(stream_socket_get_name($sock, false), ':'), 1);
fclose($sock);
$srv = proc_open([PHP_BINARY, '-S', "127.0.0.1:$port", __DIR__ . '/server.php'], [1 => ['file', '/dev/null', 'w'], 2 => ['file', '/dev/null', 'w']], $pipes, null, ['OL_LOG' => $log]);
for ($i = 0; $i < 50 && @fsockopen('127.0.0.1', $port) === false; $i++) {
    usleep(100000);
}

$ol = new OpenLens("http://127.0.0.1:$port/", 'pk_t', ['release' => 'v9']);
$ol->identify('u1', ['role' => 'admin']);
$ol->capture('u1', 'invoice_sent');
$ol->feature('u1', 'export');
$ol->flush();
$lines = array_values(array_filter(explode("\n", (string) file_get_contents($log))));
check(count($lines) === 1, 'one batch');
$got = json_decode($lines[0], true);
check($got['key'] === 'pk_t', 'key header');
check(array_column($got['body']['batch'], 'event') === ['$identify', 'invoice_sent', '$feature'], 'event order');
check($got['body']['batch'][0]['properties']['$role'] === 'admin', 'role');
check($got['body']['batch'][1]['properties']['$release'] === 'v9', 'release');
check($got['body']['batch'][1]['properties']['$lib'] === 'php', 'lib');
check(preg_match('/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/', $got['body']['batch'][0]['uuid']) === 1, 'uuid v4');
check($ol->isEnabled('beta', 'u1', ['role' => 'admin']), 'flag on for admin');
check(!$ol->isEnabled('beta', 'u2', ['role' => 'member']), 'flag off for member');

$ciErrors = [];
$ci = new OpenLens("http://127.0.0.1:$port", 'pk_t', ['onError' => function (\Throwable $e) use (&$ciErrors): void {
    $ciErrors[] = $e;
}]);
check($ci->checkIn('nightly-export', ['status' => 'error', 'ms' => 1234.4, 'expectEvery' => 86400]), 'check-in sent');
$entries = array_map(fn ($l) => json_decode($l, true), array_values(array_filter(explode("\n", (string) file_get_contents($log)))));
$checkIns = array_values(array_filter($entries, fn ($e) => isset($e['check_in'])));
check(count($checkIns) === 1 && $checkIns[0]['key'] === 'pk_t', 'check-in with the key header');
check(!str_contains($checkIns[0]['check_in'], 'pk_t'), 'key not in the URL');
check($checkIns[0]['check_in'] === '/check-in/nightly-export?status=error&ms=1234&expectEvery=86400', 'check-in path and query');
check(!$ci->checkIn('too-fast') && count($ciErrors) === 1, 'a refused check-in is false and reaches onError');

$errors = [];
$down = new OpenLens('http://127.0.0.1:9', 'pk_t', ['timeout' => 1.0, 'onError' => function (\Throwable $e) use (&$errors): void {
    $errors[] = $e;
}]);
$down->capture('u1', 'x');
$down->flush();
check(!$down->isEnabled('beta', 'u1'), 'flag off when down');
check(count($errors) >= 1, 'errors reach onError');

check(OpenLens::detectRelease(['RENDER_GIT_COMMIT' => 'abc', 'GITHUB_SHA' => str_repeat('0123456789abcdef', 2)]) === '0123456789ab', 'host commit, earlier variable first, shortened');
check(OpenLens::detectRelease(['SOURCE_COMMIT' => ' ', 'HEROKU_SLUG_COMMIT' => 'v12']) === 'v12', 'blank variable skipped');
check(OpenLens::detectRelease([]) === null, 'no release when the host says nothing');

// Server errors.
file_put_contents($log, '');
$ex = new OpenLens("http://127.0.0.1:$port", 'pk_t');
$ctx = OpenLens::requestContext(['HTTP_TRACEPARENT' => '00-' . str_repeat('a', 32) . '-' . str_repeat('b', 16) . '-01', 'HTTP_X_OPENLENS_SESSION' => 's1']);
$err = new \RuntimeException("entry 42 \xC3");
$ex->captureException($err, $ctx + ['route' => '/entries/{id}', 'method' => 'get']);
$ex->captureException($err); // the same object is reported once
for ($i = 0; $i < 10; $i++) {
    $ex->captureException(new \LogicException('same'));
}
$ex->flush();
$sent = [];
foreach (array_filter(explode("\n", (string) file_get_contents($log))) as $l) {
    $sent = array_merge($sent, json_decode($l, true)['body']['batch'] ?? []);
}
$first = $sent[0];
$p = $first['properties'];
check($first['distinct_id'] === '$server' && $first['session_id'] === 's1' && $p['trace_id'] === str_repeat('a', 32), 'server error with its request context');
check($p['$kind'] === 'server' && $p['exception.type'] === 'RuntimeException' && $p['http.route'] === '/entries/{id}' && $p['http.request.method'] === 'GET', 'server error properties');
check(str_contains($p['$source'], 'test.php:'), 'source is the app frame');
check(count(array_filter($sent, fn ($e) => $e['properties']['exception.type'] === 'RuntimeException')) === 1, 'one object reported once (and invalid UTF-8 does not lose the batch)');
check(count(array_filter($sent, fn ($e) => $e['properties']['$message'] === 'LogicException: same')) < 10, 'a repeated message is rate-limited');
check(OpenLens::requestContext(['traceparent' => '00-' . str_repeat('0', 32) . '-' . str_repeat('b', 16) . '-01', 'x-openlens-session' => 'a b']) === [], 'hostile context ignored');

// An uncaught exception is reported by the shutdown function.
file_put_contents($log, '');
$script = tempnam(sys_get_temp_dir(), 'olc');
file_put_contents($script, '<?php require ' . var_export(__DIR__ . '/../src/OpenLens.php', true) . '; $ol = new \\OpenLens\\OpenLens(' . var_export("http://127.0.0.1:$port", true) . ", 'pk_t'); \$ol->captureUncaught(); throw new \\DomainException('crash');");
exec(escapeshellarg(PHP_BINARY) . ' ' . escapeshellarg($script) . ' 2>/dev/null', $out, $code);
unlink($script);
$crash = json_decode(trim((string) file_get_contents($log)), true)['body']['batch'][0]['properties'] ?? [];
check($code !== 0, 'the script still fails');
check(($crash['exception.type'] ?? '') === 'DomainException' && ($crash['$message'] ?? '') === 'DomainException: crash' && str_starts_with($crash['$source'] ?? '', $script), 'uncaught exception reported once with its source');

$threw = false;
try {
    new OpenLens('http://x', '');
} catch (\InvalidArgumentException $e) {
    $threw = true;
}
check($threw, 'requires host and key');

proc_terminate($srv);
unlink($log);
echo "all passed\n";
