<?php

declare(strict_types=1);

namespace OpenLens;

/**
 * OpenLens server SDK for PHP: a thin client over the HTTP API.
 *
 * No extension beyond json (streams, not curl). PHP has no background
 * thread, so events are queued and sent when the queue reaches flushAt, and
 * at the end of the request (a shutdown function). Errors never reach the
 * app: they go to onError.
 *
 *   $ol = new \OpenLens\OpenLens('https://lens.example.com', 'pk_...', ['release' => 'v1.4.2']);
 *   $ol->identify('42', ['role' => 'owner', 'account' => 'acme']);
 *   $ol->capture('42', 'invoice_sent');
 *   if ($ol->isEnabled('new-billing', '42', ['role' => 'owner'])) { ... }
 *
 * Server errors: $ol->captureException($e) in a catch (or the framework's
 * report hook), and $ol->captureUncaught() once for crashes.
 */
final class OpenLens
{
    public const VERSION = '0.1.0';
    /** The distinct id of events that belong to no user (server errors). */
    public const SERVER_ID = '$server';
    /** The header the browser SDK adds to its own requests: the session id. */
    public const SESSION_HEADER = 'x-openlens-session';
    /** Variables hosts set to the deployed commit, in order of preference (#495). */
    public const COMMIT_VARS = ['SOURCE_COMMIT', 'GITHUB_SHA', 'VERCEL_GIT_COMMIT_SHA', 'CF_PAGES_COMMIT_SHA', 'RENDER_GIT_COMMIT', 'HEROKU_SLUG_COMMIT'];
    /** At most this many errors with one message per minute. */
    private const MAX_SAME_ERROR = 5;
    private const FATAL = E_ERROR | E_PARSE | E_CORE_ERROR | E_COMPILE_ERROR | E_USER_ERROR;

    private string $host;
    private string $key;
    private ?string $release;
    private string $environment;
    private int $flushAt;
    private float $timeout;
    /** @var callable(\Throwable): void */
    private $onError;
    /** @var list<array<string, mixed>> */
    private array $queue = [];
    /** @var \WeakMap<\Throwable, true> */
    private \WeakMap $reported;
    private int $seenMinute = -1;
    /** @var array<string, int> */
    private array $seenErrors = [];

    /**
     * @param array{release?: string, environment?: string, flushAt?: int, timeout?: float, onError?: callable} $options
     */
    public function __construct(string $host, string $key, array $options = [])
    {
        if ($host === '' || $key === '') {
            throw new \InvalidArgumentException('OpenLens: host and key are required');
        }
        $this->host = rtrim($host, '/');
        $this->key = $key;
        // An explicit release wins; else what the host exposes (#495).
        $this->release = $options['release'] ?? self::detectRelease();
        $this->reported = new \WeakMap();
        $env = getenv('OPENLENS_ENVIRONMENT');
        $this->environment = $options['environment'] ?? ($env !== false && $env !== '' ? $env : 'production');
        $this->flushAt = $options['flushAt'] ?? 50;
        $this->timeout = $options['timeout'] ?? 5.0;
        $this->onError = $options['onError'] ?? static function (\Throwable $e): void {
        };
        register_shutdown_function([$this, 'flush']);
    }

    /** Record an event for a user. @param array<string, mixed> $properties */
    public function capture(string $distinctId, string $event, array $properties = []): void
    {
        $this->push($distinctId, $event, $properties, null);
    }

    /**
     * The release the host exposes: the deployed commit (shortened to 12
     * characters when it is a full hash), else null. Reads getenv() unless
     * $env is given.
     * @param array<string, string>|null $env
     */
    public static function detectRelease(?array $env = null): ?string
    {
        foreach (self::COMMIT_VARS as $name) {
            $v = $env === null ? getenv($name) : ($env[$name] ?? false);
            $v = is_string($v) ? trim($v) : '';
            if ($v !== '') {
                return preg_match('/^[0-9a-f]{13,64}$/i', $v) === 1 ? strtolower(substr($v, 0, 12)) : substr($v, 0, 200);
            }
        }
        return null;
    }

