class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.1.0"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "BB91AE1BCC06EE3FC5030E2E2FEDAA5A77E059B598EB48D275C7C83134B6D87C"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "84E8D5263F6A9F6EBC1C5FC73B2E0B30C1D01E8A3CBDC49003325A68DB2024BD"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "2FB96B72B72E87F2B864607B36F0E3421F92E4FB824F3F8CAD54CF4D17DDFDD4"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
