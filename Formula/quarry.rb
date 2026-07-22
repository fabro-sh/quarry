class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.6/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "16567d187d5af1962d91a2da49f388adfa3d33a9c27fbbc7f677059dc5133776"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.6/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "dd11b2145a838b66707a5ff41554ba3ce3008ac3abc7a42b3f64a9b715893c08"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.6/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "5b793b50932af8f8a3be8c7acddcc894360e16c7bf5141e7360575e2be779641"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.6/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "1953895a99a017996d79efab976f13eff7d0971242a3532d8e9e1cf2d3db9f04"
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
