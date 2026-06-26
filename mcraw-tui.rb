class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.2"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.2/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "UPDATE_AFTER_RELEASE"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.2/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "UPDATE_AFTER_RELEASE"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.2/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "UPDATE_AFTER_RELEASE"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