    /**
     * `traceId` of the incoming W3C traceparent (read, never written) and
     * `sessionId` from the browser SDK's x-openlens-session header, for
     * captureException. Takes $_SERVER (HTTP_TRACEPARENT) or a header array
     * (PSR-7 getHeaders() included).
     * @param array<string, mixed> $headers
     * @return array{traceId?: string, sessionId?: string}
     */
    public static function requestContext(array $headers): array
    {
        $h = [];
        foreach ($headers as $k => $v) {
            $v = is_array($v) ? ($v[0] ?? '') : $v;
            if (is_scalar($v)) {
                $name = str_replace('_', '-', strtolower((string) $k));
                $h[str_starts_with($name, 'http-') ? substr($name, 5) : $name] = trim((string) $v);
            }
        }
        $ctx = [];
        if (preg_match('/^([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$/D', $h['traceparent'] ?? '', $m) === 1
            && $m[1] !== 'ff' && trim($m[2], '0') !== '' && trim($m[3], '0') !== '') {
            $ctx['traceId'] = $m[2];
        }
        $sid = $h[self::SESSION_HEADER] ?? '';
        if (preg_match('/^[A-Za-z0-9_-]{1,100}$/D', $sid) === 1) {
            $ctx['sessionId'] = $sid;
        }
        return $ctx;
    }

    /**
     * Report a server exception. It is sent as `$error` with `$kind: "server"`,
     * so it is grouped, alerted on and judged per release like a browser
     * error. `sessionId` and `traceId` come from requestContext($_SERVER);
     * `route` is the route template (`/entries/{id}`), never the raw URL.
     * Never throws.
     * @param array{distinctId?: string, sessionId?: string, traceId?: string, route?: string, method?: string, properties?: array<string, mixed>} $context
     */
    public function captureException(\Throwable $e, array $context = []): void
    {
        try {
            if (isset($this->reported[$e])) {
                return;
            }
            $this->reported[$e] = true;
            $frames = [['file' => $e->getFile(), 'line' => $e->getLine()]];
            foreach ($e->getTrace() as $f) {
                $frames[] = ['file' => $f['file'] ?? '', 'line' => $f['line'] ?? 0];
            }
            $stack = get_class($e) . ': ' . $e->getMessage() . "\n" . $e->getTraceAsString();
            $this->captureError(get_class($e), $e->getMessage(), $stack, $frames, $context);
        } catch (\Throwable $err) {
            ($this->onError)($err);
        }
    }

    /**
     * Report the exception or fatal error that ends the script, at the end of
     * the request, then let PHP fail as it would have (its own message and
     * exit code are kept). Opt-in.
     */
    public function captureUncaught(): void
    {
        register_shutdown_function(function (): void {
            try {
                $err = error_get_last();
                if ($err === null || ($err['type'] & self::FATAL) === 0) {
                    return;
                }
                $text = $err['message'];
                [$head] = explode("\nStack trace:", $text, 2);
                $frames = [['file' => $err['file'], 'line' => $err['line']]];
                if (preg_match('/^Uncaught ([^\s:]+): (.*) in (.+):(\d+)$/s', $head, $m) === 1) {
                    [, $type, $message] = $m;
                    if (preg_match_all('/^#\d+ (\S+?)\((\d+)\)/m', $text, $fs, PREG_SET_ORDER) > 0) {
                        foreach ($fs as $f) {
                            $frames[] = ['file' => $f[1], 'line' => (int) $f[2]];
                        }
                    }
                } else {
                    [$type, $message] = ['FatalError', $head];
                }
                $this->captureError($type, $message, $type . ': ' . $text, $frames, []);
                $this->flush();
            } catch (\Throwable $e) {
                ($this->onError)($e);
            }
        });
    }

