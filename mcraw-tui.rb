class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.3"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.3/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "AA72C4C491AE20BC894EFC7F7139E8175DD1057742DD43F6075BAB60AEF9B6EA"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.3/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "47AC17C852E219DDE62D1154DB410C6D4B46E9521CC0640FE0BC9E0C98D3D5DB"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.3/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "C3D1E855E4799436F3876AB5C76A1733599DBBCCB017F37013E685E9E913EFBB"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
