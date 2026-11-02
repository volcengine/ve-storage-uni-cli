class VeAdriveCli < Formula
  desc "Dedicated ADrive command-line interface for Volcengine storage services"
  homepage "https://github.com/volcengine/ve-storage-uni-cli"
  version "1.0.3"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.3/ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
      sha256 "baf43ca09ef86c8a8fff105c38586999767bf04bc9b807b033c358cfe12dd146"
    else
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.3/ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
      sha256 "a370454fcbac37c62c2a0422f2d6f7f145709f17de64b915ca120c82631ef090"
    end
  end

  def install
    bin.install "bin/ve-adrive-cli"
  end

  test do
    system "#{bin}/ve-adrive-cli", "--version"
  end
end