    /**
     * @param list<array{file: string, line: int}> $frames innermost first
     * @param array<string, mixed> $context
     */
    private function captureError(string $type, string $msg, string $stack, array $frames, array $context): void
    {
        $message = self::cut($type . ': ' . $msg, 500);
        if (!$this->allowError($message)) {
            return;
        }
        $props = is_array($context['properties'] ?? null) ? $context['properties'] : [];
        $props['$kind'] = 'server';
        $props['$message'] = $message;
        $props['$stack'] = self::cut($stack, 4000);
        foreach (array_slice($frames, 0, 50) as $f) {
            // The first frame of the app's own code: the error's group is its message @ this file.
            if ($f['file'] !== '' && !str_contains($f['file'], '/vendor/')) {
                $props['$source'] = $f['file'] . ':' . $f['line'] . ':0';
                break;
            }
        }
        // OpenTelemetry semantic-convention names next to OpenLens's own.
        $props['exception.type'] = substr($type, 0, 200);
        if (isset($context['route']) && $context['route'] !== '') {
            $props['http.route'] = self::cut((string) $context['route'], 300);
        }
        if (isset($context['method']) && $context['method'] !== '') {
            $props['http.request.method'] = substr(strtoupper((string) $context['method']), 0, 10);
        }
        $trace = $context['traceId'] ?? '';
        if (is_string($trace) && preg_match('/^[0-9a-f]{32}$/D', $trace) === 1 && trim($trace, '0') !== '') {
            $props['trace_id'] = $trace;
        }
        $sid = $context['sessionId'] ?? '';
        $session = is_string($sid) && preg_match('/^[A-Za-z0-9_-]{1,100}$/D', $sid) === 1 ? $sid : null;
        $distinct = $context['distinctId'] ?? '';
        $this->push(is_string($distinct) && $distinct !== '' ? $distinct : self::SERVER_ID, '$error', $props, $session);
    }

    /** At most MAX_SAME_ERROR of one message per minute. */
    private function allowError(string $message): bool
    {
        $minute = intdiv(time(), 60);
        if ($minute !== $this->seenMinute) {
            $this->seenMinute = $minute;
            $this->seenErrors = [];
        }
        $n = ($this->seenErrors[$message] ?? 0) + 1;
        if (count($this->seenErrors) < 1000 || isset($this->seenErrors[$message])) {
            $this->seenErrors[$message] = $n;
        }
        return $n <= self::MAX_SAME_ERROR;
    }

    /** @param array<string, mixed> $properties */
    private function push(string $distinctId, string $event, array $properties, ?string $sessionId): void
    {
        if ($distinctId === '' || $event === '') {
            return;
        }
        $props = ['$lib' => 'php', '$environment' => $this->environment];
        if ($this->release !== null) {
            $props['$release'] = $this->release;
        }
        $item = [
            'event' => $event,
            'distinct_id' => $distinctId,
            'uuid' => self::uuid(),
            'timestamp' => self::now(),
            'properties' => array_merge($props, $properties),
        ];
        if ($sessionId !== null) {
            $item['session_id'] = $sessionId;
        }
        $this->queue[] = $item;
        if (count($this->queue) >= $this->flushAt) {
            $this->flush();
        }
    }

    /** Mark a use of a feature (a task start). @param array<string, mixed> $properties */
    public function feature(string $distinctId, string $key, array $properties = []): void
    {
        $this->capture($distinctId, '$feature', array_merge($properties, ['$feature' => $key]));
    }

    /** Set the user's role (and account, plan). @param array{role?: string, account?: string, plan?: string} $traits */
    public function identify(string $distinctId, array $traits = []): void
    {
        $p = [];
        foreach (['role', 'account', 'plan'] as $t) {
            if (isset($traits[$t]) && $traits[$t] !== '') {
                $p['$' . $t] = $traits[$t];
            }
        }
        $this->capture($distinctId, '$identify', $p);
    }

    /**
     * Is a flag on for this user? false on any error. `plan` and
     * `properties` feed the flag's rules (never stored).
     * @param array{role?: string, account?: string, plan?: string, properties?: array<string, scalar>} $traits
     */
    public function isEnabled(string $flag, string $distinctId, array $traits = []): bool
    {
        $q = ['key' => $this->key, 'distinct_id' => $distinctId];
        foreach (['role', 'account', 'plan'] as $t) {
            if (isset($traits[$t]) && $traits[$t] !== '') {
                $q[$t] = $traits[$t];
            }
        }
        foreach (array_slice($traits['properties'] ?? [], 0, 20, true) as $k => $v) {
            if ($v !== null && $v !== '') {
                $q['p.' . strtolower((string) $k)] = is_bool($v) ? ($v ? 'true' : 'false') : (string) $v;
            }
        }
        try {
            $body = $this->request('GET', '/flags?' . http_build_query($q), null);
            $data = json_decode($body, true, 16, JSON_THROW_ON_ERROR);
            $flags = is_array($data) ? ($data['flags'] ?? []) : [];
            return is_array($flags) && in_array($flag, $flags, true);
        } catch (\Throwable $e) {
            ($this->onError)($e);
            return false;
        }
    }

