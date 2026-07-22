class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.5/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "fe24f0c7b075035e297482dc76c6db6ede845f4540fe7ba26b6a70ef6924ce02"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.5/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "8ddea5f9bf75f701cfbfbb0e29083402f579923bfc1ef8867f31c7322f548313"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.5/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "df6a8f8735dce0eb1ab4ca473e6d7a44507712f8b16895e11ae95b54239fe106"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.5/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "303d3dcac543f7a3944d9800000fc5a3972994caf3aba9ec848b316f094dd7e9"
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
