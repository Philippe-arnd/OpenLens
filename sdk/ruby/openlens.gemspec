# frozen_string_literal: true

require_relative "lib/openlens"

Gem::Specification.new do |s|
  s.name = "openlens"
  s.version = OpenLens::VERSION
  s.summary = "OpenLens server SDK: send events and check feature flags from Ruby"
  s.authors = ["OpenLens contributors"]
  s.license = "AGPL-3.0-only"
  s.homepage = "https://github.com/Philippe-arnd/OpenLens"
  s.files = ["lib/openlens.rb"]
  s.required_ruby_version = ">= 3.0"
end
