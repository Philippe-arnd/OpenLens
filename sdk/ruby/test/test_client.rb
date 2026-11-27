# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "minitest/autorun"
require "socket"
require "openlens"

# A tiny HTTP server on a random port: POST bodies are recorded, GET /flags
# answers "beta" for admins.
class FakeServer
  attr_reader :port, :posts, :check_ins

  def initialize
    @server = TCPServer.new("127.0.0.1", 0)
    @port = @server.addr[1]
    @posts = []
    @check_ins = []
    @thread = Thread.new { loop { handle(@server.accept) } }
  end

  def handle(sock)
    line = sock.gets.to_s
    headers = {}
    while (h = sock.gets) && h != "\r\n"
      k, v = h.split(":", 2)
      headers[k.downcase] = v.strip
    end
    method, path = line.split
    if method == "POST" && path.start_with?("/check-in/")
      @check_ins << [path, headers["x-openlens-key"]]
      reply(sock, "{}", @check_ins.size == 1 ? "200 OK" : "429 Too Many Requests")
    elsif method == "POST"
      body = sock.read(headers["content-length"].to_i)
      @posts << [headers["x-openlens-key"], JSON.parse(body)]
      reply(sock, '{"accepted":1}')
    else
      flags = path.include?("role=admin") ? ["beta"] : []
      reply(sock, JSON.generate("flags" => flags))
    end
  ensure
    sock.close
  end

  def reply(sock, body, status = "200 OK")
    sock.write("HTTP/1.1 #{status}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
  end

  def stop
    @thread.kill
    @server.close
  end
end

class ClientTest < Minitest::Test
  def test_batches_and_flags
    srv = FakeServer.new
    ol = OpenLens::Client.new(host: "http://127.0.0.1:#{srv.port}/", key: "pk_t", release: "v9", flush_interval: 60)
    ol.identify("u1", role: "admin")
    ol.capture("u1", "invoice_sent")
    ol.feature("u1", "export")
    ol.shutdown
    assert_equal 1, srv.posts.size
    key, body = srv.posts[0]
    assert_equal "pk_t", key
    assert_equal %w[$identify invoice_sent $feature], body["batch"].map { |e| e["event"] }
    assert_equal "admin", body["batch"][0]["properties"]["$role"]
    assert_equal "v9", body["batch"][1]["properties"]["$release"]
    assert_equal "ruby", body["batch"][1]["properties"]["$lib"]
    assert ol.enabled?("beta", "u1", role: "admin")
    refute ol.enabled?("beta", "u2", role: "member")
  ensure
    srv&.stop
  end

  def test_never_raises_when_down
    errors = []
    ol = OpenLens::Client.new(host: "http://127.0.0.1:9", key: "pk_t", on_error: ->(e) { errors << e },
                              flush_interval: 60, timeout: 1)
    ol.capture("u1", "x")
    ol.shutdown
    refute ol.enabled?("beta", "u1")
    assert_operator errors.size, :>=, 1
  end

  def test_check_in_posts_the_slug_and_never_raises
    srv = FakeServer.new
    errors = []
    ol = OpenLens::Client.new(host: "http://127.0.0.1:#{srv.port}", key: "pk_t", flush_interval: 60,
                              on_error: ->(e) { errors << e })
    assert ol.check_in("nightly-export", status: "error", ms: 1234.4, expect_every: 86_400)
    path, key = srv.check_ins[0]
    assert_equal "pk_t", key
    refute_includes path, "pk_t"
    uri = URI(path)
    assert_equal "/check-in/nightly-export", uri.path
    assert_equal({ "status" => "error", "ms" => "1234", "expectEvery" => "86400" }, URI.decode_www_form(uri.query).to_h)
    refute ol.check_in("nightly-export"), "a refusal is false"
    assert_operator errors.size, :>=, 1
    ol.shutdown
  ensure
    srv&.stop
  end

  def test_detects_the_host_commit
    sha = "0123456789abcdef0123456789abcdef01234567"
    assert_equal sha[0, 12], OpenLens.detect_release("RENDER_GIT_COMMIT" => "abc", "GITHUB_SHA" => sha)
    assert_equal "v12", OpenLens.detect_release("SOURCE_COMMIT" => " ", "HEROKU_SLUG_COMMIT" => "v12")
    assert_nil OpenLens.detect_release({})
    ENV["SOURCE_COMMIT"] = "deadbeef"
    assert_equal "deadbeef", OpenLens::Client.new(host: "http://x", key: "k", flush_interval: 60).instance_variable_get(:@release)
    assert_equal "v2", OpenLens::Client.new(host: "http://x", key: "k", release: "v2", flush_interval: 60).instance_variable_get(:@release)
  ensure
    ENV.delete("SOURCE_COMMIT")
  end

  def test_server_error_with_request_context
    srv = FakeServer.new
    ol = OpenLens::Client.new(host: "http://127.0.0.1:#{srv.port}", key: "pk_t", flush_interval: 60)
    ctx = OpenLens.request_context("HTTP_TRACEPARENT" => "00-#{"a" * 32}-#{"b" * 16}-01", "HTTP_X_OPENLENS_SESSION" => "s1")
    err = begin
      raise KeyError, "entry 42"
    rescue KeyError => e
      e
    end
    ol.capture_exception(err, route: "/entries/:id", method: "get", **ctx)
    ol.capture_exception(err) # the same object is reported once
    10.times { ol.capture_exception(ArgumentError.new("same")) }
    ol.shutdown
    errs = srv.posts.flat_map { |_, b| b["batch"] }.select { |e| e["event"] == "$error" }
    first = errs[0]
    p = first["properties"]
    assert_equal ["$server", "s1"], [first["distinct_id"], first["session_id"]]
    assert_equal ["server", "KeyError", "/entries/:id", "GET", "a" * 32],
                 [p["$kind"], p["exception.type"], p["http.route"], p["http.request.method"], p["trace_id"]]
    assert_includes p["$source"], "test_client.rb:"
    assert_operator errs.count { |e| e["properties"]["$message"] == "ArgumentError: same" }, :<, 10
    assert_equal 1, errs.count { |e| e["properties"]["exception.type"] == "KeyError" }
  ensure
    srv&.stop
  end

  def test_crash_is_reported_and_still_fails
    srv = FakeServer.new
    code = "require 'openlens'; ol = OpenLens::Client.new(host: 'http://127.0.0.1:#{srv.port}', key: 'pk_t', " \
           "flush_interval: 60); ol.capture_uncaught; raise IndexError, 'crash'"
    ok = system(RbConfig.ruby, "-I", File.expand_path("../lib", __dir__), "-e", code, err: File::NULL)
    refute ok, "the process still fails"
    errs = srv.posts.flat_map { |_, b| b["batch"] }.select { |e| e["event"] == "$error" }
    assert_equal ["IndexError: crash"], errs.map { |e| e["properties"]["$message"] }
  ensure
    srv&.stop
  end

  def test_rejects_hostile_context
    assert_empty OpenLens.request_context("traceparent" => "00-#{"0" * 32}-#{"b" * 16}-01", "x-openlens-session" => "a b")
    assert_empty OpenLens.request_context("traceparent" => "ff-#{"a" * 32}-#{"b" * 16}-01")
  end

  def test_requires_host_and_key
    assert_raises(ArgumentError) { OpenLens::Client.new(host: "http://x", key: "") }
  end
end
