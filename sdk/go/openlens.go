// Package openlens is the OpenLens server SDK for Go: a thin client over the
// HTTP API, with the standard library only.
//
// Events are queued and sent in batches from a background goroutine. Errors
// never reach the app: they go to Config.OnError.
//
//	ol, err := openlens.New(openlens.Config{Host: "https://lens.example.com", Key: "pk_...", Release: "v1.4.2"})
//	ol.Identify("42", openlens.Traits{Role: "owner", Account: "acme"})
//	ol.Capture("42", "invoice_sent", nil)
//	if ol.IsEnabled(ctx, "new-billing", "42", openlens.Traits{Role: "owner"}) { ... }
//	ol.Shutdown()
package openlens

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"sync"
	"time"
)

// Version of this SDK.
const Version = "0.1.0"

// Config holds the client settings. Host and Key are required.
type Config struct {
	Host          string
	Key           string
	Release       string
	Environment   string        // default: $OPENLENS_ENVIRONMENT, else "production"
	FlushAt       int           // default 50 events
	FlushInterval time.Duration // default 5 s
	Timeout       time.Duration // default 5 s
	OnError       func(error)
	HTTPClient    *http.Client
}

// Traits are what OpenLens slices every KPI by.
type Traits struct {
	Role    string
	Account string
	Plan    string
	// Properties feed flag rules (IsEnabled only, never stored).
	Properties map[string]string
}

type event struct {
	Event      string         `json:"event"`
	DistinctID string         `json:"distinct_id"`
	UUID       string         `json:"uuid"`
	Timestamp  string         `json:"timestamp"`
	Properties map[string]any `json:"properties"`
}

// Client sends events and checks flags. It is safe for concurrent use.
type Client struct {
	cfg   Config
	host  string
	http  *http.Client
	mu    sync.Mutex
	queue []event
	stop  chan struct{}
	done  chan struct{}
	once  sync.Once
}

// New starts a client. It fails only when Host or Key is missing.
func New(cfg Config) (*Client, error) {
	if cfg.Host == "" || cfg.Key == "" {
		return nil, errors.New("openlens: Host and Key are required")
	}
	if cfg.Environment == "" {
		cfg.Environment = os.Getenv("OPENLENS_ENVIRONMENT")
	}
	if cfg.Environment == "" {
		cfg.Environment = "production"
	}
	if cfg.FlushAt <= 0 {
		cfg.FlushAt = 50
	}
	if cfg.FlushInterval <= 0 {
		cfg.FlushInterval = 5 * time.Second
	}
	if cfg.Timeout <= 0 {
		cfg.Timeout = 5 * time.Second
	}
	if cfg.OnError == nil {
		cfg.OnError = func(error) {}
	}
	hc := cfg.HTTPClient
	if hc == nil {
		hc = &http.Client{Timeout: cfg.Timeout}
	}
	c := &Client{cfg: cfg, host: strings.TrimRight(cfg.Host, "/"), http: hc, stop: make(chan struct{}), done: make(chan struct{})}
	go c.loop()
	return c, nil
}

func now() string { return time.Now().UTC().Format("2006-01-02T15:04:05.000Z") }

func newUUID() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}

// Capture records an event for a user.
func (c *Client) Capture(distinctID, name string, properties map[string]any) {
	if distinctID == "" || name == "" {
		return
	}
	props := map[string]any{"$lib": "go", "$environment": c.cfg.Environment}
	if c.cfg.Release != "" {
		props["$release"] = c.cfg.Release
	}
	for k, v := range properties {
		props[k] = v
	}
	c.mu.Lock()
	c.queue = append(c.queue, event{Event: name, DistinctID: distinctID, UUID: newUUID(), Timestamp: now(), Properties: props})
	full := len(c.queue) >= c.cfg.FlushAt
	c.mu.Unlock()
	if full {
		c.Flush()
	}
}

// Feature marks a use of a feature (a task start).
func (c *Client) Feature(distinctID, key string, properties map[string]any) {
	p := map[string]any{}
	for k, v := range properties {
		p[k] = v
	}
	p["$feature"] = key
	c.Capture(distinctID, "$feature", p)
}

// Identify sets the user's role (and account, plan).
func (c *Client) Identify(distinctID string, t Traits) {
	p := map[string]any{}
	if t.Role != "" {
		p["$role"] = t.Role
	}
	if t.Account != "" {
		p["$account"] = t.Account
	}
	if t.Plan != "" {
		p["$plan"] = t.Plan
	}
	c.Capture(distinctID, "$identify", p)
}

