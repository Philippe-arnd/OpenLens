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
//
// Server errors: ol.CaptureException(err, openlens.RequestContext(r.Header))
// where an error is handled, and defer ol.CapturePanic() for panics.
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
	"path/filepath"
	"regexp"
	"runtime"
	"runtime/debug"
	"strings"
	"sync"
	"time"
)

// Version of this SDK.
const Version = "0.1.0"

// ServerID is the distinct id of events that belong to no user (server errors).
const ServerID = "$server"

// SessionHeader is the header the browser SDK adds to its own requests: the session id.
const SessionHeader = "X-Openlens-Session"

// maxSameError is the most errors with one message sent per minute.
const maxSameError = 5

// commitVars are the variables hosts set to the deployed commit, in order of preference (#495).
var commitVars = []string{
	"SOURCE_COMMIT",
	"GITHUB_SHA",
	"VERCEL_GIT_COMMIT_SHA",
	"CF_PAGES_COMMIT_SHA",
	"RENDER_GIT_COMMIT",
	"HEROKU_SLUG_COMMIT",
}

var (
	fullSHA     = regexp.MustCompile(`^[0-9a-fA-F]{13,64}$`)
	traceparent = regexp.MustCompile(`^([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}$`)
	traceID     = regexp.MustCompile(`^[0-9a-f]{32}$`)
	sessionID   = regexp.MustCompile(`^[A-Za-z0-9_-]{1,100}$`)
	selfFile    = func() string { _, f, _, _ := runtime.Caller(0); return f }()
)

// DetectRelease returns the release the host exposes: the deployed commit
// from getenv (shortened to 12 characters when it is a full hash), else the
// VCS revision Go stamped into the binary, else "".
func DetectRelease(getenv func(string) string) string {
	for _, name := range commitVars {
		if v := strings.TrimSpace(getenv(name)); v != "" {
			return shortRelease(v)
		}
	}
	if bi, ok := debug.ReadBuildInfo(); ok {
		for _, s := range bi.Settings {
			if s.Key == "vcs.revision" && s.Value != "" {
				return shortRelease(s.Value)
			}
		}
	}
	return ""
}

func shortRelease(v string) string {
	if fullSHA.MatchString(v) {
		return strings.ToLower(v[:12])
	}
	if len(v) > 200 {
		return v[:200]
	}
	return v
}

// Config holds the client settings. Host and Key are required.
type Config struct {
	Host string
	Key  string
	// Release default: the commit the host exposes (SOURCE_COMMIT,
	// GITHUB_SHA, ...), else the binary's VCS revision.
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
	SessionID  string         `json:"session_id,omitempty"`
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
	// errors sent per message in the current minute
	seenMinute int64
	seenErrors map[string]int
}

