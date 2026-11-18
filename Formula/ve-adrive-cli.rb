class VeAdriveCli < Formula
  desc "Dedicated ADrive command-line interface for Volcengine storage services"
  homepage "https://github.com/volcengine/ve-storage-uni-cli"
  version "1.0.4"
  license "Apache-2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.4/ve-storage-uni-cli-aarch64-apple-darwin.tar.gz"
      sha256 "e52ef3c8d5ac5e47d343d4e1a35c54df28688b9252becea569bd7cab97997135"
    else
      url "https://github.com/volcengine/ve-storage-uni-cli/releases/download/v1.0.4/ve-storage-uni-cli-x86_64-apple-darwin.tar.gz"
      sha256 "adf12c7abe4c86a2a95b6b27d44596b0138d625cbe9149e6d784a26678cba826"
    end
  end

  def install
    bin.install "bin/ve-adrive-cli"
  end

  test do
    system "#{bin}/ve-adrive-cli", "--version"
  end
end
