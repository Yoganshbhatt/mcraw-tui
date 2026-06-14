class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.1.0"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "8fdeaea6b8f72758138987acc0e6cb098531fe9412703f223e27ce4998e137cd"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "15fcadd5bcaf5d7441ba93a4f165f8e496da0382f2b3bd343d74ecda4e840bbd"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.0/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "adad299bc4fa1f89750e02dc0e5ebe1c77f9160fdc3b610a48a223a39707c1be"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
