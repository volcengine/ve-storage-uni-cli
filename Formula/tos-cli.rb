class TosCli < Formula
  desc "Dedicated ByteCloud TOS command-line interface"
  homepage "https://github.com/volcengine/ve-storage-uni-cli"
  version "1.0.2"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.2/ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
      sha256 "2786236f1a59e1e180989d881919af728ef968fc5d4450a5d99957dd5e563cd7"
    else
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.2/ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
      sha256 "b7688deb3997be1f41279568bd78e985112aff13b90fdae3603d90db09185c4e"
    end
  end

  def install
    bin.install "bin/tos-cli"
  end

  test do
    system "#{bin}/tos-cli", "--version"
  end
end
