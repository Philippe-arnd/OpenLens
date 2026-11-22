<?php
// Fake OpenLens for the test, run with `php -S`: records POST bodies in a
// file, and answers /flags with "beta" for admins.
$log = getenv('OL_LOG') ?: sys_get_temp_dir() . '/openlens-php-test.jsonl';
if ($_SERVER['REQUEST_METHOD'] === 'POST') {
    $key = $_SERVER['HTTP_X_OPENLENS_KEY'] ?? '';
    file_put_contents($log, json_encode(['key' => $key, 'body' => json_decode(file_get_contents('php://input'), true)]) . "\n", FILE_APPEND);
    echo '{"accepted":1}';
    return true;
}
header('content-type: application/json');
echo json_encode(['flags' => (($_GET['role'] ?? '') === 'admin') ? ['beta'] : []]);
return true;
