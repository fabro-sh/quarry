class Quarry < Formula
  desc "Local-first document workspace for humans and AI agents"
  homepage "https://github.com/fabro-sh/quarry"
  version "0.1.1"
  license "MIT"
  head "ssh://git@github.com/fabro-sh/quarry.git", branch: "main"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.0/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "78a9b625ce85ad0aad213c72b10f27e2e0e0d92a37c54210e908d88c43fb8ae8"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.0/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "6a6748e8645400c14e608888f839a147efa8676f8a96040c37880f068e486c54"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.0/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "6f47a091c4d1586ad68db303dc33c9d5a0db57269fd6961afb656bb37ef60a01"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.0/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "07959ada8bc1cb338e2c7249cb9126e02d09aea3da72769bb5d2cda5811cf8f7"
    end
  end

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.1/quarry-aarch64-apple-darwin.tar.gz"
      sha256 "7ec352a9cce59f5011962676f6570d276a41b40041cdd9747ab62b52410dcfc4"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.1/quarry-x86_64-apple-darwin.tar.gz"
      sha256 "79affad36da0d367fca9dd8484955aeee06baa9302d2b7646a88213c5ba00930"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.1/quarry-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "5755ac20c1d326ea121b5e6325bcca020263ca0f519474def1b9ddc23be491cc"
    else
      url "https://github.com/fabro-sh/quarry/releases/download/v0.1.1/quarry-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "66ec37fd878bf70fe34e8559faba5ece69d897fae69b3eb939421aa1524de9d6"
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
