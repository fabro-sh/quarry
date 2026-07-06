#!/usr/bin/env ruby
# frozen_string_literal: true

RELEASE_BASE_URL = "https://github.com/fabro-sh/quarry/releases/download"
ASSET_NAME = "quarry"
FORMULA_USAGE = "Usage: update-homebrew-formula.rb FORMULA_PATH RELEASE_TAG SHA_AARCH64_DARWIN SHA_X86_64_DARWIN SHA_AARCH64_LINUX SHA_X86_64_LINUX"
TARGETS = %w[
  aarch64-apple-darwin
  x86_64-apple-darwin
  aarch64-unknown-linux-gnu
  x86_64-unknown-linux-gnu
].freeze

formula_path, release_tag, *sha256_values = ARGV
abort FORMULA_USAGE unless formula_path && release_tag && sha256_values.length == TARGETS.length
abort "Release tag must start with v: #{release_tag}" unless release_tag.start_with?("v")

checksums = TARGETS.zip(sha256_values).to_h
checksums.each do |target, checksum|
  unless checksum.match?(/\A[0-9a-f]{64}\z/i)
    abort "#{target} SHA256 must be 64 hexadecimal characters: #{checksum}"
  end
end

platform_release = [
  "  on_macos do",
  "    if Hardware::CPU.arm?",
  "      url \"#{RELEASE_BASE_URL}/#{release_tag}/#{ASSET_NAME}-aarch64-apple-darwin.tar.gz\"",
  "      sha256 \"#{checksums.fetch("aarch64-apple-darwin")}\"",
  "    else",
  "      url \"#{RELEASE_BASE_URL}/#{release_tag}/#{ASSET_NAME}-x86_64-apple-darwin.tar.gz\"",
  "      sha256 \"#{checksums.fetch("x86_64-apple-darwin")}\"",
  "    end",
  "  end",
  "",
  "  on_linux do",
  "    if Hardware::CPU.arm?",
  "      url \"#{RELEASE_BASE_URL}/#{release_tag}/#{ASSET_NAME}-aarch64-unknown-linux-gnu.tar.gz\"",
  "      sha256 \"#{checksums.fetch("aarch64-unknown-linux-gnu")}\"",
  "    else",
  "      url \"#{RELEASE_BASE_URL}/#{release_tag}/#{ASSET_NAME}-x86_64-unknown-linux-gnu.tar.gz\"",
  "      sha256 \"#{checksums.fetch("x86_64-unknown-linux-gnu")}\"",
  "    end",
  "  end"
].join("\n")

formula = File.read(formula_path)
formula_parts = formula.match(
  /\A(?<prefix>.*?^  homepage "[^"]+"\n)(?<version_block>.*?)(?<license>^  license "[^"]+"\n)(?<pre_head_block>.*?)(?<head>^  head [^\n]*\n)(?<platform_block>.*?)(?<body>^  def install.*)\z/m
)
abort "Unable to find Homebrew homepage/license/head/install declarations in #{formula_path}" unless formula_parts

head = formula_parts[:head].rstrip
updated_formula = "#{formula_parts[:prefix]}#{formula_parts[:license]}#{head}\n\n#{platform_release}\n\n#{formula_parts[:body]}"

File.write(formula_path, updated_formula)
