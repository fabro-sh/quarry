#!/usr/bin/env ruby
# frozen_string_literal: true

require "minitest/autorun"
require "open3"
require "tmpdir"

class UpdateHomebrewFormulaTest < Minitest::Test
  SCRIPT = File.expand_path("update-homebrew-formula.rb", __dir__)
  SHA_AARCH64_DARWIN = "a" * 64
  SHA_X86_64_DARWIN = "b" * 64
  SHA_AARCH64_LINUX = "c" * 64
  SHA_X86_64_LINUX = "d" * 64

  FORMULA = <<~RUBY
    class Quarry < Formula
      desc "Local-first document workspace for humans and AI agents"
      homepage "https://github.com/fabro-sh/quarry"
      license "MIT"
      head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

      def install
        bin.install "quarry"
      end
    end
  RUBY

  def test_replaces_release_block_with_arch_specific_prebuilt_assets
    Dir.mktmpdir do |dir|
      formula_path = File.join(dir, "quarry.rb")
      File.write(formula_path, FORMULA)

      _stdout, stderr, status = Open3.capture3(
        "ruby",
        SCRIPT,
        formula_path,
        "v1.2.3",
        SHA_AARCH64_DARWIN,
        SHA_X86_64_DARWIN,
        SHA_AARCH64_LINUX,
        SHA_X86_64_LINUX
      )

      assert status.success?, stderr

      formula = File.read(formula_path)
      assert_includes formula, '  version "1.2.3"'
      assert_includes formula, "  on_macos do"
      assert_includes formula, '      url "https://github.com/fabro-sh/quarry/releases/download/v1.2.3/quarry-aarch64-apple-darwin.tar.gz"'
      assert_includes formula, "      sha256 \"#{SHA_AARCH64_DARWIN}\""
      assert_includes formula, '      url "https://github.com/fabro-sh/quarry/releases/download/v1.2.3/quarry-x86_64-apple-darwin.tar.gz"'
      assert_includes formula, "      sha256 \"#{SHA_X86_64_DARWIN}\""
      assert_includes formula, "  on_linux do"
      assert_includes formula, '      url "https://github.com/fabro-sh/quarry/releases/download/v1.2.3/quarry-aarch64-unknown-linux-gnu.tar.gz"'
      assert_includes formula, "      sha256 \"#{SHA_AARCH64_LINUX}\""
      assert_includes formula, '      url "https://github.com/fabro-sh/quarry/releases/download/v1.2.3/quarry-x86_64-unknown-linux-gnu.tar.gz"'
      assert_includes formula, "      sha256 \"#{SHA_X86_64_LINUX}\""
      assert_includes formula, '  license "MIT"'
      assert_includes formula, '  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"'
    end
  end

  def test_rejects_non_sha256_checksum
    Dir.mktmpdir do |dir|
      formula_path = File.join(dir, "quarry.rb")
      File.write(formula_path, FORMULA)

      _stdout, stderr, status = Open3.capture3(
        "ruby",
        SCRIPT,
        formula_path,
        "v1.2.3",
        "not-a-checksum",
        SHA_X86_64_DARWIN,
        SHA_AARCH64_LINUX,
        SHA_X86_64_LINUX
      )

      refute status.success?
      assert_includes stderr, "aarch64-apple-darwin SHA256 must be 64 hexadecimal characters"
    end
  end
end
