package openlens

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

func TestBatchesAndFlags(t *testing.T) {
	var mu sync.Mutex
	var got []map[string]any
	var key string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == http.MethodPost {
			var b map[string]any
			_ = json.NewDecoder(r.Body).Decode(&b)
			mu.Lock()
			got = append(got, b)
			key = r.Header.Get("x-openlens-key")
			mu.Unlock()
			_, _ = w.Write([]byte(`{"accepted":3}`))
			return
		}
		flags := []string{}
		if r.URL.Query().Get("role") == "admin" {
			flags = append(flags, "beta")
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"flags": flags})
	}))
	defer srv.Close()
	ol, err := New(Config{Host: srv.URL + "/", Key: "pk_t", Release: "v9", FlushInterval: time.Minute})
	if err != nil {
		t.Fatal(err)
	}
	ol.Identify("u1", Traits{Role: "admin"})
	ol.Capture("u1", "invoice_sent", nil)
	ol.Feature("u1", "export", nil)
	ol.Shutdown()
	if len(got) != 1 || key != "pk_t" {
		t.Fatalf("want one batch with the key, got %d batches, key %q", len(got), key)
	}
	batch := got[0]["batch"].([]any)
	names := []string{}
	for _, e := range batch {
		names = append(names, e.(map[string]any)["event"].(string))
	}
	if len(names) != 3 || names[0] != "$identify" || names[1] != "invoice_sent" || names[2] != "$feature" {
		t.Fatalf("events %v", names)
	}
	p0 := batch[0].(map[string]any)["properties"].(map[string]any)
	p1 := batch[1].(map[string]any)["properties"].(map[string]any)
	if p0["$role"] != "admin" || p1["$release"] != "v9" || p1["$lib"] != "go" {
		t.Fatalf("properties %v %v", p0, p1)
	}
	if !ol.IsEnabled(context.Background(), "beta", "u1", Traits{Role: "admin"}) {
		t.Fatal("beta should be on for admin")
	}
	if ol.IsEnabled(context.Background(), "beta", "u2", Traits{Role: "member"}) {
		t.Fatal("beta should be off for member")
	}
}

func TestNeverPanicsWhenDown(t *testing.T) {
	var errs []error
	var mu sync.Mutex
	ol, err := New(Config{Host: "http://127.0.0.1:9", Key: "pk_t", Timeout: time.Second, FlushInterval: time.Minute,
		OnError: func(e error) { mu.Lock(); errs = append(errs, e); mu.Unlock() }})
	if err != nil {
		t.Fatal(err)
	}
	ol.Capture("u1", "x", nil)
	ol.Shutdown()
	if ol.IsEnabled(context.Background(), "beta", "u1", Traits{}) {
		t.Fatal("flag on while down")
	}
	if len(errs) < 1 {
		t.Fatal("errors should reach OnError")
	}
}

func TestRequiresHostAndKey(t *testing.T) {
	if _, err := New(Config{Host: "http://x"}); err == nil {
		t.Fatal("missing key accepted")
	}
}

func TestCheckIn(t *testing.T) {
	var mu sync.Mutex
	var seen []*http.Request
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		seen = append(seen, r)
		n := len(seen)
		mu.Unlock()
		if n > 1 {
			w.WriteHeader(http.StatusTooManyRequests)
		}
		_, _ = w.Write([]byte(`{}`))
	}))
	defer srv.Close()
	var errs int
	ol, err := New(Config{Host: srv.URL, Key: "pk_t", FlushInterval: time.Minute, OnError: func(error) { mu.Lock(); errs++; mu.Unlock() }})
	if err != nil {
		t.Fatal(err)
	}
	defer ol.Shutdown()
	ctx := context.Background()
	if !ol.CheckIn(ctx, "nightly-export", CheckInOptions{Status: "error", Duration: 1500 * time.Millisecond, ExpectEvery: 24 * time.Hour}) {
		t.Fatal("first check-in should succeed")
	}
	mu.Lock()
	r := seen[0]
	mu.Unlock()
	q := r.URL.Query()
	if r.Method != http.MethodPost || r.URL.Path != "/check-in/nightly-export" || r.Header.Get("x-openlens-key") != "pk_t" {
		t.Fatalf("request %s %s", r.Method, r.URL)
	}
	if q.Get("status") != "error" || q.Get("ms") != "1500" || q.Get("expectEvery") != "86400" || q.Get("key") != "" {
		t.Fatalf("query %v", q)
	}
	if ol.CheckIn(ctx, "nightly-export", CheckInOptions{}) {
		t.Fatal("a refused check-in reports false")
	}
	mu.Lock()
	defer mu.Unlock()
	if errs < 1 {
		t.Fatal("the refusal goes to OnError")
	}
}
