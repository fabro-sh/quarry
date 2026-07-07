class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.2/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "63d32b85098f40f6a429b5f5a5acffa4b2a84eff5c47934ee2094be379976cb5"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.2/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "f92b0d64c95846cda49f6e35868f3c9602fc46af5b34d0222874cb288a42625f"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.2/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "e6f3367ee4289066cc741d6c8734be2be403dc624dc1ca9b22ac3e6cbe56ed74"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.2/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "4224615d1fe28c5e9028c6de6e32735d1294d32134da10252294b94aeb19859c"
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
