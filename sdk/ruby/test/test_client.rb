# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)
require "minitest/autorun"
require "socket"
require "openlens"

# A tiny HTTP server on a random port: POST bodies are recorded, GET /flags
# answers "beta" for admins.
class FakeServer
  attr_reader :port, :posts

  def initialize
    @server = TCPServer.new("127.0.0.1", 0)
    @port = @server.addr[1]
    @posts = []
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
    if method == "POST"
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

  def reply(sock, body)
    sock.write("HTTP/1.1 200 OK\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
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

  def test_requires_host_and_key
    assert_raises(ArgumentError) { OpenLens::Client.new(host: "http://x", key: "") }
  end
end