// IsEnabled reports whether a flag is on for this user; false on any error.
func (c *Client) IsEnabled(ctx context.Context, flag, distinctID string, t Traits) bool {
	q := url.Values{"key": {c.cfg.Key}, "distinct_id": {distinctID}}
	if t.Role != "" {
		q.Set("role", t.Role)
	}
	if t.Account != "" {
		q.Set("account", t.Account)
	}
	if t.Plan != "" {
		q.Set("plan", t.Plan)
	}
	n := 0
	for k, v := range t.Properties {
		if v == "" || n >= 20 {
			continue
		}
		q.Set("p."+strings.ToLower(k), v)
		n++
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.host+"/flags?"+q.Encode(), nil)
	if err != nil {
		c.cfg.OnError(err)
		return false
	}
	resp, err := c.http.Do(req)
	if err != nil {
		c.cfg.OnError(err)
		return false
	}
	defer resp.Body.Close()
	var body struct {
		Flags []string `json:"flags"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&body); err != nil {
		c.cfg.OnError(err)
		return false
	}
	for _, f := range body.Flags {
		if f == flag {
			return true
		}
	}
	return false
}

// CheckInOptions describe one run of a job; every field is optional.
type CheckInOptions struct {
	// Status is "ok" (default) or "error".
	Status string
	// Duration is how long the run took (0: not given).
	Duration time.Duration
	// ExpectEvery is the time between runs, instead of the schedule
	// OpenLens learns (0: not given; 10 s to 31 days).
	ExpectEvery time.Duration
}

// CheckIn reports a run of a cron or background job (#609), so OpenLens
// alerts when the job does not run, fails or gets slower. Call it at the
// end of the job. It is sent at once (not queued) and returns false on any
// error; it never panics.
//
//	ol.CheckIn(ctx, "nightly-export", openlens.CheckInOptions{Duration: time.Since(start)})
func (c *Client) CheckIn(ctx context.Context, slug string, o CheckInOptions) bool {
	q := url.Values{}
	if o.Status != "" {
		q.Set("status", o.Status)
	}
	if o.Duration > 0 {
		q.Set("ms", fmt.Sprint(o.Duration.Milliseconds()))
	}
	if o.ExpectEvery > 0 {
		q.Set("expectEvery", fmt.Sprint(int64(o.ExpectEvery.Seconds())))
	}
	u := c.host + "/check-in/" + url.PathEscape(slug)
	if len(q) > 0 {
		u += "?" + q.Encode()
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, u, nil)
	if err != nil {
		c.cfg.OnError(err)
		return false
	}
	req.Header.Set("x-openlens-key", c.cfg.Key)
	resp, err := c.http.Do(req)
	if err != nil {
		c.cfg.OnError(err)
		return false
	}
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 1<<20))
	resp.Body.Close()
	if resp.StatusCode >= 300 {
		c.cfg.OnError(fmt.Errorf("openlens: check-in returned %d", resp.StatusCode))
		return false
	}
	return true
}

// Flush sends queued events now.
func (c *Client) Flush() {
	c.mu.Lock()
	batch := c.queue
	c.queue = nil
	c.mu.Unlock()
	if len(batch) == 0 {
		return
	}
	body, err := json.Marshal(map[string]any{"sent_at": now(), "batch": batch})
	if err != nil {
		c.cfg.OnError(err)
		return
	}
	req, err := http.NewRequest(http.MethodPost, c.host+"/capture", bytes.NewReader(body))
	if err != nil {
		c.cfg.OnError(err)
		return
	}
	req.Header.Set("content-type", "application/json")
	req.Header.Set("x-openlens-key", c.cfg.Key)
	resp, err := c.http.Do(req)
	if err != nil {
		c.cfg.OnError(err)
		return
	}
	_, _ = io.Copy(io.Discard, resp.Body)
	resp.Body.Close()
	if resp.StatusCode >= 300 {
		c.cfg.OnError(fmt.Errorf("openlens: capture returned %d", resp.StatusCode))
	}
}

// Shutdown flushes and stops the background goroutine (call on exit).
func (c *Client) Shutdown() {
	c.once.Do(func() {
		close(c.stop)
		<-c.done
		c.Flush()
	})
}

func (c *Client) loop() {
	defer close(c.done)
	t := time.NewTicker(c.cfg.FlushInterval)
	defer t.Stop()
	for {
		select {
		case <-c.stop:
			return
		case <-t.C:
			c.Flush()
		}
	}
}
