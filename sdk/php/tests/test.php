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

$errors = [];
$down = new OpenLens('http://127.0.0.1:9', 'pk_t', ['timeout' => 1.0, 'onError' => function (\Throwable $e) use (&$errors): void {
    $errors[] = $e;
}]);
$down->capture('u1', 'x');
$down->flush();
check(!$down->isEnabled('beta', 'u1'), 'flag off when down');
check(count($errors) >= 1, 'errors reach onError');

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
