class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.1.1"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.1/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "6251BCCE91A931A6EC1505CA79279781BE6E64D4C3355941235656D669656E7B"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.1/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "067C152DEFFE34217D31CF7B326EFCE0109D4EAD248537B2B6E89ACB44472598"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.1.1/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "41F785ACF244E2DC4A54A78BC365C57E8346954DDC9053EA8DA422575DD692F8"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
