# frozen_string_literal: true

# OpenLens server SDK for Ruby: a thin client over the HTTP API.
#
# Standard library only. Events are queued and sent in batches from a
# background thread; errors never raise into the app (they go to on_error).
#
#   ol = OpenLens::Client.new(host: "https://lens.example.com", key: "pk_...", release: "v1.4.2")
#   ol.identify("42", role: "owner", account: "acme")
#   ol.capture("42", "invoice_sent")
#   ol.enabled?("new-billing", "42", role: "owner")
#   ol.shutdown
require "json"
require "net/http"
require "securerandom"
require "time"
require "uri"

module OpenLens
  VERSION = "0.1.0"

  class Client
    def initialize(host:, key:, release: nil, environment: nil, flush_at: 50,
                   flush_interval: 5.0, timeout: 5.0, on_error: nil)
      raise ArgumentError, "OpenLens: host and key are required" if host.to_s.empty? || key.to_s.empty?

      @host = host.to_s.chomp("/")
      @key = key
      @release = release
      @environment = environment || ENV.fetch("OPENLENS_ENVIRONMENT", "production")
      @flush_at = flush_at
      @timeout = timeout
      @on_error = on_error || ->(_e) {}
      @queue = []
      @lock = Mutex.new
      @stop = Queue.new
      @thread = Thread.new do
        loop do
          break if timed_pop(flush_interval)

          flush
        end
      end
    end

    # Record an event for a user.
    def capture(distinct_id, event, properties = {})
      return if distinct_id.to_s.empty? || event.to_s.empty?

      props = { "$lib" => "ruby", "$environment" => @environment }
      props["$release"] = @release if @release
      props.merge!(properties.transform_keys(&:to_s)) if properties
      item = {
        "event" => event.to_s,
        "distinct_id" => distinct_id.to_s,
        "uuid" => SecureRandom.uuid,
        "timestamp" => now,
        "properties" => props
      }
      full = @lock.synchronize do
        @queue << item
        @queue.size >= @flush_at
      end
      flush if full
    end

    # Mark a use of a feature (a task start).
    def feature(distinct_id, key, properties = {})
      capture(distinct_id, "$feature", (properties || {}).merge("$feature" => key))
    end

    # Set the user's role (and account, plan): what OpenLens slices every KPI by.
    def identify(distinct_id, role: nil, account: nil, plan: nil)
      p = { "$role" => role, "$account" => account, "$plan" => plan }.compact
      capture(distinct_id, "$identify", p)
    end

    # Is a flag on for this user? false on any error.
    def enabled?(flag, distinct_id, role: nil, account: nil)
      q = { "key" => @key, "distinct_id" => distinct_id.to_s, "role" => role, "account" => account }.compact
      uri = URI("#{@host}/flags?#{URI.encode_www_form(q)}")
      res = http(uri) { |h| h.get(uri.request_uri) }
      flags = JSON.parse(res.body)["flags"]
      flags.is_a?(Array) && flags.include?(flag)
    rescue StandardError => e
      @on_error.call(e)
      false
    end

    # Send queued events now.
    def flush
      batch = @lock.synchronize do
        b = @queue
        @queue = []
        b
      end
      return if batch.empty?

      uri = URI("#{@host}/capture")
      body = JSON.generate("sent_at" => now, "batch" => batch)
      res = http(uri) do |h|
        h.post(uri.request_uri, body, "content-type" => "application/json", "x-openlens-key" => @key)
      end
      @on_error.call(RuntimeError.new("OpenLens: capture returned #{res.code}")) unless res.code.to_i < 300
    rescue StandardError => e
      @on_error.call(e)
    end

    # Flush and stop the background thread (call on exit).
    def shutdown
      @stop << true
      @thread.join(@timeout)
      flush
    end

    private

    def now
      Time.now.utc.iso8601(3)
    end

    def timed_pop(seconds)
      @stop.pop(timeout: seconds)
    rescue ArgumentError # Ruby < 3.2: no timeout on Queue#pop
      sleep(seconds)
      !@stop.empty?
    end

    def http(uri, &block)
      Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == "https",
                                          open_timeout: @timeout, read_timeout: @timeout, &block)
    end
  end
end
