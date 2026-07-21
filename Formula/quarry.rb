class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.4/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "c7aba45fb6dac46195b07d0b0be9be0847f3abfcf471f8b65992eed779a749e9"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.4/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "de493813c89888ad8842ce440a9cb4685b48d37d6b67f5d9db1c54a76ab7ed71"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.4/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "a0491da74992e3fc13bd84320665c1beaa8f316d05c8501780c3bd29b96fa1c9"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.4/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "8820ba806b82e6ec00d1a1974ec1a3fb85d432532da6945230bda6ee16d88719"
    end
  end

  def install
    release_binary = File.exist?("quarry") ? "quarry" : Dir["*/quarry"].first

    if build.head? || release_binary.nil?
      system "cargo", "install", *std_cargo_args(path: "crates/quarry")
    else
      bin.install release_binary => "quarry"
    end
  end

  test do
    assert_match "Shared, real-time document workspace for you and your AI agents",
                 shell_output("#{bin}/quarry --help")
  end
end