    /**
     * Job check-in (#609): call it at the end of a cron or background job, so
     * OpenLens alerts when the job does not run, fails or gets slower. Sent at
     * once (not queued); false on any error, never throws.
     * `status`: 'ok' (default) or 'error'; `ms`: the run's duration;
     * `expectEvery`: seconds between runs, instead of the learned schedule.
     * @param array{status?: string, ms?: int|float, expectEvery?: int|float} $options
     */
    public function checkIn(string $slug, array $options = []): bool
    {
        $q = [];
        if (isset($options['status']) && $options['status'] !== '') {
            $q['status'] = $options['status'];
        }
        if (isset($options['ms'])) {
            $q['ms'] = (string) max(0, (int) round((float) $options['ms']));
        }
        if (isset($options['expectEvery'])) {
            $q['expectEvery'] = (string) (int) round((float) $options['expectEvery']);
        }
        $path = '/check-in/' . rawurlencode($slug) . ($q === [] ? '' : '?' . http_build_query($q));
        try {
            $this->request('POST', $path, null);
            return true;
        } catch (\Throwable $e) {
            ($this->onError)($e);
            return false;
        }
    }

    /** Send queued events now. */
    public function flush(): void
    {
        if ($this->queue === []) {
            return;
        }
        $batch = $this->queue;
        $this->queue = [];
        try {
            $body = json_encode(['sent_at' => self::now(), 'batch' => $batch], JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES | JSON_INVALID_UTF8_SUBSTITUTE);
            $this->request('POST', '/capture', $body);
        } catch (\Throwable $e) {
            ($this->onError)($e);
        }
    }

    private function request(string $method, string $path, ?string $body): string
    {
        $headers = "x-openlens-key: {$this->key}\r\n";
        if ($body !== null) {
            $headers .= "content-type: application/json\r\n";
        }
        $ctx = stream_context_create(['http' => [
            'method' => $method,
            'header' => $headers,
            'content' => $body ?? '',
            'timeout' => $this->timeout,
            'ignore_errors' => true,
        ]]);
        $err = null;
        set_error_handler(static function (int $no, string $msg) use (&$err): bool {
            $err = $msg;
            return true;
        });
        try {
            $res = file_get_contents($this->host . $path, false, $ctx);
        } finally {
            restore_error_handler();
        }
        if ($res === false) {
            throw new \RuntimeException('OpenLens: ' . ($err ?? 'request failed'));
        }
        // PHP < 8.4 sets $http_response_header in this scope.
        $status = 0;
        $lines = function_exists('http_get_last_response_headers') ? (http_get_last_response_headers() ?? []) : ($http_response_header ?? []);
        if (isset($lines[0]) && preg_match('#^HTTP/\S+ (\d{3})#', $lines[0], $m) === 1) {
            $status = (int) $m[1];
        }
        if ($status < 200 || $status >= 300) {
            throw new \RuntimeException("OpenLens: {$method} {$path} returned {$status}");
        }
        return $res;
    }

    /** At most $n bytes, without a UTF-8 character cut in the middle (no mbstring). */
    private static function cut(string $s, int $n): string
    {
        if (strlen($s) <= $n) {
            return $s;
        }
        return (string) preg_replace('/[\xC0-\xFF][\x80-\xBF]*$/', '', substr($s, 0, $n));
    }

    private static function now(): string
    {
        return (new \DateTimeImmutable('now', new \DateTimeZone('UTC')))->format('Y-m-d\TH:i:s.v\Z');
    }

    private static function uuid(): string
    {
        $b = random_bytes(16);
        $b[6] = chr((ord($b[6]) & 0x0f) | 0x40);
        $b[8] = chr((ord($b[8]) & 0x3f) | 0x80);
        return vsprintf('%s%s-%s-%s-%s-%s%s%s', str_split(bin2hex($b), 4));
    }
}
