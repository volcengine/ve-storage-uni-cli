class VeTosCli < Formula
  desc "Dedicated TOS command-line interface for Volcengine storage services"
  homepage "https://github.com/volcengine/ve-storage-uni-cli"
  version "1.0.0"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.0/ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
      sha256 "a5968225c619f7e94a1e055c299c38c93338c2220c1e582eb2b8235e05f22740"
    else
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.0/ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
      sha256 "2ef4d30381b089ca239cc98e0cd795729b73cf70ae6fea11da852a8cbc6b3c3d"
    end
  end

  def install
    bin.install "bin/ve-tos-cli"
  end

  test do
    system "#{bin}/ve-tos-cli", "--version"
  end
end
