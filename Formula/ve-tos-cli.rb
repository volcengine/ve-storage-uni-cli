class VeTosCli < Formula
  desc "Dedicated TOS command-line interface for Volcengine storage services"
  homepage "https://github.com/volcengine/ve-storage-uni-cli"
  version "1.0.1"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.1/ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
      sha256 "4cc4c7c2b685135bca5667dd547dad74c498ce40d6d2448e8c7e3f604c15c555"
    else
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.1/ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
      sha256 "064ca6cff25ba0eb31d4209d1daf69cac43c5499f92349a550ee72fe737fd8e4"
    end
  end

  def install
    bin.install "bin/ve-tos-cli"
  end

  test do
    system "#{bin}/ve-tos-cli", "--version"
  end
end
