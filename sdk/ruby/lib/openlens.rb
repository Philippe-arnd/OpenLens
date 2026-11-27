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
#
# Server errors: ol.capture_exception(e) in a rescue, or ol.capture_uncaught
# once at start-up for crashes.
require "json"
require "net/http"
require "rbconfig"
require "securerandom"
require "time"
require "uri"

module OpenLens
  VERSION = "0.1.0"
  # The distinct id of events that belong to no user (server errors).
  SERVER_ID = "$server"
  # The header the browser SDK adds to its own requests: the session id.
  SESSION_HEADER = "x-openlens-session"
  # Variables hosts set to the deployed commit, in order of preference (#495).
  COMMIT_VARS = %w[SOURCE_COMMIT GITHUB_SHA VERCEL_GIT_COMMIT_SHA CF_PAGES_COMMIT_SHA
                   RENDER_GIT_COMMIT HEROKU_SLUG_COMMIT].freeze
  # At most this many errors with one message per minute.
  MAX_SAME_ERROR = 5
  TRACEPARENT = /\A([0-9a-f]{2})-([0-9a-f]{32})-([0-9a-f]{16})-[0-9a-f]{2}\z/.freeze
  TRACE_ID = /\A[0-9a-f]{32}\z/.freeze
  SESSION_ID = /\A[A-Za-z0-9_-]{1,100}\z/.freeze
  LIB_DIRS = [RbConfig::CONFIG["rubylibdir"], RbConfig::CONFIG["rubyarchdir"]].compact.freeze

  # The release the host exposes: the deployed commit (shortened to 12
  # characters when it is a full hash), else nil.
  def self.detect_release(env)
    COMMIT_VARS.each do |name|
      v = env[name].to_s.strip
      next if v.empty?

      return v.match?(/\A\h{13,64}\z/) ? v[0, 12].downcase : v[0, 200]
    end
    nil
  end

  # trace_id of the incoming W3C traceparent (read, never written) and
  # session_id from the browser SDK's x-openlens-session header, for
  # capture_exception. Takes plain headers or a Rack env (HTTP_TRACEPARENT).
  def self.request_context(headers)
    h = {}
    headers.each { |k, v| h[k.to_s.downcase.sub(/\Ahttp_/, "").tr("_", "-")] = v.to_s.strip }
    ctx = {}
    m = TRACEPARENT.match(h["traceparent"].to_s)
    if m && m[1] != "ff" && m[2] =~ /[^0]/ && m[3] =~ /[^0]/
      ctx[:trace_id] = m[2]
    end
    sid = h[SESSION_HEADER].to_s
    ctx[:session_id] = sid if sid.match?(SESSION_ID)
    ctx
  end

  # The first backtrace line outside gems and Ruby's own library, as
  # "file:line:0": the error's group is its message @ this file.
  def self.app_frame(backtrace)
    Array(backtrace).first(50).each do |line|
      m = /\A(.+?):(\d+)(?::in |\z)/.match(line.to_s)
      next unless m

      file = m[1]
      next if file.start_with?("<") || file.include?("/gems/") || LIB_DIRS.any? { |d| file.start_with?(d) }

      return "#{file}:#{m[2]}:0"
    end
    nil
  end

  class Client
    def initialize(host:, key:, release: nil, environment: nil, flush_at: 50,
                   flush_interval: 5.0, timeout: 5.0, on_error: nil)
      raise ArgumentError, "OpenLens: host and key are required" if host.to_s.empty? || key.to_s.empty?

      @host = host.to_s.chomp("/")
      @key = key
      # An explicit release wins; else what the host exposes (#495).
      @release = release || OpenLens.detect_release(ENV)
      @environment = environment || ENV.fetch("OPENLENS_ENVIRONMENT", "production")
      @flush_at = flush_at
      @timeout = timeout
      @on_error = on_error || ->(_e) {}
      @queue = []
      @seen_minute = -1
      @seen_errors = {}
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
      push(distinct_id, event, properties, nil)
    end

    # Report a server exception. It is sent as $error with $kind "server", so
    # it is grouped, alerted on and judged per release like a browser error.
    # session_id: and trace_id: come from OpenLens.request_context(headers);
    # route: is the route template ("/entries/:id"), never the raw URL.
    # Never raises.
    def capture_exception(err, distinct_id: nil, session_id: nil, trace_id: nil, route: nil, method: nil,
                          properties: {})
      return if err.nil? || reported?(err)

      name = err.is_a?(Exception) ? err.class.name.to_s : "Error"
      message = "#{name}: #{err.is_a?(Exception) ? err.message : err}"[0, 500]
      return unless allow_error?(message)

      props = (properties || {}).transform_keys(&:to_s).merge("$kind" => "server", "$message" => message)
      trace = err.is_a?(Exception) ? err.backtrace : nil
      if trace
        props["$stack"] = ([message] + trace.first(50)).join("\n")[0, 4000]
        source = OpenLens.app_frame(trace)
        props["$source"] = source if source
      end
      # OpenTelemetry semantic-convention names next to OpenLens's own.
      props["exception.type"] = name[0, 200]
      props["http.route"] = route.to_s[0, 300] if route
      props["http.request.method"] = method.to_s.upcase[0, 10] if method
      props["trace_id"] = trace_id if trace_id.is_a?(String) && trace_id.match?(TRACE_ID) && trace_id =~ /[^0]/
      sid = session_id if session_id.is_a?(String) && session_id.match?(SESSION_ID)
      push(distinct_id || SERVER_ID, "$error", props, sid)
    rescue StandardError => e
      @on_error.call(e)
    end

    # Report the exception that ends the process (not exit or Ctrl-C), send
    # it at once, then let Ruby fail as it would have. Opt-in.
    def capture_uncaught
      at_exit do
        e = $!
        unless e.nil? || e.is_a?(SystemExit) || e.is_a?(Interrupt)
          capture_exception(e)
          flush
        end
      end
    end

    private def push(distinct_id, event, properties, session_id)
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
      item["session_id"] = session_id if session_id
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

    # The same exception object is reported once.
    def reported?(err)
      return true if err.instance_variable_defined?(:@__openlens_reported)

      err.instance_variable_set(:@__openlens_reported, true)
      false
    rescue StandardError # a frozen exception
      false
    end

    # At most MAX_SAME_ERROR of one message per minute.
    def allow_error?(message)
      minute = Time.now.to_i / 60
      n = @lock.synchronize do
        if minute != @seen_minute
          @seen_minute = minute
          @seen_errors.clear
        end
        c = @seen_errors.fetch(message, 0) + 1
        @seen_errors[message] = c if @seen_errors.size < 1000 || @seen_errors.key?(message)
        c
      end
      n <= MAX_SAME_ERROR
    end

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
