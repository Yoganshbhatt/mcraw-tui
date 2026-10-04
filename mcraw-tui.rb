class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.5"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.5/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "9E874F92CD862CB7B3EFCF2279146E2C0115929FF4A7B31D491A15A1C9CD725B"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.5/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "4B363F31147CE64B160173C22AE2D5BB01F64B96C20E75F504C08718FEBC56DF"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.5/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "05A048054061D20DFB707DC39D0AF1086A18CC414B47464D39ADA398EAB2CB57"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