// New starts a client. It fails only when Host or Key is missing.
func New(cfg Config) (*Client, error) {
	if cfg.Host == "" || cfg.Key == "" {
		return nil, errors.New("openlens: Host and Key are required")
	}
	if cfg.Release == "" {
		// An explicit release wins; else what the host exposes (#495).
		cfg.Release = DetectRelease(os.Getenv)
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
	c.push(distinctID, name, properties, "")
}

func (c *Client) push(distinctID, name string, properties map[string]any, session string) {
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
	c.queue = append(c.queue, event{Event: name, DistinctID: distinctID, UUID: newUUID(), Timestamp: now(), Properties: props, SessionID: session})
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

// ErrorContext describes where a server error happened; every field is optional.
type ErrorContext struct {
	DistinctID string // default ServerID
	SessionID  string // the browser's x-openlens-session header, see RequestContext
	TraceID    string // the incoming traceparent's trace id, see RequestContext
	Route      string // the route template (/api/entries/{id}), never the raw URL
	Method     string
	Properties map[string]any
}

// RequestContext reads the trace id of the incoming W3C traceparent (read,
// never written) and the session the browser SDK sent in x-openlens-session.
func RequestContext(h http.Header) ErrorContext {
	var ctx ErrorContext
	if m := traceparent.FindStringSubmatch(strings.TrimSpace(h.Get("traceparent"))); m != nil &&
		m[1] != "ff" && strings.Trim(m[2], "0") != "" && strings.Trim(m[3], "0") != "" {
		ctx.TraceID = m[2]
	}
	if sid := strings.TrimSpace(h.Get(SessionHeader)); sessionID.MatchString(sid) {
		ctx.SessionID = sid
	}
	return ctx
}

// CaptureException reports a server error. It is sent as $error with
// $kind "server", so it is grouped, alerted on and judged per release like
// a browser error. The stack is the caller's. It never panics.
func (c *Client) CaptureException(err error, ctx ErrorContext) {
	if err == nil {
		return
	}
	c.captureError(fmt.Sprintf("%T", err), err.Error(), 3, ctx)
}

// CapturePanic reports a panic, sends it at once and panics again with the
// same value, so the program fails as it would have. Defer it directly:
//
//	defer ol.CapturePanic()
func (c *Client) CapturePanic() {
	v := recover()
	if v == nil {
		return
	}
	if err, ok := v.(error); ok {
		c.captureError(fmt.Sprintf("%T", err), err.Error(), 3, ErrorContext{})
	} else {
		c.captureError("panic", fmt.Sprint(v), 3, ErrorContext{})
	}
	c.Flush()
	panic(v)
}

func (c *Client) captureError(typ, msg string, skip int, ctx ErrorContext) {
	defer func() {
		if r := recover(); r != nil {
			c.cfg.OnError(fmt.Errorf("openlens: capture error: %v", r))
		}
	}()
	message := truncate(typ+": "+msg, 500)
	if !c.allowError(message) {
		return
	}
	props := map[string]any{}
	for k, v := range ctx.Properties {
		props[k] = v
	}
	props["$kind"] = "server"
	props["$message"] = message
	stack, source := callerStack(skip)
	if stack != "" {
		props["$stack"] = stack
	}
	if source != "" {
		props["$source"] = source
	}
	// OpenTelemetry semantic-convention names next to OpenLens's own.
	props["exception.type"] = truncate(typ, 200)
	if ctx.Route != "" {
		props["http.route"] = truncate(ctx.Route, 300)
	}
	if ctx.Method != "" {
		props["http.request.method"] = truncate(strings.ToUpper(ctx.Method), 10)
	}
	if traceID.MatchString(ctx.TraceID) && strings.Trim(ctx.TraceID, "0") != "" {
		props["trace_id"] = ctx.TraceID
	}
	session := ""
	if sessionID.MatchString(ctx.SessionID) {
		session = ctx.SessionID
	}
	distinct := ctx.DistinctID
	if distinct == "" {
		distinct = ServerID
	}
	c.push(distinct, "$error", props, session)
}

// allowError lets at most maxSameError errors of one message per minute through.
func (c *Client) allowError(message string) bool {
	minute := time.Now().Unix() / 60
	c.mu.Lock()
	defer c.mu.Unlock()
	if minute != c.seenMinute || c.seenErrors == nil {
		c.seenMinute = minute
		c.seenErrors = map[string]int{}
	}
	n, seen := c.seenErrors[message]
	n++
	if len(c.seenErrors) < 1000 || seen {
		c.seenErrors[message] = n
	}
	return n <= maxSameError
}

// callerStack formats the stack from skip frames above its caller (at most
// 50 frames, 4000 bytes) and the first frame of the app's own code as
// file:line:0, outside this SDK, the standard library and the module cache.
func callerStack(skip int) (string, string) {
	pcs := make([]uintptr, 50)
	n := runtime.Callers(skip+1, pcs)
	frames := runtime.CallersFrames(pcs[:n])
	goroot := runtime.GOROOT()
	var b strings.Builder
	source := ""
	for {
		f, more := frames.Next()
		if f.Function != "" {
			fmt.Fprintf(&b, "%s\n\t%s:%d\n", f.Function, f.File, f.Line)
			if source == "" && isAppFrame(f, goroot) {
				source = fmt.Sprintf("%s:%d:0", f.File, f.Line)
			}
		}
		if !more {
			break
		}
	}
	return truncate(b.String(), 4000), source
}

func isAppFrame(f runtime.Frame, goroot string) bool {
	switch {
	case f.File == "" || f.File == selfFile || strings.HasPrefix(f.Function, "runtime."):
		return false
	case strings.Contains(f.File, "/pkg/mod/") || strings.Contains(f.File, "@v"):
		return false
	case goroot != "" && strings.HasPrefix(f.File, filepath.ToSlash(goroot)+"/src/"):
		return false
	case !filepath.IsAbs(f.File):
		// -trimpath: the standard library's first path element has no dot.
		first, _, _ := strings.Cut(f.File, "/")
		return strings.Contains(first, ".")
	}
	return true
}

func truncate(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return strings.ToValidUTF8(s[:n], "")
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
