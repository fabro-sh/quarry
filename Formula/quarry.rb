class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.3/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "247ad8f9778eeb6cd2bfc0611f0d898f665a9a1f2f35a7cf90a511cc034382b4"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.3/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "023b56d1dbb005618c53df604e6e844450b7d8d9363b8190bc604692d0211632"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.3/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "836e64b8b858f9dbfc03627848fe1ed569d4714d0b6bd099f8cddf574d8b4c8a"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.3/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "2ce3ea2da3dd5f376307863960d22e12a8fa73cca538ad6e2caf38ff834208ab"
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
