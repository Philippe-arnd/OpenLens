<?php
// Fake OpenLens for the test, run with `php -S`: records POST bodies in a
// file, and answers /flags with "beta" for admins.
$log = getenv('OL_LOG') ?: sys_get_temp_dir() . '/openlens-php-test.jsonl';
$path = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);
if ($_SERVER['REQUEST_METHOD'] === 'POST' && str_starts_with((string) $path, '/check-in/')) {
    // Job check-ins: recorded with their URI; the job `too-fast` is refused.
    file_put_contents($log, json_encode(['key' => $_SERVER['HTTP_X_OPENLENS_KEY'] ?? '', 'check_in' => $_SERVER['REQUEST_URI']]) . "\n", FILE_APPEND);
    http_response_code($path === '/check-in/too-fast' ? 429 : 200);
    echo '{}';
    return true;
}
if ($_SERVER['REQUEST_METHOD'] === 'POST') {
    $key = $_SERVER['HTTP_X_OPENLENS_KEY'] ?? '';
    file_put_contents($log, json_encode(['key' => $key, 'body' => json_decode(file_get_contents('php://input'), true)]) . "\n", FILE_APPEND);
    echo '{"accepted":1}';
    return true;
}
header('content-type: application/json');
echo json_encode(['flags' => (($_GET['role'] ?? '') === 'admin') ? ['beta'] : []]);
return true;
