class McrawTui < Formula
  desc "Cross Platform TUI for encoding your motioncam MCRAW files to professional video formats. All in the Terminal."
  homepage "https://github.com/Yoganshbhatt/mcraw-tui"
  version "0.2.1"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.1/mcraw-tui-aarch64-apple-darwin.zip"
    sha256 "C8E239F1F11F9480659519A209A8FED87AFD326D1813767049A26A175771C907"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.1/mcraw-tui-x86_64-apple-darwin.zip"
    sha256 "629E9913A4009687F3E50F93D4B588A60A8717E9132F2EC4318D414C865EB8A7"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoganshbhatt/mcraw-tui/releases/download/v0.2.1/mcraw-tui-x86_64-unknown-linux-gnu.zip"
    sha256 "5030F2C2A07DBB2DE118441938532DF88FB62DBEE47394ED126529B60CD8A04B"
  end

  depends_on "ffmpeg"

  def install
    bin.install "mcraw-tui"
  end
end
