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
 */
final class OpenLens
{
    public const VERSION = '0.1.0';

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
        $this->release = $options['release'] ?? null;
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
        if ($distinctId === '' || $event === '') {
            return;
        }
        $props = ['$lib' => 'php', '$environment' => $this->environment];
        if ($this->release !== null) {
            $props['$release'] = $this->release;
        }
        $this->queue[] = [
            'event' => $event,
            'distinct_id' => $distinctId,
            'uuid' => self::uuid(),
            'timestamp' => self::now(),
            'properties' => array_merge($props, $properties),
        ];
        if (count($this->queue) >= $this->flushAt) {
            $this->flush();
        }
    }

    /** Mark a use of a feature (a task start). @param array<string, mixed> $properties */
    public function feature(string $distinctId, string $key, array $properties = []): void
    {
        $this->capture($distinctId, '$feature', array_merge($properties, ['$feature' => $key]));
    }

    /**
     * Set the user's role (and account, plan). `created_at` (a DateTimeInterface
     * or an RFC 3339 string) is when the account was created: its signup.
     * @param array{role?: string, account?: string, plan?: string, created_at?: string|\DateTimeInterface} $traits
     */
    public function identify(string $distinctId, array $traits = []): void
    {
        $p = [];
        foreach (['role', 'account', 'plan'] as $t) {
            if (isset($traits[$t]) && $traits[$t] !== '') {
                $p['$' . $t] = $traits[$t];
            }
        }
        $created = $traits['created_at'] ?? null;
        if ($created instanceof \DateTimeInterface) {
            $p['$created_at'] = $created->format(\DateTimeInterface::RFC3339);
        } elseif (is_string($created) && $created !== '') {
            $p['$created_at'] = $created;
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
            $body = json_encode(['sent_at' => self::now(), 'batch' => $batch], JSON_THROW_ON_ERROR | JSON_UNESCAPED_SLASHES);
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
