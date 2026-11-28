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
    # created_at (a Time or an RFC 3339 string) is when the account was
    # created: its signup, so a user who signed up before the wiring is not new.
    def identify(distinct_id, role: nil, account: nil, plan: nil, created_at: nil)
      p = { "$role" => role, "$account" => account, "$plan" => plan }.compact
      p["$created_at"] = created_at.respond_to?(:iso8601) ? created_at.iso8601 : created_at.to_s if created_at
      capture(distinct_id, "$identify", p)
    end

    # Is a flag on for this user? false on any error.
    # plan: and properties: feed the flag's rules (never stored).
    def enabled?(flag, distinct_id, role: nil, account: nil, plan: nil, properties: {})
      q = { "key" => @key, "distinct_id" => distinct_id.to_s, "role" => role, "account" => account, "plan" => plan }.compact
      properties.first(20).each do |k, v|
        q["p.#{k.to_s.downcase}"] = v.to_s unless v.nil? || v.to_s.empty?
      end
      uri = URI("#{@host}/flags?#{URI.encode_www_form(q)}")
      res = http(uri) { |h| h.get(uri.request_uri) }
      flags = JSON.parse(res.body)["flags"]
      flags.is_a?(Array) && flags.include?(flag)
    rescue StandardError => e
      @on_error.call(e)
      false
    end

    # Job check-in (#609): call it at the end of a cron or background job,
    # so OpenLens alerts when the job does not run, fails or gets slower.
    # status: "ok" (default) or "error"; ms: the run's duration;
    # expect_every: seconds between runs, instead of the learned schedule.
    # Sent at once; false on any error, never raises.
    def check_in(slug, status: nil, ms: nil, expect_every: nil)
      q = { "status" => status, "ms" => ms&.round&.clamp(0, nil), "expectEvery" => expect_every&.round }.compact
      path = "/check-in/#{URI.encode_www_form_component(slug.to_s)}"
      path += "?#{URI.encode_www_form(q)}" unless q.empty?
      uri = URI("#{@host}#{path}")
      res = http(uri) { |h| h.post(uri.request_uri, "", "x-openlens-key" => @key) }
      return true if res.code.to_i < 300

      @on_error.call(RuntimeError.new("OpenLens: check-in returned #{res.code}"))
      false
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
